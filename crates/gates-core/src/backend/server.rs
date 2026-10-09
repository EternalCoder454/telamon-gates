//! The llama-server Gates runs itself: started with the chosen model when a
//! reply is first asked for, stopped after a few idle minutes so the model
//! leaves the graphics card's memory, and restarted when the model or its
//! options change.
//!
//! It listens on 127.0.0.1 only, on a free port, and wants a fresh random API
//! key on every start: no other program on the computer can use it. It dies
//! with Gates, also when Gates crashes (PR_SET_PDEATHSIG).
//!
//! Every method blocks (starting a big model takes a while): call them from a
//! worker thread.

use super::BackendError;
use crate::watchdog::Watchdog;
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::net::TcpListener;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

/// How long the server may sit unused before it stops.
pub const IDLE: Duration = Duration::from_secs(5 * 60);
/// How long a model may take to load before starting counts as failed.
const LOAD: Duration = Duration::from_secs(300);
/// The server's host-side prompt cache, in MiB (`--cache-ram`).
const CACHE_RAM_MIB: u32 = 2048;

/// What the server is started with. A change restarts it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub binary: PathBuf,
    pub model: PathBuf,
    /// Layers on the graphics card; None lets llama.cpp choose.
    pub gpu_layers: Option<u32>,
    /// The context, in tokens; None lets llama.cpp choose.
    pub context: Option<u32>,
    /// The batch and micro-batch, in tokens; None for llama.cpp's. A
    /// decision model reads each prompt in one micro-batch, so it must hold
    /// the longest.
    pub batch: Option<u32>,
    /// The model's image projector (`--mmproj`), so it reads pictures.
    pub projector: Option<PathBuf>,
    /// The context cache at 8 bits (`--cache-type-k/v q8_0`).
    pub small_cache: bool,
    /// Processor threads; None for llama.cpp's choice.
    pub threads: Option<u32>,
    /// n-gram speculative decoding (`--spec-default`): the server guesses
    /// the next tokens from what it has already seen and checks them in one
    /// go. Free (about 16 MB), and 5× faster where text repeats, as when an
    /// agent rewrites a file; no slower elsewhere.
    pub speculative: bool,
    /// A DSpark draft model, used with n-gram drafting in place of
    /// `--spec-default` alone (see `llama::draft_for`).
    pub draft: Option<PathBuf>,
    /// How many layers the model has (`<arch>.block_count`), 0 when not
    /// known. Where `gpu_layers` is left to llama.cpp, a load that ran out
    /// of memory steps down from this (see `lighter`).
    pub layers: u32,
}

/// The smallest context a load that ran out of memory is retried with.
pub const MIN_CONTEXT: u32 = 4096;

/// A server that stops this many times within `CRASH_WINDOW` is not started
/// again for the same model.
pub const CRASH_LIMIT: usize = 3;
pub const CRASH_WINDOW: Duration = Duration::from_secs(5 * 60);

/// Which lighter setting a load that ran out of memory tries next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Step {
    /// First a smaller context: half, but not below `MIN_CONTEXT`.
    Context,
    /// Then fewer layers on the graphics card: half.
    Layers,
    /// Nothing is left to try.
    Done,
}

/// Which memory a load ran out of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Memory {
    /// The graphics card's.
    Graphics,
    /// The computer's own (`cannot allocate memory`, `bad_alloc`, a CPU
    /// buffer). Fewer layers on the card would make it worse.
    Host,
}

impl Memory {
    /// What to call it for the user.
    pub fn noun(self) -> &'static str {
        match self {
            Memory::Graphics => "graphics memory",
            Memory::Host => "memory",
        }
    }
}

/// The `launch` to retry with after a load that ran out of `memory`, from
/// step `from` on, and the step after it. A smaller context first (halved,
/// down to `MIN_CONTEXT`), then, for the card's memory only, half the GPU
/// layers (of the model's, when they were left to llama.cpp). None when
/// there is nothing lighter.
pub fn lighter(launch: &Launch, from: Step, memory: Memory) -> Option<(Launch, Step)> {
    if from == Step::Context
        && let Some(context) = launch.context.filter(|c| *c > MIN_CONTEXT)
    {
        let smaller = Launch {
            context: Some((context / 2).max(MIN_CONTEXT)),
            ..launch.clone()
        };
        return Some((smaller, Step::Layers));
    }
    if from <= Step::Layers && memory == Memory::Graphics {
        // A setting above the model's layers (99 means "all") counts as
        // the model's own number: half of 99 would still be all of them.
        let known = (launch.layers > 0).then_some(launch.layers);
        let layers = match (launch.gpu_layers, known) {
            (Some(set), Some(all)) => Some(set.min(all)),
            (set, all) => set.or(all),
        }
        .filter(|n| *n > 0)?;
        let fewer = Launch {
            gpu_layers: Some(layers / 2),
            ..launch.clone()
        };
        return Some((fewer, Step::Done));
    }
    None
}

/// What changed from `from` to `to`, as a phrase: "a context of 16384
/// tokens instead of 32768 and 18 layers on the graphics card instead of
/// Automatic". Empty when nothing did.
pub fn changes(from: &Launch, to: &Launch) -> String {
    let mut parts = Vec::new();
    if from.context != to.context
        && let Some(now) = to.context
    {
        parts.push(match from.context {
            Some(before) => format!("a context of {now} tokens instead of {before}"),
            None => format!("a context of {now} tokens"),
        });
    }
    if from.gpu_layers != to.gpu_layers
        && let Some(now) = to.gpu_layers
    {
        parts.push(match from.gpu_layers {
            Some(before) => format!("{now} layers on the graphics card instead of {before}"),
            None => format!("{now} layers on the graphics card instead of Automatic"),
        });
    }
    parts.join(" and ")
}

/// What a log line says once llama.cpp's timestamp and level ("0.10.780.096
/// I ") are off it.
fn log_text(line: &str) -> &str {
    let line = line.trim_start();
    let mut parts = line.splitn(3, ' ');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(stamp), Some(level), Some(rest))
            if stamp.starts_with(|c: char| c.is_ascii_digit())
                && stamp.contains('.')
                && matches!(level, "I" | "W" | "E" | "D") =>
        {
            rest
        }
        _ => line,
    }
}

/// Whether a server's log (or its tail) says it ran out of memory, and
/// which: llama.cpp and its backends say it in several ways ("failed to
/// allocate", "ErrorOutOfDeviceMemory", CUDA's "out of memory", a
/// `bad_alloc`).
///
/// Only lines that look like what a failing allocation prints count: they
/// start with what produced them (`ggml_…`, `alloc_…`, `llama_…`,
/// `load_tensors`, `terminate called`, `vk::`, `error`…). The model's
/// metadata and chat template, which the server dumps while it loads
/// (`llama_model_loader: - kv …`, `print_info: …`), can say anything
/// ("out of memory") and are skipped, as are pinned-memory warnings, which
/// don't stop a load. A graphics line wins over a host one.
pub fn out_of_memory(log: &str) -> Option<Memory> {
    const SIGNS: [&str; 9] = [
        "out of memory",
        "outofdevicememory",
        "outofhostmemory",
        "failed to allocate",
        "unable to allocate",
        "cannot allocate memory",
        "device memory allocation",
        "bad_alloc",
        "allocation of size",
    ];
    // What produces a failure line, by its start.
    const PRODUCERS: [&str; 15] = [
        "ggml",
        "alloc_",
        "llama_",
        "load_tensors",
        "graph_",
        "sched_",
        "common_",
        "terminate called",
        "what()",
        "vk::",
        "vulkan",
        "cuda",
        "hip",
        "error",
        "fatal",
    ];
    // What the server prints about the model, not about a failure.
    const DUMPS: [&str; 3] = [
        "llama_model_loader: -",
        "print_info:",
        "llama_model_loader: loaded",
    ];
    let mut found = None;
    for line in log.lines() {
        let text = log_text(line);
        let lower = text.to_ascii_lowercase();
        let looks_like_failure = PRODUCERS.iter().any(|p| lower.starts_with(p))
            && !DUMPS.iter().any(|d| lower.starts_with(d));
        if !looks_like_failure
            || lower.contains("pinned")
            || !SIGNS.iter().any(|sign| lower.contains(sign))
        {
            continue;
        }
        let host = [
            "cannot allocate memory",
            "bad_alloc",
            "outofhostmemory",
            "cpu buffer",
        ]
        .iter()
        .any(|sign| lower.contains(sign));
        if !host {
            return Some(Memory::Graphics);
        }
        found = Some(Memory::Host);
    }
    found
}

/// The deaths of one model's server, to tell a crash loop.
#[derive(Debug, Default, Clone)]
pub struct Deaths {
    at: Vec<Instant>,
    /// The last line its log held when it last died.
    last: String,
}

impl Deaths {
    /// The server died at `now`, its log ending in `last`.
    pub fn record(&mut self, now: Instant, last: String) {
        self.at
            .retain(|t| now.saturating_duration_since(*t) < CRASH_WINDOW);
        self.at.push(now);
        self.last = last;
    }

    /// How many deaths within `CRASH_WINDOW` of `now`.
    pub fn recent(&self, now: Instant) -> usize {
        self.at
            .iter()
            .filter(|t| now.saturating_duration_since(**t) < CRASH_WINDOW)
            .count()
    }

    /// Whether it died `CRASH_LIMIT` times in the window: stop restarting.
    pub fn looping(&self, now: Instant) -> bool {
        self.recent(now) >= CRASH_LIMIT
    }
}

impl Launch {
    /// The command line, after the binary. GPU layers are passed only when
    /// the user set them: left out, llama.cpp fits them to the graphics
    /// card's free memory itself (its `--fit`). The API key is not here:
    /// anyone on the computer can read a command line (`/proc/<pid>/cmdline`),
    /// so it goes in the environment (`LLAMA_API_KEY`), which only the user
    /// can read.
    pub fn args(&self, port: u16) -> Vec<String> {
        let mut args: Vec<String> = vec![
            "--model".into(),
            self.model.to_string_lossy().into_owned(),
            "--host".into(),
            "127.0.0.1".into(),
            "--port".into(),
            port.to_string(),
            // One conversation at a time: all of the context for it.
            "--parallel".into(),
            "1".into(),
            "--jinja".into(),
            "--no-webui".into(),
            "--offline".into(),
            // The prompts of conversations switched away from, kept in system
            // memory so switching back doesn't read them again: llama.cpp
            // keeps up to 8 GiB, which on a desktop competes with everything
            // else. 2 GiB holds several long conversations (BACKEND.md →
            // Performance).
            "--cache-ram".into(),
            CACHE_RAM_MIB.to_string(),
        ];
        if let Some(n) = self.gpu_layers {
            args.extend(["--n-gpu-layers".into(), n.to_string()]);
        }
        if let Some(n) = self.context {
            args.extend(["--ctx-size".into(), n.to_string()]);
        }
        match (&self.draft, self.speculative) {
            (Some(draft), _) => args.extend([
                "--model-draft".into(),
                draft.to_string_lossy().into_owned(),
                "--spec-type".into(),
                "draft-dspark,ngram-mod".into(),
                "--spec-draft-n-max".into(),
                "7".into(),
                "--flash-attn".into(),
                "on".into(),
            ]),
            (None, true) => args.push("--spec-default".into()),
            (None, false) => {}
        }
        if let Some(n) = self.threads {
            args.extend([
                "--threads".into(),
                n.to_string(),
                "--threads-batch".into(),
                n.to_string(),
            ]);
        }
        if self.small_cache {
            args.extend(["--cache-type-k", "q8_0", "--cache-type-v", "q8_0"].map(String::from));
        }
        if let Some(p) = &self.projector {
            args.extend(["--mmproj".into(), p.to_string_lossy().into_owned()]);
        }
        if let Some(n) = self.batch {
            args.extend([
                "--batch-size".into(),
                n.to_string(),
                "--ubatch-size".into(),
                n.to_string(),
            ]);
        }
        args
    }
}

/// Where a running server answers, and the key it wants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub base: String,
    pub api_key: String,
}

struct Running {
    child: Child,
    /// What it runs with: `requested`, or lighter after a load that ran out
    /// of memory.
    launch: Launch,
    /// What was asked for; a later request for the same is served by this
    /// server even when `launch` is lighter.
    requested: Launch,
    endpoint: Endpoint,
    /// What was changed to get it started, for the user; taken by the
    /// first reply that asks to be told (`acquire_noting`).
    note: Option<String>,
}

#[derive(Default)]
struct State {
    running: Option<Running>,
    /// Deaths of each model's server: a crash loop is not restarted.
    crashes: HashMap<PathBuf, Deaths>,
    /// Replies under way: the server is not idle while one is.
    busy: usize,
    /// Started with options that changed since: it stops when the last
    /// reply using it ends, and the next one starts a new one.
    stale: bool,
    last_used: Option<Instant>,
}

impl State {
    /// The server for `model` died now, its log ending in `last`.
    fn died(&mut self, model: PathBuf, last: String) {
        let now = Instant::now();
        // Only the models that died lately are remembered: it stays small.
        self.crashes.retain(|_, d| d.recent(now) > 0);
        let deaths = self.crashes.entry(model.clone()).or_default();
        deaths.record(now, last.clone());
        let (n, limit) = (deaths.recent(now), CRASH_LIMIT);
        crate::applog::warn(&format!(
            "the model server for {} stopped ({n} of {limit} in the last {} minutes{}); its last line: {last}",
            model_name(&model),
            CRASH_WINDOW.as_secs() / 60,
            if n >= limit {
                ": it won't be started again for now"
            } else {
                ""
            }
        ));
    }
}

/// Why Gates won't start `model` again for now.
fn crash_loop_message(model: &Path, deaths: &Deaths, now: Instant) -> String {
    let name = model_name(model);
    let mut text = format!(
        "“{name}” keeps stopping: its server stopped {} times in the last {} minutes, so Gates won't start it again for now.",
        deaths.recent(now),
        CRASH_WINDOW.as_secs() / 60
    );
    if !deaths.last.is_empty() {
        text.push_str(&format!(" Its last words: {}", deaths.last));
    }
    text.push_str(" Choose another model, or change a setting to try again.");
    text
}

/// A model file's name without `.gguf`.
pub fn model_name(path: &Path) -> String {
    let file = path.file_name().map_or_else(
        || path.to_string_lossy().into_owned(),
        |f| f.to_string_lossy().into_owned(),
    );
    file.strip_suffix(".gguf").unwrap_or(&file).to_string()
}

/// A command to start, and where to send the started process.
type Spawn = (Command, Sender<io::Result<Child>>);

pub struct Server {
    state: Mutex<State>,
    /// How long a start may take before it counts as failed.
    load: Duration,
    /// Where the server's output goes; its last line explains a failed start.
    log: PathBuf,
    /// The thread that starts the server. PR_SET_PDEATHSIG fires when the
    /// *thread* that forked the child ends, not the process: started from a
    /// reply's short-lived worker, the server would die with that reply. This
    /// thread lives as long as the Server.
    spawner: Mutex<Sender<Spawn>>,
    /// Watches the graphics card's memory while this server is up, and
    /// stops it when the card is full (`watchdog.rs`).
    watchdog: Option<Arc<Watchdog>>,
    /// The watchdog stopped this server: a start under way gives up.
    /// Cleared by every start.
    halted: AtomicBool,
}

impl Server {
    /// A server that is not started yet, and the thread that stops it when
    /// idle (it ends with the server).
    pub fn new(log: PathBuf) -> Arc<Server> {
        Server::with_load_limit(log, LOAD, None)
    }

    /// As `new`, watched by `watchdog`.
    pub fn watched(log: PathBuf, watchdog: Arc<Watchdog>) -> Arc<Server> {
        Server::with_load_limit(log, LOAD, Some(watchdog))
    }

    /// As `new`, giving up on a start after `load` (a small model that
    /// isn't up quickly won't be).
    pub fn with_load_limit(
        log: PathBuf,
        load: Duration,
        watchdog: Option<Arc<Watchdog>>,
    ) -> Arc<Server> {
        let (spawner, jobs) = mpsc::channel::<Spawn>();
        let _ = std::thread::Builder::new()
            .name("llama-spawn".into())
            .spawn(move || {
                // Ends when the Server (the only sender) goes.
                for (mut command, reply) in jobs {
                    let _ = reply.send(command.spawn());
                }
            });
        let server = Arc::new(Server {
            state: Mutex::new(State::default()),
            load,
            log,
            spawner: Mutex::new(spawner),
            watchdog,
            halted: AtomicBool::new(false),
        });
        if let Some(watchdog) = &server.watchdog {
            watchdog.register(&server);
        }
        let weak: Weak<Server> = Arc::downgrade(&server);
        let _ = std::thread::Builder::new()
            .name("llama-idle".into())
            .spawn(move || {
                // Every 15 s while a server runs; with none, a minute is
                // plenty (it only has to notice the Server is gone).
                let mut wait = Duration::from_secs(15);
                loop {
                    std::thread::sleep(wait);
                    let Some(server) = weak.upgrade() else {
                        return;
                    };
                    server.stop_if_idle(Instant::now());
                    wait = if server.is_running() {
                        Duration::from_secs(15)
                    } else {
                        Duration::from_secs(60)
                    };
                }
            });
        server
    }

    /// The running server for `launch`: started, or restarted when started
    /// with something else, and ready to answer. Counts as in use until
    /// `release`.
    pub fn acquire(&self, launch: &Launch) -> Result<Endpoint, BackendError> {
        self.acquire_until(launch, &AtomicBool::new(false))
    }

    /// As `acquire`; `cancel` turning true while the model loads stops the
    /// start (Stop, before the reply even began).
    pub fn acquire_until(
        &self,
        launch: &Launch,
        cancel: &AtomicBool,
    ) -> Result<Endpoint, BackendError> {
        self.acquire_inner(launch, cancel, None)
    }

    /// As `acquire_until`; when the server had to start with something
    /// lighter than asked for (a load that ran out of graphics memory),
    /// `notes` gets a sentence saying what changed, once per start.
    pub fn acquire_noting(
        &self,
        launch: &Launch,
        cancel: &AtomicBool,
        notes: &mut dyn FnMut(&str),
    ) -> Result<Endpoint, BackendError> {
        self.acquire_inner(launch, cancel, Some(notes))
    }

    fn acquire_inner(
        &self,
        launch: &Launch,
        cancel: &AtomicBool,
        notes: Option<&mut dyn FnMut(&str)>,
    ) -> Result<Endpoint, BackendError> {
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let state = &mut *guard;
        // A server that died by itself since the last reply counts.
        let died = state.running.as_mut().and_then(|r| {
            matches!(r.child.try_wait(), Ok(Some(_))).then(|| r.launch.model.clone())
        });
        if let Some(model) = died {
            state.running = None;
            state.died(model, last_line(&self.log).unwrap_or_default());
        }
        let alive = match state.running.as_mut() {
            Some(r) => r.requested == *launch && matches!(r.child.try_wait(), Ok(None)),
            None => false,
        } && !state.stale;
        if !alive {
            state.stale = false;
            if let Some(old) = state.running.take() {
                stop(old.child);
            }
            if let Some(deaths) = state.crashes.get(&launch.model)
                && deaths.looping(Instant::now())
            {
                return Err(BackendError::Other(crash_loop_message(
                    &launch.model,
                    deaths,
                    Instant::now(),
                )));
            }
            // A card that is nearly full is no place to load a model (the
            // old server, if any, is gone: its memory counts as free).
            if let Some(watchdog) = &self.watchdog {
                watchdog.check_start().map_err(BackendError::Other)?;
            }
            self.halted.store(false, Ordering::Relaxed);
            // Watched while it loads too: that is when the card fills. The
            // watchdog never waits for the lock held here (`is_live`).
            if let Some(watchdog) = &self.watchdog {
                watchdog.wake();
            }
            match self.start(launch, cancel) {
                Ok(running) => state.running = Some(running),
                Err(_) if self.halted.load(Ordering::Relaxed) => {
                    // Stopped by the watchdog while it loaded: not a crash.
                    return Err(BackendError::Other(self.halted_message()));
                }
                Err(failed) => {
                    if failed.exited {
                        let last = last_line(&self.log).unwrap_or_default();
                        state.died(launch.model.clone(), last);
                    }
                    return Err(failed.error);
                }
            }
        }
        let Some(running) = state.running.as_mut() else {
            return Err(BackendError::Other(
                "The model server isn't running.".into(),
            ));
        };
        let endpoint = running.endpoint.clone();
        let note = if notes.is_some() {
            running.note.take()
        } else {
            None
        };
        state.busy += 1;
        state.last_used = Some(Instant::now());
        drop(guard);
        // Already up: watched since it started, unless the thread ended
        // (the card had nothing to read, the limit was Off).
        if let Some(watchdog) = &self.watchdog {
            watchdog.wake();
        }
        if let (Some(note), Some(notes)) = (note, notes) {
            notes(&note);
        }
        Ok(endpoint)
    }

    /// A reply asked for with `acquire` is over.
    pub fn release(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.busy = state.busy.saturating_sub(1);
        state.last_used = Some(Instant::now());
        if state.stale && state.busy == 0 {
            state.stale = false;
            if let Some(r) = state.running.take() {
                stop(r.child);
            }
        }
    }

    /// The options changed: stops the server now when no reply uses it,
    /// else when the last one ends. A reply under way finishes on it.
    pub fn retire(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        // A changed setting may well be the fix: try the models again.
        state.crashes.clear();
        if state.busy > 0 {
            state.stale = true;
        } else if let Some(r) = state.running.take() {
            stop(r.child);
        }
    }

    /// Whether a server is running now.
    pub fn is_running(&self) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state
            .running
            .as_mut()
            .is_some_and(|r| matches!(r.child.try_wait(), Ok(None)))
    }

    /// Whether a server runs or is starting: the lock is held while one
    /// loads, so a lock that can't be taken counts. For the watchdog,
    /// which must not wait for a load.
    pub(crate) fn is_live(&self) -> bool {
        match self.state.try_lock() {
            Ok(mut state) => state
                .running
                .as_mut()
                .is_some_and(|r| matches!(r.child.try_wait(), Ok(None))),
            Err(std::sync::TryLockError::WouldBlock) => true,
            Err(std::sync::TryLockError::Poisoned(e)) => e
                .into_inner()
                .running
                .as_mut()
                .is_some_and(|r| matches!(r.child.try_wait(), Ok(None))),
        }
    }

    /// The watchdog stops the server: one that is loading gives up (within
    /// a quarter of a second), one that runs quits or is killed (3 s).
    pub(crate) fn halt(&self) {
        self.halted.store(true, Ordering::Relaxed);
        self.stop();
    }

    /// What a start the watchdog stopped says.
    fn halted_message(&self) -> String {
        self.watchdog
            .as_ref()
            .and_then(|w| w.last_trip())
            .map_or_else(
                || "The model server was stopped: graphics memory ran out.".to_string(),
                |trip| trip.message(),
            )
    }

    /// Stops the server now, if it runs.
    pub fn stop(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(r) = state.running.take() {
            stop(r.child);
        }
    }

    fn stop_if_idle(&self, now: Instant) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if should_stop(state.busy, state.last_used, now)
            && let Some(r) = state.running.take()
        {
            log::info!("stopping llama-server: idle");
            stop(r.child);
        }
    }

    /// Starts the server for `requested`. A load that ran out of memory is
    /// retried with a lighter launch (`lighter`): a smaller context, then
    /// fewer layers on the graphics card. Anything else that stops it is
    /// not: that would only fail again.
    fn start(&self, requested: &Launch, cancel: &AtomicBool) -> Result<Running, StartError> {
        let mut current = requested.clone();
        let mut step = Step::Context;
        let mut short = Memory::Graphics;
        loop {
            let mut failed = match self.start_once(&current, cancel) {
                Ok((child, endpoint)) => {
                    let note = (current != *requested).then(|| {
                        format!(
                            "There wasn't enough {} to load “{}” as set, so it loaded with {}.",
                            short.noun(),
                            model_name(&requested.model),
                            changes(requested, &current)
                        )
                    });
                    return Ok(Running {
                        child,
                        launch: current,
                        requested: requested.clone(),
                        endpoint,
                        note,
                    });
                }
                Err(failed) => failed,
            };
            let Some(memory) = failed.out_of_memory else {
                return Err(failed);
            };
            short = memory;
            if let Some((next, after)) = lighter(&current, step, memory) {
                crate::applog::warn(&format!(
                    "{} ran out of memory loading: retrying with {}",
                    model_name(&requested.model),
                    changes(&current, &next)
                ));
                current = next;
                step = after;
                continue;
            }
            // Nothing lighter left: say so, and what was tried.
            let tried = match changes(requested, &current) {
                c if c.is_empty() => String::new(),
                c => format!(" (also tried with {c})"),
            };
            let line = last_line(&self.log)
                .map(|l| format!(" It said: {l}"))
                .unwrap_or_default();
            failed.error = BackendError::Unreachable(format!(
                "There isn't enough {} to load “{}”{tried}. Try a smaller model, or a smaller context in Settings.{line}",
                memory.noun(),
                model_name(&requested.model)
            ));
            return Err(failed);
        }
    }

    fn start_once(
        &self,
        launch: &Launch,
        cancel: &AtomicBool,
    ) -> Result<(Child, Endpoint), StartError> {
        let port = free_port()
            .map_err(|e| BackendError::Other(format!("No free port for the model server: {e}.")))?;
        let api_key = random_key().map_err(|e| {
            BackendError::Other(format!("Couldn't make a key for the model server: {e}."))
        })?;
        if let Some(dir) = self.log.parent() {
            let _ = crate::store::private_dir(dir);
        }
        let log = File::create(&self.log).map_err(|e| {
            BackendError::Other(format!("Couldn't open {}: {e}.", self.log.display()))
        })?;
        let log2 = log.try_clone().map_err(|e| {
            BackendError::Other(format!("Couldn't open {}: {e}.", self.log.display()))
        })?;
        let mut command = Command::new(&launch.binary);
        command
            .args(launch.args(port))
            .env("LLAMA_API_KEY", &api_key)
            .stdin(Stdio::null())
            .stdout(log)
            .stderr(log2);
        // SAFETY: prctl only sets a flag of the new process, between fork
        // and exec; it allocates nothing and takes no lock.
        unsafe {
            command.pre_exec(|| {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        log::info!(
            "starting llama-server with {} on port {port}",
            launch.model.display()
        );
        let mut child = self.spawn(command).map_err(|e| {
            BackendError::Unreachable(format!(
                "Couldn't start the model server ({}): {e}.",
                launch.binary.display()
            ))
        })?;
        let endpoint = Endpoint {
            base: format!("http://127.0.0.1:{port}"),
            api_key,
        };
        match wait_ready(&mut child, &endpoint.base, self.load, cancel, &self.halted) {
            Ok(()) => Ok((child, endpoint)),
            Err(unready) => {
                stop(child);
                let last = last_line(&self.log);
                // Only a server that ended on its own says why in its log.
                let out_of_memory = unready
                    .exited
                    .then(|| out_of_memory(&read_tail(&self.log, LOG_TAIL)))
                    .flatten();
                Err(StartError {
                    error: BackendError::Unreachable(match last {
                        Some(line) => format!(
                            "The model server didn't start: {} It said: {line}",
                            unready.why
                        ),
                        None => format!("The model server didn't start: {}", unready.why),
                    }),
                    exited: unready.exited,
                    out_of_memory,
                })
            }
        }
    }
}

/// A start that failed: what to tell the user, whether the server ended on
/// its own (not stopped, not too slow), and whether its log says it ran out
/// of memory.
struct StartError {
    error: BackendError,
    exited: bool,
    out_of_memory: Option<Memory>,
}

impl From<BackendError> for StartError {
    fn from(error: BackendError) -> StartError {
        StartError {
            error,
            exited: false,
            out_of_memory: None,
        }
    }
}

/// Why a server isn't ready: the text, and whether its process had ended.
struct Unready {
    why: String,
    exited: bool,
}

impl Server {
    /// Starts `command` on the spawner thread (see `spawner`).
    fn spawn(&self, command: Command) -> io::Result<Child> {
        let (reply, started) = mpsc::channel();
        self.spawner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .send((command, reply))
            .map_err(|_| io::Error::other("the starter thread is gone"))?;
        started
            .recv()
            .unwrap_or_else(|_| Err(io::Error::other("the starter thread is gone")))
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let state = self.state.get_mut().unwrap_or_else(|e| e.into_inner());
        if let Some(r) = state.running.take() {
            stop(r.child);
        }
    }
}

/// Whether a server with `busy` replies under way, last used at
/// `last_used`, should stop at `now`.
pub fn should_stop(busy: usize, last_used: Option<Instant>, now: Instant) -> bool {
    busy == 0 && last_used.is_some_and(|t| now.saturating_duration_since(t) >= IDLE)
}

/// Asks the server to quit, then makes sure.
fn stop(mut child: Child) {
    // Already gone (and reaped): its number may be another process's now.
    if matches!(child.try_wait(), Ok(Some(_))) {
        return;
    }
    let pid = child.id() as libc::pid_t;
    // SAFETY: a signal to our own child, which has not been reaped yet.
    unsafe {
        libc::kill(pid, libc::SIGTERM);
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Polls `/health` (no key needed) until the model is loaded. Fails when
/// the process ends or `limit` passes.
fn wait_ready(
    child: &mut Child,
    base: &str,
    limit: Duration,
    cancel: &AtomicBool,
    halted: &AtomicBool,
) -> Result<(), Unready> {
    let agent = super::llama::agent(Some(Duration::from_secs(2)));
    let url = format!("{base}/health");
    let deadline = Instant::now() + limit;
    let waiting = |why: String, exited: bool| Unready { why, exited };
    loop {
        if cancel.load(Ordering::Relaxed) || halted.load(Ordering::Relaxed) {
            return Err(waiting("it was stopped while it loaded.".into(), false));
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(waiting(format!("it stopped ({status})."), true));
        }
        if let Ok(response) = agent.get(&url).call()
            && response.status().as_u16() == 200
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(waiting(
                format!("it wasn't ready after {} seconds.", limit.as_secs()),
                false,
            ));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// A port nothing listens on now (the server takes it a moment later).
fn free_port() -> io::Result<u16> {
    Ok(TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

/// 32 random hex characters.
fn random_key() -> io::Result<String> {
    let mut bytes = [0u8; 16];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// How much of the end of the log is read to see why a server stopped.
const LOG_TAIL: u64 = 64 * 1024;

/// The last `max` bytes of the file as text (lossy: a log can hold
/// anything); empty when it can't be read.
fn read_tail(path: &Path, max: u64) -> String {
    let read = || -> io::Result<Vec<u8>> {
        let mut file = File::open(path)?;
        let len = file.metadata()?.len();
        file.seek(SeekFrom::Start(len.saturating_sub(max)))?;
        let mut bytes = Vec::new();
        file.take(max).read_to_end(&mut bytes)?;
        Ok(bytes)
    };
    String::from_utf8_lossy(&read().unwrap_or_default()).into_owned()
}

/// The last non-empty line of the log, shortened: why a start failed.
fn last_line(path: &Path) -> Option<String> {
    let text = read_tail(path, 16 * 1024);
    let line = text.lines().rev().map(str::trim).find(|l| !l.is_empty())?;
    Some(line.chars().take(200).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn launch() -> Launch {
        Launch {
            binary: PathBuf::from("/usr/libexec/telamon-llama/llama-server"),
            model: PathBuf::from("/m/qwen.gguf"),
            gpu_layers: Some(30),
            context: Some(8192),
            batch: None,
            projector: None,
            small_cache: false,
            threads: None,
            speculative: false,
            draft: None,
            layers: 36,
        }
    }

    /// A server that is "running" `child`, started for `launch()`.
    fn running(child: Child) -> Running {
        Running {
            child,
            launch: launch(),
            requested: launch(),
            endpoint: Endpoint {
                base: "http://127.0.0.1:1".into(),
                api_key: String::new(),
            },
            note: None,
        }
    }

    #[test]
    fn the_command_line() {
        let args = launch().args(4242);
        let pair = |flag: &str| {
            let i = args.iter().position(|a| a == flag).expect(flag);
            args[i + 1].clone()
        };
        assert_eq!(pair("--model"), "/m/qwen.gguf");
        assert_eq!(pair("--host"), "127.0.0.1");
        assert_eq!(pair("--port"), "4242");
        // The key is never on the command line.
        assert!(!args.iter().any(|a| a == "--api-key" || a == "k3y"));
        assert_eq!(pair("--n-gpu-layers"), "30");
        assert_eq!(pair("--ctx-size"), "8192");
        assert!(args.iter().any(|a| a == "--no-webui"));
        assert!(args.iter().any(|a| a == "--offline"));
        // The prompt cache in system memory is capped.
        let at = args.iter().position(|a| a == "--cache-ram").unwrap();
        assert_eq!(args[at + 1], "2048");
        // Context shift stays off (llama.cpp's default): with --keep 0 it
        // can drop the system prompt.
        assert!(!args.iter().any(|a| a == "--context-shift"));
        assert!(!args.iter().any(|a| a == "--ubatch-size"));
        let decision = Launch {
            batch: Some(2048),
            ..launch()
        }
        .args(1);
        let i = decision.iter().position(|a| a == "--ubatch-size").unwrap();
        assert_eq!(decision[i + 1], "2048");
        // Automatic: neither is passed, so llama.cpp's fit chooses.
        let auto = Launch {
            gpu_layers: None,
            context: None,
            ..launch()
        }
        .args(1);
        assert!(
            !auto
                .iter()
                .any(|a| a == "--n-gpu-layers" || a == "--ctx-size")
        );
    }

    #[test]
    fn idle_only_when_nothing_runs_for_a_while() {
        let now = Instant::now();
        let long_ago = now - IDLE - Duration::from_secs(1);
        assert!(should_stop(0, Some(long_ago), now));
        assert!(!should_stop(1, Some(long_ago), now), "a reply is under way");
        assert!(!should_stop(0, Some(now), now));
        assert!(!should_stop(0, None, now), "never used");
    }

    #[test]
    fn keys_and_ports() {
        let a = random_key().unwrap();
        let b = random_key().unwrap();
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
        assert!(free_port().unwrap() > 0);
    }

    #[test]
    fn the_server_outlives_the_thread_that_asked_for_it() {
        // A reply runs on a worker that ends with it: the server it started
        // must not get the parent-death signal then.
        let server = Server::new(std::env::temp_dir().join("gates-spawn.log"));
        let s = server.clone();
        let mut child = std::thread::spawn(move || {
            let mut command = Command::new("sleep");
            command.arg("30");
            // SAFETY: as in `start`.
            unsafe {
                command.pre_exec(|| {
                    libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                    Ok(())
                });
            }
            s.spawn(command).unwrap()
        })
        .join()
        .unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert!(
            matches!(child.try_wait(), Ok(None)),
            "the server died with the thread that started it"
        );
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn new_options_wait_for_the_reply_under_way() {
        let server = Server::new(std::env::temp_dir().join("gates-retire.log"));
        let mut command = Command::new("sleep");
        command.arg("30");
        let child = server.spawn(command).unwrap();
        {
            let mut state = server.state.lock().unwrap();
            state.running = Some(running(child));
            state.busy = 1;
        }
        // A reply is under way: the server stays until it ends.
        server.retire();
        assert!(server.is_running());
        server.release();
        assert!(!server.is_running());
        // Nothing under way: it goes at once.
        let mut command = Command::new("sleep");
        command.arg("30");
        let child = server.spawn(command).unwrap();
        server.state.lock().unwrap().running = Some(running(child));
        server.retire();
        assert!(!server.is_running());
    }

    #[test]
    fn a_server_that_cannot_start_says_why() {
        let dir = std::env::temp_dir().join(format!("gates-server-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        // A "server" that prints a reason and exits at once.
        let fake = dir.join("fake-server");
        fs::write(
            &fake,
            "#!/bin/sh\necho 'error: failed to load model'\nexit 1\n",
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
        let server = Server::new(dir.join("server.log"));
        let failing = Launch {
            binary: fake,
            ..launch()
        };
        let err = server.acquire(&failing).unwrap_err().to_string();
        assert!(err.contains("didn't start"), "{err}");
        assert!(err.contains("failed to load model"), "{err}");
        assert!(!server.is_running());

        let missing = Launch {
            binary: dir.join("nope"),
            ..launch()
        };
        let err = server.acquire(&missing).unwrap_err().to_string();
        assert!(err.contains("Couldn't start"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn logs_that_say_the_memory_ran_out() {
        let graphics = [
            "ggml_vulkan: Device memory allocation of size 17179869184 failed.",
            "llama_model_load: error loading model: vk::Device::allocateMemory: ErrorOutOfDeviceMemory",
            "ggml_backend_cuda_buffer_type_alloc_buffer: allocating 9000 MiB on device 0: cudaMalloc failed: out of memory",
            "alloc_tensor_range: failed to allocate Vulkan0 buffer of size 4294967296",
            "llama_init_from_model: failed to initialize the context: failed to allocate buffer for kv cache",
            // With llama.cpp's timestamp and level in front.
            "0.10.780.096 E alloc_tensor_range: failed to allocate Vulkan0 buffer of size 99",
        ];
        for log in graphics {
            assert_eq!(out_of_memory(log), Some(Memory::Graphics), "{log}");
        }
        // The computer's own memory, not the card's.
        for log in [
            "terminate called after throwing an instance of 'std::bad_alloc'",
            "load_tensors: unable to allocate CPU buffer",
            "alloc_tensor_range: failed to allocate CPU buffer of size 99",
            "ggml_aligned_malloc: insufficient memory: cannot allocate memory",
        ] {
            assert_eq!(out_of_memory(log), Some(Memory::Host), "{log}");
        }
        // A card's failure wins over a host one in the same log.
        let both = "terminate called after throwing an instance of 'std::bad_alloc'\nggml_vulkan: Device memory allocation of size 9 failed.\n";
        assert_eq!(out_of_memory(both), Some(Memory::Graphics));
        // Among a long log, anywhere.
        let log = "load: loading\nload: ok\nggml_vulkan: Failed to allocate memory\nllama: exit\n";
        assert_eq!(out_of_memory(log), Some(Memory::Graphics));
        for log in [
            "",
            "error: failed to load model",
            "common_init_from_params: failed to create context with model",
            "error loading model: unknown model architecture: 'foo'",
            "warning: failed to allocate 512 MiB of pinned memory: out of memory",
            "ggml_cuda_host_malloc: failed to allocate pinned memory",
            "listening on 127.0.0.1:8080",
        ] {
            assert_eq!(out_of_memory(log), None, "{log}");
        }
    }

    #[test]
    fn the_models_own_text_is_not_a_failure() {
        // The server dumps the GGUF's metadata and the chat template while
        // it loads: they can say anything.
        let log = "\
0.00.001.000 I llama_model_loader: - kv   3: general.description str = A model that never runs out of memory\n\
llama_model_loader: - kv  30: tokenizer.chat_template str = {% if x %}failed to allocate{% endif %}\n\
print_info: general.name = Out Of Memory 7B (failed to allocate)\n\
srv    load_model: chat template, example_format: 'it ran out of memory'\n\
main: the model said: cannot allocate memory\n\
llama_model_load: error loading model: invalid tensor shape\n";
        assert_eq!(out_of_memory(log), None);
        // The same words from a failing allocation count.
        let log = format!("{log}alloc_tensor_range: failed to allocate Vulkan0 buffer of size 1\n");
        assert_eq!(out_of_memory(&log), Some(Memory::Graphics));
    }

    #[test]
    fn host_memory_only_shrinks_the_context() {
        let big = Launch {
            context: Some(32_768),
            gpu_layers: Some(30),
            ..launch()
        };
        let (one, step) = lighter(&big, Step::Context, Memory::Host).unwrap();
        assert_eq!((one.context, one.gpu_layers), (Some(16_384), Some(30)));
        // Fewer layers on the card would put more in the computer's memory.
        assert!(lighter(&one, step, Memory::Host).is_none());
        assert!(lighter(&one, step, Memory::Graphics).is_some());
        assert_eq!(Memory::Host.noun(), "memory");
        assert_eq!(Memory::Graphics.noun(), "graphics memory");
    }

    #[test]
    fn a_load_that_runs_out_of_memory_steps_down() {
        let big = Launch {
            context: Some(32_768),
            gpu_layers: None,
            layers: 36,
            ..launch()
        };
        // First the context, halved...
        let (one, step) = lighter(&big, Step::Context, Memory::Graphics).unwrap();
        assert_eq!((one.context, one.gpu_layers), (Some(16_384), None));
        assert_eq!(step, Step::Layers);
        // ...then half the model's layers (they were left to llama.cpp)...
        let (two, step) = lighter(&one, step, Memory::Graphics).unwrap();
        assert_eq!((two.context, two.gpu_layers), (Some(16_384), Some(18)));
        assert_eq!(step, Step::Done);
        // ...and then there is nothing.
        assert!(lighter(&two, step, Memory::Graphics).is_none());
        // A context already at the floor goes straight to the layers.
        let floor = Launch {
            context: Some(MIN_CONTEXT),
            gpu_layers: Some(30),
            ..launch()
        };
        let (fewer, _) = lighter(&floor, Step::Context, Memory::Graphics).unwrap();
        assert_eq!((fewer.context, fewer.gpu_layers), (Some(4096), Some(15)));
        // "All layers" set as 99 on a 36-layer model: half of 36, not of 99.
        let all = Launch {
            context: Some(MIN_CONTEXT),
            gpu_layers: Some(99),
            ..launch()
        };
        assert_eq!(
            lighter(&all, Step::Context, Memory::Graphics)
                .unwrap()
                .0
                .gpu_layers,
            Some(18)
        );
        // Halving stops at the floor.
        let near = Launch {
            context: Some(6000),
            ..launch()
        };
        assert_eq!(
            lighter(&near, Step::Context, Memory::Graphics)
                .unwrap()
                .0
                .context,
            Some(4096)
        );
        // No layers to take: all on the processor already, or not known.
        let cpu = Launch {
            context: Some(4096),
            gpu_layers: Some(0),
            ..launch()
        };
        assert!(lighter(&cpu, Step::Context, Memory::Graphics).is_none());
        let unknown = Launch {
            context: None,
            gpu_layers: None,
            layers: 0,
            ..launch()
        };
        assert!(lighter(&unknown, Step::Context, Memory::Graphics).is_none());
    }

    #[test]
    fn what_changed_is_said_in_words() {
        let before = Launch {
            context: Some(32_768),
            gpu_layers: None,
            ..launch()
        };
        let after = Launch {
            context: Some(16_384),
            gpu_layers: Some(18),
            ..before.clone()
        };
        assert_eq!(
            changes(&before, &after),
            "a context of 16384 tokens instead of 32768 and 18 layers on the graphics card instead of Automatic"
        );
        assert_eq!(
            changes(
                &after,
                &Launch {
                    gpu_layers: Some(9),
                    ..after.clone()
                }
            ),
            "9 layers on the graphics card instead of 18"
        );
        assert_eq!(changes(&before, &before), "");
    }

    #[test]
    fn three_deaths_in_a_few_minutes_are_a_loop() {
        let start = Instant::now();
        let mut deaths = Deaths::default();
        assert!(!deaths.looping(start));
        deaths.record(start, "one".into());
        deaths.record(start + Duration::from_secs(30), "two".into());
        assert!(!deaths.looping(start + Duration::from_secs(31)));
        deaths.record(start + Duration::from_secs(60), "three".into());
        assert!(deaths.looping(start + Duration::from_secs(61)));
        assert_eq!(deaths.recent(start + Duration::from_secs(61)), 3);
        assert_eq!(deaths.last, "three");
        // Once the first falls out of the window, it may start again...
        let later = start + CRASH_WINDOW + Duration::from_secs(1);
        assert!(!deaths.looping(later));
        // ...and one more death at once makes it a loop again.
        deaths.record(later, "four".into());
        assert!(deaths.looping(later));
        // Deaths spread over a long time never add up.
        let mut slow = Deaths::default();
        for i in 0..10 {
            slow.record(start + CRASH_WINDOW * i, "x".into());
        }
        assert!(!slow.looping(start + CRASH_WINDOW * 10));
    }

    /// A directory with a fake `llama-server` in it: a shell script that
    /// notes its start (context, layers) in `starts`, then runs `body`
    /// (with `$ctx`, `$layers` and `$port` set).
    fn fake_server(name: &str, body: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("gates-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let script = format!(
            "#!/bin/sh\n\
             dir='{dir}'\n\
             while [ $# -gt 0 ]; do\n\
               case \"$1\" in --port) port=$2;; --ctx-size) ctx=$2;; --n-gpu-layers) layers=$2;; esac\n\
               shift\n\
             done\n\
             echo \"ctx=$ctx layers=$layers\" >> \"$dir/starts\"\n\
             {body}\n",
            dir = dir.display()
        );
        let fake = dir.join("fake-server");
        fs::write(&fake, script).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
        (dir, fake)
    }

    /// How many times the fake server was started.
    fn starts(dir: &Path) -> usize {
        fs::read_to_string(dir.join("starts")).map_or(0, |s| s.lines().count())
    }

    /// Answers `/health` on the port a fake server writes to `port`, until
    /// `stop`.
    fn answer_health(dir: PathBuf, stop: Arc<AtomicBool>) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            let port = loop {
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                if let Some(p) = fs::read_to_string(dir.join("port"))
                    .ok()
                    .and_then(|p| p.trim().parse::<u16>().ok())
                {
                    break p;
                }
                std::thread::sleep(Duration::from_millis(20));
            };
            let Ok(listener) = TcpListener::bind(("127.0.0.1", port)) else {
                return;
            };
            let _ = listener.set_nonblocking(true);
            while !stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut socket, _)) => {
                        use std::io::Write;
                        let _ = socket.set_nonblocking(false);
                        let _ = socket.read(&mut [0u8; 1024]);
                        let _ = socket.write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                        );
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(10)),
                }
            }
        })
    }

    #[test]
    fn a_load_without_enough_graphics_memory_is_retried_smaller() {
        // Fails (as Vulkan does) with more than 16384 tokens of context, or
        // more than 20 layers on the card.
        let (dir, fake) = fake_server(
            "oom-retry",
            "if [ \"$ctx\" -gt 16384 ] || [ \"${layers:-0}\" -gt 20 ]; then\n\
               echo 'ggml_vulkan: Device memory allocation of size 8589934592 failed.' >&2\n\
               echo 'llama_model_load: error loading model: vk::Device::allocateMemory: ErrorOutOfDeviceMemory'\n\
               exit 1\n\
             fi\n\
             echo $port > \"$dir/port\"\n\
             exec sleep 30",
        );
        let stop = Arc::new(AtomicBool::new(false));
        let health = answer_health(dir.clone(), stop.clone());
        let server = Server::new(dir.join("server.log"));
        let wanted = Launch {
            binary: fake,
            context: Some(32_768),
            gpu_layers: Some(30),
            ..launch()
        };
        let mut said = Vec::new();
        server
            .acquire_noting(&wanted, &AtomicBool::new(false), &mut |n| {
                said.push(n.to_string())
            })
            .unwrap();
        // 32768 and 30 layers, 16384 and 30, then 16384 and 15.
        assert_eq!(
            starts(&dir),
            3,
            "{:?}",
            fs::read_to_string(dir.join("starts"))
        );
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(said[0].contains("“qwen”"), "{}", said[0]);
        assert!(
            said[0].contains("a context of 16384 tokens instead of 32768"),
            "{}",
            said[0]
        );
        assert!(
            said[0].contains("15 layers on the graphics card instead of 30"),
            "{}",
            said[0]
        );
        // The same request is served by the lighter server, and the user
        // is told only once.
        let mut again = Vec::new();
        server
            .acquire_noting(&wanted, &AtomicBool::new(false), &mut |n| {
                again.push(n.to_string())
            })
            .unwrap();
        assert_eq!(starts(&dir), 3, "not restarted");
        assert!(again.is_empty(), "{again:?}");
        server.stop();
        stop.store(true, Ordering::Relaxed);
        let _ = health.join();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_load_that_never_fits_says_what_was_tried() {
        let (dir, fake) = fake_server(
            "oom-never",
            "echo 'alloc_tensor_range: failed to allocate Vulkan0 buffer of size 99' >&2\nexit 1",
        );
        let server = Server::new(dir.join("server.log"));
        let wanted = Launch {
            binary: fake,
            context: Some(16_384),
            gpu_layers: Some(30),
            ..launch()
        };
        let err = server.acquire(&wanted).unwrap_err().to_string();
        // 16384, then 8192, then 15 layers; then it gives up.
        assert_eq!(starts(&dir), 3);
        assert!(err.contains("isn't enough graphics memory"), "{err}");
        assert!(err.contains("“qwen”"), "{err}");
        assert!(err.contains("8192 tokens instead of 16384"), "{err}");
        assert!(err.contains("failed to allocate Vulkan0"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn another_failure_is_not_retried() {
        let (dir, fake) = fake_server(
            "no-retry",
            "echo 'error loading model: unknown model architecture: foo' >&2\nexit 1",
        );
        let server = Server::new(dir.join("server.log"));
        let wanted = Launch {
            binary: fake,
            context: Some(32_768),
            ..launch()
        };
        let err = server.acquire(&wanted).unwrap_err().to_string();
        assert_eq!(starts(&dir), 1);
        assert!(err.contains("unknown model architecture"), "{err}");
        assert!(!err.contains("enough memory"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_metadata_line_that_says_out_of_memory_is_not_one() {
        // The model's own description mentions it; the server fails for
        // another reason: no retry.
        let (dir, fake) = fake_server(
            "metadata",
            "echo 'llama_model_loader: - kv   3: general.description str = never runs out of memory, failed to allocate' >&2\n\
             echo 'print_info: general.name = out of memory' >&2\n\
             echo 'llama_model_load: error loading model: invalid tensor shape' >&2\n\
             exit 1",
        );
        let server = Server::new(dir.join("server.log"));
        let wanted = Launch {
            binary: fake,
            context: Some(32_768),
            gpu_layers: Some(30),
            ..launch()
        };
        let err = server.acquire(&wanted).unwrap_err().to_string();
        assert_eq!(starts(&dir), 1, "no retries");
        assert!(err.contains("invalid tensor shape"), "{err}");
        assert!(!err.contains("enough"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn running_out_of_the_computers_memory_only_shrinks_the_context() {
        // Always fails with a bad_alloc: the context goes down once, and
        // the layers on the card are left alone.
        let (dir, fake) = fake_server(
            "host-oom",
            "echo \"terminate called after throwing an instance of 'std::bad_alloc'\" >&2\nexit 134",
        );
        let server = Server::new(dir.join("server.log"));
        let wanted = Launch {
            binary: fake,
            context: Some(32_768),
            gpu_layers: Some(30),
            ..launch()
        };
        let err = server.acquire(&wanted).unwrap_err().to_string();
        assert_eq!(
            starts(&dir),
            2,
            "{:?}",
            fs::read_to_string(dir.join("starts"))
        );
        let seen = fs::read_to_string(dir.join("starts")).unwrap();
        assert!(!seen.contains("layers=15"), "{seen}");
        assert!(err.contains("isn't enough memory"), "{err}");
        assert!(!err.contains("graphics"), "{err}");
        let _ = fs::remove_dir_all(&dir);

        // And when the smaller context loads, the note says "memory".
        let (dir, fake) = fake_server(
            "host-oom-ok",
            "if [ \"$ctx\" -gt 16384 ]; then\n\
               echo 'terminate called after throwing an instance of std::bad_alloc' >&2\n\
               exit 134\n\
             fi\n\
             echo $port > \"$dir/port\"\n\
             exec sleep 30",
        );
        let stop = Arc::new(AtomicBool::new(false));
        let health = answer_health(dir.clone(), stop.clone());
        let server = Server::new(dir.join("server.log"));
        let wanted = Launch {
            binary: fake,
            context: Some(32_768),
            gpu_layers: Some(30),
            ..launch()
        };
        let mut said = Vec::new();
        server
            .acquire_noting(&wanted, &AtomicBool::new(false), &mut |n| {
                said.push(n.to_string())
            })
            .unwrap();
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(said[0].contains("enough memory to load"), "{}", said[0]);
        assert!(!said[0].contains("graphics"), "{}", said[0]);
        assert!(said[0].contains("a context of 16384 tokens"), "{}", said[0]);
        server.stop();
        stop.store(true, Ordering::Relaxed);
        let _ = health.join();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_server_that_keeps_dying_is_not_started_again() {
        let (dir, fake) = fake_server(
            "crash-loop",
            "echo 'GGML_ASSERT(x) failed: boom' >&2\nexit 1",
        );
        let server = Server::new(dir.join("server.log"));
        let wanted = Launch {
            binary: fake,
            ..launch()
        };
        for _ in 0..CRASH_LIMIT {
            let err = server.acquire(&wanted).unwrap_err().to_string();
            assert!(err.contains("didn't start"), "{err}");
        }
        assert_eq!(starts(&dir), CRASH_LIMIT);
        // The next ask is turned away without a start, naming the model and
        // quoting the last line.
        let err = server.acquire(&wanted).unwrap_err().to_string();
        assert_eq!(starts(&dir), CRASH_LIMIT, "no new start");
        assert!(err.contains("“qwen”"), "{err}");
        assert!(err.contains("won't start it again"), "{err}");
        assert!(err.contains("GGML_ASSERT(x) failed: boom"), "{err}");
        // Another model is not held against it.
        let other = Launch {
            model: PathBuf::from("/m/other.gguf"),
            ..wanted.clone()
        };
        let err = server.acquire(&other).unwrap_err().to_string();
        assert!(err.contains("didn't start"), "{err}");
        assert_eq!(starts(&dir), CRASH_LIMIT + 1);
        // A changed setting lets it try again.
        server.retire();
        let err = server.acquire(&wanted).unwrap_err().to_string();
        assert!(err.contains("didn't start"), "{err}");
        assert_eq!(starts(&dir), CRASH_LIMIT + 2);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_server_found_dead_counts_as_a_death() {
        let (dir, fake) = fake_server("found-dead", "echo 'segfault' >&2\nexit 1");
        let server = Server::new(dir.join("server.log"));
        let mut command = Command::new("true");
        command.stdout(Stdio::null());
        let child = server.spawn(command).unwrap();
        server.state.lock().unwrap().running = Some(running(child));
        std::thread::sleep(Duration::from_millis(300));
        let wanted = Launch {
            binary: fake,
            ..launch()
        };
        // The dead one is one death; the failed start is another.
        let _ = server.acquire(&wanted).unwrap_err();
        let now = Instant::now();
        let recent = server.state.lock().unwrap().crashes[&wanted.model].recent(now);
        assert_eq!(recent, 2);
        let _ = fs::remove_dir_all(&dir);
    }

    // ---- The graphics-memory watchdog

    use crate::vram::Vram;
    use crate::watchdog::{Cap, Trip, Watchdog};
    use std::sync::atomic::AtomicU64;

    const GIB: u64 = 1 << 30;

    /// A card of 24 GiB whose use (in tenths of a GiB) the test sets, and
    /// the number of times it was read.
    struct Card {
        tenths: AtomicU64,
        reads: AtomicU64,
        /// Readings to give first, in order, before `tenths`.
        script: Mutex<Vec<u64>>,
    }

    impl Card {
        fn new(used_gib: f64) -> Arc<Card> {
            Arc::new(Card {
                tenths: AtomicU64::new((used_gib * 10.0) as u64),
                reads: AtomicU64::new(0),
                script: Mutex::new(Vec::new()),
            })
        }

        fn use_gib(&self, gib: f64) {
            self.tenths.store((gib * 10.0) as u64, Ordering::Relaxed);
        }

        /// The next readings, in tenths of a GiB, before the steady use.
        fn then(&self, tenths: &[u64]) {
            *self.script.lock().unwrap() = tenths.iter().rev().copied().collect();
        }

        fn reads(&self) -> u64 {
            self.reads.load(Ordering::Relaxed)
        }

        /// A watchdog reading this card every 20 ms.
        fn watchdog(self: &Arc<Self>, cap: Cap) -> Arc<Watchdog> {
            let card = self.clone();
            Watchdog::with_reader(
                cap,
                Box::new(move || {
                    card.reads.fetch_add(1, Ordering::Relaxed);
                    let tenths = card
                        .script
                        .lock()
                        .unwrap()
                        .pop()
                        .unwrap_or_else(|| card.tenths.load(Ordering::Relaxed));
                    Some(Vram {
                        used: tenths * GIB / 10,
                        total: 24 * GIB,
                    })
                }),
                Duration::from_millis(20),
            )
        }
    }

    /// Waits up to `secs` for `done`.
    fn wait_for(secs: u64, mut done: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < deadline {
            if done() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        done()
    }

    /// A watched server in the temp folder.
    fn watched(name: &str, watchdog: &Arc<Watchdog>) -> Arc<Server> {
        Server::watched(
            std::env::temp_dir().join(format!("gates-watch-{name}-{}.log", std::process::id())),
            watchdog.clone(),
        )
    }

    /// `server` "runs" a `sleep` (or `script`, run by sh), with a reply under
    /// way.
    fn run_fake(server: &Server, script: Option<&str>) {
        let mut command = match script {
            Some(script) => {
                let mut c = Command::new("sh");
                c.args(["-c", script]);
                c
            }
            None => {
                let mut c = Command::new("sleep");
                c.arg("30");
                c
            }
        };
        command.stdin(Stdio::null());
        let child = server.spawn(command).unwrap();
        let mut state = server.state.lock().unwrap();
        state.running = Some(running(child));
        state.busy = 1;
    }

    /// Set when the watchdog says the limit is reached.
    fn flag_trips(watchdog: &Watchdog) -> Arc<AtomicBool> {
        let flag = Arc::new(AtomicBool::new(false));
        let f = flag.clone();
        watchdog.on_trip(move |_| f.store(true, Ordering::Relaxed));
        flag
    }

    #[test]
    fn a_full_card_stops_every_server_and_cancels_the_reply() {
        let card = Card::new(10.0);
        let watchdog = card.watchdog(Cap::Percent(95));
        // The chat model's server and SystemOne's.
        let chat = watched("chat", &watchdog);
        let one = watched("one", &watchdog);
        run_fake(&chat, None);
        run_fake(&one, None);

        // A reply under way: it ends when it is cancelled, as Ok.
        let cancel = Arc::new(AtomicBool::new(false));
        let reply = {
            let cancel = cancel.clone();
            std::thread::spawn(move || {
                let mut pieces = 0;
                while !cancel.load(Ordering::Relaxed) {
                    pieces += 1;
                    std::thread::sleep(Duration::from_millis(5));
                }
                pieces
            })
        };
        // What the window does when told: cancel the reply. It must still
        // find the server up (cancelled before it is stopped).
        let told: Arc<Mutex<Vec<(Trip, bool)>>> = Arc::default();
        {
            let (cancel, told, chat) = (cancel.clone(), told.clone(), chat.clone());
            watchdog.on_trip(move |trip| {
                told.lock().unwrap().push((*trip, chat.is_running()));
                cancel.store(true, Ordering::Relaxed);
            });
        }

        watchdog.wake();
        // Under the limit, it only reads.
        assert!(wait_for(2, || card.reads() >= 3));
        assert!(chat.is_running() && one.is_running());
        assert!(told.lock().unwrap().is_empty());

        card.use_gib(23.5);
        assert!(
            wait_for(3, || !chat.is_running() && !one.is_running()),
            "both servers stop"
        );
        assert!(
            reply.join().unwrap() > 0,
            "the reply was cancelled, not lost"
        );
        let told = told.lock().unwrap();
        assert_eq!(told.len(), 1, "told once");
        assert_eq!(told[0].0.percent, 95);
        assert!(
            told[0].1,
            "the server was still up when the window was told"
        );
        assert_eq!(watchdog.last_trip(), Some(told[0].0));
        assert!(
            wait_for(2, || !watchdog.watching()),
            "no server, no watching"
        );
    }

    #[test]
    fn a_spike_does_not_stop_the_server() {
        let card = Card::new(10.0);
        let watchdog = card.watchdog(Cap::Percent(95));
        let server = watched("spike", &watchdog);
        run_fake(&server, None);
        let told = flag_trips(&watchdog);
        watchdog.wake();
        assert!(wait_for(2, || card.reads() >= 2));
        // Over now and then, never twice in a row.
        let reads = card.reads();
        card.then(&[238, 100, 239, 100, 240, 100, 238]);
        assert!(wait_for(2, || card.reads() >= reads + 12));
        assert!(server.is_running());
        assert!(!told.load(Ordering::Relaxed));
        server.stop();
    }

    #[test]
    fn it_watches_only_while_a_server_runs() {
        let card = Card::new(10.0);
        let watchdog = card.watchdog(Cap::Percent(95));
        let server = watched("only", &watchdog);
        // Nothing runs: nothing is read.
        watchdog.wake();
        assert!(wait_for(2, || !watchdog.watching()));
        let reads = card.reads();
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(card.reads(), reads, "no reads without a server");
        // A server runs: it reads; it stops: the thread ends.
        run_fake(&server, None);
        watchdog.wake();
        assert!(wait_for(2, || card.reads() >= reads + 3));
        server.stop();
        assert!(wait_for(2, || !watchdog.watching()));
        let reads = card.reads();
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(card.reads(), reads);
    }

    #[test]
    fn off_says_nothing_until_a_limit_is_set() {
        let card = Card::new(23.9);
        let watchdog = card.watchdog(Cap::Off);
        let server = watched("off", &watchdog);
        run_fake(&server, None);
        let told = flag_trips(&watchdog);
        watchdog.wake();
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(card.reads(), 0, "Off reads nothing");
        assert!(server.is_running() && !told.load(Ordering::Relaxed));
        // Turned on with the card full and the server up: it stops.
        watchdog.set_cap(Cap::Percent(90));
        assert!(wait_for(3, || !server.is_running()));
        assert!(told.load(Ordering::Relaxed));
    }

    #[test]
    fn a_card_with_nothing_to_read_says_nothing() {
        // No `mem_info` (not AMD, the dev container, Xvfb).
        let blind =
            Watchdog::with_reader(Cap::default(), Box::new(|| None), Duration::from_millis(20));
        let server = watched("blind", &blind);
        run_fake(&server, None);
        let told = flag_trips(&blind);
        blind.wake();
        assert!(wait_for(2, || !blind.watching()), "the thread ends");
        assert!(server.is_running() && !told.load(Ordering::Relaxed));
        assert_eq!(blind.last_trip(), None);
        // And a start is not held up by it.
        assert_eq!(blind.check_start(), Ok(()));
        server.stop();
    }

    #[test]
    fn a_server_that_ignores_the_quit_is_killed() {
        let card = Card::new(23.9);
        let watchdog = card.watchdog(Cap::Percent(95));
        let server = watched("kill", &watchdog);
        // Deaf to SIGTERM.
        run_fake(&server, Some("trap '' TERM; while true; do sleep 1; done"));
        std::thread::sleep(Duration::from_millis(300));
        let started = Instant::now();
        watchdog.wake();
        assert!(wait_for(8, || !server.is_running()), "killed");
        assert!(
            started.elapsed() < Duration::from_secs(7),
            "after about 3 s: {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_nearly_full_card_refuses_a_start() {
        let (dir, fake) = fake_server("watch-refuse", "echo $port > \"$dir/port\"\nexec sleep 30");
        let stop = Arc::new(AtomicBool::new(false));
        let health = answer_health(dir.clone(), stop.clone());
        // 22 GiB of 24 is 92%: over the 90% a start needs under a 95% limit.
        let card = Card::new(22.0);
        let watchdog = card.watchdog(Cap::Percent(95));
        let server = Server::watched(dir.join("server.log"), watchdog.clone());
        let wanted = Launch {
            binary: fake,
            ..launch()
        };
        let err = server.acquire(&wanted).unwrap_err().to_string();
        assert!(err.starts_with("Not starting the model"), "{err}");
        assert!(
            err.contains("92%") && err.contains("22.0 of 24.0 GiB"),
            "{err}"
        );
        assert_eq!(starts(&dir), 0, "never started");
        assert!(!server.is_running());
        // Below the margin it starts.
        card.use_gib(21.0);
        server.acquire(&wanted).unwrap();
        server.release();
        assert_eq!(starts(&dir), 1);
        server.stop();
        stop.store(true, Ordering::Relaxed);
        let _ = health.join();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn with_no_limit_a_server_starts_at_any_use() {
        let (dir, fake) = fake_server("watch-off", "echo $port > \"$dir/port\"\nexec sleep 30");
        let stop = Arc::new(AtomicBool::new(false));
        let health = answer_health(dir.clone(), stop.clone());
        let card = Card::new(23.5);
        let watchdog = card.watchdog(Cap::Off);
        let server = Server::watched(dir.join("server.log"), watchdog.clone());
        let wanted = Launch {
            binary: fake,
            ..launch()
        };
        server.acquire(&wanted).unwrap();
        server.release();
        assert_eq!(starts(&dir), 1);
        assert!(server.is_running(), "and it is left alone");
        // A limit set while it runs stops it, and the next start is refused.
        watchdog.set_cap(Cap::Percent(95));
        assert!(wait_for(3, || !server.is_running()));
        let err = server.acquire(&wanted).unwrap_err().to_string();
        assert!(err.starts_with("Not starting the model"), "{err}");
        assert_eq!(starts(&dir), 1);
        stop.store(true, Ordering::Relaxed);
        let _ = health.join();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_load_that_fills_the_card_is_stopped() {
        // A server that never gets ready: it is loading when the card fills.
        let (dir, fake) = fake_server("watch-load", "echo $port > \"$dir/port\"\nexec sleep 30");
        let card = Card::new(10.0);
        let watchdog = card.watchdog(Cap::Percent(95));
        let server = Server::watched(dir.join("server.log"), watchdog.clone());
        let wanted = Launch {
            binary: fake,
            ..launch()
        };
        let loading = {
            let (server, wanted) = (server.clone(), wanted.clone());
            std::thread::spawn(move || server.acquire(&wanted))
        };
        assert!(wait_for(5, || starts(&dir) == 1), "it started loading");
        // The lock is held for the whole load: the watchdog sees it anyway.
        let reads = card.reads();
        assert!(wait_for(2, || card.reads() > reads));
        card.use_gib(23.9);
        let started = Instant::now();
        let err = loading.join().unwrap().unwrap_err().to_string();
        assert!(
            started.elapsed() < Duration::from_secs(8),
            "{:?}",
            started.elapsed()
        );
        assert!(
            err.starts_with("Stopped the model: graphics memory reached 95%"),
            "{err}"
        );
        assert!(!server.is_running());
        // Not a crash of the model.
        assert!(server.state.lock().unwrap().crashes.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }
}
