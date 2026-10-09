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
        .filter(|m| m.info.decision.is_empty() && draft_kind(&m.name).is_none())
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

/// The vision projectors in `dir`: its `mmproj-….gguf` files, which
/// `local_models` leaves out. Reads the folder, not the files.
pub fn local_projectors(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|e| e.metadata().is_ok_and(|m| m.is_file()))
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(is_projector)
        })
        .collect();
    found.sort();
    found
}

/// Whether a file name is a vision projector: `mmproj…` and `.gguf`.
fn is_projector(file: &str) -> bool {
    let file = file.to_ascii_lowercase();
    file.starts_with("mmproj") && file.ends_with(".gguf")
}

/// The projector that lets `model` read images, among `projectors` (from
/// `local_projectors`), or None. `all` is every model in the folder.
/// - **By name:** `mmproj-gemma-3-4b-it-F16.gguf` belongs to
///   `gemma-3-4b-it-Q4_K_M.gguf`: the names are the same before the
///   quantisation.
/// - **By being alone:** a projector with no model name in it
///   (`mmproj-model-f16.gguf`, `mmproj-F16.gguf`) belongs to the only model in
///   the folder.
///
/// With several precisions of one projector, F16 wins, then BF16.
pub fn projector_for(
    model: &LocalModel,
    all: &[LocalModel],
    projectors: &[PathBuf],
) -> Option<PathBuf> {
    let wanted = strip_quant(&model.name.to_ascii_lowercase()).to_string();
    projectors
        .iter()
        .filter_map(|path| {
            let file = path.file_name()?.to_str()?;
            let stem = file.strip_suffix(".gguf").unwrap_or(file).to_lowercase();
            let rest = stem.strip_prefix("mmproj")?;
            let rest = rest.trim_start_matches(['-', '_', '.']);
            let base = strip_quant(rest);
            let fits = if base.is_empty() || base == "model" {
                all.len() == 1
            } else {
                base == wanted
            };
            // F16 first, then BF16, then the rest by name.
            let rank = match stem.rsplit(['-', '.']).next() {
                Some("f16") => 0,
                Some("bf16") => 1,
                _ => 2,
            };
            fits.then(|| (rank, file.to_string(), path.clone()))
        })
        .min()
        .map(|(_, _, path)| path)
}

/// Whether a name part is a precision: `Q4_K_M`, `IQ3_XS`, `F16`, `BF16`.
fn is_quant(part: &str) -> bool {
    let p = part.to_ascii_lowercase();
    ["f16", "bf16", "f32"].contains(&p.as_str())
        || ["q", "iq", "tq"].iter().any(|prefix| {
            p.strip_prefix(prefix)
                .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
        })
}

/// `name` without the precision it ends in ("gemma-3-4b-it-q4_k_m" is
/// "gemma-3-4b-it"), and without Unsloth's "-ud" before it.
fn strip_quant(name: &str) -> &str {
    let mut name = name;
    let mut stripped = false;
    while let Some(cut) = name.rfind(['-', '.']) {
        let tail = &name[cut + 1..];
        if is_quant(tail) || (stripped && tail.eq_ignore_ascii_case("ud")) {
            stripped = true;
            name = &name[..cut];
        } else {
            return name;
        }
    }
    // A bare precision ("f16") is all there was.
    if is_quant(name) { "" } else { name }
}

/// A speculative-decoding draft (by its name without `.gguf`), as ggml-org
/// publishes them beside their model: `dspark-Qwen3-8B-Q8_0` drafts for
/// `Qwen3-8B-*`. Gives its kind; such a file can't chat on its own.
pub fn draft_kind(name: &str) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase();
    if lower.starts_with("dspark-") {
        Some("dspark")
    } else if lower.starts_with("dflash-") {
        Some("dflash")
    } else if lower.contains("eagle3") {
        Some("eagle3")
    } else {
        None
    }
}

/// The draft that speeds `model` up, if one sits beside it: a DSpark draft
/// of the same model, whatever the quantisation of either.
///
/// Measured on the RX 7900 with Qwen3-8B Q8_0, DSpark with n-gram: chat 86
/// → 123 tok/s, a story 85 → 130, rewriting a file 82 → 826. DFlash drafts
/// slowed chat (8% of their tokens accepted), so they aren't used.
pub fn draft_for(model: &LocalModel, all: &[LocalModel]) -> Option<PathBuf> {
    let wanted = strip_quant(&model.name.to_ascii_lowercase()).to_string();
    all.iter()
        .filter(|m| draft_kind(&m.name) == Some("dspark"))
        .find(|m| {
            let lower = m.name.to_ascii_lowercase();
            let base = lower.trim_start_matches("dspark-");
            !wanted.is_empty() && strip_quant(base) == wanted
        })
        .map(|m| m.path.clone())
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
/// gives way to a short note. The last user message, the newest tool
/// output, and every call with its result, stay. Unmeasurable (`count`
/// gives None): as it is.
///
/// Once over, it trims to three quarters of `budget`, not just under it:
/// the server keeps what it read of the conversation and reads only what
/// changed, but a cut at the start changes everything after it. With room
/// to spare, the next messages keep the same start and come from the cache
/// instead of the whole conversation being read again for each.
pub fn trim_to_budget(
    request: &Request,
    budget: usize,
    mut count: impl FnMut(&Request) -> Option<usize>,
) -> Request {
    let mut trimmed = request.clone();
    let mut limit = budget;
    // Bounded: each round drops a message or shortens an output.
    loop {
        match count(&trimmed) {
            Some(n) if n > limit => limit = budget * 3 / 4,
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
        let newest = trimmed.messages.iter().rposition(|m| m.role == Role::Tool);
        let oldest = trimmed
            .messages
            .iter_mut()
            .enumerate()
            .find(|(i, m)| m.role == Role::Tool && m.text != TRIMMED_OUTPUT && Some(*i) != newest)
            .map(|(_, m)| m);
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
    fn endpoint(&self, model: &str, cancel: &AtomicBool) -> Result<(Endpoint, bool), BackendError> {
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
        let all_models = local_models(&self.models_dir);
        // Its image projector, when one sits beside it: it reads pictures.
        let projector = projector_for(chosen, &models, &local_projectors(&self.models_dir));
        let launch = Launch {
            binary,
            model: chosen.path.clone(),
            gpu_layers: (options.gpu_layers > 0).then_some(options.gpu_layers),
            context: Some(context_for(options.context, chosen.info.context_length)),
            batch: None,
            projector,
            small_cache: options.small_cache,
            threads: None,
            speculative: true,
            draft: draft_for(chosen, &all_models),
        };
        Ok((self.server.acquire_until(&launch, cancel)?, true))
    }

    fn stream(
        &self,
        endpoint: &Endpoint,
        request: &Request,
        cancel: &AtomicBool,
        emit: &mut dyn FnMut(Event<'_>),
    ) -> Result<(), BackendError> {
        let body = request_body(&self.fit(endpoint, request));
        if cancel.load(Ordering::Relaxed) {
            return Ok(());
        }
        // Our own client: Stop shuts its connection, also while the server
        // is still reading the prompt, and the server drops the work.
        let bearer = format!("Bearer {}", endpoint.api_key);
        let mut headers = vec![
            ("Content-Type", "application/json"),
            ("Accept", "text/event-stream"),
        ];
        if !endpoint.api_key.is_empty() {
            headers.push(("Authorization", bearer.as_str()));
        }
        let mut response = super::stream::post(
            &format!("{}/v1/chat/completions", endpoint.base),
            &headers,
            &body.to_string(),
        )
        .map_err(io_error)?;
        if response.status != 200 {
            let status = response.status;
            let text = response.text();
            return Err(BackendError::Refused(format!(
                "The model server refused the request ({status}): {}",
                sse::error_message(&text)
            )));
        }
        response.until_stopped(cancel, |response| Self::read_events(response, cancel, emit))
    }

    /// The events of a streamed answer, to `[DONE]`.
    fn read_events(
        response: &mut super::stream::Response,
        cancel: &AtomicBool,
        emit: &mut dyn FnMut(Event<'_>),
    ) -> Result<(), BackendError> {
        // Tool calls come in pieces, by index; they go out whole at the end.
        let mut calls: Vec<ToolCall> = Vec::new();
        // Until `[DONE]`: a stream that just ends was cut off (the server
        // crashed or the network went).
        loop {
            if cancel.load(Ordering::Relaxed) {
                // The connection is shut: the server stops.
                return Ok(());
            }
            let line = match response.line() {
                Ok(Some(line)) => line,
                Ok(None) => break,
                Err(_) if cancel.load(Ordering::Relaxed) => return Ok(()),
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    return Err(BackendError::Unreachable(
                        "The model server stopped answering.".into(),
                    ));
                }
                Err(e) => return Err(BackendError::Other(format!("The reply was cut off: {e}."))),
            };
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

    fn warm(&self, model: &str) {
        // The same server the reply will ask for, started now; released at
        // once, so the idle stop still counts from here.
        if let Ok((_, true)) = self.endpoint(model, &AtomicBool::new(false)) {
            self.server.release();
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
        let (endpoint, managed) = self.endpoint(&request.model, cancel)?;
        // Released however the reply ends, a panic included: else the idle
        // stop never comes and the model stays in the graphics card.
        struct Release<'a>(Option<&'a Server>);
        impl Drop for Release<'_> {
            fn drop(&mut self) {
                if let Some(server) = self.0 {
                    server.release();
                }
            }
        }
        let _release = Release(managed.then_some(&*self.server));
        self.stream(&endpoint, request, cancel, emit)
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
    let store = crate::store::attachments_dir();
    for m in &request.messages {
        let mut message = json!({"role": m.role.as_str(), "content": m.text});
        if !m.attachments.is_empty() {
            // Your text with each text file; pictures as parts beside it.
            let text = crate::attach::text_for_model(&m.text, &m.attachments);
            let images = crate::attach::image_urls(&m.attachments, &store);
            message["content"] = if images.is_empty() {
                json!(text)
            } else {
                let mut parts = vec![json!({"type": "text", "text": text})];
                parts.extend(
                    images
                        .into_iter()
                        .map(|url| json!({"type": "image_url", "image_url": {"url": url}})),
                );
                json!(parts)
            };
        }
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
    if let Some(format) = &request.response_format {
        body["response_format"] = format.clone();
    }
    if request.brief {
        // Each model reads its own: gpt-oss the effort, Qwen3 the template
        // switch. Other templates ignore both.
        body["reasoning_effort"] = json!("low");
        body["chat_template_kwargs"] = json!({"enable_thinking": false});
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
/// What a failed connection means, for the window.
fn io_error(e: std::io::Error) -> BackendError {
    use std::io::ErrorKind::*;
    match e.kind() {
        ConnectionRefused => {
            BackendError::Unreachable("Nothing answered at the model server's address.".into())
        }
        TimedOut | WouldBlock => {
            BackendError::Unreachable("The model server didn't answer in time.".into())
        }
        NotFound => BackendError::Unreachable("The model server's address wasn't found.".into()),
        _ => BackendError::Other(format!("The model server couldn't be reached: {e}.")),
    }
}

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
            response_format: None,
            brief: false,
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
        assert!(body.get("response_format").is_none());
        assert!(body.get("reasoning_effort").is_none());
        assert!(body.get("chat_template_kwargs").is_none());
        // Chat and Story: short reasoning, in both models' words.
        let mut brief = request();
        brief.brief = true;
        let body = request_body(&brief);
        assert_eq!(body["reasoning_effort"], "low");
        assert_eq!(
            body["chat_template_kwargs"],
            json!({"enable_thinking": false})
        );
        // A constrained reply (the Fleet's plan): the format goes as given.
        let mut plan = request();
        plan.response_format = Some(json!({"type": "json_object", "schema": {"type": "object"}}));
        assert_eq!(
            request_body(&plan)["response_format"],
            json!({"type": "json_object", "schema": {"type": "object"}})
        );
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
        // Room to spare once trimmed: 11 messages at budget 100 trim to 7
        // (70, under 75), not 9 (90, just under 100).
        long.messages.insert(0, Message::assistant("a-1"));
        long.messages.insert(0, Message::user("q-2"));
        assert_eq!(trim_to_budget(&long, 100, count).messages.len(), 7);
        long.messages.drain(..2);
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
    fn drafts_pair_with_their_model() {
        let m = |name: &str| LocalModel {
            name: name.into(),
            path: PathBuf::from(format!("/m/{name}.gguf")),
            size: 1,
            info: Default::default(),
        };
        let all = vec![
            m("Qwen3-8B-Q4_K_M"),
            m("dspark-Qwen3-8B-Q8_0"),
            m("dflash-Qwen3-8B-Q8_0"),
            m("Qwen3-4B-Q4_K_M"),
        ];
        assert_eq!(
            draft_for(&all[0], &all),
            Some(PathBuf::from("/m/dspark-Qwen3-8B-Q8_0.gguf"))
        );
        assert_eq!(draft_for(&all[3], &all), None);
        assert_eq!(draft_kind("dspark-Qwen3-8B-Q8_0"), Some("dspark"));
        assert_eq!(draft_kind("Qwen3-8B-Q8_0"), None);
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

    fn model(name: &str) -> LocalModel {
        LocalModel {
            name: name.to_string(),
            path: PathBuf::from(format!("/m/{name}.gguf")),
            size: 1,
            info: Default::default(),
        }
    }

    fn projectors(names: &[&str]) -> Vec<PathBuf> {
        names
            .iter()
            .map(|n| PathBuf::from(format!("/m/{n}")))
            .collect()
    }

    #[test]
    fn a_projector_belongs_to_the_model_with_its_name() {
        let gemma = model("gemma-3-4b-it-Q4_K_M");
        let qwen = model("Qwen3.5-9B-Q4_K_M");
        let all = [gemma.clone(), qwen.clone()];
        let found = projectors(&[
            "mmproj-gemma-3-4b-it-F16.gguf",
            "mmproj-Qwen3.5-9B-BF16.gguf",
        ]);
        assert_eq!(
            projector_for(&gemma, &all, &found),
            Some(PathBuf::from("/m/mmproj-gemma-3-4b-it-F16.gguf"))
        );
        assert_eq!(
            projector_for(&qwen, &all, &found),
            Some(PathBuf::from("/m/mmproj-Qwen3.5-9B-BF16.gguf"))
        );
        // Another quantisation of the same model uses it too; another model
        // does not (4b is not 12b).
        assert!(projector_for(&model("gemma-3-4b-it-Q8_0"), &all, &found).is_some());
        assert_eq!(
            projector_for(&model("gemma-3-12b-it-Q4_K_M"), &all, &found),
            None
        );
        assert_eq!(projector_for(&gemma, &all, &[]), None);
    }

    #[test]
    fn a_nameless_projector_belongs_to_a_lone_model() {
        let found = projectors(&["mmproj-model-f16.gguf"]);
        let lone = model("anything-Q4_K_M");
        assert_eq!(
            projector_for(&lone, std::slice::from_ref(&lone), &found),
            Some(PathBuf::from("/m/mmproj-model-f16.gguf"))
        );
        // With two models there is no telling whose it is.
        let two = [lone.clone(), model("other-Q4_K_M")];
        assert_eq!(projector_for(&lone, &two, &found), None);
        let bare = projectors(&["mmproj-F16.gguf"]);
        assert!(projector_for(&lone, std::slice::from_ref(&lone), &bare).is_some());
        // A projector named for another model is not taken by a lone one.
        let named = projectors(&["mmproj-gemma-3-4b-it-F16.gguf"]);
        assert_eq!(
            projector_for(&lone, std::slice::from_ref(&lone), &named),
            None
        );
    }

    #[test]
    fn the_best_precision_of_a_projector_wins() {
        let m = model("gemma-3-4b-it-Q4_K_M");
        let found = projectors(&[
            "mmproj-gemma-3-4b-it-Q8_0.gguf",
            "mmproj-gemma-3-4b-it-BF16.gguf",
            "mmproj-gemma-3-4b-it-F16.gguf",
        ]);
        let all = std::slice::from_ref(&m);
        assert_eq!(
            projector_for(&m, all, &found),
            Some(PathBuf::from("/m/mmproj-gemma-3-4b-it-F16.gguf"))
        );
        assert_eq!(
            projector_for(&m, all, &found[..2]),
            Some(PathBuf::from("/m/mmproj-gemma-3-4b-it-BF16.gguf"))
        );
    }

    #[test]
    fn names_lose_their_precision() {
        assert_eq!(strip_quant("gemma-3-4b-it-q4_k_m"), "gemma-3-4b-it");
        assert_eq!(strip_quant("model.q8_0"), "model");
        assert_eq!(strip_quant("qwen3-vl-ud-q4_k_xl"), "qwen3-vl");
        assert_eq!(strip_quant("llama-3.1-8b"), "llama-3.1-8b");
        assert_eq!(strip_quant("f16"), "");
        assert_eq!(strip_quant("qwen"), "qwen");
    }

    #[test]
    fn projectors_are_found_in_the_folder() {
        let dir = std::env::temp_dir().join(format!("gates-mmproj-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for name in [
            "mmproj-a-F16.gguf",
            "MMPROJ-b.gguf",
            "a-Q4_K_M.gguf",
            "mmproj-c.txt",
            ".mmproj-d.gguf",
        ] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        let names: Vec<String> = local_projectors(&dir)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["MMPROJ-b.gguf", "mmproj-a-F16.gguf"]);
        let _ = std::fs::remove_dir_all(&dir);
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
