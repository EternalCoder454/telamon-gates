//! The llama.cpp backend: replies from `llama-server`, either the one Gates
//! runs itself (telamon-llama's, with a model from the models folder; see
//! `server.rs`) or one already running at an address the user gives.
//!
//! It speaks the OpenAI-compatible API: `GET /v1/models` and a streamed
//! `POST /v1/chat/completions` (`sse.rs`).

use super::server::{Endpoint, Launch, Server};
use super::sse::{self, Line};
use super::{Backend, BackendError, Event, Options, Request};
use crate::conversation::{Role, ToolCall};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Where telamon-llama installs the server.
pub const PACKAGED_SERVER: &str = "/usr/libexec/telamon-llama/llama-server";

/// The server binary: telamon-llama's, else a `llama-server` on `$PATH`.
pub fn find_server() -> Option<PathBuf> {
    let packaged = PathBuf::from(PACKAGED_SERVER);
    if packaged.is_file() {
        return Some(packaged);
    }
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join("llama-server"))
            .find(|p| p.is_file())
    })
}

/// A blocking HTTP client; `connect` bounds connecting, nothing bounds a
/// reply (a long one streams for minutes; Stop ends it).
pub(crate) fn agent(connect: Option<Duration>) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(connect)
        .http_status_as_error(false)
        // No HTTP_PROXY: the server is on this computer or the local
        // network, and a proxy would also see its key.
        .proxy(None)
        .build()
        .into()
}

/// Automatic's context, in tokens: what llama.cpp's own fit would choose
/// can fill a 24 GiB card with cache for one conversation (129,024 tokens
/// for a 4B model), leaving no room for anything else. This is plenty for
/// a long chat or an agent's files; Settings goes higher.
pub const AUTO_CONTEXT: u32 = 32_768;

/// The context to start a model with: the one set in Settings, else
/// `AUTO_CONTEXT` or the model's own if smaller (`trained`, 0 if unknown).
pub fn context_for(setting: u32, trained: u32) -> u32 {
    if setting > 0 {
        setting
    } else if trained > 0 {
        trained.min(AUTO_CONTEXT)
    } else {
        AUTO_CONTEXT
    }
}

/// A model in the models folder: `name` is the file name without `.gguf`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalModel {
    pub name: String,
    pub path: PathBuf,
    pub size: u64,
    /// What its header says (quantisation, size label, and whether it is a
    /// decision model, which can't chat).
    pub info: crate::gguf::Info,
}

/// The models in `dir` that chat: decision models answer SystemOne's
/// questions and write no text.
pub fn chat_models(dir: &Path) -> Vec<LocalModel> {
    local_models(dir)
        .into_iter()
        .filter(|m| m.info.decision.is_empty())
        .collect()
}

/// The `.gguf` files in `dir`, by name. Reads each one's header.
pub fn local_models(dir: &Path) -> Vec<LocalModel> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut models: Vec<LocalModel> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let path = e.path();
            let name = path
                .file_name()?
                .to_str()?
                .strip_suffix(".gguf")?
                .to_string();
            let meta = e.metadata().ok()?;
            let usable =
                meta.is_file() && !name.is_empty() && !name.starts_with('.') && is_model(&name);
            if !usable {
                return None;
            }
            // A file that isn't GGUF still shows: llama.cpp says why.
            let info = crate::gguf::read(&path).unwrap_or_default();
            Some(LocalModel {
                name,
                size: meta.len(),
                path,
                info,
            })
        })
        .collect();
    models.sort_by_key(|m| m.name.to_lowercase());
    models
}

/// Whether a `.gguf` (by its name without `.gguf`) is a model to chat with:
/// not a vision projector (`mmproj-…`), and of a model split in parts only
/// the first (`…-00001-of-00003`), which llama.cpp loads the rest from.
fn is_model(name: &str) -> bool {
    if name.to_ascii_lowercase().starts_with("mmproj") {
        return false;
    }
    match name.rsplit_once("-of-") {
        Some((head, tail)) if tail.len() == 5 && tail.bytes().all(|b| b.is_ascii_digit()) => {
            head.ends_with("-00001")
        }
        _ => true,
    }
}

pub struct Llama {
    models_dir: PathBuf,
    /// The binary Gates starts; None runs only an external server.
    binary: Option<PathBuf>,
    server: Arc<Server>,
    options: Mutex<Options>,
    /// The context of the server last asked: its address, and its size.
    context: Mutex<Option<(String, u32)>>,
}

/// Room kept for the reply when the conversation is trimmed to the context:
/// a quarter of it, at most 2048 tokens.
pub fn reply_room(n_ctx: u32) -> usize {
    (n_ctx as usize / 4).min(2048)
}

/// What stands in for a tool's output that no longer fits.
pub const TRIMMED_OUTPUT: &str =
    "(This output was removed to fit the context. Run the tool again if it is needed.)";

/// The conversation cut to fit `budget` tokens, as `count` measures it.
/// Whole turns before the last user message go first, oldest first; then,
/// within the task under way (an agent's steps), the oldest tools' output
/// gives way to a short note. The last user message, and every call with
/// its result, stay. Unmeasurable (`count` gives None): as it is.
pub fn trim_to_budget(
    request: &Request,
    budget: usize,
    mut count: impl FnMut(&Request) -> Option<usize>,
) -> Request {
    let mut trimmed = request.clone();
    // Bounded: each round drops a message or shortens an output.
    loop {
        match count(&trimmed) {
            Some(n) if n > budget => {}
            _ => break,
        }
        let task = trimmed
            .messages
            .iter()
            .rposition(|m| m.role == Role::User)
            .unwrap_or(0);
        if task > 0 {
            trimmed.messages.remove(0);
            // A conversation starts with the user's turn (a tool result
            // without its call means nothing either).
            while trimmed.messages[0].role != Role::User {
                trimmed.messages.remove(0);
            }
            continue;
        }
        let oldest = trimmed
            .messages
            .iter_mut()
            .find(|m| m.role == Role::Tool && m.text != TRIMMED_OUTPUT);
        match oldest {
            Some(m) => m.text = TRIMMED_OUTPUT.to_string(),
            None => break,
        }
    }
    trimmed
}

impl Llama {
    pub fn new(
        models_dir: PathBuf,
        binary: Option<PathBuf>,
        log: PathBuf,
        options: Options,
    ) -> Llama {
        Llama {
            models_dir,
            binary,
            server: Server::new(log),
            options: Mutex::new(options),
            context: Mutex::new(None),
        }
    }

    /// The server's context in tokens (`/v1/models`: `meta.n_ctx`), asked
    /// once per server.
    fn n_ctx(&self, endpoint: &Endpoint) -> Option<u32> {
        if let Some((base, n)) = self
            .context
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            && base == endpoint.base
        {
            return Some(n);
        }
        let text = authorized(
            agent(Some(Duration::from_secs(5))).get(format!("{}/v1/models", endpoint.base)),
            endpoint,
        )
        .call()
        .ok()?
        .into_body()
        .read_to_string()
        .ok()?;
        let n = serde_json::from_str::<Value>(&text)
            .ok()?
            .pointer("/data/0/meta/n_ctx")?
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .filter(|n| *n > 0)?;
        *self.context.lock().unwrap_or_else(|e| e.into_inner()) = Some((endpoint.base.clone(), n));
        Some(n)
    }

    /// How many tokens `request` is, by the server's own chat template and
    /// tokenizer (`/apply-template`, then `/tokenize`). None when the server
    /// can't say (another kind of server).
    fn count_tokens(&self, endpoint: &Endpoint, request: &Request) -> Option<usize> {
        let a = agent(Some(Duration::from_secs(5)));
        let mut body = request_body(request);
        body.as_object_mut()?.remove("stream");
        let prompt = authorized(
            a.post(format!("{}/apply-template", endpoint.base)),
            endpoint,
        )
        .header("Content-Type", "application/json")
        .send(body.to_string())
        .ok()?
        .into_body()
        .read_to_string()
        .ok()?;
        let prompt = serde_json::from_str::<Value>(&prompt)
            .ok()?
            .get("prompt")?
            .as_str()?
            .to_string();
        let tokens = authorized(a.post(format!("{}/tokenize", endpoint.base)), endpoint)
            .header("Content-Type", "application/json")
            .send(json!({"content": prompt, "add_special": true}).to_string())
            .ok()?
            .into_body()
            .read_to_string()
            .ok()?;
        Some(
            serde_json::from_str::<Value>(&tokens)
                .ok()?
                .get("tokens")?
                .as_array()?
                .len(),
        )
    }

    /// The conversation as much of it as fits the context, with room for the
    /// reply.
    fn fit(&self, endpoint: &Endpoint, request: &Request) -> Request {
        let Some(n_ctx) = self.n_ctx(endpoint) else {
            return request.clone();
        };
        let budget = (n_ctx as usize).saturating_sub(reply_room(n_ctx));
        let fitted = trim_to_budget(request, budget, |r| self.count_tokens(endpoint, r));
        let dropped = request.messages.len() - fitted.messages.len();
        if dropped > 0 {
            log::info!(
                "the conversation was trimmed to the context ({n_ctx} tokens): {dropped} older messages left out"
            );
        }
        fitted
    }

    fn options(&self) -> Options {
        self.options
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Where to send the request, and whether it holds the managed server
    /// (to release after).
    fn endpoint(&self, model: &str) -> Result<(Endpoint, bool), BackendError> {
        let options = self.options();
        if !options.server_url.is_empty() {
            return Ok((
                Endpoint {
                    base: options.server_url.trim_end_matches('/').to_string(),
                    api_key: String::new(),
                },
                false,
            ));
        }
        let Some(binary) = self.binary.clone() else {
            return Err(BackendError::Unreachable(
                "No model server: install telamon-llama, or set a server address in Settings."
                    .into(),
            ));
        };
        let models = chat_models(&self.models_dir);
        let Some(chosen) = models
            .iter()
            .find(|m| m.name == model)
            .or_else(|| models.first())
        else {
            return Err(BackendError::Other(format!(
                "No model yet: put a .gguf file in {}.",
                self.models_dir.display()
            )));
        };
        let launch = Launch {
            binary,
            model: chosen.path.clone(),
            gpu_layers: (options.gpu_layers > 0).then_some(options.gpu_layers),
            context: Some(context_for(options.context, chosen.info.context_length)),
            batch: None,
        };
        Ok((self.server.acquire(&launch)?, true))
    }

    fn stream(
        &self,
        endpoint: &Endpoint,
        request: &Request,
        cancel: &AtomicBool,
        emit: &mut dyn FnMut(Event<'_>),
    ) -> Result<(), BackendError> {
        let body = request_body(&self.fit(endpoint, request));
        let call = authorized(
            agent(Some(Duration::from_secs(10)))
                .post(format!("{}/v1/chat/completions", endpoint.base)),
            endpoint,
        )
        .header("Content-Type", "application/json")
        .header("Accept", "text/event-stream");
        let response = call.send(body.to_string()).map_err(http_error)?;
        let status = response.status().as_u16();
        let mut body = response.into_body();
        if status != 200 {
            let text = body.read_to_string().unwrap_or_default();
            return Err(BackendError::Refused(format!(
                "The model server refused the request ({status}): {}",
                sse::error_message(&text)
            )));
        }
        let reader = BufReader::new(body.into_reader());
        // Tool calls come in pieces, by index; they go out whole at the end.
        let mut calls: Vec<ToolCall> = Vec::new();
        // Until `[DONE]`: a stream that just ends was cut off (the server
        // crashed or the network went).
        for line in reader.lines() {
            if cancel.load(Ordering::Relaxed) {
                // Dropping the reader closes the connection: the server stops.
                return Ok(());
            }
            let line =
                line.map_err(|e| BackendError::Other(format!("The reply was cut off: {e}.")))?;
            for event in sse::parse(&line) {
                match event {
                    Line::Text(t) => emit(Event::Text(&t)),
                    Line::Speed(s) => emit(Event::Speed(s)),
                    Line::Error(e) => return Err(BackendError::Refused(e)),
                    Line::ToolCall {
                        index,
                        id,
                        name,
                        arguments,
                    } => {
                        // A bounded list: no model asks for hundreds.
                        if index >= 64 {
                            continue;
                        }
                        while calls.len() <= index {
                            calls.push(ToolCall {
                                id: String::new(),
                                name: String::new(),
                                arguments: String::new(),
                            });
                        }
                        let call = &mut calls[index];
                        if let Some(id) = id {
                            call.id = id;
                        }
                        if let Some(name) = name {
                            call.name = name;
                        }
                        call.arguments.push_str(&arguments);
                    }
                    Line::Done => {
                        calls.retain(|c| !c.name.is_empty());
                        for (i, c) in calls.iter_mut().enumerate() {
                            if c.id.is_empty() {
                                c.id = format!("call_{i}");
                            }
                        }
                        if !calls.is_empty() {
                            emit(Event::ToolCalls(&calls));
                        }
                        return Ok(());
                    }
                }
            }
        }
        if cancel.load(Ordering::Relaxed) {
            return Ok(());
        }
        Err(BackendError::Other(
            "The reply was cut off: the model server stopped answering.".into(),
        ))
    }
}

impl Backend for Llama {
    fn name(&self) -> String {
        let options = self.options();
        if !options.server_url.is_empty() {
            format!("llama.cpp at {}", options.server_url)
        } else if let Some(binary) = &self.binary {
            format!("llama.cpp ({})", binary.display())
        } else {
            "llama.cpp (not installed)".to_string()
        }
    }

    fn models(&self) -> Result<Vec<String>, BackendError> {
        let options = self.options();
        if options.server_url.is_empty() {
            return Ok(chat_models(&self.models_dir)
                .into_iter()
                .map(|m| m.name)
                .collect());
        }
        let url = format!("{}/v1/models", options.server_url.trim_end_matches('/'));
        let response = agent(Some(Duration::from_secs(5)))
            .get(&url)
            .call()
            .map_err(http_error)?;
        let status = response.status().as_u16();
        let text = response
            .into_body()
            .read_to_string()
            .map_err(|e| BackendError::Other(format!("Couldn't read the model list: {e}.")))?;
        if status != 200 {
            return Err(BackendError::Refused(format!(
                "The server at {} refused the model list ({status}).",
                options.server_url
            )));
        }
        Ok(model_ids(&text))
    }

    fn set_options(&self, options: &Options) {
        let changed = {
            let mut current = self.options.lock().unwrap_or_else(|e| e.into_inner());
            let changed = *current != *options;
            *current = options.clone();
            changed
        };
        // The next reply starts the server with the new options (or uses the
        // external one). The old one goes once no reply uses it, leaving the
        // graphics card; waiting for that is the server's thread's, not ours.
        if changed {
            let server = self.server.clone();
            std::thread::spawn(move || server.retire());
        }
    }

    fn models_folder(&self) -> Option<PathBuf> {
        Some(self.models_dir.clone())
    }

    fn context_size(&self) -> Option<u32> {
        self.context
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|(_, n)| *n)
    }

    fn complete(
        &self,
        request: &Request,
        cancel: &AtomicBool,
        emit: &mut dyn FnMut(Event<'_>),
    ) -> Result<(), BackendError> {
        let (endpoint, managed) = self.endpoint(&request.model)?;
        let result = self.stream(&endpoint, request, cancel, emit);
        if managed {
            self.server.release();
        }
        result
    }
}

/// The request with the server's key, when it has one.
pub(crate) fn authorized<B>(
    request: ureq::RequestBuilder<B>,
    endpoint: &Endpoint,
) -> ureq::RequestBuilder<B> {
    if endpoint.api_key.is_empty() {
        request
    } else {
        request.header("Authorization", format!("Bearer {}", endpoint.api_key))
    }
}

/// The JSON body for `/v1/chat/completions`.
pub fn request_body(request: &Request) -> Value {
    let mut messages = Vec::new();
    if !request.system_prompt.trim().is_empty() {
        messages.push(json!({"role": "system", "content": request.system_prompt}));
    }
    for m in &request.messages {
        let mut message = json!({"role": m.role.as_str(), "content": m.text});
        if !m.tool_calls.is_empty() {
            message["tool_calls"] = m
                .tool_calls
                .iter()
                .map(|c| {
                    json!({"id": c.id, "type": "function",
                           "function": {"name": c.name, "arguments": c.arguments}})
                })
                .collect();
        }
        if let Some(id) = &m.tool_call_id {
            message["tool_call_id"] = json!(id);
        }
        messages.push(message);
    }
    let mut body = json!({
        "messages": messages,
        "stream": true,
    });
    if !request.model.is_empty() {
        body["model"] = json!(request.model);
    }
    if let Some(s) = request.sampling {
        body["temperature"] = json!(s.temperature);
        body["top_p"] = json!(s.top_p);
    }
    if !request.tools.is_empty() {
        body["tools"] = json!(request.tools);
    }
    body
}

/// The ids in a `/v1/models` answer.
pub fn model_ids(text: &str) -> Vec<String> {
    let Ok(json) = serde_json::from_str::<Value>(text) else {
        return Vec::new();
    };
    json.get("data")
        .and_then(Value::as_array)
        .map(|models| {
            models
                .iter()
                .filter_map(|m| m.get("id").and_then(Value::as_str).map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// A failed request as the user reads it.
fn http_error(e: ureq::Error) -> BackendError {
    match e {
        ureq::Error::Io(io) if io.kind() == std::io::ErrorKind::ConnectionRefused => {
            BackendError::Unreachable("Nothing answered at the model server's address.".into())
        }
        ureq::Error::Timeout(_) => {
            BackendError::Unreachable("The model server didn't answer in time.".into())
        }
        ureq::Error::HostNotFound => {
            BackendError::Unreachable("The model server's address wasn't found.".into())
        }
        ureq::Error::ConnectionFailed => {
            BackendError::Unreachable("Couldn't connect to the model server.".into())
        }
        other => BackendError::Other(format!("The model server couldn't be reached: {other}.")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::Message;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    fn request() -> Request {
        Request {
            model: "tiny".into(),
            system_prompt: "Be brief.".into(),
            messages: vec![
                Message::user("Hi"),
                Message::assistant("Hello"),
                Message::user("Again"),
            ],
            sampling: None,
            tools: Vec::new(),
        }
    }

    #[test]
    fn the_request_body() {
        let body = request_body(&request());
        assert_eq!(body["stream"], true);
        assert_eq!(body["model"], "tiny");
        let m = body["messages"].as_array().unwrap();
        assert_eq!(m.len(), 4);
        assert_eq!(m[0], json!({"role": "system", "content": "Be brief."}));
        assert_eq!(m[3], json!({"role": "user", "content": "Again"}));
        assert!(body.get("temperature").is_none());
        assert!(body.get("tools").is_none());
        // Agent mode: the tools, a call and its result.
        let mut agent = request();
        agent.tools = crate::tools::schema();
        agent.messages.push(Message {
            tool_calls: vec![ToolCall {
                id: "c1".into(),
                name: "list_dir".into(),
                arguments: "{}".into(),
            }],
            ..Message::assistant("")
        });
        agent.messages.push(Message::tool("c1", "src/"));
        let body = request_body(&agent);
        assert_eq!(
            body["tools"].as_array().unwrap().len(),
            crate::tools::TOOLS.len()
        );
        let m = body["messages"].as_array().unwrap();
        assert_eq!(m[4]["tool_calls"][0]["function"]["name"], "list_dir");
        assert_eq!(
            m[5],
            json!({"role": "tool", "content": "src/", "tool_call_id": "c1"})
        );
        let mut story = request();
        story.sampling = crate::modes::STORY.sampling;
        let body = request_body(&story);
        assert_eq!(body["temperature"], 1.0);
        assert_eq!(
            body["top_p"].as_f64().map(|p| (p * 100.0).round()),
            Some(95.0)
        );
        let mut no_prompt = request();
        no_prompt.system_prompt = "  ".into();
        assert_eq!(
            request_body(&no_prompt)["messages"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
    }

    #[test]
    fn long_conversations_lose_their_oldest_turns() {
        let mut long = request();
        // As a request is: ending with the user's message (q8).
        long.messages = (0..9)
            .map(|i| {
                if i % 2 == 0 {
                    Message::user(format!("q{i}"))
                } else {
                    Message::assistant(format!("a{i}"))
                }
            })
            .collect();
        // 10 tokens a message.
        let count = |r: &Request| Some(r.messages.len() * 10);
        let fitted = trim_to_budget(&long, 45, count);
        assert_eq!(fitted.messages.len(), 3, "{:?}", fitted.messages);
        assert_eq!(fitted.messages[0].role, Role::User, "starts with the user");
        assert_eq!(
            fitted.messages.last().unwrap().text,
            "q8",
            "the last message stays"
        );
        assert_eq!(fitted.system_prompt, long.system_prompt);
        // Fits already, or can't be measured: as it is.
        assert_eq!(trim_to_budget(&long, 1000, count).messages.len(), 9);
        assert_eq!(trim_to_budget(&long, 1, |_| None).messages.len(), 9);
        // Never below the last message, even when it alone is too long.
        assert_eq!(trim_to_budget(&long, 1, count).messages.len(), 1);
    }

    #[test]
    fn room_for_the_reply() {
        assert_eq!(reply_room(4096), 1024);
        assert_eq!(reply_room(32768), 2048);
    }

    #[test]
    fn model_lists() {
        assert_eq!(
            model_ids(r#"{"object":"list","data":[{"id":"qwen3-8b"},{"id":"gemma"}]}"#),
            vec!["qwen3-8b", "gemma"]
        );
        assert!(model_ids("nope").is_empty());
    }

    #[test]
    fn an_agents_task_stays_when_trimmed() {
        let call = |id: &str| ToolCall {
            id: id.into(),
            name: "read_file".into(),
            arguments: "{}".into(),
        };
        let mut agent = request();
        agent.messages = vec![
            Message::user("old question"),
            Message::assistant("old answer"),
            Message::user("Fix the bug"),
            Message {
                tool_calls: vec![call("a")],
                ..Message::assistant("")
            },
            Message::tool("a", "x".repeat(1000)),
            Message {
                tool_calls: vec![call("b")],
                ..Message::assistant("")
            },
            Message::tool("b", "y".repeat(1000)),
        ];
        // Characters as tokens: room for the task and one output.
        let size = |r: &Request| Some(r.messages.iter().map(|m| m.text.len()).sum::<usize>());
        let trimmed = trim_to_budget(&agent, 1200, size);
        let texts: Vec<&str> = trimmed.messages.iter().map(|m| m.text.as_str()).collect();
        assert_eq!(texts[0], "Fix the bug");
        assert_eq!(trimmed.messages.len(), 5);
        // The older output gave way; the newer stays; calls and results pair.
        assert_eq!(trimmed.messages[2].text, TRIMMED_OUTPUT);
        assert_eq!(trimmed.messages[4].text.len(), 1000);
        assert_eq!(trimmed.messages[2].tool_call_id.as_deref(), Some("a"));
    }

    #[test]
    fn automatic_context() {
        assert_eq!(context_for(0, 262_144), AUTO_CONTEXT);
        assert_eq!(context_for(0, 8192), 8192);
        assert_eq!(context_for(0, 0), AUTO_CONTEXT);
        assert_eq!(context_for(131_072, 262_144), 131_072);
    }

    #[test]
    fn the_models_folder() {
        let dir = std::env::temp_dir().join(format!("gates-models-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub.gguf")).unwrap();
        std::fs::write(dir.join("b-model.gguf"), b"GGUF").unwrap();
        std::fs::write(dir.join("A-model.gguf"), b"GGUFGGUF").unwrap();
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();
        std::fs::write(dir.join(".partial.gguf"), b"x").unwrap();
        let models = local_models(&dir);
        let names: Vec<&str> = models.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, vec!["A-model", "b-model"]);
        assert_eq!(models[0].size, 8);
        assert!(local_models(&dir.join("missing")).is_empty());
        // A decision model is in the folder, but not one to chat with.
        let mut laya = b"GGUF".to_vec();
        laya.extend(3u32.to_le_bytes());
        laya.extend(0u64.to_le_bytes());
        laya.extend(1u64.to_le_bytes());
        let key = "modern-bert.decision.type";
        laya.extend((key.len() as u64).to_le_bytes());
        laya.extend(key.as_bytes());
        laya.extend(8u32.to_le_bytes());
        laya.extend(4u64.to_le_bytes());
        laya.extend(b"laya");
        std::fs::write(dir.join("Laya-Q8_0.gguf"), laya).unwrap();
        assert_eq!(local_models(&dir).len(), 3);
        let chat: Vec<String> = chat_models(&dir).into_iter().map(|m| m.name).collect();
        assert_eq!(chat, vec!["A-model", "b-model"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A small HTTP server on a free port: `/v1/chat/completions` gets
    /// `response`, anything else a 404 (so the context is unknown and the
    /// conversation is sent whole). Hands back the chat request it got.
    fn serve_once(response: String) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            for _ in 0..8 {
                let (mut socket, _) = listener.accept().unwrap();
                let request = read_request(&mut socket);
                if request.starts_with("POST /v1/chat/completions") {
                    socket.write_all(response.as_bytes()).unwrap();
                    return request;
                }
                let _ = socket.write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
            }
            String::new()
        });
        (base, handle)
    }

    /// One request: headers, then a body of Content-Length.
    fn read_request(socket: &mut std::net::TcpStream) -> String {
        let mut got = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let n = socket.read(&mut buf).unwrap();
            got.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&got).to_string();
            if let Some(end) = text.find("\r\n\r\n") {
                let length = text
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                    })
                    .unwrap_or(0);
                if got.len() >= end + 4 + length {
                    return text;
                }
            }
            if n == 0 {
                return text;
            }
        }
    }

    fn external(base: &str) -> Llama {
        Llama::new(
            std::env::temp_dir(),
            None,
            std::env::temp_dir().join("gates-test-llama.log"),
            Options {
                server_url: base.to_string(),
                ..Options::default()
            },
        )
    }

    #[test]
    fn tool_calls_stream_in_pieces() {
        let (base, _server) = serve_once(
            concat!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
                "data: {\"choices\":[{\"delta\":{\"content\":\"Let me look.\"}}]}\n\n",
                "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"a\",\"type\":\"function\",\"function\":{\"name\":\"read_file\"}}]}}]}\n\n",
                "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"path\\\":\"}}]}}]}\n\n",
                "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"a.rs\\\"}\"}}]}}]}\n\n",
                "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
                "data: [DONE]\n\n",
            )
            .to_string(),
        );
        let mut text = String::new();
        let mut calls = Vec::new();
        external(&base)
            .complete(&request(), &AtomicBool::new(false), &mut |e| match e {
                Event::Text(t) => text.push_str(t),
                Event::ToolCalls(c) => calls = c.to_vec(),
                Event::Speed(_) => {}
            })
            .unwrap();
        assert_eq!(text, "Let me look.");
        assert_eq!(
            calls,
            vec![ToolCall {
                id: "a".into(),
                name: "read_file".into(),
                arguments: r#"{"path":"a.rs"}"#.into()
            }]
        );
    }

    #[test]
    fn a_stream_that_just_ends_was_cut_off() {
        let (base, _server) = serve_once(
            concat!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
                "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n",
            )
            .to_string(),
        );
        let mut text = String::new();
        let err = external(&base)
            .complete(&request(), &AtomicBool::new(false), &mut |e| {
                if let Event::Text(t) = e {
                    text.push_str(t);
                }
            })
            .unwrap_err();
        assert_eq!(text, "Hel");
        assert!(err.to_string().contains("cut off"), "{err}");
    }

    #[test]
    fn split_parts_and_projectors_are_not_models() {
        assert!(is_model("Qwen3.5-9B-Q4_K_M"));
        assert!(is_model("big-Q4_K_M-00001-of-00003"));
        assert!(!is_model("big-Q4_K_M-00002-of-00003"));
        assert!(!is_model("mmproj-gemma-4-F16"));
        assert!(!is_model("MMPROJ-model"));
        assert!(is_model("one-of-a-kind"));
    }

    #[test]
    fn streams_a_reply_from_a_server() {
        let (base, server) = serve_once(
            concat!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
                "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n",
                "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n",
                "data: {\"choices\":[{\"delta\":{}}],\"timings\":{\"predicted_per_second\":33.5}}\n\n",
                "data: [DONE]\n\n",
            )
            .to_string(),
        );
        let llama = external(&base);
        let mut text = String::new();
        let mut speed = None;
        llama
            .complete(&request(), &AtomicBool::new(false), &mut |e| match e {
                Event::Text(t) => text.push_str(t),
                Event::Speed(s) => speed = Some(s),
                Event::ToolCalls(_) => panic!("no tools asked for"),
            })
            .unwrap();
        assert_eq!(text, "Hello");
        assert_eq!(speed, Some(33.5));
        let sent = server.join().unwrap();
        assert!(sent.starts_with("POST /v1/chat/completions"));
        assert!(sent.contains("\"stream\":true"));
    }

    #[test]
    fn a_refusal_is_explained() {
        let body = r#"{"error":{"message":"the context is too long"}}"#;
        let (base, _server) = serve_once(format!(
            "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ));
        let err = external(&base)
            .complete(&request(), &AtomicBool::new(false), &mut |_| {})
            .unwrap_err()
            .to_string();
        assert!(err.contains("400"), "{err}");
        assert!(err.contains("the context is too long"), "{err}");
    }

    #[test]
    fn nothing_listening() {
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let err = external(&format!("http://127.0.0.1:{port}"))
            .complete(&request(), &AtomicBool::new(false), &mut |_| {})
            .unwrap_err();
        assert!(matches!(err, BackendError::Unreachable(_)), "{err:?}");
    }

    #[test]
    fn without_a_server_or_a_model() {
        let dir = std::env::temp_dir().join(format!("gates-nomodel-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let none = Llama::new(dir.clone(), None, dir.join("log"), Options::default());
        let err = none
            .complete(&request(), &AtomicBool::new(false), &mut |_| {})
            .unwrap_err()
            .to_string();
        assert!(err.contains("telamon-llama"), "{err}");
        let no_model = Llama::new(
            dir.clone(),
            Some(PathBuf::from("/bin/true")),
            dir.join("log"),
            Options::default(),
        );
        let err = no_model
            .complete(&request(), &AtomicBool::new(false), &mut |_| {})
            .unwrap_err()
            .to_string();
        assert!(err.contains("No model yet"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
