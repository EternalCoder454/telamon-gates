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

/// What the demo writes in Agent mode (`Demo::code_step`).
const DEMO_SCRIPT: &str = "def greet(name):\n    print(\"Hello, \" + name)\n\n\ngreet(\"world\")\n";

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

    /// The reply to one of Deep Research's three kinds of request (its plan,
    /// its notes, its report), made up from what the request holds; None for
    /// any other.
    fn research_reply(request: &Request) -> Option<String> {
        use crate::research::{NOTES_PROMPT, REPORT_RULES};
        let user = request.messages.last().map(|m| m.text.as_str())?;
        if request.response_format.is_some() {
            let asked = user.strip_prefix("Question: ").unwrap_or(user);
            let topic: String = asked
                .lines()
                .next()
                .unwrap_or("")
                .trim_end_matches('?')
                .chars()
                .take(60)
                .collect();
            return Some(
                json!({"questions": [
                    format!("What is known about {topic}?"),
                    format!("What are the main points of view on {topic}?"),
                    format!("What do recent sources say about {topic}?"),
                ]})
                .to_string(),
            );
        }
        if request.system_prompt == NOTES_PROMPT {
            let page = user
                .split_once("Pages:")
                .and_then(|(_, pages)| pages.split_once('['))
                .and_then(|(_, rest)| rest.split_once(']'))
                .map_or("1", |(n, _)| n);
            return Some(format!(
                "- A made-up fact from the page [{page}].\n- The demo backend wrote these notes; \
                 nothing was read from the internet."
            ));
        }
        if request.system_prompt.ends_with(REPORT_RULES) {
            let k = user
                .split_once("Sources (cite by number):")
                .map_or(0, |(_, list)| {
                    list.lines().filter(|l| l.starts_with('[')).count()
                });
            let cite = |n: usize| {
                if n <= k {
                    format!(" [{n}]")
                } else {
                    String::new()
                }
            };
            return Some(format!(
                "# Report from the demo\n\n## Summary\n\nThis report is made up by the demo \
                 backend: its pages, notes and sources are not real{}.\n\n## Findings\n\n\
                 - The plan split the question into three parts and each was searched in turn{}.\n\
                 - Pages were read and short notes taken from each{}.\n\n## Limits\n\n\
                 Nothing here came from the internet, so none of it should be relied on.\n",
                cite(1),
                cite(2),
                cite(3)
            ));
        }
        None
    }

    /// The step of a coding session `request` is at, when the workspace
    /// tools are offered (Agent mode): the demo writes a small Python
    /// script, rewrites it in two edits and runs it, so the workspace
    /// panel's live edits and console can be seen. The text to say, and the
    /// tools to ask for (none when it is done).
    fn code_step(request: &Request) -> Option<(String, Vec<ToolCall>)> {
        let offered = |name: &str| {
            request
                .tools
                .iter()
                .any(|t| t["function"]["name"].as_str() == Some(name))
        };
        if !offered("edit_file") || !offered("run_command") {
            return None;
        }
        let user = request
            .messages
            .iter()
            .rposition(|m| m.role == Role::User)?;
        let results = request.messages[user..]
            .iter()
            .filter(|m| m.role == Role::Tool)
            .count();
        let call = |n: usize, name: &str, arguments: serde_json::Value| ToolCall {
            id: format!("demo-code-{}-{n}", request.messages.len()),
            name: name.to_string(),
            arguments: arguments.to_string(),
        };
        // The two edits of the second turn are two results.
        Some(match results {
            0 => (
                "I'll start with a small script. (The demo backend pretends: it writes the \
                 same file whatever you ask.)"
                    .to_string(),
                vec![call(
                    0,
                    "write_file",
                    json!({"path": "hello.py", "content": DEMO_SCRIPT}),
                )],
            ),
            1 => (
                "Now it greets several times, and says when it is done.".to_string(),
                vec![
                    call(
                        0,
                        "edit_file",
                        json!({
                            "path": "hello.py",
                            "old_text": "def greet(name):\n    print(\"Hello, \" + name)\n",
                            "new_text": "def greet(name, times=1):\n    for i in range(times):\n        print(f\"Hello, {name}! ({i + 1})\")\n",
                        }),
                    ),
                    call(
                        1,
                        "edit_file",
                        json!({
                            "path": "hello.py",
                            "old_text": "greet(\"world\")\n",
                            "new_text": "greet(\"Telamon\", times=3)\n\nprint(\"Done\")\n",
                        }),
                    ),
                ],
            ),
            3 => (
                "Let me run it.".to_string(),
                vec![call(
                    0,
                    "run_command",
                    json!({"command": "python3 hello.py"}),
                )],
            ),
            _ => (
                "Done: `hello.py` greets three times. This was the **demo backend**, so none of \
                 it came from a model."
                    .to_string(),
                Vec::new(),
            ),
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
        let code = Demo::code_step(request);
        let step = if code.is_some() {
            None
        } else {
            Demo::web_step(request)
        };
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
            None => code
                .as_ref()
                .map(|(text, _)| text.clone())
                .or_else(|| Demo::research_reply(request))
                .unwrap_or_else(|| Demo::reply(request)),
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
        if let Some((_, calls)) = code
            && !calls.is_empty()
        {
            emit(Event::ToolCalls(&calls));
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
        assert!(rows.0[1].starts_with("Read example.org/demo/what-is-rust/1"));
        // Without the tools, nothing changes.
        assert!(Demo::reply(&request()).contains("demo backend"));
    }

    #[test]
    fn plays_a_model_that_edits_and_runs_code() {
        use crate::agent::{self, Approval, Host};
        use crate::conversation::{Message, ToolCall};
        use crate::tools::Workspace;
        use crate::workbench::Touch;

        #[derive(Default)]
        struct Panel {
            touched: Vec<Touch>,
            results: Vec<String>,
        }
        impl Host for Panel {
            fn text(&mut self, _: &str) {}
            fn speed(&mut self, _: f64) {}
            fn calls(&mut self, _: &[ToolCall]) {}
            fn approve(&mut self, _: &ToolCall, _: &str, _: &str) -> Approval {
                Approval::Allow
            }
            fn result(&mut self, m: Message) {
                self.results.push(m.summary.unwrap_or_default());
            }
            fn next_turn(&mut self) {}
            fn touched(&mut self, touch: Touch) {
                self.touched.push(touch);
            }
        }

        let dir = std::env::temp_dir().join(format!("gates-demo-code-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ws = Workspace::open(&dir).unwrap();
        let demo = Demo {
            think: Duration::ZERO,
            delay: Duration::ZERO,
        };
        let mut panel = Panel::default();
        agent::run(&demo, request(), &ws, &AtomicBool::new(false), &mut panel).unwrap();
        // A file written, two edits of it, a command.
        let edits: Vec<_> = panel
            .touched
            .iter()
            .filter_map(|t| match t {
                Touch::Edited(c) => Some(c),
                Touch::Read(_) => None,
            })
            .collect();
        assert_eq!(edits.len(), 3, "{:?}", panel.results);
        assert!(edits[0].created);
        assert!(!edits[1].diff.marks.is_empty() && !edits[2].diff.marks.is_empty());
        assert_eq!(panel.results.len(), 4, "{:?}", panel.results);
        let script = std::fs::read_to_string(dir.join("hello.py")).unwrap();
        assert!(
            script.contains("times=3") && script.contains("Done"),
            "{script}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn plays_a_whole_deep_research_run() {
        use crate::agent::{Approval, Host};
        use crate::conversation::{Message, ToolCall};
        use crate::research::{self, Limits};
        use crate::web::Canned;

        #[derive(Default)]
        struct Rows {
            statuses: Vec<String>,
            replaced: Option<String>,
        }
        impl Host for Rows {
            fn text(&mut self, _: &str) {}
            fn speed(&mut self, _: f64) {}
            fn calls(&mut self, _: &[ToolCall]) {}
            fn approve(&mut self, _: &ToolCall, _: &str, _: &str) -> Approval {
                Approval::Deny
            }
            fn result(&mut self, _: Message) {}
            fn next_turn(&mut self) {}
            fn status(&mut self, line: &str) {
                self.statuses.push(line.to_string());
            }
            fn replace(&mut self, text: &str) {
                self.replaced = Some(text.to_string());
            }
        }

        let demo = Demo {
            think: Duration::ZERO,
            delay: Duration::ZERO,
        };
        let canned = Canned {
            delay: Duration::ZERO,
        };
        let mut rows = Rows::default();
        research::run(
            &demo,
            &canned,
            request(),
            Limits::default(),
            &AtomicBool::new(false),
            &mut rows,
        )
        .unwrap();
        let report = rows.replaced.unwrap();
        // Cited, linked to pages that were read, with a list of them.
        assert!(
            report.contains("[\\[1\\]](https://example.org/demo/"),
            "{report}"
        );
        let sources = report.split("## Sources\n\n").nth(1).unwrap();
        assert!(
            sources.lines().filter(|l| !l.is_empty()).count() >= 6,
            "{sources}"
        );
        assert_eq!(
            rows.statuses.first().map(String::as_str),
            Some("Planning the research")
        );
        assert_eq!(
            rows.statuses.last().map(String::as_str),
            Some("Writing the report")
        );
    }
}
