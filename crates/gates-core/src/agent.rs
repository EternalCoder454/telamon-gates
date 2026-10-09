//! The tool loop: the model answers with tool calls, Gates runs them
//! (asking the user first for any that change something), sends the
//! results back, and the model goes on, until it answers without a tool
//! or the steps run out. llama-server keeps the conversation's prefix in its
//! cache, so each step costs only what is new.
//!
//! Agent mode's tools work in a folder (`run`). Chat and Code replies, when
//! Settings turn the web on, get only the web tools and a few steps
//! (`run_tools` with no workspace); Agent mode gets them beside its own.
//!
//! Blocks (the model, the tools, the user's answer): call it from a worker.

use crate::backend::{Backend, BackendError, Event, Request};
use crate::conversation::{Message, ToolCall};
use crate::tools::{self, Effect, Outcome, Workspace};
use crate::web::Session;
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
    /// What is being done now, for a progress line ("Searching: rust
    /// async"); "" when nothing is.
    fn status(&mut self, _line: &str) {}
    /// The reply's text so far is replaced by `text` (Deep Research links
    /// its citations once the report is written).
    fn replace(&mut self, _text: &str) {}
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

/// Model turns in a reply with only the web tools at most: the last one is
/// asked for without tools, so it has to answer with what it found.
pub const WEB_STEPS: usize = 6;

/// What a run offers the model.
pub struct Tools<'a> {
    /// Agent mode's folder and its tools; None for the web tools alone.
    pub workspace: Option<&'a Workspace>,
    /// The web tools; None for none.
    pub web: Option<&'a Session<'a>>,
    pub max_steps: usize,
}

/// Runs the agent on `request` (whose `tools` it sets) in `workspace`.
pub fn run(
    backend: &dyn Backend,
    request: Request,
    workspace: &Workspace,
    cancel: &AtomicBool,
    host: &mut dyn Host,
) -> Result<(), BackendError> {
    let tools = Tools {
        workspace: Some(workspace),
        web: None,
        max_steps: MAX_STEPS,
    };
    run_tools(backend, request, &tools, cancel, host)
}

/// The loop, with the tools `tools` offers.
pub fn run_tools(
    backend: &dyn Backend,
    mut request: Request,
    tools: &Tools<'_>,
    cancel: &AtomicBool,
    host: &mut dyn Host,
) -> Result<(), BackendError> {
    let mut offered = Vec::new();
    if tools.workspace.is_some() {
        offered.extend(tools::schema());
    }
    if tools.web.is_some() {
        offered.extend(tools::web_schema());
    }
    request.tools = offered;
    let mut edits_allowed = false;
    let max_steps = tools.max_steps.max(1);
    for step in 0..max_steps {
        if step > 0 {
            host.next_turn();
        }
        // Web tools alone: the last turn has none, so the answer comes.
        if tools.workspace.is_none() && step + 1 == max_steps {
            request.tools = Vec::new();
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
        // No calls, or the turn that had no tools to ask for: the answer.
        if cancel.load(Ordering::Relaxed) || calls.is_empty() || request.tools.is_empty() {
            return Ok(());
        }
        host.calls(&calls);
        request.messages.push(Message {
            tool_calls: calls.clone(),
            ..Message::assistant(text)
        });
        let mut web_calls = 0;
        for call in &calls {
            if cancel.load(Ordering::Relaxed) {
                return Ok(());
            }
            let outcome = if tools::is_web(&call.name) {
                match tools.web {
                    Some(session) if web_calls < tools::MAX_WEB_CALLS_PER_TURN => {
                        web_calls += 1;
                        host.status(&tools::progress(&call.name, &call.arguments));
                        tools::run_web(session, &call.name, &call.arguments, cancel)
                    }
                    Some(_) => Outcome {
                        ok: false,
                        summary: "Too many web calls at once".into(),
                        output: "Error: Too many web calls at once. Ask for fewer, or one \
                                 after another."
                            .into(),
                    },
                    None => tools::unavailable(&call.name),
                }
            } else if let Some(workspace) = tools.workspace {
                match tools::spec(&call.name) {
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
                }
            } else {
                tools::unavailable(&call.name)
            };
            let message = Message {
                failed: !outcome.ok,
                summary: Some(outcome.summary),
                ..Message::tool(call.id.clone(), outcome.output)
            };
            request.messages.push(message.clone());
            host.result(message);
        }
        host.status("");
    }
    Err(BackendError::Other(format!(
        "The agent stopped after {max_steps} steps without finishing. Tell it how to go on."
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

    // ---- the web tools

    use crate::web::testing::Fake;
    use std::time::Duration;

    /// Writes what it hears down; Stop can be wired to a line it sees.
    struct Spy<'a> {
        events: Vec<String>,
        stop_on: Option<(&'a str, &'a AtomicBool)>,
    }

    impl<'a> Spy<'a> {
        fn new() -> Spy<'a> {
            Spy {
                events: Vec::new(),
                stop_on: None,
            }
        }
    }

    impl Host for Spy<'_> {
        fn text(&mut self, piece: &str) {
            self.events.push(format!("text {piece}"));
        }
        fn speed(&mut self, _: f64) {}
        fn calls(&mut self, calls: &[ToolCall]) {
            self.events.push(format!("calls {}", calls.len()));
        }
        fn approve(&mut self, _: &ToolCall, title: &str, _: &str) -> Approval {
            panic!("a web tool asked the user: {title}");
        }
        fn result(&mut self, m: Message) {
            self.events
                .push(format!("result {}", m.summary.unwrap_or_default()));
        }
        fn next_turn(&mut self) {
            self.events.push("next".into());
        }
        fn status(&mut self, line: &str) {
            self.events.push(format!("status {line}"));
            if let Some((when, flag)) = self.stop_on
                && line.starts_with(when)
            {
                flag.store(true, Ordering::Relaxed);
            }
        }
    }

    fn web_fake() -> Fake {
        Fake::default()
            .with_results(
                "rust async",
                vec![Fake::result("The Book", "https://example.org/r1", "Async.")],
            )
            .with_page(Fake::page(
                "https://example.org/r1",
                "The Book",
                "Futures are lazy.",
            ))
    }

    fn only_web<'a>(session: &'a Session<'a>) -> Tools<'a> {
        Tools {
            workspace: None,
            web: Some(session),
            max_steps: WEB_STEPS,
        }
    }

    fn tool_names(request: &Request) -> Vec<String> {
        request
            .tools
            .iter()
            .map(|t| t["function"]["name"].as_str().unwrap_or("").to_string())
            .collect()
    }

    #[test]
    fn the_web_loop_searches_reads_and_answers() {
        let fake = web_fake();
        let session = Session::new(&fake, &request().messages);
        let backend = Scripted {
            turns: Mutex::new(vec![
                vec![call("1", "web_search", r#"{"query":"rust async"}"#)],
                vec![call(
                    "2",
                    "fetch_page",
                    r#"{"url":"https://example.org/r1"}"#,
                )],
            ]),
            seen: Mutex::new(Vec::new()),
        };
        let mut host = Spy::new();
        run_tools(
            &backend,
            request(),
            &only_web(&session),
            &AtomicBool::new(false),
            &mut host,
        )
        .unwrap();
        assert_eq!(
            host.events,
            vec![
                "text Looking.",
                "calls 1",
                "status Searching: rust async",
                "result Searched for \"rust async\" (1 result)",
                "status ",
                "next",
                "text Looking.",
                "calls 1",
                "status Reading: example.org/r1",
                "result Read example.org/r1 (1 KB)",
                "status ",
                "next",
                "text All done.",
            ]
        );
        let seen = backend.seen.lock().unwrap();
        assert_eq!(seen.len(), 3);
        // Only the web tools were offered: no folder, no files, no commands.
        for r in seen.iter() {
            assert_eq!(tool_names(r), vec!["web_search", "fetch_page"]);
        }
        // The last turn saw the whole exchange, results included.
        let last = seen.last().unwrap();
        assert_eq!(last.messages.len(), 5);
        assert!(last.messages[2].text.contains("https://example.org/r1"));
        assert!(last.messages[4].text.contains("Futures are lazy."));
        assert_eq!(
            fake.calls(),
            vec!["search rust async", "fetch https://example.org/r1"]
        );
    }

    #[test]
    fn the_last_web_step_has_no_tools_so_the_answer_comes() {
        let fake = web_fake();
        let session = Session::new(&fake, &request().messages);
        let turns: Vec<Vec<ToolCall>> = (0..20)
            .map(|i| {
                vec![call(
                    &i.to_string(),
                    "web_search",
                    r#"{"query":"rust async"}"#,
                )]
            })
            .collect();
        let backend = Scripted {
            turns: Mutex::new(turns),
            seen: Mutex::new(Vec::new()),
        };
        let mut host = Spy::new();
        // A model that never stops searching still ends, without an error.
        run_tools(
            &backend,
            request(),
            &only_web(&session),
            &AtomicBool::new(false),
            &mut host,
        )
        .unwrap();
        let seen = backend.seen.lock().unwrap();
        assert_eq!(seen.len(), WEB_STEPS);
        assert_eq!(tool_names(&seen[WEB_STEPS - 2]).len(), 2);
        assert!(seen[WEB_STEPS - 1].tools.is_empty());
        // Five searches ran; the sixth turn's calls were not run.
        assert_eq!(fake.calls().len(), WEB_STEPS - 1);
    }

    #[test]
    fn agent_mode_gets_the_web_beside_its_own_tools() {
        let (dir, ws) = workspace("web");
        let fake = web_fake();
        let session = Session::new(&fake, &request().messages);
        let backend = Scripted {
            turns: Mutex::new(vec![vec![
                call("1", "list_dir", "{}"),
                call("2", "web_search", r#"{"query":"rust async"}"#),
            ]]),
            seen: Mutex::new(Vec::new()),
        };
        let tools = Tools {
            workspace: Some(&ws),
            web: Some(&session),
            max_steps: MAX_STEPS,
        };
        let mut host = Spy::new();
        run_tools(
            &backend,
            request(),
            &tools,
            &AtomicBool::new(false),
            &mut host,
        )
        .unwrap();
        let seen = backend.seen.lock().unwrap();
        assert_eq!(seen[0].tools.len(), tools::TOOLS.len() + 2);
        assert!(host.events.iter().any(|e| e.starts_with("result Listed")));
        assert!(host.events.iter().any(|e| e.starts_with("result Searched")));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn web_calls_are_few_per_turn_and_unoffered_ones_are_refused() {
        let fake = web_fake();
        let session = Session::new(&fake, &request().messages);
        let six: Vec<ToolCall> = (0..6)
            .map(|i| call(&i.to_string(), "web_search", r#"{"query":"rust async"}"#))
            .collect();
        let backend = Scripted {
            turns: Mutex::new(vec![six]),
            seen: Mutex::new(Vec::new()),
        };
        let mut host = Spy::new();
        run_tools(
            &backend,
            request(),
            &only_web(&session),
            &AtomicBool::new(false),
            &mut host,
        )
        .unwrap();
        assert_eq!(fake.calls().len(), tools::MAX_WEB_CALLS_PER_TURN);
        let refused = host
            .events
            .iter()
            .filter(|e| e.starts_with("result Too many"))
            .count();
        assert_eq!(refused, 2);

        // The web off: a model that asks anyway is told there is no such tool.
        let backend = Scripted {
            turns: Mutex::new(vec![vec![call("1", "web_search", r#"{"query":"x"}"#)]]),
            seen: Mutex::new(Vec::new()),
        };
        let none = Tools {
            workspace: None,
            web: None,
            max_steps: WEB_STEPS,
        };
        let mut host = Spy::new();
        run_tools(
            &backend,
            request(),
            &none,
            &AtomicBool::new(false),
            &mut host,
        )
        .unwrap();
        // Nothing offered, so the first reply is the answer.
        assert!(backend.seen.lock().unwrap()[0].tools.is_empty());
    }

    #[test]
    fn stop_ends_a_search_at_once() {
        let fake = Fake {
            delay: Duration::from_secs(5),
            ..web_fake()
        };
        let session = Session::new(&fake, &request().messages);
        let backend = Scripted {
            turns: Mutex::new(vec![
                vec![call("1", "web_search", r#"{"query":"rust async"}"#)],
                vec![call("2", "web_search", r#"{"query":"rust async"}"#)],
            ]),
            seen: Mutex::new(Vec::new()),
        };
        let cancel = AtomicBool::new(false);
        let mut host = Spy::new();
        host.stop_on = Some(("Searching", &cancel));
        let started = std::time::Instant::now();
        run_tools(&backend, request(), &only_web(&session), &cancel, &mut host).unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        // It went no further than the second turn's start.
        assert!(backend.seen.lock().unwrap().len() <= 2);
        assert_eq!(fake.calls().len(), 1);
    }
}
