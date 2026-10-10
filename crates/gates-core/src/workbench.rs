//! The coding workspace's side of the conversation's folder: the files the
//! panel lists, opens and saves, the lines an edit changed (to mark them in
//! the editor), which files the user has unsaved changes in (an edit of one
//! of those asks first), what a command printed (to show it as it comes),
//! and a watch on the folders the tree shows.
//!
//! Every path is checked against the workspace the way the agent's tools
//! check theirs (`Workspace::resolve`: links are resolved, and a path that
//! ends outside is refused), and a file is written the way `write_file`
//! writes it (a new name and a rename, never in place). What a file holds is
//! untrusted: nothing here reads it as anything but text.
//!
//! Blocks (files, a command): call from a worker thread.

use crate::tools::{self, Workspace};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The most entries one folder of the tree shows.
pub const MAX_ENTRIES: usize = 2000;
/// The largest file the panel opens. The editor edits up to 1 MiB and shows
/// more read-only.
pub const MAX_OPEN: u64 = 8 * 1024 * 1024;
/// The most a save writes.
const MAX_SAVE: usize = 64 * 1024 * 1024;
/// An edit whose file is larger than this is not sent as text: the editor
/// reads the file from disk when it opens it.
pub const MAX_LIVE: usize = 1024 * 1024;

// ---------------------------------------------------------------- the tree

/// One name in a folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Entry {
    #[serde(rename = "n")]
    pub name: String,
    #[serde(rename = "d")]
    pub dir: bool,
}

/// A folder's entries: folders first, then files, each by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    pub entries: Vec<Entry>,
    /// Entries left out because the folder has more than `MAX_ENTRIES`.
    pub more: usize,
}

impl Listing {
    /// The entries as JSON for the window: `[{"n": name, "d": is a folder}]`.
    pub fn json(&self) -> String {
        serde_json::to_string(&self.entries).unwrap_or_else(|_| "[]".into())
    }
}

/// `dirs` as a JSON list of strings.
pub fn dirs_json(dirs: &[String]) -> String {
    serde_json::to_string(dirs).unwrap_or_else(|_| "[]".into())
}

/// What is in folder `rel` (relative to the workspace). Build output,
/// dependencies and version control (`.git`, `target`, `node_modules`, …)
/// are left out, and so is a link that ends outside the workspace.
pub fn list(ws: &Workspace, rel: &str) -> Result<Listing, String> {
    let dir = ws.resolve(rel)?;
    if !dir.is_dir() {
        return Err(format!("{} isn't a folder.", ws.show(&dir)));
    }
    let read = fs::read_dir(&dir).map_err(|e| format!("Can't list {}: {e}.", ws.show(&dir)))?;
    let mut entries = Vec::new();
    for entry in read.filter_map(Result::ok) {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_dir = if kind.is_symlink() {
            match entry.path().canonicalize() {
                Ok(real) if real.starts_with(ws.root()) && (real.is_dir() || real.is_file()) => {
                    real.is_dir()
                }
                _ => continue,
            }
        } else if kind.is_dir() {
            true
        } else if kind.is_file() {
            false
        } else {
            // A socket, a pipe, a device.
            continue;
        };
        if is_dir && tools::SKIP.contains(&name.as_str()) {
            continue;
        }
        entries.push(Entry { name, dir: is_dir });
    }
    entries.sort_by(|a, b| {
        b.dir
            .cmp(&a.dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
    let more = entries.len().saturating_sub(MAX_ENTRIES);
    entries.truncate(MAX_ENTRIES);
    Ok(Listing { entries, more })
}

// ------------------------------------------------------- opening and saving

/// A file as the editor takes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opened {
    /// The file, relative to the workspace, as it is kept in the tabs.
    pub path: String,
    pub text: String,
    /// Every line ends in CRLF; the editor reads LF, so a save puts CRLF
    /// back.
    pub crlf: bool,
    /// Not safe to write back as the editor will have it: lines that end
    /// differently, or a lone CR.
    pub read_only: bool,
    /// It has bidirectional control characters, which make code read in
    /// another order than it runs.
    pub bidi: bool,
}

/// Whether `text` has a bidirectional control character (U+202A–U+202E,
/// U+2066–U+2069).
pub fn has_bidi(text: &str) -> bool {
    text.chars()
        .any(|c| matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'))
}

/// File `rel`, if it is a text file inside the workspace of a size to open.
pub fn open(ws: &Workspace, rel: &str) -> Result<Opened, String> {
    let real = ws.resolve(rel)?;
    let shown = ws.show(&real);
    let meta = fs::metadata(&real).map_err(|e| format!("Can't open {shown}: {e}."))?;
    if !meta.is_file() {
        return Err(format!("{shown} isn't a file."));
    }
    if meta.len() > MAX_OPEN {
        return Err(format!(
            "{shown} is too big to open here ({} MiB).",
            meta.len() / (1024 * 1024)
        ));
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    fs::File::open(&real)
        .and_then(|f| f.take(MAX_OPEN).read_to_end(&mut bytes))
        .map_err(|e| format!("Can't open {shown}: {e}."))?;
    if bytes.contains(&0) {
        return Err(format!("{shown} isn't a text file."));
    }
    let text = String::from_utf8(bytes).map_err(|_| format!("{shown} isn't UTF-8 text."))?;
    let lf = text.matches('\n').count();
    let crlf_lines = text.matches("\r\n").count();
    let lone_cr = text.matches('\r').count() - crlf_lines;
    Ok(Opened {
        path: shown,
        crlf: crlf_lines > 0 && crlf_lines == lf && lone_cr == 0,
        read_only: lone_cr > 0 || (crlf_lines > 0 && crlf_lines != lf),
        bidi: has_bidi(&text),
        text,
    })
}

/// Writes `text` (what the editor holds: LF lines) to file `rel`, with CRLF
/// lines when `crlf`. Inside the workspace only, and replacing the file the
/// way the agent's `write_file` does. The file's path, as the tabs have it.
pub fn save(ws: &Workspace, rel: &str, text: &str, crlf: bool) -> Result<String, String> {
    let real = ws.resolve(rel)?;
    let shown = ws.show(&real);
    if real.is_dir() {
        return Err(format!("{shown} is a folder."));
    }
    let out = if crlf {
        text.replace("\r\n", "\n").replace('\n', "\r\n")
    } else {
        text.to_string()
    };
    if out.len() > MAX_SAVE {
        return Err(format!("{shown} is too big to save here."));
    }
    tools::replace(&real, &out).map_err(|e| format!("Can't save {shown}: {e}."))?;
    Ok(shown)
}

/// Makes an empty file `rel` in a folder of the workspace that exists. Never
/// over a file that is there. The file's path, as the tabs have it.
pub fn create(ws: &Workspace, rel: &str) -> Result<String, String> {
    if rel.trim().is_empty() {
        return Err("Give the file a name.".into());
    }
    let real = ws.resolve(rel)?;
    let shown = ws.show(&real);
    if fs::symlink_metadata(&real).is_ok() {
        return Err(format!("{shown} is there already."));
    }
    tools::replace(&real, "").map_err(|e| format!("Can't make {shown}: {e}."))?;
    Ok(shown)
}

/// The text of file `rel` for comparing before and after an edit; None when
/// it isn't there, isn't text or is larger than `MAX_LIVE`.
pub fn snapshot(ws: &Workspace, rel: &str) -> Option<String> {
    let real = ws.resolve(rel).ok()?;
    let meta = fs::metadata(&real).ok()?;
    if !meta.is_file() || meta.len() > MAX_LIVE as u64 {
        return None;
    }
    tools::text_of(&real).ok()
}

// ---------------------------------------------------------------- the diff

/// Lines of the new text that an edit changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mark {
    /// Line numbers from 1, both included.
    pub first: usize,
    pub last: usize,
    /// New lines only (nothing of the old text was in their place); else
    /// they replace some.
    pub added: bool,
}

/// What changed from `old` to `new`, by lines.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Diff {
    pub marks: Vec<Mark>,
    /// The first line to look at (from 1): the first change, also when it
    /// only removed lines. None when the texts are the same.
    pub first: Option<usize>,
}

/// The longest comparison done line by line: lines of the old text times
/// lines of the new text, once the lines both start and end with are
/// taken off. Beyond it the lines in between are all marked changed.
const LCS_CELLS: usize = 2_000_000;

/// The lines of `new` that are not in `old`, found by the longest run of
/// lines the two share. A run of new lines with no old line in its place is
/// `added`; one that replaces some is changed.
pub fn changed_lines(old: &str, new: &str) -> Diff {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let mut start = 0;
    while start < a.len() && start < b.len() && a[start] == b[start] {
        start += 1;
    }
    let (mut end_a, mut end_b) = (a.len(), b.len());
    while end_a > start && end_b > start && a[end_a - 1] == b[end_b - 1] {
        end_a -= 1;
        end_b -= 1;
    }
    let (am, bm) = (&a[start..end_a], &b[start..end_b]);
    if am.is_empty() && bm.is_empty() {
        return Diff::default();
    }
    // Pairs of equal lines in order (old index, new index), within the
    // middle, with a sentinel at each end.
    let mut pairs: Vec<(usize, usize)> = vec![(usize::MAX, usize::MAX)];
    if !am.is_empty() && !bm.is_empty() && am.len().saturating_mul(bm.len()) <= LCS_CELLS {
        let (n, m) = (am.len(), bm.len());
        // len[i][j]: the longest run in am[i..] and bm[j..].
        let mut len = vec![0u32; (n + 1) * (m + 1)];
        let at = |i: usize, j: usize| i * (m + 1) + j;
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                len[at(i, j)] = if am[i] == bm[j] {
                    len[at(i + 1, j + 1)] + 1
                } else {
                    len[at(i + 1, j)].max(len[at(i, j + 1)])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n && j < m {
            if am[i] == bm[j] {
                pairs.push((i, j));
                i += 1;
                j += 1;
            } else if len[at(i + 1, j)] >= len[at(i, j + 1)] {
                i += 1;
            } else {
                j += 1;
            }
        }
    }
    pairs.push((am.len(), bm.len()));
    let mut diff = Diff::default();
    // The gap between two shared lines: the old lines dropped, the new ones
    // put in.
    for w in pairs.windows(2) {
        let (i0, j0) = (w[0].0.wrapping_add(1), w[0].1.wrapping_add(1));
        let (i1, j1) = w[1];
        let (dropped, put) = (i1 - i0, j1 - j0);
        if dropped == 0 && put == 0 {
            continue;
        }
        let line = start + j0 + 1;
        diff.first.get_or_insert(line);
        if put > 0 {
            diff.marks.push(Mark {
                first: line,
                last: line + put - 1,
                added: dropped == 0,
            });
        }
    }
    diff
}

impl Diff {
    /// The marks as JSON for the window: `[[first, last, added]]`.
    pub fn marks_json(&self) -> String {
        let marks: Vec<(usize, usize, bool)> = self
            .marks
            .iter()
            .map(|m| (m.first, m.last, m.added))
            .collect();
        serde_json::to_string(&marks).unwrap_or_else(|_| "[]".into())
    }
}

/// An edit the agent made to a file, as the editor shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// The file, relative to the workspace.
    pub path: String,
    /// The file did not exist before.
    pub created: bool,
    /// The whole file now; None when it is too large or not text (the
    /// editor reads the file).
    pub text: Option<String>,
    pub diff: Diff,
}

/// The change from `before` to `after`, the texts of file `path`
/// (`snapshot`s taken around the edit); `existed`: the file was there before
/// (its text may still be None, when it was too large or not text).
pub fn change(path: &str, existed: bool, before: Option<&str>, after: Option<String>) -> Change {
    let diff = match (&after, before) {
        (Some(new), Some(old)) => changed_lines(old, new),
        (Some(new), None) if !existed => changed_lines("", new),
        _ => Diff::default(),
    };
    Change {
        path: path.to_string(),
        created: !existed,
        text: after,
        diff,
    }
}

/// What the agent did to a file that the panel shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Touch {
    /// `read_file` read it.
    Read(String),
    /// `write_file` or `edit_file` changed it.
    Edited(Change),
}

// ------------------------------------------------------ unsaved changes

/// Which files the user has unsaved changes in (the editor tells), and which
/// they have saved since the model last heard of them. Shared between the
/// window and the agent's worker.
#[derive(Default)]
pub struct Edits {
    state: Mutex<EditState>,
}

#[derive(Default)]
struct EditState {
    unsaved: HashSet<String>,
    saved: Vec<String>,
}

impl Edits {
    fn state(&self) -> std::sync::MutexGuard<'_, EditState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The editor has (or no longer has) unsaved changes in `path`.
    pub fn set_unsaved(&self, path: &str, unsaved: bool) {
        let mut state = self.state();
        if unsaved {
            state.unsaved.insert(path.to_string());
        } else {
            state.unsaved.remove(path);
        }
    }

    /// The user has unsaved changes in `path`: an edit of it asks first.
    pub fn is_unsaved(&self, path: &str) -> bool {
        self.state().unsaved.contains(path)
    }

    /// The user saved `path`: it is no longer unsaved, and the model is
    /// told at its next turn that the file changed.
    pub fn note_saved(&self, path: &str) {
        let mut state = self.state();
        state.unsaved.remove(path);
        if !state.saved.iter().any(|p| p == path) {
            state.saved.push(path.to_string());
        }
    }

    /// The files saved since the last call.
    pub fn take_saved(&self) -> Vec<String> {
        std::mem::take(&mut self.state().saved)
    }

    /// Another folder is shown: nothing is known of the old one's files.
    pub fn reset(&self) {
        *self.state() = EditState::default();
    }
}

/// How many saved files the model is told of at most.
const NOTE_FILES: usize = 20;

/// What the model is told when the user has saved files since its last turn.
/// Only saved files: it reads from the disk, and what is unsaved is not
/// there. None when `paths` is empty.
pub fn saved_note(paths: &[String]) -> Option<String> {
    if paths.is_empty() {
        return None;
    }
    let mut list: Vec<String> = paths
        .iter()
        .take(NOTE_FILES)
        .map(|p| p.chars().take(200).collect::<String>().replace('\n', " "))
        .collect();
    if paths.len() > NOTE_FILES {
        list.push(format!("and {} more", paths.len() - NOTE_FILES));
    }
    Some(format!(
        "The user changed and saved these files in the workspace's editor since your last \
         turn: {}. Read a file again before you edit it.",
        list.join(", ")
    ))
}

// ----------------------------------------------------- what a command prints

/// Where what a command prints goes while it runs, and how it ended.
pub trait Sink: Send + Sync {
    /// A piece of its output, as the bytes came.
    fn write(&self, bytes: &[u8]);
    /// The command ended: `line` says how (one line), `ok` whether it
    /// succeeded.
    fn finish(&self, line: &str, ok: bool);
}

/// How often what a command printed is handed on.
const TICK: Duration = Duration::from_millis(33);
/// The most output kept between two ticks; a command that prints faster has
/// its oldest bytes left out.
const MAX_PENDING: usize = 512 * 1024;
const KEEP_PENDING: usize = 128 * 1024;

struct Batched {
    pending: Mutex<Pending>,
    emit: Box<dyn Fn(String) + Send + Sync>,
    done: AtomicBool,
}

struct Pending {
    bytes: Vec<u8>,
    dropped: usize,
    /// The last text handed on ended a line (or nothing was yet).
    at_line_start: bool,
}

/// A `Sink` that hands the output on in pieces at most every 33 ms, as
/// complete text (a character is never cut in two), so a command that
/// prints a flood costs the window one update per tick. How it ended comes
/// as a dim (or, on failure, red) line, using the console's colour
/// sequences.
pub struct Batcher {
    shared: Arc<Batched>,
}

impl Batcher {
    pub fn new(emit: impl Fn(String) + Send + Sync + 'static) -> Arc<Batcher> {
        let shared = Arc::new(Batched {
            pending: Mutex::new(Pending {
                bytes: Vec::new(),
                dropped: 0,
                at_line_start: true,
            }),
            emit: Box::new(emit),
            done: AtomicBool::new(false),
        });
        let weak = Arc::downgrade(&shared);
        let _ = std::thread::Builder::new()
            .name("gates-console".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(TICK);
                    let Some(shared) = weak.upgrade() else {
                        break;
                    };
                    if shared.done.load(Ordering::Relaxed) {
                        break;
                    }
                    shared.flush(false);
                }
            });
        Arc::new(Batcher { shared })
    }

    /// A line of the app's own (what is run), dim, before the output.
    pub fn note(&self, line: &str) {
        let one: String = line
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(300)
            .collect();
        let more = if line.contains('\n') { " …" } else { "" };
        self.write(format!("\x1b[2m{one}{more}\x1b[0m\n").as_bytes());
    }
}

impl Batched {
    fn pending(&self) -> std::sync::MutexGuard<'_, Pending> {
        self.pending.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Hands on what is pending; with `all`, also an unfinished character.
    fn flush(&self, all: bool) {
        let text = {
            let mut p = self.pending();
            if p.bytes.is_empty() && p.dropped == 0 {
                return;
            }
            let mut text = String::new();
            if p.dropped > 0 {
                text.push_str(&format!("\n… {} bytes left out …\n", p.dropped));
                p.dropped = 0;
            }
            text.push_str(&complete_text(&mut p.bytes, all));
            if text.is_empty() {
                return;
            }
            p.at_line_start = text.ends_with('\n');
            text
        };
        (self.emit)(text);
    }
}

/// The text at the start of `bytes`, which is taken off: everything but a
/// character cut off by the end (kept for the next piece) unless `all`.
fn complete_text(bytes: &mut Vec<u8>, all: bool) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => {
            let s = s.to_string();
            bytes.clear();
            s
        }
        Err(e) if e.error_len().is_none() && !all => {
            let rest = bytes.split_off(e.valid_up_to());
            let s = String::from_utf8_lossy(bytes).into_owned();
            *bytes = rest;
            s
        }
        Err(_) => {
            let s = String::from_utf8_lossy(bytes).into_owned();
            bytes.clear();
            s
        }
    }
}

impl Sink for Batcher {
    fn write(&self, bytes: &[u8]) {
        let mut p = self.shared.pending();
        p.bytes.extend_from_slice(bytes);
        if p.bytes.len() > MAX_PENDING {
            let over = p.bytes.len() - KEEP_PENDING;
            p.bytes.drain(..over);
            p.dropped += over;
        }
    }

    fn finish(&self, line: &str, ok: bool) {
        self.shared.flush(true);
        let at_line_start = self.shared.pending().at_line_start;
        let style = if ok { "\x1b[2m" } else { "\x1b[31m" };
        let lead = if at_line_start { "" } else { "\n" };
        (self.shared.emit)(format!("{lead}{style}{line}\x1b[0m\n"));
        self.shared.done.store(true, Ordering::Relaxed);
    }
}

impl Drop for Batcher {
    fn drop(&mut self) {
        self.shared.done.store(true, Ordering::Relaxed);
    }
}

/// Runs `command` for the user in the workspace's sandbox, without a time
/// limit: what it prints goes to `sink`, and `cancel` stops it (its whole
/// process group). True when it exited with status 0.
pub fn run(ws: &Workspace, command: &str, cancel: &AtomicBool, sink: &Arc<dyn Sink>) -> bool {
    match tools::execute(ws, command, Duration::MAX, cancel, Some(sink)) {
        Ok(ran) => ran.success,
        // It could not start (no bubblewrap, no process left): say so in the
        // console, where the output would be.
        Err(e) => {
            sink.finish(&e, false);
            false
        }
    }
}

// ------------------------------------------------- watching the shown folders

/// A watch on the folders the tree shows (inotify): `changed` is called,
/// from a thread of its own, with the folders (relative to the workspace,
/// "" for its root) where something was made, removed or moved, a moment
/// after the last change. "*" means too much changed to say where.
pub struct Watcher {
    fd: i32,
    watches: Arc<Mutex<HashMap<i32, String>>>,
    stop: Arc<AtomicBool>,
}

/// The most folders watched.
const MAX_WATCHES: usize = 256;
/// How long after the last change the folders are reported.
const SETTLE: Duration = Duration::from_millis(120);

impl Watcher {
    pub fn new(changed: impl Fn(Vec<String>) + Send + 'static) -> io::Result<Watcher> {
        // SAFETY: plain system call; the descriptor is ours.
        let fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC | libc::IN_NONBLOCK) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let watches: Arc<Mutex<HashMap<i32, String>>> = Arc::default();
        let stop = Arc::new(AtomicBool::new(false));
        {
            let (watches, stop) = (watches.clone(), stop.clone());
            // SAFETY: the thread owns the descriptor from here and closes it.
            let owned = unsafe { OwnedFd::from_raw_fd(fd) };
            std::thread::Builder::new()
                .name("gates-watch".into())
                .spawn(move || watch_loop(owned, &watches, &stop, &changed))?;
        }
        Ok(Watcher { fd, watches, stop })
    }

    /// Watches folder `rel` of `ws`. A folder watched already is kept.
    pub fn watch(&self, ws: &Workspace, rel: &str) -> Result<(), String> {
        let dir = ws.resolve(rel)?;
        if !dir.is_dir() {
            return Err(format!("{} isn't a folder.", ws.show(&dir)));
        }
        let shown = ws.show(&dir);
        let shown = if shown == "." { String::new() } else { shown };
        {
            let watches = self.watches.lock().unwrap_or_else(|e| e.into_inner());
            if watches.values().any(|r| *r == shown) {
                return Ok(());
            }
            if watches.len() >= MAX_WATCHES {
                return Err("too many folders to watch".into());
            }
        }
        let path = std::ffi::CString::new(std::os::unix::ffi::OsStrExt::as_bytes(dir.as_os_str()))
            .map_err(|_| "bad folder name".to_string())?;
        let mask = libc::IN_CREATE
            | libc::IN_DELETE
            | libc::IN_MOVED_FROM
            | libc::IN_MOVED_TO
            | libc::IN_DELETE_SELF
            | libc::IN_MOVE_SELF
            | libc::IN_ONLYDIR
            | libc::IN_DONT_FOLLOW;
        // SAFETY: a valid descriptor and a NUL-terminated path.
        let wd = unsafe { libc::inotify_add_watch(self.fd, path.as_ptr(), mask) };
        if wd < 0 {
            return Err(io::Error::last_os_error().to_string());
        }
        self.watches
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(wd, shown);
        Ok(())
    }

    /// Stops watching folder `rel`.
    pub fn unwatch(&self, rel: &str) {
        let mut watches = self.watches.lock().unwrap_or_else(|e| e.into_inner());
        let found: Vec<i32> = watches
            .iter()
            .filter(|(_, r)| **r == rel)
            .map(|(wd, _)| *wd)
            .collect();
        for wd in found {
            // SAFETY: a valid descriptor; removing an unknown watch is an error only.
            unsafe { libc::inotify_rm_watch(self.fd, wd) };
            watches.remove(&wd);
        }
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn watch_loop(
    fd: OwnedFd,
    watches: &Mutex<HashMap<i32, String>>,
    stop: &AtomicBool,
    changed: &dyn Fn(Vec<String>),
) {
    let mut buf = vec![0u8; 16 * 1024];
    let mut dirty: Vec<String> = Vec::new();
    let mut last = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        let mut poll = libc::pollfd {
            fd: fd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one valid pollfd.
        let ready = unsafe { libc::poll(&mut poll, 1, 100) };
        if ready > 0 {
            // SAFETY: reads into our buffer, at most its length.
            let n = unsafe { libc::read(fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
            if n > 0 {
                let mut at = 0usize;
                let n = n as usize;
                let head = std::mem::size_of::<libc::inotify_event>();
                while at + head <= n {
                    // SAFETY: a whole event header is in the buffer; read
                    // unaligned as the buffer is bytes.
                    let event: libc::inotify_event =
                        unsafe { std::ptr::read_unaligned(buf.as_ptr().add(at).cast()) };
                    at += head + event.len as usize;
                    let mut map = watches.lock().unwrap_or_else(|e| e.into_inner());
                    if event.mask & libc::IN_Q_OVERFLOW != 0 {
                        dirty.push("*".into());
                    } else if let Some(rel) = map.get(&event.wd) {
                        dirty.push(rel.clone());
                    }
                    if event.mask & libc::IN_IGNORED != 0 {
                        map.remove(&event.wd);
                    }
                    last = Instant::now();
                }
            }
        }
        if !dirty.is_empty() && last.elapsed() >= SETTLE {
            dirty.sort();
            dirty.dedup();
            changed(std::mem::take(&mut dirty));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gates-bench-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn workspace(name: &str) -> (PathBuf, Workspace) {
        let dir = dir(name);
        let ws = Workspace::open(&dir).unwrap();
        (dir, ws)
    }

    fn lines(n: usize) -> String {
        (1..=n).map(|i| format!("line {i}\n")).collect()
    }

    fn marks(old: &str, new: &str) -> Vec<(usize, usize, bool)> {
        changed_lines(old, new)
            .marks
            .iter()
            .map(|m| (m.first, m.last, m.added))
            .collect()
    }

    #[test]
    fn the_diff_marks_new_and_rewritten_lines() {
        assert_eq!(changed_lines("a\nb\n", "a\nb\n"), Diff::default());
        // Lines put in between: added, from line 2.
        assert_eq!(marks("a\nd\n", "a\nb\nc\nd\n"), [(2, 3, true)]);
        // A line rewritten: changed.
        assert_eq!(marks("a\nb\nc\n", "a\nB\nc\n"), [(2, 2, false)]);
        // One line rewritten as three: changed, all three.
        assert_eq!(marks("a\nb\nc\n", "a\nx\ny\nz\nc\n"), [(2, 4, false)]);
        // Lines added at the end and at the start.
        assert_eq!(marks("a\n", "a\nb\n"), [(2, 2, true)]);
        assert_eq!(marks("b\n", "a\nb\n"), [(1, 1, true)]);
        // Two separate places.
        assert_eq!(
            marks("a\nb\nc\nd\ne\n", "a\nB\nc\nd\nE\nF\n"),
            [(2, 2, false), (5, 6, false)]
        );
        // A new file: every line.
        assert_eq!(marks("", "a\nb\nc\n"), [(1, 3, true)]);
        // Lines taken out leave nothing to mark, but there is a place to look at.
        let removed = changed_lines("a\nb\nc\n", "a\nc\n");
        assert!(removed.marks.is_empty());
        assert_eq!(removed.first, Some(2));
        // Moved lines are found as removed and added.
        assert_eq!(marks("a\nb\nc\nd\n", "a\nd\nb\nc\n"), [(2, 2, true)]);
        // The first change is where to scroll.
        assert_eq!(
            changed_lines(&lines(100), &lines(100).replace("line 60\n", "x\n")).first,
            Some(60)
        );
    }

    #[test]
    fn the_diff_of_a_huge_rewrite_still_answers() {
        let old = lines(5000);
        let new: String = (1..=5000).map(|i| format!("other {i}\n")).collect();
        // Past the line-by-line limit: the whole middle is one change.
        assert_eq!(marks(&old, &new), [(1, 5000, false)]);
        // A small edit in a big file is exact (the shared lines at both ends
        // are taken off first).
        let edited = old.replace("line 2500\n", "line 2500 changed\n");
        assert_eq!(marks(&old, &edited), [(2500, 2500, false)]);
    }

    #[test]
    fn lists_a_folder_without_the_heavy_ones() {
        let (dir, ws) = workspace("list");
        for d in [".git", "target", "node_modules", "src", "Docs"] {
            fs::create_dir(dir.join(d)).unwrap();
        }
        for f in ["main.rs", "a.txt", ".gitignore", "B.md"] {
            fs::write(dir.join(f), "x").unwrap();
        }
        let names: Vec<(String, bool)> = list(&ws, "")
            .unwrap()
            .entries
            .into_iter()
            .map(|e| (e.name, e.dir))
            .collect();
        assert_eq!(
            names,
            [
                ("Docs".to_string(), true),
                ("src".to_string(), true),
                (".gitignore".to_string(), false),
                ("a.txt".to_string(), false),
                ("B.md".to_string(), false),
                ("main.rs".to_string(), false),
            ]
        );
        assert!(list(&ws, "src").unwrap().entries.is_empty());
        assert!(list(&ws, "a.txt").is_err());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn the_tree_does_not_follow_links_out() {
        let (dir, ws) = workspace("links");
        let outside = dir.with_extension("outside");
        let _ = fs::remove_dir_all(&outside);
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret.txt"), "secret").unwrap();
        fs::write(dir.join("inside.txt"), "ok").unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("out-dir")).unwrap();
        std::os::unix::fs::symlink(outside.join("secret.txt"), dir.join("out-file")).unwrap();
        std::os::unix::fs::symlink(dir.join("inside.txt"), dir.join("in-link")).unwrap();
        let names: Vec<String> = list(&ws, "")
            .unwrap()
            .entries
            .into_iter()
            .map(|e| e.name)
            .collect();
        // The links out are not there; the link that stays inside is.
        assert_eq!(names, ["in-link", "inside.txt"]);
        // Asking for them anyway is refused, for listing, opening and saving.
        assert!(list(&ws, "out-dir").is_err());
        assert!(open(&ws, "out-file").is_err());
        assert!(open(&ws, "out-dir/secret.txt").is_err());
        assert!(save(&ws, "out-file", "overwritten", false).is_err());
        assert_eq!(
            fs::read_to_string(outside.join("secret.txt")).unwrap(),
            "secret"
        );
        let _ = fs::remove_dir_all(dir);
        let _ = fs::remove_dir_all(outside);
    }

    #[test]
    fn opens_text_and_refuses_the_rest() {
        let (dir, ws) = workspace("open");
        fs::write(dir.join("a.rs"), "fn main() {}\n").unwrap();
        fs::write(dir.join("crlf.txt"), "a\r\nb\r\n").unwrap();
        fs::write(dir.join("mixed.txt"), "a\r\nb\n").unwrap();
        fs::write(dir.join("bin.dat"), [1u8, 0, 2]).unwrap();
        fs::write(dir.join("latin.txt"), [0x66u8, 0xe9]).unwrap();
        fs::write(dir.join("bidi.rs"), "// \u{202E}x\n").unwrap();
        fs::create_dir(dir.join("d")).unwrap();
        let a = open(&ws, "a.rs").unwrap();
        assert_eq!(
            (a.path.as_str(), a.text.as_str()),
            ("a.rs", "fn main() {}\n")
        );
        assert!(!a.crlf && !a.read_only && !a.bidi);
        // Absolute and dotted paths are the same file.
        assert_eq!(
            open(&ws, &dir.join("./d/../a.rs").to_string_lossy())
                .unwrap()
                .path,
            "a.rs"
        );
        let crlf = open(&ws, "crlf.txt").unwrap();
        assert!(crlf.crlf && !crlf.read_only);
        let mixed = open(&ws, "mixed.txt").unwrap();
        assert!(!mixed.crlf && mixed.read_only);
        assert!(open(&ws, "bidi.rs").unwrap().bidi);
        for bad in ["bin.dat", "latin.txt", "d", "missing.txt", "../x"] {
            assert!(open(&ws, bad).is_err(), "{bad}");
        }
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn saves_inside_the_workspace_only() {
        let (dir, ws) = workspace("save");
        fs::create_dir(dir.join("src")).unwrap();
        fs::write(dir.join("src/a.rs"), "old").unwrap();
        // The permissions stay, and no temporary file is left.
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(dir.join("src/a.rs"), fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert_eq!(save(&ws, "src/a.rs", "new\n", false).unwrap(), "src/a.rs");
        assert_eq!(fs::read_to_string(dir.join("src/a.rs")).unwrap(), "new\n");
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(dir.join("src/a.rs"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o755);
        }
        let leftovers: Vec<_> = fs::read_dir(dir.join("src")).unwrap().collect();
        assert_eq!(leftovers.len(), 1);
        // A new file in a folder that exists.
        assert_eq!(save(&ws, "src/b.rs", "b", false).unwrap(), "src/b.rs");
        // CRLF comes back when the file had it.
        save(&ws, "src/c.txt", "a\nb\n", true).unwrap();
        assert_eq!(fs::read(dir.join("src/c.txt")).unwrap(), b"a\r\nb\r\n");
        // Outside the workspace, or a folder: refused, nothing written.
        let outside = dir.with_extension("outside.txt");
        for bad in [
            "../escape.txt".to_string(),
            "src/../../escape.txt".to_string(),
            outside.to_string_lossy().into_owned(),
            "/etc/passwd".to_string(),
            "src".to_string(),
            "nofolder/x.txt".to_string(),
        ] {
            assert!(save(&ws, &bad, "x", false).is_err(), "{bad}");
        }
        assert!(!outside.exists());
        assert!(!dir.parent().unwrap().join("escape.txt").exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_change_has_the_text_and_the_marks() {
        let c = change("a.rs", true, Some("a\nb\n"), Some("a\nB\nc\n".into()));
        assert!(!c.created);
        assert_eq!(c.text.as_deref(), Some("a\nB\nc\n"));
        assert_eq!(c.diff.first, Some(2));
        assert_eq!(
            c.diff.marks,
            [Mark {
                first: 2,
                last: 3,
                added: false
            }]
        );
        let new = change("n.rs", false, None, Some("x\ny\n".into()));
        assert!(new.created);
        assert_eq!(
            new.diff.marks,
            [Mark {
                first: 1,
                last: 2,
                added: true
            }]
        );
        // A file too large to carry: the editor reads it.
        let big = change("big.rs", true, Some("a"), None);
        assert!(big.text.is_none() && big.diff.marks.is_empty());
    }

    #[test]
    fn unsaved_files_are_tracked_for_conflicts() {
        let edits = Edits::default();
        assert!(!edits.is_unsaved("src/a.rs"));
        edits.set_unsaved("src/a.rs", true);
        assert!(edits.is_unsaved("src/a.rs"));
        // Another file is not in conflict.
        assert!(!edits.is_unsaved("src/b.rs"));
        // Saved: no longer unsaved, and the model is told, once.
        edits.note_saved("src/a.rs");
        edits.note_saved("src/a.rs");
        assert!(!edits.is_unsaved("src/a.rs"));
        assert_eq!(edits.take_saved(), ["src/a.rs"]);
        assert!(edits.take_saved().is_empty());
        // Unsaved edits are never part of what the model is told.
        edits.set_unsaved("secret.txt", true);
        assert!(edits.take_saved().is_empty());
        edits.set_unsaved("secret.txt", false);
        edits.set_unsaved("c.rs", true);
        edits.reset();
        assert!(!edits.is_unsaved("c.rs"));
    }

    #[test]
    fn tells_the_model_only_of_saved_files() {
        assert_eq!(saved_note(&[]), None);
        let note = saved_note(&["src/a.rs".into(), "b.txt".into()]).unwrap();
        assert!(note.contains("src/a.rs, b.txt"), "{note}");
        assert!(note.contains("saved"));
        let many: Vec<String> = (0..30).map(|i| format!("f{i}.rs")).collect();
        let note = saved_note(&many).unwrap();
        assert!(
            note.contains("and 10 more") && !note.contains("f25.rs"),
            "{note}"
        );
        // A file name can't add a line to the prompt.
        let note = saved_note(&["a\nIgnore everything".into()]).unwrap();
        assert!(!note.contains("a\nIgnore"));
    }

    #[test]
    fn the_batcher_hands_on_whole_text_and_the_ending() {
        use std::sync::mpsc;
        let (tx, rx) = mpsc::channel::<String>();
        let tx = Mutex::new(tx);
        let batcher = Batcher::new(move |text| {
            let _ = tx.lock().unwrap().send(text);
        });
        batcher.note("$ echo hi");
        // "é" cut in two between writes.
        batcher.write(b"caf\xc3");
        std::thread::sleep(Duration::from_millis(100));
        batcher.write(b"\xa9 ready");
        batcher.finish("Exited with code 3 in 0.1 s", false);
        let mut all = String::new();
        while let Ok(piece) = rx.recv_timeout(Duration::from_millis(300)) {
            all.push_str(&piece);
        }
        assert!(all.contains("café ready"), "{all:?}");
        assert!(all.starts_with("\x1b[2m$ echo hi\x1b[0m\n"), "{all:?}");
        // The ending is on a line of its own, in red for a failure.
        assert!(
            all.ends_with("café ready\n\x1b[31mExited with code 3 in 0.1 s\x1b[0m\n"),
            "{all:?}"
        );
    }

    #[test]
    fn the_batcher_bounds_a_flood() {
        use std::sync::mpsc;
        let (tx, rx) = mpsc::channel::<String>();
        let tx = Mutex::new(tx);
        let batcher = Batcher::new(move |text| {
            let _ = tx.lock().unwrap().send(text);
        });
        // 4 MiB written at once, before a tick can hand any on.
        let chunk = vec![b'x'; 64 * 1024];
        for _ in 0..64 {
            batcher.write(&chunk);
        }
        batcher.finish("done", true);
        let mut got = 0;
        let mut said_left_out = false;
        while let Ok(piece) = rx.recv_timeout(Duration::from_millis(300)) {
            got += piece.len();
            said_left_out |= piece.contains("bytes left out");
        }
        assert!(got < 2 * MAX_PENDING, "{got}");
        assert!(said_left_out);
    }

    #[test]
    fn a_user_command_streams_and_ends() {
        let (dir, ws) = workspace("run");
        if !crate::sandbox::available() {
            eprintln!("no bubblewrap here: skipped");
            return;
        }
        struct Lines(Mutex<Vec<String>>);
        impl Sink for Lines {
            fn write(&self, bytes: &[u8]) {
                self.0
                    .lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(bytes).into_owned());
            }
            fn finish(&self, line: &str, ok: bool) {
                self.0.lock().unwrap().push(format!("[{ok}] {line}"));
            }
        }
        let sink = Arc::new(Lines(Mutex::new(Vec::new())));
        let dynamic: Arc<dyn Sink> = sink.clone();
        let cancel = AtomicBool::new(false);
        // Both streams, a file made in the workspace, an exit code.
        let ok = run(
            &ws,
            "echo out; echo err >&2; echo made > f.txt; exit 3",
            &cancel,
            &dynamic,
        );
        assert!(!ok);
        let all = sink.0.lock().unwrap().join("");
        assert!(all.contains("out") && all.contains("err"), "{all}");
        assert!(all.contains("[false] Exited with code 3 in "), "{all}");
        assert_eq!(fs::read_to_string(dir.join("f.txt")).unwrap(), "made\n");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn stop_ends_a_running_command_and_everything_it_started() {
        let (dir, ws) = workspace("stop");
        if !crate::sandbox::available() {
            eprintln!("no bubblewrap here: skipped");
            return;
        }
        struct Last(Mutex<String>);
        impl Sink for Last {
            fn write(&self, _: &[u8]) {}
            fn finish(&self, line: &str, _: bool) {
                *self.0.lock().unwrap() = line.to_string();
            }
        }
        let sink = Arc::new(Last(Mutex::new(String::new())));
        let dynamic: Arc<dyn Sink> = sink.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let stopper = {
            let cancel = cancel.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(600));
                cancel.store(true, Ordering::Relaxed);
            })
        };
        let started = Instant::now();
        // A background child that would outlive the shell, and a sleep.
        let ok = run(&ws, "sleep 60 & sleep 60", &cancel, &dynamic);
        stopper.join().unwrap();
        assert!(!ok);
        assert!(started.elapsed() < Duration::from_secs(10));
        let line = sink.0.lock().unwrap().clone();
        assert!(line.starts_with("Stopped after "), "{line}");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn the_watcher_reports_the_folders_that_changed() {
        let (dir, ws) = workspace("watch");
        fs::create_dir(dir.join("sub")).unwrap();
        let (tx, rx) = std::sync::mpsc::channel::<Vec<String>>();
        let watcher = Watcher::new(move |dirs| {
            let _ = tx.send(dirs);
        })
        .unwrap();
        watcher.watch(&ws, "").unwrap();
        watcher.watch(&ws, "sub").unwrap();
        // Twice is once.
        watcher.watch(&ws, "sub").unwrap();
        fs::write(dir.join("sub/new.txt"), "x").unwrap();
        fs::write(dir.join("top.txt"), "x").unwrap();
        let mut seen = HashSet::new();
        while let Ok(dirs) = rx.recv_timeout(Duration::from_secs(3)) {
            seen.extend(dirs);
            if seen.len() >= 2 {
                break;
            }
        }
        assert!(seen.contains("") && seen.contains("sub"), "{seen:?}");
        // Not watched any more: silent.
        watcher.unwatch("sub");
        fs::write(dir.join("sub/other.txt"), "x").unwrap();
        assert!(rx.recv_timeout(Duration::from_millis(600)).is_err());
        // Outside the workspace: refused.
        assert!(watcher.watch(&ws, "..").is_err());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn the_target_of_a_tool_call_is_checked_like_a_path() {
        let (dir, ws) = workspace("target");
        fs::create_dir(dir.join("src")).unwrap();
        let edits = Edits::default();
        edits.set_unsaved("src/a.rs", true);
        let conflict = |name: &str, path: &str| {
            let args = format!("{{\"path\":{}}}", serde_json::to_string(path).unwrap());
            tools::target(&ws, name, &args).is_some_and(|p| edits.is_unsaved(&p))
        };
        // The same file by any spelling is the file the user is editing.
        assert!(conflict("write_file", "src/a.rs"));
        assert!(conflict("edit_file", "./src/../src/a.rs"));
        assert!(conflict(
            "edit_file",
            &dir.join("src/a.rs").to_string_lossy()
        ));
        assert!(!conflict("write_file", "src/b.rs"));
        // A path outside has no target.
        assert!(tools::target(&ws, "write_file", "{\"path\":\"../x\"}").is_none());
        assert!(tools::target(&ws, "run_command", "{\"command\":\"ls\"}").is_none());
        let _ = fs::remove_dir_all(dir);
    }
}
