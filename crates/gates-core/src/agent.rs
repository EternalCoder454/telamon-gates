//! Agent mode's loop: the model answers with tool calls, Gates runs them
//! (asking the user first for any that change something), sends the
//! results back, and the model goes on, until it answers without a tool
//! or `MAX_STEPS` pass. llama-server keeps the conversation's prefix in its
//! cache, so each step costs only what is new.
//!
//! Blocks (the model, the tools, the user's answer): call it from a worker.

use crate::backend::{Backend, BackendError, Event, Request};
use crate::conversation::{Message, ToolCall};
use crate::tools::{self, Effect, Outcome, Workspace};
use std::sync::atomic::{AtomicBool, Ordering};

/// Model turns in one reply at most: past this, it is going round.
pub const MAX_STEPS: usize = 25;

/// The user's answer to a tool that changes something.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Approval {
    Deny,
    Allow,
    /// Allow this and every later edit in this reply (commands still ask).
    AllowEdits,
}

/// What the loop tells the window, in order.
pub trait Host {
    /// A piece of the model's text, in the turn under way.
    fn text(&mut self, piece: &str);
    fn speed(&mut self, speed: f64);
    /// The turn under way ended by asking for `calls`.
    fn calls(&mut self, calls: &[ToolCall]);
    /// May `call` run? `title` and `detail` say what it does.
    fn approve(&mut self, call: &ToolCall, title: &str, detail: &str) -> Approval;
    /// A tool ran (or didn't): its result, as it goes to the model.
    fn result(&mut self, message: Message);
    /// The model starts its next turn, after the results.
    fn next_turn(&mut self);
}

/// `messages` made safe to send: every tool call answered by a result
/// right after it. A reply stopped half-way can leave calls without
/// results, which servers refuse; those get "Stopped before it ran.", and
/// results whose call is gone are left out.
pub fn repair(messages: Vec<Message>) -> Vec<Message> {
    use crate::conversation::Role;
    let mut out: Vec<Message> = Vec::with_capacity(messages.len());
    let mut open: Vec<String> = Vec::new();
    let close = |out: &mut Vec<Message>, open: &mut Vec<String>| {
        for id in open.drain(..) {
            out.push(Message {
                failed: true,
                summary: Some("Stopped".into()),
                ..Message::tool(id, "Stopped before it ran.")
            });
        }
    };
    for m in messages {
        match m.role {
            Role::Tool => {
                let id = m.tool_call_id.clone().unwrap_or_default();
                if let Some(at) = open.iter().position(|o| *o == id) {
                    open.remove(at);
                    out.push(m);
                }
            }
            _ => {
                close(&mut out, &mut open);
                open = m.tool_calls.iter().map(|c| c.id.clone()).collect();
                out.push(m);
            }
        }
    }
    close(&mut out, &mut open);
    out
}

/// Runs the agent on `request` (whose `tools` it sets) in `workspace`.
pub fn run(
    backend: &dyn Backend,
    mut request: Request,
    workspace: &Workspace,
    cancel: &AtomicBool,
    host: &mut dyn Host,
) -> Result<(), BackendError> {
    request.tools = tools::schema();
    let mut edits_allowed = false;
    for step in 0..MAX_STEPS {
        if step > 0 {
            host.next_turn();
        }
        let mut text = String::new();
        let mut calls: Vec<ToolCall> = Vec::new();
        backend.complete(&request, cancel, &mut |event| match event {
            Event::Text(piece) => {
                text.push_str(piece);
                host.text(piece);
            }
            Event::Speed(s) => host.speed(s),
            Event::ToolCalls(c) => calls = c.to_vec(),
        })?;
        if cancel.load(Ordering::Relaxed) || calls.is_empty() {
            return Ok(());
        }
        host.calls(&calls);
        request.messages.push(Message {
            tool_calls: calls.clone(),
            ..Message::assistant(text)
        });
        for call in &calls {
            if cancel.load(Ordering::Relaxed) {
                return Ok(());
            }
            let outcome = match tools::spec(&call.name) {
                None => tools::run(workspace, &call.name, &call.arguments, cancel),
                Some(spec) if spec.effect == Effect::Read => {
                    tools::run(workspace, &call.name, &call.arguments, cancel)
                }
                Some(spec) => {
                    // Arguments that can't run are refused without asking.
                    let (title, detail) =
                        match tools::describe(workspace, &call.name, &call.arguments) {
                            Ok(asked) => asked,
                            Err(e) => {
                                let outcome = Outcome {
                                    ok: false,
                                    summary: e.clone(),
                                    output: format!("Error: {e}"),
                                };
                                let message = Message {
                                    failed: true,
                                    summary: Some(outcome.summary),
                                    ..Message::tool(call.id.clone(), outcome.output)
                                };
                                request.messages.push(message.clone());
                                host.result(message);
                                continue;
                            }
                        };
                    // "Allow All Edits" covers plain edits; version control,
                    // build scripts, hidden files and programs always ask.
                    let asked = if spec.effect == Effect::Write
                        && edits_allowed
                        && !tools::sensitive(workspace, &call.name, &call.arguments)
                    {
                        Approval::Allow
                    } else {
                        host.approve(call, &title, &detail)
                    };
                    if asked == Approval::AllowEdits && spec.effect == Effect::Write {
                        edits_allowed = true;
                    }
                    if cancel.load(Ordering::Relaxed) {
                        return Ok(());
                    }
                    match asked {
                        Approval::Deny => Outcome::declined(),
                        _ => tools::run(workspace, &call.name, &call.arguments, cancel),
                    }
                }
            };
            let message = Message {
                failed: !outcome.ok,
                summary: Some(outcome.summary),
                ..Message::tool(call.id.clone(), outcome.output)
            };
            request.messages.push(message.clone());
            host.result(message);
        }
    }
    Err(BackendError::Other(format!(
        "The agent stopped after {MAX_STEPS} steps without finishing. Tell it how to go on."
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::Role;
    use std::cell::RefCell;
    use std::path::PathBuf;
    use std::sync::Mutex;

    /// A model that asks for one tool per turn from a script, then answers.
    struct Scripted {
        turns: Mutex<Vec<Vec<ToolCall>>>,
        seen: Mutex<Vec<Request>>,
    }

    impl Backend for Scripted {
        fn name(&self) -> String {
            "scripted".into()
        }
        fn models(&self) -> Result<Vec<String>, BackendError> {
            Ok(Vec::new())
        }
        fn complete(
            &self,
            request: &Request,
            _cancel: &AtomicBool,
            emit: &mut dyn FnMut(Event<'_>),
        ) -> Result<(), BackendError> {
            self.seen.lock().unwrap().push(request.clone());
            let mut turns = self.turns.lock().unwrap();
            if turns.is_empty() {
                emit(Event::Text("All done."));
            } else {
                let calls = turns.remove(0);
                emit(Event::Text("Looking."));
                emit(Event::ToolCalls(&calls));
            }
            Ok(())
        }
    }

    #[derive(Default)]
    struct Log {
        events: RefCell<Vec<String>>,
        answer: Option<Approval>,
    }

    impl Host for Log {
        fn text(&mut self, piece: &str) {
            self.events.borrow_mut().push(format!("text {piece}"));
        }
        fn speed(&mut self, _: f64) {}
        fn calls(&mut self, calls: &[ToolCall]) {
            self.events
                .borrow_mut()
                .push(format!("calls {}", calls.len()));
        }
        fn approve(&mut self, _: &ToolCall, title: &str, _: &str) -> Approval {
            self.events.borrow_mut().push(format!("ask {title}"));
            self.answer.unwrap_or(Approval::Deny)
        }
        fn result(&mut self, m: Message) {
            assert_eq!(m.role, Role::Tool);
            self.events
                .borrow_mut()
                .push(format!("result {}", m.summary.unwrap_or_default()));
        }
        fn next_turn(&mut self) {
            self.events.borrow_mut().push("next".into());
        }
    }

    fn call(id: &str, name: &str, args: &str) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: name.into(),
            arguments: args.into(),
        }
    }

    fn workspace(name: &str) -> (PathBuf, Workspace) {
        let dir = std::env::temp_dir().join(format!("gates-agent-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "alpha\n").unwrap();
        let ws = Workspace::open(&dir).unwrap();
        (dir, ws)
    }

    fn request() -> Request {
        Request {
            model: String::new(),
            system_prompt: String::new(),
            messages: vec![Message::user("Tidy up")],
            sampling: None,
            tools: Vec::new(),
            response_format: None,
            brief: false,
        }
    }

    #[test]
    fn reads_run_and_changes_ask() {
        let (dir, ws) = workspace("ask");
        let backend = Scripted {
            turns: Mutex::new(vec![
                vec![call("1", "read_file", r#"{"path":"a.txt"}"#)],
                vec![call(
                    "2",
                    "write_file",
                    r#"{"path":"b.txt","content":"beta"}"#,
                )],
            ]),
            seen: Mutex::new(Vec::new()),
        };
        let mut host = Log::default();
        run(&backend, request(), &ws, &AtomicBool::new(false), &mut host).unwrap();
        let events = host.events.borrow().clone();
        assert_eq!(
            events,
            vec![
                "text Looking.",
                "calls 1",
                "result Read a.txt (lines 1–1 of 1)",
                "next",
                "text Looking.",
                "calls 1",
                "ask Write b.txt (1 lines, 4 characters)",
                "result Declined",
                "next",
                "text All done.",
            ]
        );
        // Declined: nothing written.
        assert!(!dir.join("b.txt").exists());
        // The last request carries the whole exchange, and the tools.
        let seen = backend.seen.lock().unwrap();
        let last = seen.last().unwrap();
        assert_eq!(last.messages.len(), 5);
        assert_eq!(last.messages[1].tool_calls[0].name, "read_file");
        assert!(last.messages[2].text.contains("alpha"));
        assert_eq!(last.tools.len(), tools::TOOLS.len());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn repairs_a_stopped_run() {
        let messages = vec![
            Message::user("go"),
            Message {
                tool_calls: vec![call("a", "list_dir", "{}"), call("b", "read_file", "{}")],
                ..Message::assistant("")
            },
            Message::tool("a", "src/"),
            // "b" never ran; a stray result for "z" has no call.
            Message::tool("z", "?"),
            Message::user("go on"),
        ];
        let fixed = repair(messages);
        let roles: Vec<&str> = fixed.iter().map(|m| m.role.as_str()).collect();
        assert_eq!(roles, vec!["user", "assistant", "tool", "tool", "user"]);
        assert_eq!(fixed[3].tool_call_id.as_deref(), Some("b"));
        assert!(fixed[3].failed);
    }

    #[test]
    fn allowed_edits_and_a_step_limit() {
        let (dir, ws) = workspace("limit");
        let many: Vec<Vec<ToolCall>> = (0..MAX_STEPS)
            .map(|i| {
                vec![call(
                    &i.to_string(),
                    "write_file",
                    r#"{"path":"c.txt","content":"x"}"#,
                )]
            })
            .collect();
        let backend = Scripted {
            turns: Mutex::new(many),
            seen: Mutex::new(Vec::new()),
        };
        let mut host = Log {
            answer: Some(Approval::AllowEdits),
            ..Log::default()
        };
        let err = run(&backend, request(), &ws, &AtomicBool::new(false), &mut host).unwrap_err();
        assert!(err.to_string().contains("25 steps"));
        assert!(dir.join("c.txt").exists());
        // Asked once; the later edits ran on that answer.
        let asks = host
            .events
            .borrow()
            .iter()
            .filter(|e| e.starts_with("ask"))
            .count();
        assert_eq!(asks, 1);
        let _ = std::fs::remove_dir_all(dir);
    }
}
