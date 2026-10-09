//! The llama.cpp backend: replies from `llama-server`, either the one Gates
//! runs itself (telamon-llama's, with a model from the models folder; see
//! `server.rs`) or one already running at an address the user gives.
//!
//! It speaks the OpenAI-compatible API: `GET /v1/models` and a streamed
//! `POST /v1/chat/completions` (`sse.rs`).

use super::server::{Endpoint, Launch, Server};
use super::sse::{self, Line};
use super::{Backend, BackendError, Event, Options, Request};
use crate::conversation::Role;
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
        .build()
        .into()
}

/// A model in the models folder: `name` is the file name without `.gguf`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalModel {
    pub name: String,
    pub path: PathBuf,
    pub size: u64,
}

/// The `.gguf` files in `dir`, by name.
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
            (meta.is_file() && !name.is_empty() && !name.starts_with('.')).then_some(LocalModel {
                name,
                size: meta.len(),
                path,
            })
        })
        .collect();
    models.sort_by_key(|m| m.name.to_lowercase());
    models
}

pub struct Llama {
    models_dir: PathBuf,
    /// The binary Gates starts; None runs only an external server.
    binary: Option<PathBuf>,
    server: Arc<Server>,
    options: Mutex<Options>,
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
        }
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
        let models = local_models(&self.models_dir);
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
            context: (options.context > 0).then_some(options.context),
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
        let body = request_body(request);
        let mut call = agent(Some(Duration::from_secs(10)))
            .post(format!("{}/v1/chat/completions", endpoint.base))
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream");
        if !endpoint.api_key.is_empty() {
            call = call.header("Authorization", format!("Bearer {}", endpoint.api_key));
        }
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
                    Line::Done => return Ok(()),
                }
            }
        }
        Ok(())
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
            return Ok(local_models(&self.models_dir)
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
        // external one): the old one goes now, leaving the graphics card.
        if changed {
            self.server.stop();
        }
    }

    fn models_folder(&self) -> Option<PathBuf> {
        Some(self.models_dir.clone())
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

/// The JSON body for `/v1/chat/completions`.
pub fn request_body(request: &Request) -> Value {
    let mut messages = Vec::new();
    if !request.system_prompt.trim().is_empty() {
        messages.push(json!({"role": "system", "content": request.system_prompt}));
    }
    for m in &request.messages {
        let role = match m.role {
            Role::User => "user",
            Role::Assistant => "assistant",
        };
        messages.push(json!({"role": role, "content": m.text}));
    }
    let mut body = json!({
        "messages": messages,
        "stream": true,
    });
    if !request.model.is_empty() {
        body["model"] = json!(request.model);
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
    fn model_lists() {
        assert_eq!(
            model_ids(r#"{"object":"list","data":[{"id":"qwen3-8b"},{"id":"gemma"}]}"#),
            vec!["qwen3-8b", "gemma"]
        );
        assert!(model_ids("nope").is_empty());
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
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A one-shot HTTP server on a free port: answers one request with
    /// `response` and hands back what it was sent.
    fn serve_once(response: String) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut got = Vec::new();
            let mut buf = [0u8; 4096];
            // Headers, then a body of Content-Length.
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
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            socket.write_all(response.as_bytes()).unwrap();
            String::from_utf8_lossy(&got).to_string()
        });
        (base, handle)
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
