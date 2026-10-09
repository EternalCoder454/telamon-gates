//! Conversations on disk: one JSON file each, `<id>.json`, in
//! `$XDG_DATA_HOME/telamon-gates/conversations`. Plain files, so they can be
//! backed up, read or moved by anything.
//!
//! Besides `<id>.json` the folder can hold:
//! - `<id>.json.bak`: the version before the last save (one generation);
//! - `.<id>.json.tmp` and `.<id>.json.bak.tmp`: a write under way, which a
//!   crash can leave behind;
//! - `damaged/<id>.<UTC time>.json`: a file that couldn't be read, set aside
//!   rather than skipped or overwritten.
//!
//! Every method does file I/O: call them from a worker thread, never the GUI's.

use crate::conversation::{Conversation, Summary, VERSION, now_ms, utc};
use serde::Deserialize;
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// The folder (inside the conversations folder) that files which can't be
/// read are set aside in.
pub const DAMAGED_DIR: &str = "damaged";

#[derive(Debug, Clone)]
pub struct Store {
    dir: PathBuf,
}

/// What a read found wrong, and what was done about it. Counts only; the
/// window words them (`describe`).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    /// Files that couldn't be read, moved to `damaged/` (the restored ones
    /// included).
    pub set_aside: usize,
    /// Of those, the ones whose `.bak` was good and now stands in.
    pub restored: usize,
    /// Conversations finished from the temporary file of an interrupted save.
    pub finished: usize,
    /// Files from a newer Gates, left exactly as they are.
    pub newer: usize,
}

impl Report {
    pub fn is_empty(&self) -> bool {
        *self == Report::default()
    }

    /// Adds what another read found. The newer files stay where they are, so
    /// finding them again is not finding more of them.
    pub fn merge(&mut self, other: &Report) {
        self.set_aside += other.set_aside;
        self.restored += other.restored;
        self.finished += other.finished;
        self.newer = self.newer.max(other.newer);
    }

    /// The notice for the window, in a few sentences (empty when there is
    /// nothing to say). `damaged` is the folder files were set aside in.
    pub fn describe(&self, damaged: &Path) -> String {
        let lost = self.set_aside.saturating_sub(self.restored);
        let mut parts = Vec::new();
        if lost > 0 {
            parts.push(format!(
                "{} couldn't be read and {} set aside in {}.",
                conversations(lost),
                was(lost),
                damaged.display()
            ));
        }
        if self.restored > 0 {
            let n = self.restored;
            parts.push(format!(
                "{} couldn't be read and {} restored from {}; the damaged {} in {}.",
                conversations(n),
                was(n),
                if n == 1 {
                    "its backup"
                } else {
                    "their backups"
                },
                if n == 1 { "file is" } else { "files are" },
                damaged.display()
            ));
        }
        if self.finished > 0 {
            let n = self.finished;
            parts.push(format!(
                "{} {} recovered from an interrupted save.",
                conversations(n),
                was(n)
            ));
        }
        if self.newer > 0 {
            let n = self.newer;
            parts.push(format!(
                "{} {} saved by a newer version of Telamon Gates and {} left as {}.",
                conversations(n),
                was(n),
                was(n),
                if n == 1 { "it is" } else { "they are" }
            ));
        }
        parts.join(" ")
    }
}

fn conversations(n: usize) -> String {
    if n == 1 {
        "1 conversation".to_string()
    } else {
        format!("{n} conversations")
    }
}

fn was(n: usize) -> &'static str {
    if n == 1 { "was" } else { "were" }
}

/// What listing the folder found: the conversations, and what had to be put
/// right on the way.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Listing {
    pub conversations: Vec<Summary>,
    pub report: Report,
}

/// A file saved by a newer Gates than this one.
#[derive(Debug)]
struct NewerFormat(u32);

impl fmt::Display for NewerFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "This conversation was saved by a newer version of Telamon Gates (file format {}). Update Gates to open it; the file has been left as it is.",
            self.0
        )
    }
}

impl std::error::Error for NewerFormat {}

fn newer_error(version: u32) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, NewerFormat(version))
}

/// Whether `e` says the file was saved by a newer Gates.
pub fn is_newer_format(e: &io::Error) -> bool {
    e.get_ref().is_some_and(|inner| inner.is::<NewerFormat>())
}

fn damaged_error() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "This conversation file couldn't be read, so it was set aside in the damaged folder.",
    )
}

/// What a file's bytes are.
enum Parsed {
    Ok(Conversation),
    /// Saved by a newer Gates (its format version).
    Newer(u32),
    /// Not a conversation this Gates can use. The reason names a place in
    /// the file, never its text: conversations are private, and the reason
    /// goes in the log.
    Damaged(String),
}

fn parse(bytes: &[u8], id: &str) -> Parsed {
    #[derive(Deserialize)]
    struct Probe {
        version: Option<u32>,
    }
    match serde_json::from_slice::<Conversation>(bytes) {
        Ok(c) if c.version > VERSION => Parsed::Newer(c.version),
        Ok(c) if c.id != id => Parsed::Damaged("the file's id is not its name".into()),
        Ok(c) => Parsed::Ok(c),
        // A newer format may not fit this one's fields: its version still
        // says what it is.
        Err(e) => match serde_json::from_slice::<Probe>(bytes) {
            Ok(Probe { version: Some(v) }) if v > VERSION => Parsed::Newer(v),
            _ => Parsed::Damaged(format!(
                "{:?} error at line {}, column {}",
                e.classify(),
                e.line(),
                e.column()
            )),
        },
    }
}

impl Store {
    pub fn at(dir: impl Into<PathBuf>) -> Store {
        Store { dir: dir.into() }
    }

    /// `$XDG_DATA_HOME/telamon-gates/conversations`, or under
    /// `~/.local/share` when that is unset.
    pub fn default_dir() -> PathBuf {
        data_dir().join("conversations")
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Where files that couldn't be read are set aside.
    pub fn damaged_dir(&self) -> PathBuf {
        self.dir.join(DAMAGED_DIR)
    }

    /// Every conversation that can be read, newest first (see `scan`).
    pub fn list(&self) -> Vec<Summary> {
        self.scan().conversations
    }

    /// `list`, with what had to be put right on the way:
    /// - a temporary file left by an interrupted save is finished (when its
    ///   conversation is whole and the file it was for is gone) or removed;
    /// - a file that doesn't parse is moved to `damaged/`, and its `.bak` put
    ///   in its place when that is good;
    /// - a file saved by a newer Gates is left alone and not listed.
    pub fn scan(&self) -> Listing {
        let mut report = Report::default();
        // First: a finished save adds a conversation to the folder.
        self.clean_temporary(&mut report);
        let entries = match fs::read_dir(&self.dir) {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Listing::default(),
            Err(e) => {
                log::warn!("cannot list {}: {e}", self.dir.display());
                return Listing::default();
            }
        };
        // All names first: reading moves files about.
        let ids: Vec<String> = entries
            .filter_map(Result::ok)
            .filter_map(|e| {
                let name = e.file_name();
                let id = name.to_str()?.strip_suffix(".json")?;
                valid_id(id).then(|| id.to_string())
            })
            .collect();
        let mut conversations = Vec::new();
        for id in ids {
            match self.read(&id, &mut report) {
                Ok(c) => conversations.push(c.summary()),
                Err(e) if is_newer_format(&e) => {
                    report.newer += 1;
                    log::warn!("conversation {id} is from a newer version; left as it is");
                }
                Err(e) => log::warn!("cannot read conversation {id}: {e}"),
            }
        }
        conversations.sort_by(|a, b| b.updated.cmp(&a.updated).then_with(|| b.id.cmp(&a.id)));
        Listing {
            conversations,
            report,
        }
    }

    pub fn load(&self, id: &str) -> io::Result<Conversation> {
        self.load_reporting(id).0
    }

    /// `load`, and what it had to put right (a file that doesn't parse is set
    /// aside, and its backup used when that is good).
    pub fn load_reporting(&self, id: &str) -> (io::Result<Conversation>, Report) {
        let mut report = Report::default();
        let result = self.read(id, &mut report);
        (result, report)
    }

    /// Reads one conversation. A file that doesn't parse is moved to
    /// `damaged/` first, so nothing writes over it, and the `.bak` takes its
    /// place when that parses. One from a newer Gates is left alone and the
    /// error says so.
    fn read(&self, id: &str, report: &mut Report) -> io::Result<Conversation> {
        let path = self.path(id)?;
        let bytes = fs::read(&path)?;
        let why = match parse(&bytes, id) {
            Parsed::Ok(c) => return Ok(c),
            Parsed::Newer(v) => return Err(newer_error(v)),
            Parsed::Damaged(why) => why,
        };
        log::warn!("conversation {id} can't be read ({why})");
        // Failing to move it leaves it where it is, and says so.
        let moved = self.set_aside(id)?;
        report.set_aside += 1;
        log::warn!("conversation {id} set aside as {}", moved.display());
        let Some((c, backup)) = self.backup_of(id) else {
            return Err(damaged_error());
        };
        match write_private(&path, &backup) {
            Ok(()) => {
                report.restored += 1;
                log::warn!("conversation {id} restored from its backup");
                Ok(c)
            }
            Err(e) => {
                log::warn!("cannot restore conversation {id} from its backup: {e}");
                Err(damaged_error())
            }
        }
    }

    /// The `.bak` of `id`, when it is a whole conversation of this format.
    fn backup_of(&self, id: &str) -> Option<(Conversation, Vec<u8>)> {
        let bytes = match fs::read(self.backup_path(id)) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return None,
            Err(e) => {
                log::warn!("cannot read the backup of conversation {id}: {e}");
                return None;
            }
        };
        match parse(&bytes, id) {
            Parsed::Ok(c) => Some((c, bytes)),
            Parsed::Newer(_) | Parsed::Damaged(_) => {
                log::warn!("the backup of conversation {id} can't be used either");
                None
            }
        }
    }

    /// Moves `<id>.json` to `damaged/<id>.<UTC time>.json` (owner only, like
    /// the other files) and returns where it went.
    fn set_aside(&self, id: &str) -> io::Result<PathBuf> {
        use std::os::unix::fs::PermissionsExt;
        let from = self.path(id)?;
        let dir = self.damaged_dir();
        private_dir(&dir)?;
        let t = utc(now_ms());
        let stamp = format!(
            "{:04}{:02}{:02}-{:02}{:02}{:02}",
            t.year, t.month, t.day, t.hour, t.minute, t.second
        );
        // Two in the same second must not replace one another.
        let to = (0..)
            .map(|n| match n {
                0 => dir.join(format!("{id}.{stamp}.json")),
                n => dir.join(format!("{id}.{stamp}-{n}.json")),
            })
            .find(|p| !p.exists())
            .expect("an unbounded range");
        fs::rename(&from, &to)?;
        fs::set_permissions(&to, fs::Permissions::from_mode(0o600))?;
        Ok(to)
    }

    /// The leftovers of writes a crash interrupted. A `.<id>.json.tmp` is the
    /// whole next version when the crash came between its sync and its
    /// rename: if `<id>.json` is gone, it is moved into place, but only when
    /// it parses. Otherwise (the file it was to replace is there, or it is
    /// cut short) it is removed, and the file on disk stays the last saved
    /// state. A `.<id>.json.bak.tmp` is only ever a half-made backup.
    fn clean_temporary(&self, report: &mut Report) {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return;
        };
        let names: Vec<String> = entries
            .filter_map(Result::ok)
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        for name in names {
            let Some(inner) = name.strip_prefix('.').and_then(|n| n.strip_suffix(".tmp")) else {
                continue;
            };
            let path = self.dir.join(&name);
            if let Some(id) = inner.strip_suffix(".json").filter(|id| valid_id(id)) {
                let main = self.dir.join(format!("{id}.json"));
                if !main.exists()
                    && let Ok(bytes) = fs::read(&path)
                    && matches!(parse(&bytes, id), Parsed::Ok(_))
                    && fs::rename(&path, &main).is_ok()
                {
                    report.finished += 1;
                    log::warn!("conversation {id} finished from an interrupted save");
                    continue;
                }
            } else if !inner.strip_suffix(".json.bak").is_some_and(valid_id) {
                // Not one of ours.
                continue;
            }
            match fs::remove_file(&path) {
                Ok(()) => log::warn!("removed the leftover of an interrupted save: {name}"),
                Err(e) => log::warn!("cannot remove {name}: {e}"),
            }
        }
    }

    /// Writes the conversation whole, as the current format version: a
    /// temporary file, then a rename, so a crash never leaves half a file.
    /// The file it replaces becomes `<id>.json.bak`. A file that is there
    /// but can't be read is set aside first, never written over; one from a
    /// newer Gates is not touched, and the save fails.
    pub fn save(&self, c: &Conversation) -> io::Result<()> {
        let path = self.path(&c.id)?;
        private_dir(&self.dir)?;
        let json = if c.version == VERSION {
            serde_json::to_vec_pretty(c)
        } else {
            serde_json::to_vec_pretty(&Conversation {
                version: VERSION,
                ..c.clone()
            })
        }
        .map_err(io::Error::other)?;
        match fs::read(&path) {
            Ok(old) => match parse(&old, &c.id) {
                // Only a good file becomes the backup: a damaged one must
                // not replace a good backup.
                Parsed::Ok(_) => {
                    if let Err(e) = write_private(&self.backup_path(&c.id), &old) {
                        log::warn!("cannot back up conversation {}: {e}", c.id);
                    }
                }
                Parsed::Newer(v) => return Err(newer_error(v)),
                Parsed::Damaged(why) => {
                    log::warn!(
                        "conversation {} can't be read ({why}); setting it aside",
                        c.id
                    );
                    self.set_aside(&c.id)?;
                }
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        write_private(&path, &json)
    }

    /// Removes the conversation, its backup and any leftover temporary file.
    pub fn delete(&self, id: &str) -> io::Result<()> {
        let main = self.path(id)?;
        for extra in [
            self.backup_path(id),
            self.dir.join(format!(".{id}.json.bak.tmp")),
            self.dir.join(format!(".{id}.json.tmp")),
        ] {
            match fs::remove_file(&extra) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => {
                    log::warn!("cannot remove {}: {e}", extra.display());
                }
                _ => {}
            }
        }
        match fs::remove_file(main) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            r => r,
        }
    }

    fn path(&self, id: &str) -> io::Result<PathBuf> {
        if !valid_id(id) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "not a conversation id",
            ));
        }
        Ok(self.dir.join(format!("{id}.json")))
    }

    fn backup_path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json.bak"))
    }
}

/// Writes `bytes` to `path` whole and private to the user (conversations can
/// hold anything): a temporary file next to it, synced, then a rename.
fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?;
    let tmp = path.with_file_name(format!(".{name}.tmp"));
    {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)
}

/// Makes `dir` (and the folders above it it needs) private to the user:
/// Gates' own folders hold conversations, pictures and agents' work.
pub fn private_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
}

/// A conversation's own folder for Agent mode: the sandbox it works in
/// unless the user chooses a folder on the computer.
pub fn sandbox_dir(conversation: &str) -> PathBuf {
    data_dir().join("workspaces").join(conversation)
}

/// Where pictures sent with messages are kept.
pub fn attachments_dir() -> PathBuf {
    data_dir().join("attachments")
}

/// `$XDG_DATA_HOME/telamon-gates` (`~/.local/share/telamon-gates`): the
/// conversations and the models.
pub fn data_dir() -> PathBuf {
    xdg("XDG_DATA_HOME", ".local/share").join("telamon-gates")
}

/// `$XDG_STATE_HOME/telamon-gates` (`~/.local/state/telamon-gates`): Gates'
/// own log (`applog`) and the model server's.
pub fn state_dir() -> PathBuf {
    xdg("XDG_STATE_HOME", ".local/state").join("telamon-gates")
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(fallback)))
        .unwrap_or_else(|| PathBuf::from("/nonexistent"))
}

/// Ids name files: lowercase hex and dashes only, so none can leave the
/// folder.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b) || b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::Message;
    use std::os::unix::fs::PermissionsExt;

    fn temp_store(name: &str) -> Store {
        let dir =
            std::env::temp_dir().join(format!("gates-core-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        Store::at(dir)
    }

    fn mode(p: &Path) -> u32 {
        fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    /// A conversation with one message `text`, saved in `store`.
    fn saved(store: &Store, id: &str, text: &str) -> Conversation {
        let mut c = Conversation::new(text);
        c.id = id.to_string();
        c.messages.push(Message::user(text));
        store.save(&c).unwrap();
        c
    }

    fn file(store: &Store, name: &str) -> PathBuf {
        store.dir().join(name)
    }

    /// The files in the damaged folder, by name.
    fn damaged(store: &Store) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(store.damaged_dir())
            .map(|d| {
                d.filter_map(Result::ok)
                    .map(|e| e.file_name().into_string().unwrap())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    #[test]
    fn files_are_private() {
        let dir = std::env::temp_dir().join(format!("gates-private-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let store = Store::at(dir.join("conversations"));
        let c = Conversation::new("secret");
        store.save(&c).unwrap();
        store.save(&c).unwrap();
        assert_eq!(mode(store.dir()), 0o700);
        assert_eq!(mode(&store.dir().join(format!("{}.json", c.id))), 0o600);
        assert_eq!(mode(&store.dir().join(format!("{}.json.bak", c.id))), 0o600);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn save_list_load_delete() {
        let store = temp_store("cycle");
        assert!(store.list().is_empty());

        let mut old = Conversation::new("first");
        old.updated = 1;
        old.messages.push(Message::user("first"));
        store.save(&old).unwrap();
        let mut new = Conversation::new("second");
        new.updated = 2;
        store.save(&new).unwrap();

        let list = store.list();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, new.id, "newest first");
        assert_eq!(store.load(&old.id).unwrap(), old);

        store.delete(&old.id).unwrap();
        store.delete(&old.id).unwrap();
        assert_eq!(store.list().len(), 1);
        let _ = fs::remove_dir_all(store.dir());
    }

    #[test]
    fn deleting_takes_the_backup_and_leftovers_too() {
        let store = temp_store("delete");
        let c = saved(&store, "aa-1", "one");
        saved(&store, "aa-1", "two");
        assert!(file(&store, "aa-1.json.bak").exists());
        fs::write(file(&store, ".aa-1.json.tmp"), "{").unwrap();
        fs::write(file(&store, ".aa-1.json.bak.tmp"), "{").unwrap();
        store.delete(&c.id).unwrap();
        let left: Vec<_> = fs::read_dir(store.dir()).unwrap().collect();
        assert!(left.is_empty(), "{left:?}");
        let _ = fs::remove_dir_all(store.dir());
    }

    #[test]
    fn ids_cannot_leave_the_folder() {
        assert!(valid_id("0192abc-0001"));
        assert!(!valid_id("../x"));
        assert!(!valid_id("a/b"));
        assert!(!valid_id(""));
        assert!(!valid_id("ABC"));
        let store = temp_store("ids");
        assert!(store.load("../etc").is_err());
        assert!(store.delete("..").is_err());
    }

    // --- Format version ---

    #[test]
    fn a_file_without_a_version_is_version_1_and_is_saved_with_one() {
        let store = temp_store("v1");
        fs::create_dir_all(store.dir()).unwrap();
        fs::write(
            file(&store, "ab-1.json"),
            r#"{"id":"ab-1","title":"t","created":1,"updated":1,"messages":[]}"#,
        )
        .unwrap();
        let mut c = store.load("ab-1").unwrap();
        assert_eq!(c.version, 1);
        store.save(&c).unwrap();
        let text = fs::read_to_string(file(&store, "ab-1.json")).unwrap();
        assert!(text.contains(r#""version": 1"#), "{text}");
        // Whatever the version in memory, the current one is what is written.
        c.version = 0;
        store.save(&c).unwrap();
        let text = fs::read_to_string(file(&store, "ab-1.json")).unwrap();
        assert!(text.contains(&format!(r#""version": {VERSION}"#)), "{text}");
        let _ = fs::remove_dir_all(store.dir());
    }

    #[test]
    fn a_newer_version_is_never_touched() {
        let store = temp_store("newer");
        fs::create_dir_all(store.dir()).unwrap();
        // One that still fits today's fields, and one that doesn't.
        let fits = r#"{"version":2,"id":"ab-1","title":"t","created":1,"updated":1,"messages":[],"future":true}"#;
        let odd = r#"{"version":7,"id":"ab-2","title":["a list now"]}"#;
        fs::write(file(&store, "ab-1.json"), fits).unwrap();
        fs::write(file(&store, "ab-2.json"), odd).unwrap();

        let listing = store.scan();
        assert!(listing.conversations.is_empty());
        assert_eq!(listing.report.newer, 2);
        assert_eq!(listing.report.set_aside, 0);
        assert!(damaged(&store).is_empty());

        let e = store.load("ab-1").unwrap_err();
        assert!(is_newer_format(&e));
        assert!(e.to_string().contains("newer version"), "{e}");

        // Saving a conversation with that id fails and leaves the file.
        let mut c = Conversation::new("x");
        c.id = "ab-1".into();
        let e = store.save(&c).unwrap_err();
        assert!(is_newer_format(&e));
        assert_eq!(fs::read_to_string(file(&store, "ab-1.json")).unwrap(), fits);
        assert_eq!(fs::read_to_string(file(&store, "ab-2.json")).unwrap(), odd);
        assert!(!file(&store, "ab-1.json.bak").exists());
        let _ = fs::remove_dir_all(store.dir());
    }

    // --- Damaged files ---

    #[test]
    fn a_damaged_file_is_set_aside_not_skipped_or_overwritten() {
        let store = temp_store("bad");
        fs::create_dir_all(store.dir()).unwrap();
        fs::write(file(&store, "abc.json"), "not json").unwrap();
        // Not a conversation file at all: left alone.
        fs::write(file(&store, "notes.txt"), "{}").unwrap();

        let listing = store.scan();
        assert!(listing.conversations.is_empty());
        assert_eq!(
            listing.report,
            Report {
                set_aside: 1,
                ..Report::default()
            }
        );
        assert!(!file(&store, "abc.json").exists());
        assert!(file(&store, "notes.txt").exists());

        let names = damaged(&store);
        assert_eq!(names.len(), 1, "{names:?}");
        // abc.<YYYYMMDD-HHMMSS>.json
        let name = &names[0];
        let stamp = name
            .strip_prefix("abc.")
            .and_then(|n| n.strip_suffix(".json"))
            .unwrap();
        assert_eq!(stamp.len(), 15, "{name}");
        assert_eq!(&stamp[8..9], "-");
        assert!(
            stamp
                .bytes()
                .filter(|b| *b != b'-')
                .all(|b| b.is_ascii_digit())
        );
        let moved = store.damaged_dir().join(name);
        assert_eq!(fs::read_to_string(&moved).unwrap(), "not json");
        assert_eq!(mode(&moved), 0o600);
        assert_eq!(mode(&store.damaged_dir()), 0o700);

        // It is not found again, nor reported twice.
        assert_eq!(store.scan(), Listing::default());
        let _ = fs::remove_dir_all(store.dir());
    }

    #[test]
    fn a_set_aside_file_keeps_private_permissions_whatever_it_had() {
        let store = temp_store("perm");
        fs::create_dir_all(store.dir()).unwrap();
        fs::write(file(&store, "abc.json"), "").unwrap();
        fs::set_permissions(file(&store, "abc.json"), fs::Permissions::from_mode(0o644)).unwrap();
        store.scan();
        let name = &damaged(&store)[0];
        assert_eq!(mode(&store.damaged_dir().join(name)), 0o600);
        let _ = fs::remove_dir_all(store.dir());
    }

    #[test]
    fn a_file_whose_id_is_not_its_name_is_set_aside() {
        let store = temp_store("wrongid");
        let c = Conversation::new("x");
        fs::create_dir_all(store.dir()).unwrap();
        fs::write(file(&store, "abc.json"), serde_json::to_vec(&c).unwrap()).unwrap();
        let listing = store.scan();
        assert!(listing.conversations.is_empty());
        assert_eq!(listing.report.set_aside, 1);
        let _ = fs::remove_dir_all(store.dir());
    }

    #[test]
    fn two_damaged_files_with_one_name_do_not_replace_each_other() {
        let store = temp_store("twice");
        fs::create_dir_all(store.dir()).unwrap();
        for text in ["first", "second"] {
            fs::write(file(&store, "abc.json"), text).unwrap();
            store.scan();
        }
        assert_eq!(damaged(&store).len(), 2);
        let _ = fs::remove_dir_all(store.dir());
    }

    #[test]
    fn the_reason_never_quotes_the_file() {
        // serde's own messages quote the value that was wrong.
        let text = br#"{"id":"abc","title":"t","created":"MY SECRET","updated":1,"messages":[]}"#;
        match parse(text, "abc") {
            Parsed::Damaged(why) => {
                assert!(!why.contains("SECRET"), "{why}");
                assert!(why.contains("line 1"), "{why}");
            }
            _ => panic!("should be damaged"),
        }
    }

    #[test]
    fn opening_a_damaged_file_sets_it_aside_and_says_so() {
        let store = temp_store("open");
        let c = saved(&store, "ab-1", "hello");
        fs::write(file(&store, "ab-1.json"), "{\"id\": ").unwrap();
        let (result, report) = store.load_reporting(&c.id);
        assert!(result.unwrap_err().to_string().contains("set aside"));
        assert_eq!(report.set_aside, 1);
        assert_eq!(report.restored, 0);
        assert_eq!(damaged(&store).len(), 1);
        let _ = fs::remove_dir_all(store.dir());
    }

    // --- Backups ---

    #[test]
    fn a_save_keeps_the_version_before_it() {
        let store = temp_store("bak");
        let mut c = saved(&store, "ab-1", "one");
        assert!(
            !file(&store, "ab-1.json.bak").exists(),
            "nothing to back up"
        );
        c.messages.push(Message::assistant("two"));
        store.save(&c).unwrap();
        let bak: Conversation =
            serde_json::from_slice(&fs::read(file(&store, "ab-1.json.bak")).unwrap()).unwrap();
        assert_eq!(bak.messages.len(), 1);
        c.messages.push(Message::user("three"));
        store.save(&c).unwrap();
        // One generation: the version before the last save.
        let bak: Conversation =
            serde_json::from_slice(&fs::read(file(&store, "ab-1.json.bak")).unwrap()).unwrap();
        assert_eq!(bak.messages.len(), 2);
        assert_eq!(store.load("ab-1").unwrap().messages.len(), 3);
        // The backup is not a conversation of its own.
        assert_eq!(store.list().len(), 1);
        let _ = fs::remove_dir_all(store.dir());
    }

    #[test]
    fn a_damaged_file_is_restored_from_its_backup() {
        let store = temp_store("restore");
        let mut c = saved(&store, "ab-1", "one");
        c.messages.push(Message::assistant("two"));
        store.save(&c).unwrap();
        fs::write(file(&store, "ab-1.json"), "{\"id\": \"ab").unwrap();

        let listing = store.scan();
        assert_eq!(listing.conversations.len(), 1);
        assert_eq!(listing.conversations[0].id, "ab-1");
        assert_eq!(
            listing.report,
            Report {
                set_aside: 1,
                restored: 1,
                ..Report::default()
            }
        );
        // The backup stands in; the damaged file is kept.
        assert_eq!(store.load("ab-1").unwrap().messages.len(), 1);
        let names = damaged(&store);
        assert_eq!(names.len(), 1);
        assert_eq!(
            fs::read_to_string(store.damaged_dir().join(&names[0])).unwrap(),
            "{\"id\": \"ab"
        );
        assert_eq!(mode(&file(&store, "ab-1.json")), 0o600);
        // And it is a good file now, backed up on the next save.
        store.save(&c).unwrap();
        assert_eq!(store.load("ab-1").unwrap().messages.len(), 2);
        let _ = fs::remove_dir_all(store.dir());
    }

    #[test]
    fn a_damaged_backup_does_not_restore() {
        let store = temp_store("badbak");
        let mut c = saved(&store, "ab-1", "one");
        store.save(&c).unwrap();
        c.messages.clear();
        fs::write(file(&store, "ab-1.json"), "x").unwrap();
        fs::write(file(&store, "ab-1.json.bak"), "y").unwrap();
        let listing = store.scan();
        assert!(listing.conversations.is_empty());
        assert_eq!(listing.report.set_aside, 1);
        assert_eq!(listing.report.restored, 0);
        // The backup is left for the user.
        assert_eq!(
            fs::read_to_string(file(&store, "ab-1.json.bak")).unwrap(),
            "y"
        );
        let _ = fs::remove_dir_all(store.dir());
    }

    #[test]
    fn saving_over_a_file_that_went_bad_sets_it_aside_and_keeps_the_good_backup() {
        let store = temp_store("overbad");
        let mut c = saved(&store, "ab-1", "one");
        c.messages.push(Message::assistant("two"));
        store.save(&c).unwrap();
        // Something broke the file while the conversation was open.
        fs::write(file(&store, "ab-1.json"), "garbage").unwrap();
        c.messages.push(Message::user("three"));
        store.save(&c).unwrap();

        let names = damaged(&store);
        assert_eq!(names.len(), 1);
        assert_eq!(
            fs::read_to_string(store.damaged_dir().join(&names[0])).unwrap(),
            "garbage"
        );
        assert_eq!(store.load("ab-1").unwrap().messages.len(), 3);
        // The backup is still the last good version, not the garbage.
        let bak: Conversation =
            serde_json::from_slice(&fs::read(file(&store, "ab-1.json.bak")).unwrap()).unwrap();
        assert_eq!(bak.messages.len(), 1);
        let _ = fs::remove_dir_all(store.dir());
    }

    // --- Interrupted writes ---

    #[test]
    fn a_cut_short_temporary_file_is_removed() {
        let store = temp_store("tmp-cut");
        let c = saved(&store, "ab-1", "one");
        let whole = serde_json::to_vec(&c).unwrap();
        fs::write(file(&store, ".ab-1.json.tmp"), &whole[..whole.len() / 2]).unwrap();
        fs::write(file(&store, ".ab-1.json.bak.tmp"), "{").unwrap();
        // A first save that never finished: no conversation to go with it.
        fs::write(file(&store, ".ab-2.json.tmp"), "{\"id\":").unwrap();

        let listing = store.scan();
        assert_eq!(listing.conversations.len(), 1);
        assert!(listing.report.is_empty(), "{:?}", listing.report);
        for name in [".ab-1.json.tmp", ".ab-1.json.bak.tmp", ".ab-2.json.tmp"] {
            assert!(!file(&store, name).exists(), "{name}");
        }
        assert_eq!(store.load("ab-1").unwrap(), c);
        let _ = fs::remove_dir_all(store.dir());
    }

    #[test]
    fn a_whole_temporary_file_beside_its_file_is_removed() {
        // The save had not been renamed in: the file is the last saved state.
        let store = temp_store("tmp-beside");
        let c = saved(&store, "ab-1", "one");
        let mut next = c.clone();
        next.messages.push(Message::assistant("never committed"));
        fs::write(
            file(&store, ".ab-1.json.tmp"),
            serde_json::to_vec(&next).unwrap(),
        )
        .unwrap();
        store.scan();
        assert!(!file(&store, ".ab-1.json.tmp").exists());
        assert_eq!(store.load("ab-1").unwrap(), c);
        let _ = fs::remove_dir_all(store.dir());
    }

    #[test]
    fn a_whole_temporary_file_without_its_file_is_finished() {
        let store = temp_store("tmp-whole");
        fs::create_dir_all(store.dir()).unwrap();
        let mut c = Conversation::new("lost");
        c.id = "ab-1".into();
        c.messages.push(Message::user("lost"));
        fs::write(
            file(&store, ".ab-1.json.tmp"),
            serde_json::to_vec_pretty(&c).unwrap(),
        )
        .unwrap();

        let listing = store.scan();
        assert_eq!(listing.conversations.len(), 1);
        assert_eq!(
            listing.report,
            Report {
                finished: 1,
                ..Report::default()
            }
        );
        assert!(!file(&store, ".ab-1.json.tmp").exists());
        assert_eq!(store.load("ab-1").unwrap(), c);
        let _ = fs::remove_dir_all(store.dir());
    }

    #[test]
    fn other_dot_files_are_left_alone() {
        let store = temp_store("tmp-other");
        fs::create_dir_all(store.dir()).unwrap();
        for name in [".keep.tmp", ".ABC.json.tmp", ".x.json.bak", ".notes"] {
            fs::write(file(&store, name), "mine").unwrap();
        }
        assert_eq!(store.scan(), Listing::default());
        for name in [".keep.tmp", ".ABC.json.tmp", ".x.json.bak", ".notes"] {
            assert!(file(&store, name).exists(), "{name}");
        }
        let _ = fs::remove_dir_all(store.dir());
    }

    // --- The notice ---

    #[test]
    fn the_notice_is_worded_for_the_window() {
        let dir = Path::new("/data/conversations/damaged");
        let report = |set_aside, restored, finished, newer| Report {
            set_aside,
            restored,
            finished,
            newer,
        };
        assert_eq!(report(0, 0, 0, 0).describe(dir), "");
        assert_eq!(
            report(1, 0, 0, 0).describe(dir),
            "1 conversation couldn't be read and was set aside in /data/conversations/damaged."
        );
        assert_eq!(
            report(3, 0, 0, 0).describe(dir),
            "3 conversations couldn't be read and were set aside in /data/conversations/damaged."
        );
        assert_eq!(
            report(1, 1, 0, 0).describe(dir),
            "1 conversation couldn't be read and was restored from its backup; the damaged file is in /data/conversations/damaged."
        );
        assert_eq!(
            report(3, 2, 0, 0).describe(dir),
            "1 conversation couldn't be read and was set aside in /data/conversations/damaged. 2 conversations couldn't be read and were restored from their backups; the damaged files are in /data/conversations/damaged."
        );
        assert_eq!(
            report(0, 0, 1, 2).describe(dir),
            "1 conversation was recovered from an interrupted save. 2 conversations were saved by a newer version of Telamon Gates and were left as they are."
        );
        assert_eq!(
            report(0, 0, 0, 1).describe(dir),
            "1 conversation was saved by a newer version of Telamon Gates and was left as it is."
        );
    }

    #[test]
    fn reports_add_up() {
        let mut total = Report::default();
        let a = Report {
            set_aside: 1,
            newer: 2,
            ..Report::default()
        };
        total.merge(&a);
        total.merge(&a);
        assert_eq!(total.set_aside, 2);
        // The same newer files, seen twice, are still two.
        assert_eq!(total.newer, 2);
    }
}
