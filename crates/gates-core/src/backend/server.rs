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
use std::fs::{self, File};
use std::io::{self, Read};
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
    launch: Launch,
    endpoint: Endpoint,
}

#[derive(Default)]
struct State {
    running: Option<Running>,
    /// Replies under way: the server is not idle while one is.
    busy: usize,
    /// Started with options that changed since: it stops when the last
    /// reply using it ends, and the next one starts a new one.
    stale: bool,
    last_used: Option<Instant>,
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
                loop {
                    std::thread::sleep(Duration::from_secs(15));
                    let Some(server) = weak.upgrade() else {
                        return;
                    };
                    server.stop_if_idle(Instant::now());
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
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let alive = match state.running.as_mut() {
            Some(r) => r.launch == *launch && matches!(r.child.try_wait(), Ok(None)),
            None => false,
        } && !state.stale;
        if !alive {
            state.stale = false;
            if let Some(old) = state.running.take() {
                stop(old.child);
            }
            state.running = Some(self.start(launch, cancel)?);
        }
        let Some(endpoint) = state.running.as_ref().map(|r| r.endpoint.clone()) else {
            return Err(BackendError::Other(
                "The model server isn't running.".into(),
            ));
        };
        state.busy += 1;
        state.last_used = Some(Instant::now());
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

    fn start(&self, launch: &Launch, cancel: &AtomicBool) -> Result<Running, BackendError> {
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
            Ok(()) => Ok(Running {
                child,
                launch: launch.clone(),
                endpoint,
            }),
            Err(why) => {
                stop(child);
                let last = last_line(&self.log);
                Err(BackendError::Unreachable(match last {
                    Some(line) => format!("The model server didn't start: {why} It said: {line}"),
                    None => format!("The model server didn't start: {why}"),
                }))
            }
        }
    }
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
) -> Result<(), String> {
    let agent = super::llama::agent(Some(Duration::from_secs(2)));
    let url = format!("{base}/health");
    let deadline = Instant::now() + limit;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("it was stopped while it loaded.".into());
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!("it stopped ({status})."));
        }
        if let Ok(response) = agent.get(&url).call()
            && response.status().as_u16() == 200
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "it wasn't ready after {} seconds.",
                limit.as_secs()
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

/// The last non-empty line of the log, shortened: why a start failed.
fn last_line(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let line = text.lines().rev().map(str::trim).find(|l| !l.is_empty())?;
    Some(line.chars().take(200).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launch() -> Launch {
        Launch {
            binary: PathBuf::from("/usr/libexec/telamon-llama/llama-server"),
            model: PathBuf::from("/m/qwen.gguf"),
            gpu_layers: Some(30),
            context: Some(8192),
            batch: None,
            projector: None,
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
            state.running = Some(Running {
                child,
                launch: launch(),
                endpoint: Endpoint {
                    base: "http://127.0.0.1:1".into(),
                    api_key: String::new(),
                },
            });
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
        server.state.lock().unwrap().running = Some(Running {
            child,
            launch: launch(),
            endpoint: Endpoint {
                base: "http://127.0.0.1:1".into(),
                api_key: String::new(),
            },
        });
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
}
