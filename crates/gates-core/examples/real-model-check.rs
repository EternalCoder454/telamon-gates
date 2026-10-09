//! The real llama backend against a real model, for CI: a tiny GGUF
//! (about 100 MB) on the processor. It asserts what holds whatever the model
//! says, so a model that talks nonsense still passes and a broken backend
//! does not:
//!
//! - **stream**: tokens arrive one by one, then the server's speed, and the
//!   context the server runs with is the one asked for.
//! - **stop**: Stop in the middle of a reply, and in the middle of reading a
//!   long prompt, ends the reply within a deadline and frees the server (the
//!   next message is answered at once).
//! - **trim**: a multi-turn conversation in a small context is trimmed, and
//!   what reaches the server (recorded on its way) still starts with the
//!   system prompt and ends with the user's latest message, and is accepted.
//! - **agent**: the tool round trip. A model this small can't call tools, so
//!   its first turn is scripted (read a file, edit it); the real model then
//!   gets the results, in the real chat template, and answers.
//!
//!   cargo run --example real-model-check -- <llama-server> <models dir> [stream|stop|trim|agent ...]
//!
//! Exits 1 when a check fails. `CHECK_LOG_DIR` keeps the server logs there
//! (else the temporary folder). Deadlines are generous: a two-core runner.

use gates_core::agent::{self, Approval, Host};
use gates_core::backend::{Backend, BackendError, Event, Llama, Options, Request};
use gates_core::conversation::{Message, Role, ToolCall};
use gates_core::modes;
use gates_core::tools::Workspace;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Stop must end the reply within this, from the moment it is pressed.
const STOP_DEADLINE: Duration = Duration::from_secs(3);
/// A short message after a Stop: its first words within this.
const NEXT_DEADLINE: Duration = Duration::from_secs(5);
/// The first words of the first reply, which includes loading the model.
const LOAD_DEADLINE: Duration = Duration::from_secs(120);

struct Report {
    failures: Vec<String>,
}

impl Report {
    fn check(&mut self, ok: bool, what: impl std::fmt::Display) {
        println!("  {} {what}", if ok { "ok  " } else { "FAIL" });
        if !ok {
            self.failures.push(what.to_string());
        }
    }
}

struct Setup {
    binary: PathBuf,
    models: PathBuf,
    logs: PathBuf,
}

impl Setup {
    fn llama(&self, name: &str, context: u32) -> Llama {
        Llama::new(
            self.models.clone(),
            Some(self.binary.clone()),
            self.logs.join(format!("real-model-{name}.log")),
            Options {
                context,
                ..Options::default()
            },
        )
    }
}

fn ask(messages: Vec<Message>) -> Request {
    Request {
        model: String::new(),
        system_prompt: String::new(),
        messages,
        sampling: None,
        tools: Vec::new(),
        response_format: None,
        brief: false,
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(binary), Some(models)) = (args.next(), args.next()) else {
        eprintln!(
            "usage: real-model-check <llama-server> <models dir> [stream|stop|trim|agent ...]"
        );
        std::process::exit(2);
    };
    let only: Vec<String> = args.collect();
    let setup = Setup {
        binary: PathBuf::from(binary),
        models: PathBuf::from(models),
        logs: std::env::var_os("CHECK_LOG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir),
    };
    let _ = std::fs::create_dir_all(&setup.logs);
    let mut report = Report {
        failures: Vec::new(),
    };
    type Scenario = fn(&Setup, &mut Report);
    let scenarios: [(&str, Scenario); 4] = [
        ("stream", stream),
        ("stop", stop),
        ("trim", trim),
        ("agent", agent_round_trip),
    ];
    for (name, run) in scenarios {
        if !only.is_empty() && !only.iter().any(|o| o == name) {
            continue;
        }
        println!("{name}:");
        let started = Instant::now();
        run(&setup, &mut report);
        println!("  ({:.1} s)", started.elapsed().as_secs_f64());
    }
    if report.failures.is_empty() {
        println!("real-model-check: ok");
    } else {
        println!("real-model-check: {} failed:", report.failures.len());
        for f in &report.failures {
            println!("  - {f}");
        }
        std::process::exit(1);
    }
}

// ---------------------------------------------------------------- stream

fn stream(setup: &Setup, r: &mut Report) {
    let llama = setup.llama("stream", 2048);
    let models = llama.models().unwrap_or_default();
    r.check(
        !models.is_empty(),
        format!("the model is listed: {models:?}"),
    );

    let started = Instant::now();
    let (mut first, mut pieces, mut text, mut speed, mut calls) = (None, 0, String::new(), None, 0);
    let mut notices = Vec::new();
    let result = llama.complete(
        &ask(vec![Message::user("Say hello in five words.")]),
        &AtomicBool::new(false),
        &mut |e| match e {
            Event::Text(t) => {
                first.get_or_insert(started.elapsed());
                pieces += 1;
                text.push_str(t);
            }
            Event::Speed(s) => speed = Some(s),
            Event::ToolCalls(_) => calls += 1,
            Event::Notice(n) => notices.push(n.to_string()),
        },
    );
    let total = started.elapsed();
    println!(
        "  {pieces} pieces in {:.1} s: {:?}",
        total.as_secs_f64(),
        text.chars().take(80).collect::<String>()
    );
    r.check(result.is_ok(), format!("the reply ends well: {result:?}"));
    r.check(pieces >= 2, format!("tokens arrive one by one ({pieces})"));
    r.check(!text.trim().is_empty(), "the text is not empty");
    r.check(
        first.is_some_and(|f| f < LOAD_DEADLINE),
        format!("the first token comes in time ({first:?})"),
    );
    r.check(
        speed.is_some_and(|s| s > 0.0),
        format!("the server's speed arrives ({speed:?})"),
    );
    r.check(calls == 0, "no tool calls without tools");
    r.check(
        notices.is_empty(),
        format!("a server that starts well gives no notice ({notices:?})"),
    );
    r.check(
        llama.context_size() == Some(2048),
        format!(
            "the server runs with the context asked for ({:?})",
            llama.context_size()
        ),
    );
}

// ------------------------------------------------------------------ stop

/// `text` repeated to about `tokens` tokens, opened by `tag` so that no two
/// prompts share a start (the server would read the shared part from its cache).
fn long_prompt(tag: &str, tokens: usize) -> String {
    format!(
        "{tag}\n{}Summarise that.",
        "The quick brown fox jumps over the lazy dog. ".repeat(tokens / 10)
    )
}

/// The time from the request to the first piece of text, if one comes.
fn first_token(llama: &Llama, messages: Vec<Message>) -> Option<Duration> {
    let started = Instant::now();
    let mut first = None;
    let cancel = AtomicBool::new(false);
    let _ = llama.complete(&ask(messages), &cancel, &mut |e| {
        if let Event::Text(_) = e {
            first.get_or_insert(started.elapsed());
            // Enough: that is Stop.
            cancel.store(true, Ordering::Relaxed);
        }
    });
    first
}

fn stop(setup: &Setup, r: &mut Report) {
    let log = setup.logs.join("real-model-stop.log");
    let llama = Arc::new(setup.llama("stop", 8192));
    // Loads the model (and is a Stop itself).
    let warm = first_token(&llama, vec![Message::user("Hi")]);
    r.check(warm.is_some(), format!("the model answers ({warm:?})"));

    // In the middle of a reply: Stop after the third piece.
    let before = cancels(&log);
    let cancel = AtomicBool::new(false);
    let (mut pieces, mut stopped_at, mut after) = (0, None::<Instant>, 0);
    let result = llama.complete(
        &ask(vec![Message::user(
            "Write a long story about a lighthouse keeper and a dragon. Make it at least a thousand words.",
        )]),
        &cancel,
        &mut |e| {
            if let Event::Text(_) = e {
                pieces += 1;
                if stopped_at.is_some() {
                    after += 1;
                } else if pieces == 3 {
                    cancel.store(true, Ordering::Relaxed);
                    stopped_at = Some(Instant::now());
                }
            }
        },
    );
    let took = stopped_at.map(|t| t.elapsed());
    r.check(
        stopped_at.is_some(),
        format!("the reply was long enough to stop in ({pieces} pieces)"),
    );
    r.check(
        result.is_ok(),
        format!("a stopped reply is not an error: {result:?}"),
    );
    r.check(
        took.is_some_and(|t| t < STOP_DEADLINE),
        format!("Stop ends the reply within {STOP_DEADLINE:?} ({took:?})"),
    );
    r.check(
        after <= 2,
        format!("no more than 2 pieces after Stop ({after})"),
    );
    r.check(
        wait_for_cancel(&log, before + 1, STOP_DEADLINE),
        format!(
            "the server was told to stop (it logged a cancelled task within {STOP_DEADLINE:?})"
        ),
    );
    let next = first_token(&llama, vec![Message::user("Say OK.")]);
    r.check(
        next.is_some_and(|t| t < NEXT_DEADLINE),
        format!("the next message is answered at once ({next:?})"),
    );

    // In the middle of reading a long prompt. On the processor that takes
    // seconds (the server reads it 2048 tokens at a time, and notices a
    // closed connection between two such batches), so this checks what is
    // Gates': Stop returns at once, and shuts the connection, which the
    // server logs as a cancelled task.
    let before = cancels(&log);
    let cancel = Arc::new(AtomicBool::new(false));
    let worker = {
        let (llama, cancel) = (llama.clone(), cancel.clone());
        std::thread::spawn(move || {
            llama.complete(
                &ask(vec![Message::user(long_prompt("Document B.", 4000))]),
                &cancel,
                &mut |_| {},
            )
        })
    };
    // Past the request going out; the prompt takes longer to read than this
    // on any processor a runner has (else Stop falls in the reply, which
    // must work as well).
    std::thread::sleep(Duration::from_secs(1));
    cancel.store(true, Ordering::Relaxed);
    let pressed = Instant::now();
    let result = worker.join().unwrap();
    let late = pressed.elapsed();
    r.check(
        result.is_ok(),
        format!("a reply stopped while the prompt is read is not an error: {result:?}"),
    );
    r.check(
        late < STOP_DEADLINE,
        format!("Stop ends it within {STOP_DEADLINE:?} ({late:?})"),
    );
    r.check(
        wait_for_cancel(&log, before + 1, STOP_DEADLINE),
        format!(
            "the server was told to stop (it logged a cancelled task within {STOP_DEADLINE:?})"
        ),
    );
}

/// How many tasks the server's log says it cancelled: a Stop shuts the
/// connection, and the server drops the work for it.
fn cancels(log: &Path) -> usize {
    std::fs::read_to_string(log)
        .map(|t| {
            t.lines()
                .filter(|l| l.contains("stop: cancel task"))
                .count()
        })
        .unwrap_or(0)
}

fn wait_for_cancel(log: &Path, count: usize, within: Duration) -> bool {
    let started = Instant::now();
    while started.elapsed() < within {
        if cancels(log) >= count {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    cancels(log) >= count
}

// ------------------------------------------------------------------ trim

/// A recording proxy in front of a server: every `POST /v1/chat/completions`
/// body that goes through is kept. One thread per connection; requests are
/// read as HTTP/1.1 with a Content-Length (what Gates sends), everything else
/// is passed on untouched.
struct Proxy {
    port: u16,
    bodies: Arc<Mutex<Vec<String>>>,
}

fn proxy(upstream: u16) -> Proxy {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let kept = bodies.clone();
    std::thread::spawn(move || {
        for client in listener.incoming().flatten() {
            let kept = kept.clone();
            std::thread::spawn(move || {
                let Ok(server) = TcpStream::connect(("127.0.0.1", upstream)) else {
                    return;
                };
                let (mut server_in, mut client_out) =
                    (server.try_clone().unwrap(), client.try_clone().unwrap());
                // The answer goes back as it is.
                std::thread::spawn(move || {
                    let _ = std::io::copy(&mut server_in, &mut client_out);
                    let _ = client_out.shutdown(std::net::Shutdown::Both);
                });
                let mut server_out = server;
                let mut reader = BufReader::new(client);
                loop {
                    let mut head = String::new();
                    let mut length = 0usize;
                    let mut line = String::new();
                    let mut request_line = String::new();
                    loop {
                        line.clear();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 {
                            let _ = server_out.shutdown(std::net::Shutdown::Both);
                            return;
                        }
                        if request_line.is_empty() {
                            request_line = line.clone();
                        }
                        if let Some((k, v)) = line.split_once(':')
                            && k.eq_ignore_ascii_case("content-length")
                        {
                            length = v.trim().parse().unwrap_or(0);
                        }
                        head.push_str(&line);
                        if line == "\r\n" {
                            break;
                        }
                    }
                    let mut body = vec![0; length];
                    if reader.read_exact(&mut body).is_err() {
                        return;
                    }
                    if request_line.starts_with("POST /v1/chat/completions") {
                        kept.lock()
                            .unwrap()
                            .push(String::from_utf8_lossy(&body).into_owned());
                    }
                    if server_out.write_all(head.as_bytes()).is_err()
                        || server_out.write_all(&body).is_err()
                    {
                        return;
                    }
                }
            });
        }
    });
    Proxy { port, bodies }
}

/// A llama-server of our own, for the trim check, killed when dropped.
struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn serve(binary: &Path, model: &Path, port: u16, context: u32, log: &Path) -> Option<Server> {
    let log = std::fs::File::create(log).ok()?;
    let child = Command::new(binary)
        .args(["--model"])
        .arg(model)
        .args(["--host", "127.0.0.1", "--port", &port.to_string()])
        .args(["--ctx-size", &context.to_string()])
        .args(["--parallel", "1", "--jinja", "--no-webui", "--offline"])
        .stdout(log.try_clone().ok()?)
        .stderr(log)
        .stdin(Stdio::null())
        .spawn()
        .ok()?;
    let server = Server(child);
    let started = Instant::now();
    while started.elapsed() < LOAD_DEADLINE {
        if let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)) {
            let _ = s.write_all(b"GET /health HTTP/1.0\r\n\r\n");
            let mut answer = String::new();
            let _ = s.read_to_string(&mut answer);
            if answer.contains("200 OK") {
                return Some(server);
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    None
}

/// A server of our own with a recording proxy in front, and a backend that
/// is set to talk to the proxy as it would to a server at an address.
struct Remote {
    _server: Server,
    proxy: Proxy,
    llama: Llama,
}

fn remote(setup: &Setup, r: &mut Report, name: &str, context: u32) -> Option<Remote> {
    let models = gates_core::backend::llama::chat_models(&setup.models);
    let Some(model) = models.first() else {
        r.check(false, "a model is in the folder");
        return None;
    };
    let port = free_port();
    let Some(server) = serve(
        &setup.binary,
        &model.path,
        port,
        context,
        &setup.logs.join(format!("real-model-{name}-server.log")),
    ) else {
        r.check(false, "llama-server starts");
        return None;
    };
    let proxy = proxy(port);
    let llama = Llama::new(
        setup.models.clone(),
        None,
        setup.logs.join(format!("real-model-{name}.log")),
        Options {
            server_url: format!("http://127.0.0.1:{}", proxy.port),
            ..Options::default()
        },
    );
    Some(Remote {
        _server: server,
        proxy,
        llama,
    })
}

fn trim(setup: &Setup, r: &mut Report) {
    const CONTEXT: u32 = 512;
    let Some(remote) = remote(setup, r, "trim", CONTEXT) else {
        return;
    };
    let (proxy, llama) = (&remote.proxy, &remote.llama);

    let system = "Answer in one short sentence.";
    let mut messages = Vec::new();
    let (mut trimmed_turns, mut sent_ok) = (0, 0);
    const TURNS: usize = 8;
    for turn in 0..TURNS {
        let marker = format!("MARKER-{turn}-7f3a");
        messages.push(Message::user(format!(
            "Question {turn} ({marker}): in a few words, what is special about the number {turn}? \
             Think about its factors, its neighbours, and where it is used in everyday life."
        )));
        let mut request = ask(messages.clone());
        request.system_prompt = system.into();
        let mut text = String::new();
        let result = llama.complete(&request, &AtomicBool::new(false), &mut |e| {
            if let Event::Text(t) = e {
                text.push_str(t);
            }
        });
        let body = proxy.bodies.lock().unwrap().last().cloned();
        let sent: Vec<serde_json::Value> = body
            .as_deref()
            .and_then(|b| serde_json::from_str::<serde_json::Value>(b).ok())
            .and_then(|v| v["messages"].as_array().cloned())
            .unwrap_or_default();
        let roles: Vec<&str> = sent.iter().filter_map(|m| m["role"].as_str()).collect();
        let last_is_task = sent
            .last()
            .and_then(|m| m["content"].as_str())
            .is_some_and(|c| c.contains(&marker));
        let kept = sent.len().saturating_sub(1);
        println!(
            "  turn {turn}: {} of {} messages sent, reply {} characters",
            kept,
            messages.len(),
            text.len()
        );
        r.check(
            result.is_ok() && !text.trim().is_empty(),
            format!("turn {turn} is answered ({result:?})"),
        );
        r.check(
            sent.first()
                .is_some_and(|m| m["role"] == "system" && m["content"].as_str() == Some(system)),
            format!("turn {turn}: the system prompt is first"),
        );
        r.check(
            last_is_task && roles.last() == Some(&"user"),
            format!("turn {turn}: the latest question is the last message sent"),
        );
        r.check(
            roles.get(1) == Some(&"user"),
            format!("turn {turn}: what is kept starts with a user turn"),
        );
        if kept < messages.len() {
            trimmed_turns += 1;
        }
        if result.is_ok() {
            sent_ok += 1;
        }
        // The reply goes into the conversation as the user would see it.
        messages.push(Message::assistant(
            text.chars().take(300).collect::<String>(),
        ));
    }
    r.check(
        trimmed_turns >= 2,
        format!("the conversation was trimmed on {trimmed_turns} of {TURNS} turns"),
    );
    r.check(
        sent_ok == TURNS,
        format!("the server took every trimmed conversation ({sent_ok}/{TURNS})"),
    );
    r.check(
        llama.context_size() == Some(CONTEXT),
        format!("the context is {CONTEXT} ({:?})", llama.context_size()),
    );
    // Without trimming, the first message sent in the last turn would be turn
    // 0's; with it, it is a later one.
    let last = proxy
        .bodies
        .lock()
        .unwrap()
        .last()
        .cloned()
        .unwrap_or_default();
    r.check(
        !last.contains("MARKER-0-7f3a"),
        "the oldest turns are the ones left out",
    );
}

// ----------------------------------------------------------------- agent

/// The first turn is scripted, the rest is the real model's. A model that
/// small can't call tools, and what the round trip needs checked is Gates':
/// the tool's result, and the call before it, in the conversation the real
/// server's chat template renders.
struct FirstTurnScripted<'a> {
    real: &'a Llama,
    turn: AtomicUsize,
    /// The real model's own tool calls are not followed: it would be
    /// random where the loop goes.
    own_calls: AtomicUsize,
}

impl Backend for FirstTurnScripted<'_> {
    fn name(&self) -> String {
        "scripted, then llama".into()
    }
    fn models(&self) -> Result<Vec<String>, BackendError> {
        self.real.models()
    }
    fn complete(
        &self,
        request: &Request,
        cancel: &AtomicBool,
        emit: &mut dyn FnMut(Event<'_>),
    ) -> Result<(), BackendError> {
        if self.turn.fetch_add(1, Ordering::Relaxed) == 0 {
            emit(Event::Text("I will read the file, then change it. "));
            emit(Event::ToolCalls(&[
                ToolCall {
                    id: "call_0".into(),
                    name: "read_file".into(),
                    arguments: r#"{"path": "greet.py"}"#.into(),
                },
                ToolCall {
                    id: "call_1".into(),
                    name: "edit_file".into(),
                    arguments:
                        r#"{"path": "greet.py", "old_text": "Hello", "new_text": "Good morning"}"#
                            .into(),
                },
            ]));
            return Ok(());
        }
        self.real.complete(request, cancel, &mut |e| match e {
            Event::ToolCalls(_) => {
                self.own_calls.fetch_add(1, Ordering::Relaxed);
            }
            other => emit(other),
        })
    }
}

#[derive(Default)]
struct Watch {
    text: String,
    texts_after_results: usize,
    asked: Vec<String>,
    results: Vec<Message>,
    turns: usize,
}

impl Host for Watch {
    fn text(&mut self, piece: &str) {
        self.text.push_str(piece);
        if !self.results.is_empty() {
            self.texts_after_results += 1;
        }
    }
    fn speed(&mut self, _: f64) {}
    fn calls(&mut self, _: &[ToolCall]) {}
    fn approve(&mut self, call: &ToolCall, title: &str, _: &str) -> Approval {
        self.asked.push(format!("{} {title}", call.name));
        Approval::Allow
    }
    fn result(&mut self, m: Message) {
        self.results.push(m);
    }
    fn next_turn(&mut self) {
        self.turns += 1;
    }
}

fn agent_round_trip(setup: &Setup, r: &mut Report) {
    let dir = std::env::temp_dir().join(format!("real-model-agent-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("greet.py"),
        "def greet(name):\n    return \"Hello, \" + name\n",
    )
    .unwrap();
    let ws = Workspace::open(&dir).unwrap();

    // The agent's real system prompt and tool list are a few thousand tokens.
    let Some(remote) = remote(setup, r, "agent", 8192) else {
        return;
    };
    let (proxy, llama) = (&remote.proxy, &remote.llama);
    let backend = FirstTurnScripted {
        real: llama,
        turn: AtomicUsize::new(0),
        own_calls: AtomicUsize::new(0),
    };
    let mode = modes::AGENT;
    let mut request = ask(vec![Message::user(
        "Change the greeting in greet.py from Hello to Good morning.",
    )]);
    request.system_prompt = format!(
        "{}\n\nThe workspace is {}.",
        modes::system_prompt(&mode, ""),
        ws.root().display()
    );
    request.sampling = mode.sampling;
    let mut host = Watch::default();
    let result = agent::run(&backend, request, &ws, &AtomicBool::new(false), &mut host);

    let code = std::fs::read_to_string(dir.join("greet.py")).unwrap_or_default();
    println!(
        "  final turn: {:?} ({} model calls of its own ignored)",
        host.text.chars().skip(37).take(80).collect::<String>(),
        backend.own_calls.load(Ordering::Relaxed)
    );
    r.check(result.is_ok(), format!("the agent finishes: {result:?}"));
    r.check(
        host.results.len() == 2 && host.results.iter().all(|m| !m.failed),
        format!(
            "both tools ran: {:?}",
            host.results
                .iter()
                .map(|m| m.summary.clone())
                .collect::<Vec<_>>()
        ),
    );
    r.check(
        host.results
            .first()
            .is_some_and(|m| m.role == Role::Tool && m.text.contains("Hello, ")),
        "the file was read",
    );
    r.check(
        host.asked.len() == 1 && host.asked[0].starts_with("edit_file"),
        format!("the edit asked first: {:?}", host.asked),
    );
    r.check(
        code.contains("Good morning, ") && !code.contains("Hello"),
        format!("the file was changed: {code:?}"),
    );
    r.check(
        host.turns == 1,
        format!("the model took a second turn ({})", host.turns),
    );
    r.check(
        host.texts_after_results >= 1,
        format!(
            "the real model answered after the tool results ({} pieces)",
            host.texts_after_results
        ),
    );

    // What the server was sent for the second turn: the call, then each
    // result under its id, with the tools on offer.
    let bodies = proxy.bodies.lock().unwrap().clone();
    r.check(
        bodies.len() == 1,
        format!(
            "the server was asked once; the first turn was scripted ({})",
            bodies.len()
        ),
    );
    let sent: serde_json::Value = bodies
        .last()
        .and_then(|b| serde_json::from_str(b).ok())
        .unwrap_or_default();
    let messages = sent["messages"].as_array().cloned().unwrap_or_default();
    let roles: Vec<&str> = messages.iter().filter_map(|m| m["role"].as_str()).collect();
    r.check(
        roles == ["system", "user", "assistant", "tool", "tool"],
        format!("the conversation sent: {roles:?}"),
    );
    let calls = messages
        .get(2)
        .and_then(|m| m["tool_calls"].as_array())
        .cloned()
        .unwrap_or_default();
    let called: Vec<(&str, &str)> = calls
        .iter()
        .map(|c| {
            (
                c["id"].as_str().unwrap_or(""),
                c["function"]["name"].as_str().unwrap_or(""),
            )
        })
        .collect();
    r.check(
        called == [("call_0", "read_file"), ("call_1", "edit_file")],
        format!("the assistant's calls: {called:?}"),
    );
    r.check(
        messages.get(3).map(|m| m["tool_call_id"].as_str()) == Some(Some("call_0"))
            && messages.get(4).map(|m| m["tool_call_id"].as_str()) == Some(Some("call_1")),
        "each result answers its call by id",
    );
    r.check(
        messages
            .get(3)
            .and_then(|m| m["content"].as_str())
            .is_some_and(|c| c.contains("Hello, ")),
        "the file's text is in the first result",
    );
    let offered = sent["tools"].as_array().map_or(0, Vec::len);
    r.check(
        offered == gates_core::tools::TOOLS.len(),
        format!("the {offered} tools are offered"),
    );
    let _ = std::fs::remove_dir_all(&dir);
}
