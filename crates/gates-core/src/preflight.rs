//! The first-run check: is there a model server to start, and a graphics
//! device for it to use? Neither is needed to open the window (the demo
//! backend answers without them), so the window opens first and shows what is
//! missing and how to fix it.
//!
//! Reads the file system and may run `vulkaninfo`: call it from a worker
//! thread, never the GUI's.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// What the graphics side looks like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gpu {
    /// A render node, and no sign that Vulkan has nothing but a software
    /// device.
    Usable,
    /// No `/dev/dri/renderD*`: no driver, or no device the user may use.
    NoRenderNode,
    /// `vulkaninfo` lists devices, all of them software (llvmpipe).
    SoftwareOnly,
}

/// The result of the check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// The `llama-server` that would be started, if there is one.
    pub server: Option<PathBuf>,
    pub gpu: Gpu,
}

impl Report {
    pub fn server_missing(&self) -> bool {
        self.server.is_none()
    }

    /// Models would run on the processor.
    pub fn no_gpu(&self) -> bool {
        self.gpu != Gpu::Usable
    }

    /// One line for the log.
    pub fn summary(&self) -> String {
        let server = match &self.server {
            Some(p) => p.display().to_string(),
            None => "missing".to_owned(),
        };
        let gpu = match self.gpu {
            Gpu::Usable => "usable",
            Gpu::NoRenderNode => "no render node",
            Gpu::SoftwareOnly => "software device only",
        };
        format!("model server: {server}; graphics: {gpu}")
    }
}

/// Where the check looks; `Where::system()` is the real computer.
#[derive(Debug, Clone)]
pub struct Where {
    /// telamon-llama's `llama-server`.
    pub packaged_server: PathBuf,
    /// `$PATH`, where a `llama-server` of the user's own may be, and
    /// `vulkaninfo`.
    pub path: Option<OsString>,
    /// `/dev/dri`, where render nodes are.
    pub dri: PathBuf,
    /// Run `vulkaninfo --summary` from `$PATH` (when installed) to tell a
    /// real device from a software one.
    pub ask_vulkaninfo: bool,
}

impl Where {
    pub fn system() -> Where {
        Where {
            packaged_server: PathBuf::from(crate::backend::llama::PACKAGED_SERVER),
            path: std::env::var_os("PATH"),
            dri: PathBuf::from("/dev/dri"),
            ask_vulkaninfo: true,
        }
    }
}

/// Runs the check.
pub fn check(at: &Where) -> Report {
    let server = crate::backend::llama::find_server_in(&at.packaged_server, at.path.clone());
    let nodes = has_render_node(&at.dri);
    let summary = if nodes && at.ask_vulkaninfo {
        vulkaninfo_summary(at.path.as_deref())
    } else {
        None
    };
    Report {
        server,
        gpu: gpu_state(nodes, summary.as_deref()),
    }
}

/// Whether `dir` (`/dev/dri`) has a render node, `renderD128` and so on.
pub fn has_render_node(dir: &Path) -> bool {
    fs::read_dir(dir).is_ok_and(|entries| {
        entries.filter_map(Result::ok).any(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.strip_prefix("renderD")
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        })
    })
}

/// The state from whether a render node exists and, when it could be had,
/// `vulkaninfo --summary`'s text.
pub fn gpu_state(render_node: bool, vulkaninfo: Option<&str>) -> Gpu {
    if !render_node {
        return Gpu::NoRenderNode;
    }
    match vulkaninfo.map(device_types) {
        // Devices are listed and none is hardware.
        Some(types) if !types.is_empty() && types.iter().all(|t| t.ends_with("_CPU")) => {
            Gpu::SoftwareOnly
        }
        // A real device, or no answer to go by: the render node stands.
        _ => Gpu::Usable,
    }
}

/// The `deviceType` of each device in `vulkaninfo --summary`, such as
/// `PHYSICAL_DEVICE_TYPE_DISCRETE_GPU`.
fn device_types(text: &str) -> Vec<&str> {
    text.lines()
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            (key.trim() == "deviceType").then(|| value.trim())
        })
        .collect()
}

/// `vulkaninfo --summary`, if it is installed and answers within a few
/// seconds; None otherwise (it is optional, and a broken driver can hang it).
fn vulkaninfo_summary(path: Option<&OsStr>) -> Option<String> {
    let binary = std::env::split_paths(path?)
        .map(|dir| dir.join("vulkaninfo"))
        .find(|p| p.is_file())?;
    let mut child = Command::new(binary)
        .arg("--summary")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) | Err(_) => return None,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    let mut text = String::new();
    child
        .stdout
        .take()?
        .take(256 * 1024)
        .read_to_string(&mut text)
        .ok()?;
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("gates-preflight-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn executable(path: &Path, body: &str) {
        fs::write(path, body).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// A computer with nothing installed, under `dir`.
    fn nowhere(dir: &Path) -> Where {
        Where {
            packaged_server: dir.join("libexec/llama-server"),
            path: Some(dir.join("bin").into()),
            dri: dir.join("dri"),
            ask_vulkaninfo: false,
        }
    }

    #[test]
    fn nothing_installed() {
        let dir = temp("none");
        let report = check(&nowhere(&dir));
        assert!(report.server_missing());
        assert_eq!(report.gpu, Gpu::NoRenderNode);
        assert!(report.no_gpu());
        assert_eq!(
            report.summary(),
            "model server: missing; graphics: no render node"
        );
    }

    #[test]
    fn packaged_server_and_render_node() {
        let dir = temp("packaged");
        let at = nowhere(&dir);
        fs::create_dir_all(at.packaged_server.parent().unwrap()).unwrap();
        executable(&at.packaged_server, "#!/bin/sh\n");
        fs::create_dir_all(&at.dri).unwrap();
        fs::write(at.dri.join("renderD128"), "").unwrap();
        let report = check(&at);
        assert_eq!(report.server.as_deref(), Some(at.packaged_server.as_path()));
        assert!(!report.server_missing());
        assert!(!report.no_gpu());
    }

    #[test]
    fn server_on_path() {
        let dir = temp("path");
        let at = nowhere(&dir);
        fs::create_dir_all(dir.join("bin")).unwrap();
        executable(&dir.join("bin/llama-server"), "#!/bin/sh\n");
        let report = check(&at);
        assert_eq!(report.server, Some(dir.join("bin/llama-server")));
    }

    #[test]
    fn render_nodes_are_told_from_other_entries() {
        let dir = temp("dri");
        assert!(!has_render_node(&dir.join("absent")));
        assert!(!has_render_node(&dir));
        // Card nodes, and names that only start like a render node, don't count.
        for name in ["card0", "renderD", "renderDx", "by-path"] {
            fs::write(dir.join(name), "").unwrap();
        }
        assert!(!has_render_node(&dir));
        fs::write(dir.join("renderD129"), "").unwrap();
        assert!(has_render_node(&dir));
    }

    const HARDWARE: &str = "Devices:\n========\nGPU0:\n\tapiVersion         = 1.4.305\n\tdeviceType         = PHYSICAL_DEVICE_TYPE_DISCRETE_GPU\n\tdeviceName         = AMD Radeon RX 7900 XTX (RADV NAVI31)\nGPU1:\n\tdeviceType         = PHYSICAL_DEVICE_TYPE_CPU\n\tdeviceName         = llvmpipe (LLVM 21.1.0, 256 bits)\n";
    const SOFTWARE: &str = "Devices:\nGPU0:\n\tdeviceType         = PHYSICAL_DEVICE_TYPE_CPU\n\tdeviceName         = llvmpipe\n";

    #[test]
    fn gpu_from_the_render_node_and_vulkaninfo() {
        assert_eq!(gpu_state(false, Some(HARDWARE)), Gpu::NoRenderNode);
        assert_eq!(gpu_state(true, None), Gpu::Usable);
        assert_eq!(gpu_state(true, Some(HARDWARE)), Gpu::Usable);
        assert_eq!(gpu_state(true, Some(SOFTWARE)), Gpu::SoftwareOnly);
        // No devices in the text (or text of another shape): not enough to go by.
        assert_eq!(gpu_state(true, Some("")), Gpu::Usable);
        assert_eq!(gpu_state(true, Some("Segmentation fault")), Gpu::Usable);
    }

    #[test]
    fn vulkaninfo_is_asked_only_when_installed() {
        let dir = temp("vulkaninfo");
        let mut at = nowhere(&dir);
        fs::create_dir_all(&at.dri).unwrap();
        fs::write(at.dri.join("renderD128"), "").unwrap();
        fs::create_dir_all(dir.join("bin")).unwrap();
        at.ask_vulkaninfo = true;
        // Not installed: the render node stands.
        assert_eq!(check(&at).gpu, Gpu::Usable);
        // Installed, and it lists only llvmpipe.
        let script = dir.join("bin/vulkaninfo");
        executable(&script, &format!("#!/bin/sh\ncat <<'EOF'\n{SOFTWARE}EOF\n"));
        assert_eq!(check(&at).gpu, Gpu::SoftwareOnly);
        // Installed but failing: no answer, the render node stands.
        executable(&script, "#!/bin/sh\nexit 1\n");
        assert_eq!(check(&at).gpu, Gpu::Usable);
    }
}
