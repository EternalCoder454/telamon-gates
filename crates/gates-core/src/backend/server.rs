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

/// The `launch` to retry with after a load that ran out of memory, from
/// step `from` on, and the step after it. A smaller context first (halved,
/// down to `MIN_CONTEXT`), then half the GPU layers (of the model's, when
/// they were left to llama.cpp). None when there is nothing lighter.
pub fn lighter(launch: &Launch, from: Step) -> Option<(Launch, Step)> {
    if from == Step::Context
        && let Some(context) = launch.context.filter(|c| *c > MIN_CONTEXT)
    {
        let smaller = Launch {
            context: Some((context / 2).max(MIN_CONTEXT)),
            ..launch.clone()
        };
        return Some((smaller, Step::Layers));
    }
    if from <= Step::Layers {
        let layers = launch
            .gpu_layers
            .or((launch.layers > 0).then_some(launch.layers))
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

/// Whether a server's log (or its tail) says it ran out of memory: llama.cpp
/// and its backends say it in several ways ("failed to allocate",
/// "ErrorOutOfDeviceMemory", CUDA's "out of memory"). Pinned-memory
/// warnings, which don't stop a load, don't count.
pub fn out_of_memory(log: &str) -> bool {
    const SIGNS: [&str; 8] = [
        "out of memory",
        "outofdevicememory",
        "outofhostmemory",
        "failed to allocate",
        "unable to allocate",
        "cannot allocate memory",
        "device memory allocation",
        "bad_alloc",
    ];
    log.lines().any(|line| {
        let line = line.to_ascii_lowercase();
        !line.contains("pinned") && SIGNS.iter().any(|sign| line.contains(sign))
    })
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
        self.crashes.entry(model).or_default().record(now, last);
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
}

impl Server {
    /// A server that is not started yet, and the thread that stops it when
    /// idle (it ends with the server).
    pub fn new(log: PathBuf) -> Arc<Server> {
        Server::with_load_limit(log, LOAD)
    }

    /// As `new`, giving up on a start after `load` (a small model that
    /// isn't up quickly won't be).
    pub fn with_load_limit(log: PathBuf, load: Duration) -> Arc<Server> {
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
        });
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
            match self.start(launch, cancel) {
                Ok(running) => state.running = Some(running),
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
        loop {
            let mut failed = match self.start_once(&current, cancel) {
                Ok((child, endpoint)) => {
                    let note = (current != *requested).then(|| {
                        format!(
                            "There wasn't enough graphics memory to load “{}” as set, so it loaded with {}.",
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
            if !failed.out_of_memory {
                return Err(failed);
            }
            if let Some((next, after)) = lighter(&current, step) {
                log::warn!(
                    "{} ran out of memory loading: retrying with {}",
                    model_name(&requested.model),
                    changes(&current, &next)
                );
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
                "There isn't enough memory to load “{}”{tried}. Try a smaller model, or a smaller context in Settings.{line}",
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
        match wait_ready(&mut child, &endpoint.base, self.load, cancel) {
            Ok(()) => Ok((child, endpoint)),
            Err(unready) => {
                stop(child);
                let last = last_line(&self.log);
                // Only a server that ended on its own says why in its log.
                let out_of_memory =
                    unready.exited && out_of_memory(&read_tail(&self.log, LOG_TAIL));
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
    out_of_memory: bool,
}

impl From<BackendError> for StartError {
    fn from(error: BackendError) -> StartError {
        StartError {
            error,
            exited: false,
            out_of_memory: false,
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
) -> Result<(), Unready> {
    let agent = super::llama::agent(Some(Duration::from_secs(2)));
    let url = format!("{base}/health");
    let deadline = Instant::now() + limit;
    let waiting = |why: String, exited: bool| Unready { why, exited };
    loop {
        if cancel.load(Ordering::Relaxed) {
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
        for log in [
            "ggml_vulkan: Device memory allocation of size 17179869184 failed.",
            "llama_model_load: error loading model: vk::Device::allocateMemory: ErrorOutOfDeviceMemory",
            "ggml_backend_cuda_buffer_type_alloc_buffer: allocating 9000 MiB on device 0: cudaMalloc failed: out of memory",
            "alloc_tensor_range: failed to allocate Vulkan0 buffer of size 4294967296",
            "llama_init_from_model: failed to initialize the context: failed to allocate buffer for kv cache",
            "terminate called after throwing an instance of 'std::bad_alloc'",
            "load_tensors: unable to allocate CPU buffer",
        ] {
            assert!(out_of_memory(log), "{log}");
        }
        // Among a long log, anywhere.
        let log = "load: loading\nload: ok\nggml_vulkan: Failed to allocate memory\nllama: exit\n";
        assert!(out_of_memory(log));
        for log in [
            "",
            "error: failed to load model",
            "common_init_from_params: failed to create context with model",
            "error loading model: unknown model architecture: 'foo'",
            "warning: failed to allocate 512 MiB of pinned memory: out of memory",
            "listening on 127.0.0.1:8080",
        ] {
            assert!(!out_of_memory(log), "{log}");
        }
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
        let (one, step) = lighter(&big, Step::Context).unwrap();
        assert_eq!((one.context, one.gpu_layers), (Some(16_384), None));
        assert_eq!(step, Step::Layers);
        // ...then half the model's layers (they were left to llama.cpp)...
        let (two, step) = lighter(&one, step).unwrap();
        assert_eq!((two.context, two.gpu_layers), (Some(16_384), Some(18)));
        assert_eq!(step, Step::Done);
        // ...and then there is nothing.
        assert!(lighter(&two, step).is_none());
        // A context already at the floor goes straight to the layers.
        let floor = Launch {
            context: Some(MIN_CONTEXT),
            gpu_layers: Some(30),
            ..launch()
        };
        let (fewer, _) = lighter(&floor, Step::Context).unwrap();
        assert_eq!((fewer.context, fewer.gpu_layers), (Some(4096), Some(15)));
        // Halving stops at the floor.
        let near = Launch {
            context: Some(6000),
            ..launch()
        };
        assert_eq!(lighter(&near, Step::Context).unwrap().0.context, Some(4096));
        // No layers to take: all on the processor already, or not known.
        let cpu = Launch {
            context: Some(4096),
            gpu_layers: Some(0),
            ..launch()
        };
        assert!(lighter(&cpu, Step::Context).is_none());
        let unknown = Launch {
            context: None,
            gpu_layers: None,
            layers: 0,
            ..launch()
        };
        assert!(lighter(&unknown, Step::Context).is_none());
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
        assert!(err.contains("isn't enough memory"), "{err}");
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
}
