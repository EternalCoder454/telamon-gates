//! How much video memory the graphics card has and uses: what a local model
//! has room for. Read from the kernel's DRM files in sysfs, which amdgpu
//! fills (`mem_info_vram_total` and `mem_info_vram_used`, in bytes). Cards
//! whose driver doesn't (NVIDIA's, Intel's integrated ones) are not listed.
//!
//! Reads files: call it from a worker thread, never the GUI's.

use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Vram {
    pub used: u64,
    pub total: u64,
}

/// The card with the most video memory, from `/sys/class/drm`.
pub fn read() -> Option<Vram> {
    read_in(Path::new("/sys/class/drm"))
}

/// As `read`, from another DRM folder (tests).
pub fn read_in(drm: &Path) -> Option<Vram> {
    fs::read_dir(drm)
        .ok()?
        .filter_map(Result::ok)
        .filter(|e| is_card(&e.file_name().to_string_lossy()))
        .filter_map(|e| {
            let device = e.path().join("device");
            let total = number(&device.join("mem_info_vram_total"))?;
            let used = number(&device.join("mem_info_vram_used"))?;
            (total > 0).then_some(Vram {
                used: used.min(total),
                total,
            })
        })
        .max_by_key(|v| v.total)
}

/// `card0`, not a connector such as `card0-DP-1`.
fn is_card(name: &str) -> bool {
    name.strip_prefix("card")
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

fn number(path: &Path) -> Option<u64> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(drm: &Path, name: &str, total: Option<&str>, used: Option<&str>) {
        let device = drm.join(name).join("device");
        fs::create_dir_all(&device).unwrap();
        if let Some(t) = total {
            fs::write(device.join("mem_info_vram_total"), t).unwrap();
        }
        if let Some(u) = used {
            fs::write(device.join("mem_info_vram_used"), u).unwrap();
        }
    }

    #[test]
    fn the_card_with_the_most_memory() {
        let drm = std::env::temp_dir().join(format!("gates-vram-{}", std::process::id()));
        let _ = fs::remove_dir_all(&drm);
        // An integrated GPU, a discrete one, a connector and an NVIDIA card
        // (no files).
        card(&drm, "card0", Some("536870912\n"), Some("100\n"));
        card(&drm, "card1", Some("25753026560\n"), Some("6442450944\n"));
        card(&drm, "card1-DP-1", Some("999999999999\n"), Some("0\n"));
        card(&drm, "card2", None, None);
        assert_eq!(
            read_in(&drm),
            Some(Vram {
                used: 6442450944,
                total: 25753026560
            })
        );
        let _ = fs::remove_dir_all(&drm);
    }

    #[test]
    fn nothing_without_amdgpu_files() {
        let drm = std::env::temp_dir().join(format!("gates-vram-none-{}", std::process::id()));
        let _ = fs::remove_dir_all(&drm);
        card(&drm, "card0", None, None);
        card(&drm, "card1", Some("garbage"), Some("1"));
        assert_eq!(read_in(&drm), None);
        assert_eq!(read_in(&drm.join("missing")), None);
        let _ = fs::remove_dir_all(&drm);
    }
}
