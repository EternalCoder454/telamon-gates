//! SystemOne: a small decision model (System 1) that answers typed questions
//! about a message in a fraction of a second, before the chat model (System
//! 2) writes anything. Gates asks it which mode a message wants.
//!
//! It runs on its own llama-server, through llama.cpp's TypeSafe-compatible
//! `/v1/systemone`: Laya (421M, on the processor, no video memory) or Kev
//! (4B, on the graphics card). Its answers are probabilities: one below
//! `MIN_CONFIDENCE` is no answer, and Gates falls back to Chat. Nothing it
//! says reaches the window as text.
//!
//! Every method blocks: call them from a worker thread.

use crate::backend::llama::{LocalModel, authorized};
use crate::backend::server::{Endpoint, Launch, Server};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Below this, a pick is a guess: the conversation stays in Chat. Set from
/// the test set (`examples/systemone-check.rs`, `docs/BACKEND.md`).
pub const MIN_CONFIDENCE: f64 = 0.5;
/// The most of a message the model reads, in characters; Laya keeps 512
/// tokens of it anyway.
const MAX_STATE: usize = 4000;
/// The context and batch it runs with: a prompt is read in one batch.
const CONTEXT: u32 = 2048;
/// How long one question may take once the model is loaded.
const ANSWER: Duration = Duration::from_secs(15);
/// How long its server may take to start: Kev loads in 2 s on the graphics
/// card and 8 s on the processor.
const LOAD: Duration = Duration::from_secs(30);
/// After a failed start, how long SystemOne steps aside (Chat answers)
/// before it tries again, so a broken setup doesn't slow every message.
const BACKOFF: Duration = Duration::from_secs(5 * 60);

/// A `choice` answer: the most likely option and how sure the model is (0
/// when every option is as likely, 1 when certain).
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    pub choice: String,
    pub confidence: f64,
}

/// The question that picks the mode for a message, with each mode's
/// description. Its ids are the modes' (`modes.rs`).
pub fn mode_question() -> Value {
    json!({
        "type": "choice",
        "instructions": "What kind of help does this message ask for?",
        "criteria": {
            "chat": "Anything else: a question, facts, advice, an explanation, a summary, a translation or small talk",
            "story": "Creative writing: a story, a scene, a character, roleplay, a poem or lyrics",
            "code": "Programming: writing, fixing, reviewing or explaining code, commands or scripts",
        }
    })
}

/// The question that judges an agent of the Fleet (a `noul` question: how
/// likely is "yes"). Its state is `done_state`.
pub fn done_question() -> Value {
    json!({
        "type": "noul",
        "instructions": "Did this agent complete its task?",
        "criteria": {
            "true": "The reply says the task is done and what was done",
            "false": "The agent gave up, failed, asked a question instead, or did only part of the task"
        }
    })
}

/// The most of the task and of the agent's last reply `done_question`
/// reads, in characters (Laya keeps 512 tokens of it anyway).
const MAX_TASK: usize = 600;
const MAX_REPLY: usize = 1400;

/// What `done_question` is asked about: the task, and the end of the
/// agent's last reply (where it says what it did).
pub fn done_state(task: &str, reply: &str) -> String {
    format!(
        "Task: {}\n\nThe agent's last reply: {}",
        clip(task.trim(), MAX_TASK),
        tail(reply.trim(), MAX_REPLY)
    )
}

/// A `/v1/systemone` request about `state`.
pub fn request_body(state: &str, questions: Value) -> Value {
    json!({ "state": clip(state, MAX_STATE), "questions": questions })
}

/// The answer to the choice question `id` in a `/v1/systemone` response.
pub fn parse_choice(response: &Value, id: &str) -> Option<Choice> {
    let answer = response.get("answers")?.get(id)?;
    Some(Choice {
        choice: answer.get("choice")?.as_str()?.to_string(),
        confidence: answer.get("confidence")?.as_f64()?.clamp(0.0, 1.0),
    })
}

/// The probability that the `noul` question `id` is answered yes, in a
/// `/v1/systemone` response.
pub fn parse_noul(response: &Value, id: &str) -> Option<f64> {
    let p = response.get("answers")?.get(id)?.get("noul")?.as_f64()?;
    p.is_finite().then(|| p.clamp(0.0, 1.0))
}

/// The last `max` characters of `text`.
fn tail(text: &str, max: usize) -> &str {
    let count = text.chars().count();
    if count <= max {
        return text;
    }
    match text.char_indices().nth(count - max) {
        Some((i, _)) => &text[i..],
        None => text,
    }
}

/// The first `max` characters of `text`.
fn clip(text: &str, max: usize) -> &str {
    match text.char_indices().nth(max) {
        Some((i, _)) => &text[..i],
        None => text,
    }
}

/// A decision model and the server it runs on.
pub struct SystemOne {
    server: Arc<Server>,
    binary: PathBuf,
    model: PathBuf,
    /// "laya", "kev", … (the model's `decision.type`).
    kind: String,
    name: String,
    /// Until when it steps aside after its server failed to start.
    resting: Mutex<Option<Instant>>,
}

impl SystemOne {
    /// SystemOne with `model` (a decision model: its `info.decision` is set),
    /// run by `binary`. The server starts on the first question; its output
    /// goes to `log`.
    pub fn new(binary: &Path, model: &LocalModel, log: PathBuf) -> SystemOne {
        SystemOne {
            server: Server::with_load_limit(log, LOAD),
            binary: binary.to_path_buf(),
            model: model.path.clone(),
            kind: model.info.decision.clone(),
            name: model.name.clone(),
            resting: Mutex::new(None),
        }
    }

    /// The model's file name, without `.gguf`.
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn kind(&self) -> &str {
        &self.kind
    }

    fn launch(&self) -> Launch {
        Launch {
            binary: self.binary.clone(),
            model: self.model.clone(),
            // Laya is small enough for the processor and leaves the
            // graphics card to the chat model; the others fit themselves.
            gpu_layers: (self.kind == "laya").then_some(0),
            context: Some(CONTEXT),
            batch: Some(CONTEXT),
            projector: None,
            small_cache: false,
            // On the processor, half the logical CPUs (up to 16): 321 ms a
            // question for Laya on the i9, against 416 ms with llama.cpp's 8.
            threads: (self.kind == "laya").then(|| {
                let cpus = std::thread::available_parallelism().map_or(8, |n| n.get());
                (cpus / 2).clamp(1, 16) as u32
            }),
            speculative: false,
            draft: None,
            layers: 0,
        }
    }

    /// The mode `text` (the user's message) asks for, or why there's none.
    pub fn pick_mode(&self, text: &str) -> Result<Choice, String> {
        let body = request_body(text, json!({ "mode": mode_question() }));
        let response = self.ask(&body)?;
        parse_choice(&response, "mode").ok_or_else(|| "SystemOne gave no answer.".to_string())
    }

    /// How likely it is that an agent given `task` finished it, going by
    /// its last `reply` (0 to 1), or why there's no answer.
    pub fn judge_done(&self, task: &str, reply: &str) -> Result<f64, String> {
        let body = request_body(&done_state(task, reply), json!({ "done": done_question() }));
        let response = self.ask(&body)?;
        parse_noul(&response, "done").ok_or_else(|| "SystemOne gave no answer.".to_string())
    }

    fn ask(&self, body: &Value) -> Result<Value, String> {
        {
            let resting = self.resting.lock().unwrap_or_else(|e| e.into_inner());
            if resting.is_some_and(|until| Instant::now() < until) {
                return Err("SystemOne is resting after a failed start.".into());
            }
        }
        let endpoint = self.server.acquire(&self.launch()).map_err(|e| {
            *self.resting.lock().unwrap_or_else(|e| e.into_inner()) =
                Some(Instant::now() + BACKOFF);
            e.to_string()
        })?;
        let result = post(&endpoint, body);
        self.server.release();
        result
    }
}

/// One `/v1/systemone` request to the server at `endpoint`.
fn post(endpoint: &Endpoint, body: &Value) -> Result<Value, String> {
    let agent = agent_with_timeout();
    let response = authorized(
        agent.post(format!("{}/v1/systemone", endpoint.base)),
        endpoint,
    )
    .header("Content-Type", "application/json")
    .send(body.to_string())
    .map_err(|e| format!("SystemOne didn't answer: {e}."))?;
    let status = response.status().as_u16();
    let text = response
        .into_body()
        .read_to_string()
        .map_err(|e| format!("SystemOne's answer was cut off: {e}."))?;
    if status != 200 {
        return Err(format!(
            "SystemOne refused the question ({status}): {}",
            crate::backend::sse::error_message(&text)
        ));
    }
    serde_json::from_str::<Value>(&text).map_err(|_| "SystemOne's answer wasn't JSON.".to_string())
}

/// As the chat client's (no proxy: the key stays here), but bounded: a
/// question that takes long is no fast answer.
fn agent_with_timeout() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(2)))
        .timeout_global(Some(ANSWER))
        .http_status_as_error(false)
        .proxy(None)
        .build()
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_request() {
        let body = request_body("Write me a poem", json!({ "mode": mode_question() }));
        assert_eq!(body["state"], "Write me a poem");
        assert_eq!(body["questions"]["mode"]["type"], "choice");
        let criteria = body["questions"]["mode"]["criteria"].as_object().unwrap();
        let ids: Vec<&str> = criteria.keys().map(String::as_str).collect();
        // Every option is a mode.
        for id in &ids {
            assert!(crate::modes::valid_choice(id), "{id}");
        }
        assert_eq!(ids.len(), crate::modes::PICKABLE.len());
        // Never Agent: tools run only when the user asks for them.
        assert!(!ids.contains(&"agent"));
        // A long message is cut on a character boundary.
        let long = "é".repeat(MAX_STATE + 10);
        let body = request_body(&long, json!({}));
        assert_eq!(body["state"].as_str().unwrap().chars().count(), MAX_STATE);
    }

    #[test]
    fn the_answer() {
        let response = json!({
            "model": "laya",
            "answers": {
                "mode": {
                    "type": "choice",
                    "choice": "story",
                    "probabilities": {"chat": 0.1, "story": 0.85, "code": 0.05},
                    "confidence": 0.77
                }
            },
            "usage": {"input_tokens": 80, "output_tokens": 0}
        });
        assert_eq!(
            parse_choice(&response, "mode"),
            Some(Choice {
                choice: "story".into(),
                confidence: 0.77
            })
        );
        assert_eq!(parse_choice(&response, "other"), None);
        assert_eq!(parse_choice(&json!({"error": "x"}), "mode"), None);
    }

    #[test]
    fn the_done_question() {
        let q = done_question();
        assert_eq!(q["type"], "noul");
        assert_eq!(q["instructions"], "Did this agent complete its task?");
        // A long reply is cut from the front: its end says what was done.
        let reply = format!("{}THE END", "x".repeat(5000));
        let state = done_state("Fix the bug", &reply);
        assert!(state.starts_with("Task: Fix the bug"));
        assert!(state.ends_with("THE END"));
        assert!(state.chars().count() < MAX_STATE);
        let long_task = "é".repeat(MAX_TASK + 50);
        assert!(done_state(&long_task, "ok").chars().count() < MAX_TASK + 100);
        let response = json!({"answers": {"done": {"type": "noul", "noul": 0.8123}}});
        assert_eq!(parse_noul(&response, "done"), Some(0.8123));
        let over = json!({"answers": {"done": {"noul": 1.7}}});
        assert_eq!(parse_noul(&over, "done"), Some(1.0));
        assert_eq!(parse_noul(&response, "other"), None);
        let choice = json!({"answers": {"done": {"choice": "a"}}});
        assert_eq!(parse_noul(&choice, "done"), None);
    }

    /// A one-shot server: answers the first request with `status` and
    /// `body`, and hands back the request it got.
    fn serve(status: &str, body: &str) -> (Endpoint, std::thread::JoinHandle<String>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = Endpoint {
            base: format!("http://{}", listener.local_addr().unwrap()),
            api_key: "k3y".into(),
        };
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let handle = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
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
                    if got.len() >= end + 4 + length || n == 0 {
                        socket.write_all(response.as_bytes()).unwrap();
                        return text;
                    }
                }
            }
        });
        (endpoint, handle)
    }

    #[test]
    fn asking_a_server() {
        let (endpoint, seen) = serve(
            "200 OK",
            r#"{"model":"laya","answers":{"done":{"type":"noul","noul":0.91}}}"#,
        );
        let body = request_body(
            &done_state("Add a README", "Added README.md."),
            json!({ "done": done_question() }),
        );
        let response = post(&endpoint, &body).unwrap();
        assert_eq!(parse_noul(&response, "done"), Some(0.91));
        let request = seen.join().unwrap();
        assert!(request.starts_with("POST /v1/systemone"), "{request}");
        assert!(request.contains("Bearer k3y"), "the key is sent");
        assert!(request.contains("Did this agent complete its task?"));
        assert!(request.contains("Added README.md."));

        // A refusal says why, and a reply that isn't JSON is no answer.
        let (endpoint, _) = serve(
            "501 Not Implemented",
            r#"{"error":{"message":"not a decision model"}}"#,
        );
        let err = post(&endpoint, &body).unwrap_err();
        assert!(
            err.contains("501") && err.contains("not a decision model"),
            "{err}"
        );
        let (endpoint, _) = serve("200 OK", "not json");
        assert!(post(&endpoint, &body).unwrap_err().contains("wasn't JSON"));
    }
}
