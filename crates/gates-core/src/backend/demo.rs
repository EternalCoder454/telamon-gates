//! The built-in demo backend: answers with sample Markdown, streamed a word
//! at a time, so the window can be used and tested before a real backend is
//! connected. Nothing leaves the computer. When the web tools are offered it
//! plays a model that uses them (a search, a page, then an answer), so
//! their rows and progress can be seen.

use super::{Backend, BackendError, Event, Request};
use crate::conversation::{Role, ToolCall};
use serde_json::json;
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

/// What the demo does next when the web tools are offered.
enum Step {
    Search(String),
    Fetch(String),
    Answer(Vec<String>),
}

impl Demo {
    /// The step of a web tool loop `request` is at; None when the web tools
    /// aren't offered.
    fn web_step(request: &Request) -> Option<Step> {
        let offered = |name: &str| {
            request
                .tools
                .iter()
                .any(|t| t["function"]["name"].as_str() == Some(name))
        };
        if !offered("web_search") {
            return None;
        }
        let user = request
            .messages
            .iter()
            .rposition(|m| m.role == Role::User)?;
        let results: Vec<&str> = request.messages[user..]
            .iter()
            .filter(|m| m.role == Role::Tool)
            .map(|m| m.text.as_str())
            .collect();
        let urls = results
            .first()
            .map(|t| crate::web::session::urls_in(t))
            .unwrap_or_default();
        Some(match results.len() {
            0 => Step::Search(
                request.messages[user]
                    .text
                    .lines()
                    .next()
                    .unwrap_or("")
                    .chars()
                    .take(80)
                    .collect(),
            ),
            1 if !urls.is_empty() => Step::Fetch(urls[0].clone()),
            _ => Step::Answer(urls),
        })
    }

    /// `text`, streamed a word at a time. False when stopped.
    fn stream(&self, text: &str, cancel: &AtomicBool, emit: &mut dyn FnMut(Event<'_>)) -> bool {
        let mut start = 0;
        for (i, c) in text.char_indices() {
            if c == ' ' && i > start {
                if cancel.load(Ordering::Relaxed) {
                    return false;
                }
                emit(Event::Text(&text[start..i]));
                start = i;
                std::thread::sleep(self.delay);
            }
        }
        if !cancel.load(Ordering::Relaxed) {
            emit(Event::Text(&text[start..]));
        }
        true
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
        let step = Demo::web_step(request);
        let reply = match &step {
            Some(Step::Search(query)) => {
                format!("I'll search the web for that. (The demo backend pretends: \"{query}\".)")
            }
            Some(Step::Fetch(_)) => "Let me read the first page.".to_string(),
            Some(Step::Answer(urls)) => {
                let links: String = urls
                    .iter()
                    .take(3)
                    .map(|u| format!("- [{}]({u})\n", crate::tools::short_url(u)))
                    .collect();
                format!(
                    "Here is what the **demo** found. None of it is real: the demo backend \
                     makes up its results and nothing was fetched from the internet.\n\n\
                     Sources:\n\n{links}"
                )
            }
            None => Demo::reply(request),
        };
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
        if !self.stream(&reply, cancel, emit) || cancel.load(Ordering::Relaxed) {
            return Ok(());
        }
        // A web step ends by asking for a tool.
        let call = match step {
            Some(Step::Search(query)) => Some(("web_search", json!({"query": query}))),
            Some(Step::Fetch(url)) => Some(("fetch_page", json!({"url": url}))),
            _ => None,
        };
        if let Some((name, arguments)) = call {
            let call = ToolCall {
                id: format!("demo-{}", request.messages.len()),
                name: name.to_string(),
                arguments: arguments.to_string(),
            };
            emit(Event::ToolCalls(&[call]));
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

    #[test]
    fn plays_a_model_that_searches_the_web() {
        use crate::agent::{self, Approval, Host, Tools};
        use crate::conversation::Message;
        use crate::web::{Canned, Session};

        struct Rows(Vec<String>);
        impl Host for Rows {
            fn text(&mut self, _: &str) {}
            fn speed(&mut self, _: f64) {}
            fn calls(&mut self, _: &[crate::conversation::ToolCall]) {}
            fn approve(&mut self, _: &crate::conversation::ToolCall, _: &str, _: &str) -> Approval {
                Approval::Deny
            }
            fn result(&mut self, m: Message) {
                self.0.push(m.summary.unwrap_or_default());
            }
            fn next_turn(&mut self) {}
        }

        let demo = Demo {
            think: Duration::ZERO,
            delay: Duration::ZERO,
        };
        let canned = Canned {
            delay: Duration::ZERO,
        };
        let session = Session::new(&canned, &request().messages);
        let tools = Tools {
            workspace: None,
            web: Some(&session),
            max_steps: agent::WEB_STEPS,
        };
        let mut rows = Rows(Vec::new());
        agent::run_tools(&demo, request(), &tools, &AtomicBool::new(false), &mut rows).unwrap();
        // A search, then the first page; the answer follows.
        assert_eq!(rows.0.len(), 2, "{:?}", rows.0);
        assert!(rows.0[0].starts_with("Searched for \"What is Rust?\" (3 results)"));
        assert!(rows.0[1].starts_with("Read example.org/demo/1"));
        // Without the tools, nothing changes.
        assert!(Demo::reply(&request()).contains("demo backend"));
    }
}
