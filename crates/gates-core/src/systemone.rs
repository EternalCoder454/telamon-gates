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
use crate::backend::server::{Launch, Server};
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
        }
    }

    /// The mode `text` (the user's message) asks for, or why there's none.
    pub fn pick_mode(&self, text: &str) -> Result<Choice, String> {
        let body = request_body(text, json!({ "mode": mode_question() }));
        let response = self.ask(&body)?;
        parse_choice(&response, "mode").ok_or_else(|| "SystemOne gave no answer.".to_string())
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
        let result = (|| {
            let agent = agent_with_timeout();
            let response = authorized(
                agent.post(format!("{}/v1/systemone", endpoint.base)),
                &endpoint,
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
            serde_json::from_str::<Value>(&text)
                .map_err(|_| "SystemOne's answer wasn't JSON.".to_string())
        })();
        self.server.release();
        result
    }
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
}
