//! The built-in demo backend: answers with sample Markdown, streamed a word
//! at a time, so the window can be used and tested before a real backend is
//! connected. Nothing leaves the computer.

use super::{Backend, BackendError, Event, Request};
use crate::conversation::Role;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub struct Demo {
    /// Before the first piece, as a model takes to start answering.
    pub think: Duration,
    /// Between two pieces of text.
    pub delay: Duration,
}

impl Default for Demo {
    fn default() -> Self {
        Demo {
            think: Duration::from_millis(700),
            delay: Duration::from_millis(25),
        }
    }
}

impl Demo {
    /// The whole reply to `request`.
    pub fn reply(request: &Request) -> String {
        let asked = request
            .messages
            .iter()
            .rev()
            .find(|m| m.role == Role::User)
            .map(|m| m.text.trim())
            .unwrap_or("");
        let quoted: String = asked.lines().take(3).map(|l| format!("> {l}\n")).collect();
        let replies = request
            .messages
            .iter()
            .filter(|m| m.role == Role::Assistant && !m.text.is_empty())
            .count();
        format!(
            "This is the **demo backend**: Telamon Gates isn't connected to a model yet, \
             so this is a sample reply to your message:\n\n{quoted}\n\
             It has the things a real reply can have:\n\n\
             - a list, with `inline code`\n\
             - a [link](https://github.com/ggml-org/llama.cpp)\n\
             - and a code block:\n\n\
             ```rust\n\
             fn main() {{\n    println!(\"Hello from Telamon Gates\");\n}}\n\
             ```\n\n\
             Replies so far in this conversation: {replies}. Connect a backend in \
             `crates/gates-core/src/backend` (see `docs/BACKEND.md`) to talk to a real model."
        )
    }
}

impl Backend for Demo {
    fn name(&self) -> String {
        "Demo (no model connected)".to_string()
    }

    fn is_demo(&self) -> bool {
        true
    }

    fn models(&self) -> Result<Vec<String>, BackendError> {
        Ok(vec!["Demo".to_string()])
    }

    fn complete(
        &self,
        request: &Request,
        cancel: &AtomicBool,
        emit: &mut dyn FnMut(Event<'_>),
    ) -> Result<(), BackendError> {
        let reply = Demo::reply(request);
        // Thinking, in short steps so Stop is quick.
        let mut waited = Duration::ZERO;
        while waited < self.think {
            if cancel.load(Ordering::Relaxed) {
                return Ok(());
            }
            let step = (self.think - waited).min(Duration::from_millis(50));
            std::thread::sleep(step);
            waited += step;
        }
        // Word by word, each with the space before it, as a model streams.
        let mut start = 0;
        for (i, c) in reply.char_indices() {
            if c == ' ' && i > start {
                if cancel.load(Ordering::Relaxed) {
                    return Ok(());
                }
                emit(Event::Text(&reply[start..i]));
                start = i;
                std::thread::sleep(self.delay);
            }
        }
        if !cancel.load(Ordering::Relaxed) {
            emit(Event::Text(&reply[start..]));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::Message;

    fn request() -> Request {
        Request {
            model: "Demo".into(),
            system_prompt: String::new(),
            messages: vec![Message::user("What is Rust?")],
            sampling: None,
            tools: Vec::new(),
            response_format: None,
            brief: false,
        }
    }

    #[test]
    fn streams_the_whole_reply_in_pieces() {
        let demo = Demo {
            think: Duration::ZERO,
            delay: Duration::ZERO,
        };
        let mut pieces = Vec::new();
        demo.complete(&request(), &AtomicBool::new(false), &mut |e| {
            if let Event::Text(t) = e {
                pieces.push(t.to_string())
            }
        })
        .unwrap();
        assert!(pieces.len() > 10);
        assert_eq!(pieces.concat(), Demo::reply(&request()));
        assert!(pieces.concat().contains("> What is Rust?"));
    }

    #[test]
    fn stops_when_cancelled() {
        let demo = Demo {
            think: Duration::ZERO,
            delay: Duration::ZERO,
        };
        let cancel = AtomicBool::new(false);
        let mut n = 0;
        demo.complete(&request(), &cancel, &mut |_| {
            n += 1;
            if n == 3 {
                cancel.store(true, Ordering::Relaxed);
            }
        })
        .unwrap();
        assert_eq!(n, 3);
    }
}
