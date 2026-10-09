//! Gates' own log file, `$XDG_STATE_HOME/telamon-gates/telamon-gates.log`
//! (`~/.local/state/telamon-gates/`), next to the model server's. The
//! framework's logger sends `log` records to the journal; this file holds
//! what Gates says at startup and what a bug report needs, kept on the
//! machine so a failure there can be diagnosed from one folder. Each line is
//! also passed to `log`, so the journal has it too.
//!
//! The folder is made if it isn't there (private to the user, like Gates'
//! other folders). A file over 256 KiB is moved to `….log.1` (replacing the
//! one before) and a new one started, so it never grows without bound.
//!
//! Does file I/O: call from a worker thread, never the GUI's.

use crate::store;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_BYTES: u64 = 256 * 1024;

/// The log file.
pub fn path() -> PathBuf {
    store::state_dir().join("telamon-gates.log")
}

pub fn info(message: &str) {
    log::info!("{message}");
    write("INFO", message);
}

pub fn warn(message: &str) {
    log::warn!("{message}");
    write("WARN", message);
}

fn write(level: &str, message: &str) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    if let Err(e) = append(&path(), level, message, now) {
        log::warn!("cannot write {}: {e}", path().display());
    }
}

/// Adds one line to the file at `file`, making its folder if needed.
pub fn append(file: &Path, level: &str, message: &str, unix_secs: u64) -> io::Result<()> {
    if let Some(dir) = file.parent() {
        store::private_dir(dir)?;
    }
    if fs::metadata(file).is_ok_and(|m| m.len() > MAX_BYTES) {
        let mut old = file.as_os_str().to_owned();
        old.push(".1");
        fs::rename(file, old)?;
    }
    // One line each: a message with a line break or control characters can't
    // forge another entry.
    let message: String = message
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(file)?;
    writeln!(f, "{} {level} {message}", timestamp(unix_secs))
}

/// `2026-10-09 14:03:07Z`.
fn timestamp(unix_secs: u64) -> String {
    let (days, rest) = (unix_secs / 86_400, unix_secs % 86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}Z",
        rest / 3_600,
        rest % 3_600 / 60,
        rest % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gates-applog-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn timestamps() {
        assert_eq!(timestamp(0), "1970-01-01 00:00:00Z");
        assert_eq!(timestamp(951_782_400), "2000-02-29 00:00:00Z");
        assert_eq!(timestamp(1_760_016_187), "2025-10-09 13:23:07Z");
    }

    #[test]
    fn makes_the_folder_and_writes_private_lines() {
        let dir = temp("make").join("state/telamon-gates");
        let file = dir.join("telamon-gates.log");
        append(&file, "INFO", "starting", 0).unwrap();
        append(&file, "WARN", "two\nlines\x1b[31m", 0).unwrap();
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "1970-01-01 00:00:00Z INFO starting\n1970-01-01 00:00:00Z WARN two lines [31m\n"
        );
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&file), 0o600);
    }

    #[test]
    fn a_big_file_is_moved_aside() {
        let dir = temp("rotate");
        let file = dir.join("telamon-gates.log");
        append(&file, "INFO", "first", 0).unwrap();
        fs::write(&file, vec![b'x'; MAX_BYTES as usize + 1]).unwrap();
        append(&file, "INFO", "second", 0).unwrap();
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "1970-01-01 00:00:00Z INFO second\n"
        );
        assert_eq!(
            fs::metadata(dir.join("telamon-gates.log.1")).unwrap().len(),
            MAX_BYTES + 1
        );
    }
}
