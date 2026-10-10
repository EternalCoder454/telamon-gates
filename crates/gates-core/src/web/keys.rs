//! Where the search API key lives: the system keyring, through the Secret
//! Service API (KWallet answers it on Plasma, GNOME Keyring elsewhere), with
//! libsecret's `secret-tool`. Never the settings file, never a conversation,
//! never a log.
//!
//! `secret-tool` is a separate process, so the app carries no D-Bus or
//! crypto code for this (an in-process client, `oo7` with `zbus`, was 5 MB of
//! the binary). The secret goes to it on standard input, never in its
//! arguments, where any process could read it.
//!
//! Without a keyring (the dev container, CI, a minimal session) there is no
//! key to keep: Settings says so and the key isn't saved anywhere. The rest
//! of the app talks to the `KeyStore` trait, so tests use `Memory`.

use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant};

/// The name the web search key is kept under.
pub const SEARCH_KEY: &str = "web-search-key";

/// The application attribute of every item Gates keeps.
const APPLICATION: &str = "net.eterneon.telamon.gates";

/// What the keyring shows for the item.
const LABEL: &str = "Telamon Gates: web search key";

/// libsecret's tool, by its full path: a program found on `$PATH` would be
/// handed the key.
const SECRET_TOOL: &str = "/usr/bin/secret-tool";

/// A locked keyring may ask the user for a password; that is waited for this
/// long, then `secret-tool` is stopped.
const WAIT: Duration = Duration::from_secs(90);

/// How often `clear` and `lookup` are tried when a name has more than one item.
const FORGET_TRIES: usize = 5;

/// The most a secret may be: API keys are far shorter, and a bound keeps the
/// write to the tool's pipe from ever blocking.
const MAX_SECRET: usize = 8 * 1024;

/// A place to keep secrets. Calls block (a locked keyring asks the user):
/// call them from a worker, never the GUI thread.
pub trait KeyStore: Send + Sync {
    /// Ok when a keyring answers; else why not, as a sentence.
    fn check(&self) -> Result<(), String>;
    /// The secret kept under `name`; None when there is none.
    fn get(&self, name: &str) -> Result<Option<String>, String>;
    /// Keeps `secret` under `name`, replacing one there.
    fn set(&self, name: &str, secret: &str) -> Result<(), String>;
    /// Forgets `name`; fine when there was none.
    fn remove(&self, name: &str) -> Result<(), String>;
}

/// The system keyring, through `secret-tool`.
#[derive(Debug, Clone)]
pub struct SecretService {
    program: PathBuf,
    wait: Duration,
}

impl Default for SecretService {
    fn default() -> Self {
        SecretService {
            program: PathBuf::from(SECRET_TOOL),
            wait: WAIT,
        }
    }
}

/// What a finished `secret-tool` left.
struct Done {
    success: bool,
    stdout: Vec<u8>,
    stderr: String,
}

fn attributes(name: &str) -> [&str; 4] {
    ["application", APPLICATION, "name", name]
}

/// Writing to a pipe whose reader has gone raises SIGPIPE, which ends a
/// process that doesn't ignore it. Rust's own `main` ignores it; this app's
/// `main` is C++ over a Rust static library, so nothing has. Ignored once and
/// for all (nothing here wants the default: every write is checked), and std
/// puts it back to the default in each child it starts. Writing from another
/// thread would not help: the signal ends the whole process.
pub fn ignore_sigpipe() {
    // SAFETY: signal(2) with SIG_IGN installs no handler code.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
}

/// The secret to the tool's standard input, which is then closed (when `pipe`
/// drops), so the tool sees the end of it. A tool that exits without reading
/// is told by its status, not by this.
fn feed(mut pipe: impl Write, text: &str) {
    ignore_sigpipe();
    let _ = pipe.write_all(text.as_bytes());
}

/// Reads a pipe to its end on a thread of its own.
fn drain(mut pipe: impl Read + Send + 'static) -> mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = pipe.read_to_end(&mut bytes);
        let _ = tx.send(bytes);
    });
    rx
}

impl SecretService {
    /// Runs the tool with `args` (and `stdin` on its standard input), for at
    /// most `self.wait`: after that it is killed. This thread keeps the
    /// `Child` until it has been reaped, so the kill can only ever reach
    /// this process (a pid kept apart from it could be reused).
    fn run(&self, args: &[&str], stdin: Option<&str>) -> Result<Done, String> {
        let mut child = Command::new(&self.program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                log::debug!("keyring: cannot run {}: {e}", self.program.display());
                if e.kind() == ErrorKind::NotFound {
                    NO_KEYRING.to_string()
                } else {
                    format!("The system keyring can't be reached: {e}.")
                }
            })?;
        let pipe = child.stdin.take();
        if let (Some(pipe), Some(text)) = (pipe, stdin) {
            feed(pipe, text);
        }
        let out = child.stdout.take().map(drain);
        let err = child.stderr.take().map(drain);
        let deadline = Instant::now() + self.wait;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {}
                Err(e) => return Err(format!("The system keyring can't be used: {e}.")),
            }
            if Instant::now() >= deadline {
                // Waiting for a password nobody gives.
                let _ = child.kill();
                let _ = child.wait();
                return Err(
                    "The system keyring didn't answer. Is it waiting to be unlocked?".to_string(),
                );
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        // The pipes end with the process (a child it left behind could hold
        // them open: not waited for).
        let grab = |rx: Option<mpsc::Receiver<Vec<u8>>>| {
            rx.and_then(|rx| rx.recv_timeout(Duration::from_secs(2)).ok())
                .unwrap_or_default()
        };
        Ok(Done {
            success: status.success(),
            stdout: grab(out),
            stderr: String::from_utf8_lossy(&grab(err)).trim().to_string(),
        })
    }

    /// Forgets every item of `name`. The tool's `clear` may leave a second
    /// item (a key saved by 1.2 has no `xdg:schema` attribute, which every
    /// item `secret-tool store` makes has, so the two are never replacements
    /// of each other, while `lookup` and `clear` match on `application` and
    /// `name` alone): clear and look again, until nothing is found.
    fn forget(&self, name: &str) -> Result<(), String> {
        for _ in 0..FORGET_TRIES {
            let mut args = vec!["clear"];
            args.extend(attributes(name));
            let done = self.run(&args, None)?;
            if !done.success {
                return Err(failure(&done.stderr));
            }
            if self.get(name)?.is_none() {
                return Ok(());
            }
        }
        Err("The key couldn't be removed from the system keyring.".to_string())
    }
}

/// A sentence for what `secret-tool` said (its first line: plain text that
/// holds no secret).
fn failure(stderr: &str) -> String {
    log::debug!("keyring: {stderr}");
    let line = stderr.lines().next().unwrap_or_default().trim();
    let line = line.strip_prefix("secret-tool:").unwrap_or(line).trim();
    if line.is_empty() {
        return "The system keyring can't be used.".to_string();
    }
    let line: String = line.chars().take(200).collect();
    format!(
        "The system keyring can't be used: {}.",
        line.trim_end_matches('.')
    )
}

impl KeyStore for SecretService {
    fn check(&self) -> Result<(), String> {
        // A search that finds nothing reaches the service and opens and
        // unlocks no wallet, so nothing asks the user anything. Exit 1 with
        // nothing said is "not found"; with a message, a service that isn't
        // there.
        let mut args = vec!["lookup"];
        args.extend(attributes("keyring-check"));
        let done = self.run(&args, None).map_err(|detail| {
            log::debug!("keyring: {detail}");
            if detail.contains("didn't answer") {
                detail
            } else {
                NO_KEYRING.to_string()
            }
        })?;
        if done.success || done.stderr.is_empty() {
            return Ok(());
        }
        // The bus's own words ("The name org.freedesktop.secrets was not
        // provided by any .service files") go to the log, not the user.
        log::debug!("keyring: {}", done.stderr);
        Err(NO_KEYRING.to_string())
    }

    fn get(&self, name: &str) -> Result<Option<String>, String> {
        let mut args = vec!["lookup"];
        args.extend(attributes(name));
        let done = self.run(&args, None)?;
        if done.success {
            return Ok(Some(String::from_utf8_lossy(&done.stdout).into_owned()));
        }
        if done.stderr.is_empty() {
            // Nothing found.
            return Ok(None);
        }
        Err(failure(&done.stderr))
    }

    fn set(&self, name: &str, secret: &str) -> Result<(), String> {
        if secret.len() > MAX_SECRET || secret.contains('\0') {
            return Err("That key can't be kept: it is too long or holds odd characters.".into());
        }
        // Replacing is clear, then store: `store` only replaces an item with
        // the very same attributes, and a 1.2 item has fewer.
        self.forget(name)?;
        let label = format!("--label={LABEL}");
        let mut args = vec!["store", label.as_str()];
        args.extend(attributes(name));
        let done = self.run(&args, Some(secret))?;
        if done.success {
            Ok(())
        } else {
            Err(failure(&done.stderr))
        }
    }

    fn remove(&self, name: &str) -> Result<(), String> {
        self.forget(name)
    }
}

/// Secrets in memory, for tests and demos. Nothing is written anywhere.
#[derive(Debug, Default)]
pub struct Memory {
    items: Mutex<HashMap<String, String>>,
}

impl KeyStore for Memory {
    fn check(&self) -> Result<(), String> {
        Ok(())
    }

    fn get(&self, name: &str) -> Result<Option<String>, String> {
        Ok(self
            .items
            .lock()
            .map_err(|_| "poisoned")?
            .get(name)
            .cloned())
    }

    fn set(&self, name: &str, secret: &str) -> Result<(), String> {
        self.items
            .lock()
            .map_err(|_| "poisoned")?
            .insert(name.to_string(), secret.to_string());
        Ok(())
    }

    fn remove(&self, name: &str) -> Result<(), String> {
        self.items.lock().map_err(|_| "poisoned")?.remove(name);
        Ok(())
    }
}

/// No keyring at all: every call says so. What the dev container and CI have.
#[derive(Debug, Default, Clone, Copy)]
pub struct Missing;

pub const NO_KEYRING: &str =
    "No system keyring is running (such as KWallet), so the key can't be kept safely.";

impl KeyStore for Missing {
    fn check(&self) -> Result<(), String> {
        Err(NO_KEYRING.into())
    }
    fn get(&self, _: &str) -> Result<Option<String>, String> {
        Err(NO_KEYRING.into())
    }
    fn set(&self, _: &str, _: &str) -> Result<(), String> {
        Err(NO_KEYRING.into())
    }
    fn remove(&self, _: &str) -> Result<(), String> {
        Err(NO_KEYRING.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::sync::OnceLock;

    #[test]
    fn the_memory_store_keeps_replaces_and_forgets() {
        let store = Memory::default();
        assert_eq!(store.get(SEARCH_KEY), Ok(None));
        store.set(SEARCH_KEY, "first").unwrap();
        store.set(SEARCH_KEY, "second").unwrap();
        assert_eq!(store.get(SEARCH_KEY), Ok(Some("second".into())));
        store.remove(SEARCH_KEY).unwrap();
        store.remove(SEARCH_KEY).unwrap();
        assert_eq!(store.get(SEARCH_KEY), Ok(None));
    }

    #[test]
    fn no_keyring_means_no_key() {
        let store: &dyn KeyStore = &Missing;
        assert!(store.check().unwrap_err().contains("keyring"));
        // Saving says no, and nothing is kept anywhere.
        assert!(store.set(SEARCH_KEY, "test-key-not-real").is_err());
        assert!(store.get(SEARCH_KEY).is_err());
    }

    /// Where there is no `secret-tool`, session bus or Secret Service (the
    /// dev container, CI), the real store reports it instead of hanging or
    /// panicking. Where there is one, this only checks that the call comes
    /// back.
    #[test]
    fn the_system_keyring_answers_or_says_why_not() {
        let started = std::time::Instant::now();
        match SecretService::default().check() {
            Ok(()) => {}
            Err(e) => assert!(e.contains("keyring"), "{e}"),
        }
        assert!(started.elapsed() < Duration::from_secs(30));
    }

    /// A stand-in for `secret-tool`, kept in a folder of its own, that keeps
    /// what libsecret does: an item is a file `<kind>__<attributes>` in
    /// `state/items`, and `store` makes (replaces) only `schema__...`, as the
    /// real one adds an `xdg:schema` attribute to what it stores. `lookup`
    /// finds the first item with the attributes it is given, whatever its
    /// kind (exit 1 and no words when there is none), and `clear` removes
    /// just one such item, the worst case. A kind of another name is an
    /// item made some other way (1.2's, which had no schema). The secret
    /// comes from standard input, every call's arguments go to `argv.log`,
    /// the names `broken-*` fail with a message and `hang-*` never answer.
    const FAKE: &str = r#"#!/bin/bash
dir=$(dirname "$0")/state
mkdir -p "$dir/items"
printf '%s\n' "$*" >> "$dir/argv.log"
cmd=$1; shift
[ "$cmd" = store ] && shift
key=$(printf '%s' "$*" | tr -c 'A-Za-z0-9.-' '_')
case "$*" in
*broken-*) echo "secret-tool: Cannot create an item in a locked collection" >&2; exit 1 ;;
*hang-*) exec sleep 30 ;;
esac
first=$(ls "$dir/items"/*__"$key" 2>/dev/null | head -n1)
case $cmd in
store) cat > "$dir/items/schema__$key" ;;
lookup) [ -n "$first" ] || exit 1; cat "$first" ;;
clear) [ -z "$first" ] || rm -f "$first" ;;
*) echo "bad command" >&2; exit 2 ;;
esac
"#;

    /// The stand-in's folder, written once before any test runs it (a file
    /// still open for writing can't be executed).
    fn fake() -> &'static Path {
        static DIR: OnceLock<PathBuf> = OnceLock::new();
        DIR.get_or_init(|| {
            let dir = std::env::temp_dir().join(format!("gates-fake-secret-tool-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let script = dir.join("secret-tool");
            std::fs::write(&script, FAKE).unwrap();
            // A service that isn't there: the tool says so on standard error.
            let down = dir.join("secret-tool-down");
            std::fs::write(
                &down,
                "#!/bin/sh\necho 'secret-tool: The name org.freedesktop.secrets was not provided' >&2\nexit 1\n",
            )
            .unwrap();
            for tool in [&script, &down] {
                std::fs::set_permissions(tool, std::fs::Permissions::from_mode(0o700)).unwrap();
                // Another test's fork may still hold the file open for a moment.
                for _ in 0..100 {
                    match Command::new(tool).arg("lookup").output() {
                        Err(e) if e.kind() == ErrorKind::ExecutableFileBusy => {
                            std::thread::sleep(Duration::from_millis(20))
                        }
                        _ => break,
                    }
                }
            }
            dir
        })
    }

    fn faked(wait: Duration) -> SecretService {
        SecretService {
            program: fake().join("secret-tool"),
            wait,
        }
    }

    fn argv_log() -> String {
        std::fs::read_to_string(fake().join("state/argv.log")).unwrap_or_default()
    }

    /// The stand-in's file name part for an item of `name`.
    fn item_key(name: &str) -> String {
        format!("application {APPLICATION} name {name}")
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    }

    /// An item of `name` made some other way than by `store` (of `kind`).
    fn legacy(name: &str, kind: &str, secret: &str) {
        let dir = fake().join("state/items");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{kind}__{}", item_key(name))), secret).unwrap();
    }

    /// How many items `name` has.
    fn items(name: &str) -> usize {
        let suffix = format!("__{}", item_key(name));
        std::fs::read_dir(fake().join("state/items"))
            .map(|d| {
                d.filter_map(Result::ok)
                    .filter(|e| e.file_name().to_string_lossy().ends_with(&suffix))
                    .count()
            })
            .unwrap_or(0)
    }

    #[test]
    fn a_key_saved_by_1_2_is_replaced_and_fully_removed() {
        let store = faked(Duration::from_secs(20));
        // 1.2's item has no schema, so a store beside it would not replace it.
        let name = "web-search-key-from-1-2";
        legacy(name, "plain", "old-key-not-real");
        assert_eq!(store.get(name), Ok(Some("old-key-not-real".into())));
        store.set(name, "new-key-not-real").unwrap();
        assert_eq!(store.get(name), Ok(Some("new-key-not-real".into())));
        assert_eq!(items(name), 1, "the old item is gone, not shadowing");
        // Saving again replaces, as before.
        store.set(name, "newer-key-not-real").unwrap();
        assert_eq!(items(name), 1);
        assert_eq!(store.get(name), Ok(Some("newer-key-not-real".into())));

        // Remove takes every item, however many `clear` leaves behind.
        legacy(name, "plain", "old-key-not-real");
        legacy(name, "other", "older-key-not-real");
        assert_eq!(items(name), 3);
        store.remove(name).unwrap();
        assert_eq!(items(name), 0);
        assert_eq!(store.get(name), Ok(None));
    }

    #[test]
    fn a_key_that_will_not_go_is_an_error() {
        let store = faked(Duration::from_secs(20));
        let name = "web-search-key-stubborn";
        for kind in ["a", "b", "c", "d", "e", "f"] {
            legacy(name, kind, "old-key-not-real");
        }
        // Five tries of one item each: one is left, and it is said.
        let e = store.remove(name).unwrap_err();
        assert!(e.contains("couldn't be removed"), "{e}");
        assert_eq!(items(name), 1);
        assert!(
            store.set(name, "new-key-not-real").is_ok(),
            "the last one goes"
        );
    }

    #[test]
    fn a_closed_pipe_does_not_end_the_process() {
        use std::os::fd::FromRawFd;
        // The app's main() is C++: SIGPIPE has its default action there.
        // SAFETY: a fresh pipe, whose read end is closed, and signal(2).
        let write_end = unsafe {
            let mut fds = [0; 2];
            assert_eq!(libc::pipe(fds.as_mut_ptr()), 0);
            libc::close(fds[0]);
            libc::signal(libc::SIGPIPE, libc::SIG_DFL);
            std::fs::File::from_raw_fd(fds[1])
        };
        // Writing to it would kill this test process with the default action.
        feed(write_end, "test-key-not-real");
        // SAFETY: signal(2); the old action comes back to be looked at.
        let was = unsafe { libc::signal(libc::SIGPIPE, libc::SIG_IGN) };
        assert_eq!(was, libc::SIG_IGN);
    }

    #[test]
    fn secret_tool_keeps_replaces_and_forgets_a_key() {
        let store = faked(Duration::from_secs(20));
        let name = "web-search-key-roundtrip";
        assert_eq!(store.get(name), Ok(None));
        store.set(name, "first-test-key-not-real").unwrap();
        store.set(name, "second-test-key-not-real").unwrap();
        assert_eq!(store.get(name), Ok(Some("second-test-key-not-real".into())));
        store.remove(name).unwrap();
        store.remove(name).unwrap();
        assert_eq!(store.get(name), Ok(None));
    }

    #[test]
    fn the_secret_is_never_in_the_arguments() {
        let store = faked(Duration::from_secs(20));
        let secret = "sk-test-secret-argv-check-not-real";
        store.set("web-search-key-argv", secret).unwrap();
        // The stand-in got it on standard input...
        assert_eq!(
            store.get("web-search-key-argv"),
            Ok(Some(secret.to_string()))
        );
        // ...and every call's arguments are in its log, without it.
        let log = argv_log();
        assert!(log.contains("store --label=Telamon Gates: web search key application net.eterneon.telamon.gates name web-search-key-argv"));
        assert!(
            log.contains("lookup application net.eterneon.telamon.gates name web-search-key-argv")
        );
        assert!(!log.contains(secret), "{log}");
        // A secret with a line break or spaces is kept whole.
        store.set("web-search-key-odd", "a b\nc").unwrap();
        assert_eq!(
            store.get("web-search-key-odd"),
            Ok(Some("a b\nc".to_string()))
        );
        // And one that can't be (NUL, or too long) is refused before any run.
        assert!(store.set("web-search-key-nul", "a\0b").is_err());
        assert!(
            store
                .set("web-search-key-long", &"k".repeat(MAX_SECRET + 1))
                .is_err()
        );
        assert!(!argv_log().contains("web-search-key-nul"));
    }

    #[test]
    fn a_failing_tool_is_an_error_not_a_missing_key() {
        let store = faked(Duration::from_secs(20));
        let e = store.get("broken-key").unwrap_err();
        assert!(e.contains("locked collection"), "{e}");
        assert!(store.set("broken-key", "test-key-not-real").is_err());
        assert!(store.remove("broken-key").is_err());
    }

    #[test]
    fn a_keyring_that_waits_is_given_up_on_and_stopped() {
        let store = faked(Duration::from_millis(300));
        let started = std::time::Instant::now();
        let e = store.get("hang-key").unwrap_err();
        assert!(e.contains("didn't answer"), "{e}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn no_secret_tool_is_no_keyring() {
        let store = SecretService {
            program: PathBuf::from("/nonexistent/secret-tool"),
            wait: Duration::from_secs(5),
        };
        assert_eq!(store.check(), Err(NO_KEYRING.to_string()));
        assert_eq!(store.get(SEARCH_KEY), Err(NO_KEYRING.to_string()));
        assert_eq!(
            store.set(SEARCH_KEY, "test-key-not-real"),
            Err(NO_KEYRING.to_string())
        );
        assert_eq!(store.remove(SEARCH_KEY), Err(NO_KEYRING.to_string()));
    }

    #[test]
    fn check_tells_a_service_from_no_service() {
        // The stand-in answers "not found" quietly: a keyring is there.
        assert_eq!(faked(Duration::from_secs(20)).check(), Ok(()));
        // A tool that says why it can't reach one: no keyring, in our words.
        let down = SecretService {
            program: fake().join("secret-tool-down"),
            wait: Duration::from_secs(20),
        };
        assert_eq!(down.check(), Err(NO_KEYRING.to_string()));
        // The other calls pass the tool's words on.
        let e = down.set(SEARCH_KEY, "test-key-not-real").unwrap_err();
        assert!(e.contains("org.freedesktop.secrets"), "{e}");
        assert!(down.get(SEARCH_KEY).is_err());
    }
}
