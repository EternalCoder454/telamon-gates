//! Where an agent's commands run: a bubblewrap sandbox that sees the
//! workspace and the system's programs (`/usr`, read-only), and nothing of
//! the user's own unless they allow it: no home folder, no network, no other
//! process, no session bus. Without bubblewrap, commands don't run.
//!
//! A nested container (the dev container, CI) can't make the sandbox its
//! own `/proc`: there it runs with none. Binding the host's `/proc` instead
//! would let a command reach every file of the user's other processes.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// What a sandboxed command may reach beyond the workspace.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Access {
    /// The network (off: no interface but loopback).
    pub network: bool,
    /// The home folder, read-only, for the user's own toolchains (cargo,
    /// mise, …). The workspace stays the only place it can write.
    pub home: bool,
}

/// How well bubblewrap works here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Its own process tree and `/proc`.
    Full,
    /// No `/proc` (inside another container).
    NoProc,
    Missing,
}

const BWRAP: &str = "/usr/bin/bwrap";

fn kind() -> Kind {
    static KIND: OnceLock<Kind> = OnceLock::new();
    *KIND.get_or_init(|| {
        if !Path::new(BWRAP).exists() {
            return Kind::Missing;
        }
        let works = |proc: bool| {
            Command::new(BWRAP)
                .args(base_args(proc))
                .args(["--", "/usr/bin/true"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        };
        if works(true) {
            Kind::Full
        } else if works(false) {
            log::warn!("the command sandbox runs without /proc (nested container?)");
            Kind::NoProc
        } else {
            Kind::Missing
        }
    })
}

/// Whether commands can run (in their sandbox) here.
pub fn available() -> bool {
    kind() != Kind::Missing
}

/// The sandbox itself: the system's programs and settings read-only, a
/// private `/tmp` and `/dev`, every namespace new.
fn base_args(proc: bool) -> Vec<String> {
    let mut a: Vec<String> = [
        "--ro-bind",
        "/usr",
        "/usr",
        "--symlink",
        "usr/bin",
        "/bin",
        "--symlink",
        "usr/sbin",
        "/sbin",
        "--symlink",
        "usr/lib",
        "/lib",
        "--symlink",
        "usr/lib64",
        "/lib64",
        "--dev",
        "/dev",
        "--tmpfs",
        "/tmp",
        "--tmpfs",
        "/run",
        "--unshare-all",
        "--die-with-parent",
        "--new-session",
        "--clearenv",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    // What programs read from /etc, nothing more (no secrets there to read
    // anyway, but no reason to show them).
    for etc in [
        "/etc/alternatives",
        "/etc/ld.so.cache",
        "/etc/ld.so.conf",
        "/etc/ld.so.conf.d",
        "/etc/localtime",
        "/etc/passwd",
        "/etc/group",
        "/etc/nsswitch.conf",
        "/etc/ssl",
        "/etc/pki",
        "/etc/crypto-policies",
        "/etc/hosts",
        "/etc/resolv.conf",
        "/etc/mime.types",
    ] {
        a.extend(["--ro-bind-try".into(), etc.into(), etc.into()]);
    }
    if proc {
        a.extend(["--proc".into(), "/proc".into()]);
    }
    a
}

/// `/bin/sh -c shell` in the sandbox, working in `root` (which it may
/// write). An Err says why commands can't run.
pub fn command(root: &Path, access: Access, shell: &str) -> Result<Command, String> {
    let kind = kind();
    if kind == Kind::Missing {
        return Err("Commands run in a sandbox, which needs bubblewrap (/usr/bin/bwrap).".into());
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut args = base_args(kind == Kind::Full);
    if access.network {
        args.push("--share-net".into());
    }
    if access.home
        && let Some(home) = &home
    {
        args.extend(["--ro-bind".into(), path(home), path(home)]);
    }
    let root_s = path(root);
    // Last, so the workspace is writable even inside a read-only home.
    args.extend([
        "--bind".into(),
        root_s.clone(),
        root_s.clone(),
        "--chdir".into(),
        root_s.clone(),
    ]);
    let home_env = match (&home, access.home) {
        (Some(h), true) => path(h),
        _ => root_s,
    };
    let path_env = match (&home, access.home) {
        // The user's own tools first, as in their shell.
        (Some(h), true) => format!(
            "{0}/.local/bin:{0}/.cargo/bin:{0}/.local/share/mise/shims:/usr/local/bin:/usr/bin",
            path(h)
        ),
        _ => "/usr/local/bin:/usr/bin".into(),
    };
    for (key, value) in [
        ("PATH", path_env),
        ("HOME", home_env),
        (
            "LANG",
            std::env::var("LANG").unwrap_or_else(|_| "C.UTF-8".into()),
        ),
        ("TERM", "dumb".into()),
        ("TMPDIR", "/tmp".into()),
    ] {
        args.extend(["--setenv".into(), key.into(), value]);
    }
    let mut cmd = Command::new(BWRAP);
    cmd.args(args).args(["--", "/bin/sh", "-c", shell]);
    Ok(cmd)
}

fn path(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(root: &Path, access: Access, shell: &str) -> (bool, String) {
        let out = command(root, access, shell).unwrap().output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout).into_owned()
            + &String::from_utf8_lossy(&out.stderr);
        (out.status.success(), text)
    }

    #[test]
    fn a_command_sees_only_its_workspace() {
        if !available() {
            eprintln!("no bubblewrap here: skipped");
            return;
        }
        let root = std::env::temp_dir().join(format!("gates-sandbox-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        // A file beside the workspace: out of sight.
        let beside = root.with_extension("secret");
        std::fs::write(&beside, "secret").unwrap();
        let (ok, out) = run(
            &root,
            Access::default(),
            "echo hi > made.txt && pwd && ls /",
        );
        assert!(ok, "{out}");
        assert!(out.contains(&root.to_string_lossy().to_string()));
        assert!(root.join("made.txt").exists());
        let (_, out) = run(
            &root,
            Access::default(),
            &format!("cat {}", beside.display()),
        );
        assert!(
            !out.contains("secret") || out.contains("No such file"),
            "{out}"
        );
        let (ok, _) = run(
            &root,
            Access::default(),
            &format!("cat {}", beside.display()),
        );
        assert!(!ok);
        // No home folder unless allowed.
        if let Some(home) = std::env::var_os("HOME") {
            let (ok, _) = run(
                &root,
                Access::default(),
                &format!("ls {}", Path::new(&home).display()),
            );
            assert!(!ok || Path::new(&home).starts_with(&root));
        }
        // No network: only loopback, if anything.
        let (_, out) = run(
            &root,
            Access::default(),
            "cat /proc/net/dev 2>/dev/null || ls /sys/class/net 2>/dev/null || true",
        );
        assert!(!out.contains("eth0") && !out.contains("enp"), "{out}");
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_file(&beside);
    }
}
