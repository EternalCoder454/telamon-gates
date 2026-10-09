//! Models from Hugging Face: searching its public API for GGUF repositories,
//! listing a repository's model files, and downloading one into the models
//! folder.
//!
//! Everything the API says is untrusted. Only https, only plain `.gguf` file
//! names (no folders, no split files), sizes capped by what the API listed,
//! and every download checked against the sha256 Hugging Face gives for it.
//! A download goes to a hidden `.part` file and is renamed only once
//! checked; an interrupted one resumes. Beside it sits a small `.part.json`
//! with the repository, so the Models page can resume a leftover. A download
//! checks the free space first, and a disk that fills up mid-way keeps the
//! part and says so.
//!
//! Every function blocks on the network: call them from a worker thread.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

pub const API: &str = "https://huggingface.co";

/// A repository in the search results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    /// `owner/name`.
    pub id: String,
    pub downloads: u64,
}

/// A model file in a repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelFile {
    pub name: String,
    pub size: u64,
    /// Lowercase hex, from the file's LFS pointer.
    pub sha256: String,
}

pub(crate) fn https_agent() -> ureq::Agent {
    use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
    ureq::Agent::config_builder()
        .https_only(true)
        .tls_config(
            TlsConfig::builder()
                .provider(TlsProvider::NativeTls)
                .root_certs(RootCerts::PlatformVerifier)
                .build(),
        )
        .timeout_connect(Some(Duration::from_secs(15)))
        .http_status_as_error(false)
        .max_redirects(5)
        .user_agent("telamon-gates")
        .build()
        .into()
}

fn get_json(url: &str) -> Result<Value, String> {
    let response = https_agent()
        .get(url)
        .call()
        .map_err(|e| format!("Couldn't reach Hugging Face: {e}."))?;
    let status = response.status().as_u16();
    if status != 200 {
        return Err(format!("Hugging Face answered {status}."));
    }
    let text = response
        .into_body()
        .with_config()
        .limit(8 * 1024 * 1024)
        .read_to_string()
        .map_err(|e| format!("Couldn't read Hugging Face's answer: {e}."))?;
    serde_json::from_str(&text).map_err(|e| format!("Hugging Face's answer wasn't JSON: {e}."))
}

/// A search term as a URL query value.
fn encode(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// GGUF repositories matching `query`, most downloaded first.
pub fn search(query: &str) -> Result<Vec<Repo>, String> {
    let url = format!(
        "{API}/api/models?search={}&filter=gguf&sort=downloads&direction=-1&limit=30",
        encode(query.trim())
    );
    Ok(parse_search(&get_json(&url)?))
}

pub fn parse_search(json: &Value) -> Vec<Repo> {
    json.as_array()
        .map(|repos| {
            repos
                .iter()
                .filter_map(|r| {
                    let id = r.get("id")?.as_str()?;
                    valid_repo(id).then(|| Repo {
                        id: id.to_string(),
                        downloads: r.get("downloads").and_then(Value::as_u64).unwrap_or(0),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The single-file GGUF models at the top of `repo`, by name.
pub fn files(repo: &str) -> Result<Vec<ModelFile>, String> {
    if !valid_repo(repo) {
        return Err("Not a repository name.".into());
    }
    Ok(parse_files(&get_json(&format!(
        "{API}/api/models/{repo}/tree/main"
    ))?))
}

pub fn parse_files(json: &Value) -> Vec<ModelFile> {
    let mut files: Vec<ModelFile> = json
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|e| {
                    if e.get("type")?.as_str()? != "file" {
                        return None;
                    }
                    let name = e.get("path")?.as_str()?;
                    let sha256 = e.pointer("/lfs/oid")?.as_str()?.to_ascii_lowercase();
                    let size = e.get("size")?.as_u64()?;
                    (valid_file(name) && is_hex64(&sha256) && size > 0).then(|| ModelFile {
                        name: name.to_string(),
                        size,
                        sha256,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    files.sort_by_key(|f| f.name.to_lowercase());
    files
}

/// `owner/name` of letters, digits, `-`, `_` and `.`, no `..`.
pub fn valid_repo(id: &str) -> bool {
    let mut parts = id.split('/');
    let (Some(owner), Some(name), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    [owner, name].iter().all(|p| {
        !p.is_empty()
            && p.len() <= 96
            && *p != "."
            && *p != ".."
            && p.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    })
}

/// A plain `.gguf` file name at the repository's top: no folder, not hidden,
/// not one part of a split model ("-00001-of-00003").
pub fn valid_file(name: &str) -> bool {
    name.ends_with(".gguf")
        && name.len() <= 200
        && !name.starts_with('.')
        && !name.contains("-of-")
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.+".contains(&b))
}

fn is_hex64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Where a download in progress is kept, next to where it will go.
pub fn part_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!(".{name}.part"))
}

/// Partial downloads untouched this long are deleted.
pub const STALE_AFTER: Duration = Duration::from_secs(30 * 24 * 3600);

const GIB: u64 = 1 << 30;

/// Where the repository of a download in progress is kept.
pub fn sidecar_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!(".{name}.part.json"))
}

#[derive(Serialize, Deserialize)]
struct Sidecar {
    repo: String,
    #[serde(default)]
    size: u64,
}

/// Notes which repository `file` comes from, next to its `.part`.
fn write_sidecar(dir: &Path, repo: &str, file: &ModelFile) -> io::Result<()> {
    let json = serde_json::to_vec(&Sidecar {
        repo: repo.to_string(),
        size: file.size,
    })
    .map_err(io::Error::other)?;
    let path = sidecar_path(dir, &file.name);
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json)?;
    fs::rename(&tmp, &path)
}

/// The repository and size noted for `name`'s part: `None` when there is no
/// note, or it isn't one of ours (it is a file on disk, so untrusted).
fn read_sidecar(dir: &Path, name: &str) -> Option<(String, u64)> {
    let mut text = Vec::new();
    File::open(sidecar_path(dir, name))
        .ok()?
        .take(4096)
        .read_to_end(&mut text)
        .ok()?;
    let note: Sidecar = serde_json::from_slice(&text).ok()?;
    valid_repo(&note.repo).then_some((note.repo, note.size))
}

/// A size as the Models page says it: "16.5 GiB", "429 MiB".
pub fn format_size(bytes: u64) -> String {
    if bytes >= GIB {
        format!("{:.1} GiB", bytes as f64 / GIB as f64)
    } else {
        format!("{:.0} MiB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// Bytes still to fetch for a file of `size` when `have` are in its part
/// (a part longer than the file is thrown away, so everything is needed).
pub fn remaining(size: u64, have: u64) -> u64 {
    if have > size { size } else { size - have }
}

/// Room kept free beyond what a download needs: 5%, but at least 1 GiB.
pub fn space_margin(needed: u64) -> u64 {
    (needed / 20).max(GIB)
}

/// The free space on the disk `dir` is (or would be) on, for a normal user.
/// A folder not made yet counts as its nearest existing parent.
pub fn free_space(dir: &Path) -> io::Result<u64> {
    use std::os::unix::ffi::OsStrExt;
    let mut probe = dir;
    while !probe.exists() {
        match probe.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => probe = parent,
            _ => {
                probe = Path::new(".");
                break;
            }
        }
    }
    let path = std::ffi::CString::new(probe.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "a path with a NUL"))?;
    // SAFETY: `path` is a valid C string, and `stat` is plain data that
    // statvfs fills in.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(path.as_ptr(), &mut stat) } != 0 {
        return Err(io::Error::last_os_error());
    }
    #[allow(clippy::unnecessary_cast)]
    Ok((stat.f_bavail as u64).saturating_mul(stat.f_frsize as u64))
}

/// The message for a download that needs `needed` more bytes where `free` is
/// left.
pub fn not_enough_space(needed: u64, free: u64) -> String {
    if free < needed {
        format!(
            "Not enough space: this model needs {} and the models folder's disk has {} free.",
            format_size(needed),
            format_size(free)
        )
    } else {
        format!(
            "Not enough space: this model needs {} plus {} to spare, and the models folder's disk has {} free.",
            format_size(needed),
            format_size(space_margin(needed)),
            format_size(free)
        )
    }
}

/// Whether `needed` more bytes fit in `free` with the margin to spare.
pub fn check_space(needed: u64, free: u64) -> Result<(), String> {
    if free >= needed.saturating_add(space_margin(needed)) {
        Ok(())
    } else {
        Err(not_enough_space(needed, free))
    }
}

/// A write that failed because the disk (or the user's quota) is full.
fn is_full(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::StorageFull
        || matches!(e.raw_os_error(), Some(libc::ENOSPC | libc::EDQUOT))
}

/// What to tell the user about a failed write to `part` (of a file of
/// `size`) in the folder `dir`. `free` is what the disk has now; `None` asks
/// the disk.
fn write_failed(e: &io::Error, dir: &Path, part: &Path, size: u64, free: Option<u64>) -> String {
    if !is_full(e) {
        return format!("Couldn't write {}: {e}.", part.display());
    }
    let have = fs::metadata(part).map(|m| m.len()).unwrap_or(0);
    let needed = remaining(size, have);
    let free = free.or_else(|| free_space(dir).ok()).unwrap_or(0);
    // The disk said it is full: whatever it reports now, say it plainly.
    let message = if free < needed {
        not_enough_space(needed, free)
    } else {
        format!(
            "Not enough space: the disk is full and this model needs {} more.",
            format_size(needed)
        )
    };
    format!("{message} The part already downloaded is kept, so it resumes once there is room.")
}

/// Downloads `file` of `repo` into `dir`, telling `progress` the bytes so
/// far (of `file.size`). Resumes a `.part` left by an earlier try. Stops,
/// keeping the part, when `cancel` turns true. Refuses, before fetching
/// anything, when the rest of the file wouldn't fit in the folder's disk.
pub fn download(
    repo: &str,
    file: &ModelFile,
    dir: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64),
) -> Result<PathBuf, String> {
    if !valid_repo(repo) || !valid_file(&file.name) || !is_hex64(&file.sha256) {
        return Err("Not a model file Gates can download.".into());
    }
    fs::create_dir_all(dir).map_err(|e| format!("Couldn't make {}: {e}.", dir.display()))?;
    let part = part_path(dir, &file.name);
    let have = fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    // A disk that can't be asked is not a reason to refuse.
    if let Ok(free) = free_space(dir) {
        check_space(remaining(file.size, have), free)?;
    }
    let _ = write_sidecar(dir, repo, file);
    let url = format!("{API}/{repo}/resolve/main/{}", file.name);
    let result = fetch(&https_agent(), &url, file, dir, cancel, progress);
    // Saved, or removed for good: nothing left to resume.
    if !part.exists() {
        let _ = fs::remove_file(sidecar_path(dir, &file.name));
    }
    result
}

/// Why the copy loop in `pump` stopped early.
enum Stop {
    Cancelled,
    Bigger,
    Read(io::Error),
    Write(io::Error),
}

/// Copies `reader` to `out`, counting into `have`, up to `size` bytes in all.
fn pump(
    reader: &mut dyn Read,
    out: &mut dyn Write,
    have: &mut u64,
    size: u64,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64),
) -> Result<(), Stop> {
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(Stop::Cancelled);
        }
        let n = reader.read(&mut buf).map_err(Stop::Read)?;
        if n == 0 {
            return Ok(());
        }
        *have += n as u64;
        if *have > size {
            return Err(Stop::Bigger);
        }
        out.write_all(&buf[..n]).map_err(Stop::Write)?;
        progress(*have);
    }
}

/// `download`, from any URL with any agent (tests serve plain http).
pub fn fetch(
    agent: &ureq::Agent,
    url: &str,
    file: &ModelFile,
    dir: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64),
) -> Result<PathBuf, String> {
    fs::create_dir_all(dir).map_err(|e| format!("Couldn't make {}: {e}.", dir.display()))?;
    let target = dir.join(&file.name);
    let part = part_path(dir, &file.name);
    let mut have = fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    if have > file.size {
        let _ = fs::remove_file(&part);
        have = 0;
    }
    let mut request = agent.get(url);
    if have > 0 {
        request = request.header("Range", format!("bytes={have}-"));
    }
    let response = request
        .call()
        .map_err(|e| format!("Couldn't reach Hugging Face: {e}."))?;
    let status = response.status().as_u16();
    // 206: the rest; 200: the whole file again (no resume).
    let append = match status {
        206 if have > 0 => true,
        200 => false,
        416 if have == file.size => true,
        _ => return Err(format!("Hugging Face answered {status} for {}.", file.name)),
    };
    if !append {
        have = 0;
    }
    let mut out = OpenOptions::new()
        .create(true)
        .write(true)
        .append(append)
        .truncate(!append)
        .open(&part)
        .map_err(|e| write_failed(&e, dir, &part, file.size, None))?;
    progress(have);
    if status != 416 {
        let mut body = response.into_body();
        let mut reader = body
            .with_config()
            .limit(file.size.saturating_sub(have) + 1)
            .reader();
        match pump(
            &mut reader,
            &mut out,
            &mut have,
            file.size,
            cancel,
            progress,
        ) {
            Ok(()) => {}
            Err(Stop::Cancelled) => return Err("Cancelled.".into()),
            Err(Stop::Bigger) => {
                let _ = fs::remove_file(&part);
                return Err(format!("{} is bigger than Hugging Face said.", file.name));
            }
            Err(Stop::Read(e)) => return Err(format!("The download was cut off: {e}.")),
            Err(Stop::Write(e)) => return Err(write_failed(&e, dir, &part, file.size, None)),
        }
    }
    // The last blocks reach the disk here, so a full disk can show up now.
    out.sync_all()
        .map_err(|e| write_failed(&e, dir, &part, file.size, None))?;
    drop(out);
    if have != file.size {
        return Err(format!(
            "The download of {} stopped at {have} of {} bytes; try again to resume it.",
            file.name, file.size
        ));
    }
    let sum = sha256_file(&part).map_err(|e| format!("Couldn't check {}: {e}.", part.display()))?;
    if sum != file.sha256 {
        let _ = fs::remove_file(&part);
        return Err(format!(
            "{} didn't match its checksum and was removed.",
            file.name
        ));
    }
    fs::rename(&part, &target).map_err(|e| format!("Couldn't save {}: {e}.", target.display()))?;
    Ok(target)
}

/// A download left behind in the models folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Partial {
    /// The model file it would become, "name.gguf".
    pub name: String,
    /// Bytes downloaded so far.
    pub bytes: u64,
    /// The whole file's size when known (0 when not).
    pub total: u64,
    /// The repository it comes from, when noted: it can be resumed.
    pub repo: Option<String>,
    pub modified: SystemTime,
}

/// The `name.gguf` of a `.name.gguf.part` file name.
fn part_name(file_name: &str) -> Option<&str> {
    let name = file_name.strip_prefix('.')?.strip_suffix(".part")?;
    valid_file(name).then_some(name)
}

/// The partial downloads in `dir`, by name. Only plain files count.
pub fn partials(dir: &Path) -> Vec<Partial> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<Partial> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let file_name = entry.file_name();
            let name = part_name(file_name.to_str()?)?;
            if !entry.file_type().ok()?.is_file() {
                return None;
            }
            let meta = entry.metadata().ok()?;
            let note = read_sidecar(dir, name);
            Some(Partial {
                name: name.to_string(),
                bytes: meta.len(),
                total: note.as_ref().map_or(0, |n| n.1),
                repo: note.map(|n| n.0),
                modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            })
        })
        .collect();
    found.sort_by_key(|p| p.name.to_lowercase());
    found
}

/// Deletes the partial download of `name` and its note.
pub fn remove_partial(dir: &Path, name: &str) -> io::Result<()> {
    if !valid_file(name) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a model file name",
        ));
    }
    let gone = |r: io::Result<()>| match r {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    };
    gone(fs::remove_file(part_path(dir, name)))?;
    gone(fs::remove_file(sidecar_path(dir, name)))
}

/// Deletes the partial downloads last written more than `max_age` before
/// `now` (not `keep`, which is being downloaded), and any note whose part is
/// gone. Returns the names of the parts it removed.
pub fn remove_stale(dir: &Path, max_age: Duration, now: SystemTime, keep: &str) -> Vec<String> {
    let mut removed = Vec::new();
    for part in partials(dir) {
        let age = now.duration_since(part.modified).unwrap_or_default();
        if age > max_age && part.name != keep && remove_partial(dir, &part.name).is_ok() {
            removed.push(part.name);
        }
    }
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.filter_map(Result::ok) {
            let file_name = entry.file_name();
            let note = file_name
                .to_str()
                .and_then(|n| n.strip_prefix('.')?.strip_suffix(".part.json"))
                .filter(|n| valid_file(n));
            if let Some(name) = note
                && !part_path(dir, name).exists()
            {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
    removed
}

fn sha256_file(path: &Path) -> io::Result<String> {
    let mut f = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;
    use std::net::TcpListener;

    #[test]
    fn search_results() {
        let repos = parse_search(&json!([
            {"id": "bartowski/Qwen-GGUF", "downloads": 1200},
            {"id": "../evil", "downloads": 5},
            {"id": "a/b/c"},
            {"nope": 1}
        ]));
        assert_eq!(
            repos,
            vec![Repo {
                id: "bartowski/Qwen-GGUF".into(),
                downloads: 1200
            }]
        );
    }

    #[test]
    fn repository_files() {
        let sha = "a".repeat(64);
        let files = parse_files(&json!([
            {"type": "file", "path": "M-Q8_0.gguf", "size": 10, "lfs": {"oid": sha}},
            {"type": "file", "path": "M-Q4_K_M.gguf", "size": 5, "lfs": {"oid": sha.to_uppercase()}},
            {"type": "file", "path": "M-Q2_K-00001-of-00002.gguf", "size": 5, "lfs": {"oid": sha}},
            {"type": "file", "path": "sub/M.gguf", "size": 5, "lfs": {"oid": sha}},
            {"type": "file", "path": "README.md", "size": 5},
            {"type": "file", "path": "x.gguf", "size": 5, "lfs": {"oid": "short"}},
            {"type": "directory", "path": "dir.gguf"}
        ]));
        let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["M-Q4_K_M.gguf", "M-Q8_0.gguf"]);
        assert_eq!(files[0].sha256, sha);
    }

    #[test]
    fn names_that_cannot_leave_the_folder() {
        assert!(valid_repo("unsloth/gemma-4-31b-it-GGUF"));
        assert!(!valid_repo("../x"));
        assert!(!valid_repo("a/.."));
        assert!(!valid_repo("a"));
        assert!(valid_file("Model.Q4_K_M.gguf"));
        assert!(!valid_file("../x.gguf"));
        assert!(!valid_file(".hidden.gguf"));
        assert!(!valid_file("a b.gguf"));
        assert!(!valid_file("x.bin"));
        assert_eq!(encode("qwen 3.5/9b"), "qwen%203.5%2F9b");
    }

    /// Serves `body` (or its tail, for a Range request) once over plain http.
    fn serve(body: &'static [u8]) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/m.gguf", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0u8; 2048];
            let n = s.read(&mut buf).unwrap();
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let from = req
                .lines()
                .find_map(|l| {
                    l.strip_prefix("Range: bytes=")
                        .or_else(|| l.strip_prefix("range: bytes="))
                })
                .and_then(|r| r.trim_end_matches('-').parse::<usize>().ok());
            let (status, part) = match from {
                Some(f) => ("206 Partial Content", &body[f..]),
                None => ("200 OK", body),
            };
            let _ = write!(
                s,
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                part.len()
            );
            let _ = s.write_all(part);
        });
        url
    }

    fn plain_agent() -> ureq::Agent {
        ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build()
            .into()
    }

    fn sha(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    #[test]
    fn downloads_resumes_and_checks() {
        const BODY: &[u8] = b"GGUF0123456789abcdefghijklmnopqrstuvwxyz";
        let dir = std::env::temp_dir().join(format!("gates-hub-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let file = ModelFile {
            name: "m.gguf".into(),
            size: BODY.len() as u64,
            sha256: sha(BODY),
        };
        // An earlier try left the first 10 bytes.
        fs::create_dir_all(&dir).unwrap();
        fs::write(part_path(&dir, "m.gguf"), &BODY[..10]).unwrap();
        let mut seen = Vec::new();
        let saved = fetch(
            &plain_agent(),
            &serve(BODY),
            &file,
            &dir,
            &AtomicBool::new(false),
            &mut |n| seen.push(n),
        )
        .unwrap();
        assert_eq!(fs::read(&saved).unwrap(), BODY);
        assert_eq!(seen.first(), Some(&10), "resumed from the part");
        assert_eq!(seen.last(), Some(&(BODY.len() as u64)));
        assert!(!part_path(&dir, "m.gguf").exists());

        // A wrong checksum: nothing kept.
        let _ = fs::remove_file(&saved);
        let bad = ModelFile {
            sha256: "0".repeat(64),
            ..file.clone()
        };
        let err = fetch(
            &plain_agent(),
            &serve(BODY),
            &bad,
            &dir,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap_err();
        assert!(err.contains("checksum"), "{err}");
        assert!(!dir.join("m.gguf").exists() && !part_path(&dir, "m.gguf").exists());

        // Bigger than listed: refused.
        let small = ModelFile {
            size: 5,
            ..file.clone()
        };
        let err = fetch(
            &plain_agent(),
            &serve(BODY),
            &small,
            &dir,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap_err();
        assert!(err.contains("bigger") || err.contains("cut off"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gates-hub-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    const MIB: u64 = 1 << 20;

    #[test]
    fn space_needed_counts_what_resumes() {
        assert_eq!(remaining(100, 0), 100);
        assert_eq!(remaining(100, 40), 60);
        assert_eq!(remaining(100, 100), 0);
        // A part longer than the file is thrown away: everything is needed.
        assert_eq!(remaining(100, 101), 100);
    }

    #[test]
    fn space_margin_is_one_gib_or_five_percent() {
        assert_eq!(space_margin(0), GIB);
        assert_eq!(space_margin(10 * GIB), GIB);
        assert_eq!(space_margin(20 * GIB), GIB);
        assert_eq!(space_margin(40 * GIB), 2 * GIB);
    }

    #[test]
    fn space_check_keeps_a_margin() {
        let need = 10 * GIB;
        assert!(check_space(need, need + GIB).is_ok());
        assert!(check_space(need, 100 * GIB).is_ok());
        // Fits, but not with the margin.
        let err = check_space(need, need + GIB - 1).unwrap_err();
        assert!(err.contains("plus 1.0 GiB to spare"), "{err}");
        // A resumed download needs only the rest.
        assert!(check_space(remaining(need, 9 * GIB), 2 * GIB + 1).is_ok());
        assert!(check_space(0, GIB).is_ok());
        assert!(check_space(0, GIB - 1).is_err());
    }

    #[test]
    fn space_message_names_both_sizes() {
        let need = 17_665_334_432; // 16.5 GiB
        let free = 9 * GIB + GIB / 5; // 9.2 GiB
        assert_eq!(
            check_space(need, free).unwrap_err(),
            "Not enough space: this model needs 16.5 GiB and the models folder's disk has 9.2 GiB free."
        );
        assert_eq!(format_size(429 * MIB), "429 MiB");
        assert_eq!(format_size(0), "0 MiB");
        assert_eq!(format_size(120 * GIB), "120.0 GiB");
    }

    #[test]
    fn free_space_of_a_folder_not_made_yet() {
        let dir = temp("free");
        let here = free_space(&dir).unwrap();
        let below = free_space(&dir.join("not/made/yet")).unwrap();
        assert!(here > 0);
        // The same disk, give or take what other processes wrote.
        assert!(below.abs_diff(here) < GIB, "{below} vs {here}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_download_too_big_for_the_disk_is_refused_before_fetching() {
        let dir = temp("toobig");
        let file = ModelFile {
            name: "huge.gguf".into(),
            size: u64::MAX / 4,
            sha256: "a".repeat(64),
        };
        let err = download("a/b", &file, &dir, &AtomicBool::new(false), &mut |_| {}).unwrap_err();
        assert!(err.starts_with("Not enough space"), "{err}");
        // Nothing was started: no part, no note.
        assert!(partials(&dir).is_empty());
        assert!(!sidecar_path(&dir, "huge.gguf").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    /// A writer that takes `room` bytes, then fails like a full disk.
    struct Filling {
        room: usize,
        error: fn() -> io::Error,
        got: Vec<u8>,
    }

    impl Write for Filling {
        fn write(&mut self, data: &[u8]) -> io::Result<usize> {
            if self.room == 0 {
                return Err((self.error)());
            }
            let n = data.len().min(self.room);
            self.room -= n;
            self.got.extend_from_slice(&data[..n]);
            Ok(n)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn enospc() -> io::Error {
        io::Error::from_raw_os_error(libc::ENOSPC)
    }

    #[test]
    fn a_full_disk_keeps_the_part_and_says_so() {
        let dir = temp("enospc");
        let part = part_path(&dir, "m.gguf");
        let data = vec![7u8; 1000];
        let mut out = Filling {
            room: 600,
            error: enospc,
            got: Vec::new(),
        };
        let mut have = 0;
        let mut seen = 0;
        let stop = pump(
            &mut &data[..],
            &mut out,
            &mut have,
            1000,
            &AtomicBool::new(false),
            &mut |n| seen = n,
        );
        let Err(Stop::Write(e)) = stop else {
            panic!("expected a failed write");
        };
        assert!(is_full(&e));
        assert_eq!(out.got.len(), 600, "what fit was written");
        // The part on disk is what fit.
        fs::write(&part, &out.got).unwrap();
        let message = write_failed(&e, &dir, &part, 1000, Some(0));
        assert_eq!(
            message,
            "Not enough space: this model needs 0 MiB and the models folder's disk has 0 MiB free. The part already downloaded is kept, so it resumes once there is room."
        );
        assert!(part.exists(), "the part stays for resuming");
        // With real sizes: 16.5 GiB file, 600 bytes downloaded, 205 MiB left on the disk.
        let message = write_failed(&e, &dir, &part, 17_665_334_432 + 600, Some(GIB / 5));
        assert!(
            message.starts_with(
                "Not enough space: this model needs 16.5 GiB and the models folder's disk has 205 MiB free."
            ),
            "{message}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn full_disk_errors_in_every_form_are_recognised() {
        assert!(is_full(&enospc()));
        assert!(is_full(&io::Error::from_raw_os_error(libc::EDQUOT)));
        assert!(is_full(&io::Error::from(io::ErrorKind::StorageFull)));
        assert!(!is_full(&io::Error::from_raw_os_error(libc::EACCES)));
        let message = write_failed(
            &io::Error::from_raw_os_error(libc::EACCES),
            Path::new("/tmp"),
            Path::new("/tmp/.m.gguf.part"),
            10,
            None,
        );
        assert!(
            message.starts_with("Couldn't write /tmp/.m.gguf.part"),
            "{message}"
        );
        assert!(!message.contains("space"));
    }

    #[test]
    fn pump_stops_on_cancel_and_on_too_much() {
        let data = [1u8; 100];
        let mut sink = Vec::new();
        let mut have = 0;
        let cancelled = AtomicBool::new(true);
        let stop = pump(
            &mut &data[..],
            &mut sink,
            &mut have,
            100,
            &cancelled,
            &mut |_| {},
        );
        assert!(matches!(stop, Err(Stop::Cancelled)) && sink.is_empty());
        let stop = pump(
            &mut &data[..],
            &mut sink,
            &mut have,
            50,
            &AtomicBool::new(false),
            &mut |_| {},
        );
        assert!(matches!(stop, Err(Stop::Bigger)));
    }

    #[test]
    fn the_sidecar_remembers_the_repository() {
        let dir = temp("sidecar");
        let file = ModelFile {
            name: "m.gguf".into(),
            size: 5000,
            sha256: "a".repeat(64),
        };
        write_sidecar(&dir, "owner/Repo-GGUF", &file).unwrap();
        assert_eq!(
            read_sidecar(&dir, "m.gguf"),
            Some(("owner/Repo-GGUF".to_string(), 5000))
        );
        assert!(!dir.join(".m.gguf.part.json.tmp").exists());
        // Nothing noted, or a note that isn't ours or names a bad repository.
        assert_eq!(read_sidecar(&dir, "other.gguf"), None);
        for bad in [
            "not json",
            r#"{"repo":"../../etc"}"#,
            r#"{"repo":"a/b/c","size":1}"#,
            r#"{"size":1}"#,
        ] {
            fs::write(sidecar_path(&dir, "m.gguf"), bad).unwrap();
            assert_eq!(read_sidecar(&dir, "m.gguf"), None, "{bad}");
        }
        // Older notes without a size still read.
        fs::write(sidecar_path(&dir, "m.gguf"), r#"{"repo":"a/b"}"#).unwrap();
        assert_eq!(read_sidecar(&dir, "m.gguf"), Some(("a/b".to_string(), 0)));
        // A huge file is not read whole.
        fs::write(
            sidecar_path(&dir, "m.gguf"),
            " ".repeat(1 << 20) + r#"{"repo":"a/b"}"#,
        )
        .unwrap();
        assert_eq!(read_sidecar(&dir, "m.gguf"), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn leftovers_are_listed_with_their_repository() {
        let dir = temp("partials");
        let file = ModelFile {
            name: "B-Q4.gguf".into(),
            size: 900,
            sha256: "a".repeat(64),
        };
        fs::write(part_path(&dir, "B-Q4.gguf"), [0u8; 300]).unwrap();
        write_sidecar(&dir, "owner/B-GGUF", &file).unwrap();
        // One with no note (cannot be resumed), and things that aren't parts.
        fs::write(part_path(&dir, "a-Q8.gguf"), [0u8; 10]).unwrap();
        fs::write(dir.join("done.gguf"), [0u8; 10]).unwrap();
        fs::write(dir.join(".x.part"), [0u8; 10]).unwrap();
        fs::write(dir.join(".bad name.gguf.part"), [0u8; 10]).unwrap();
        fs::create_dir(part_path(&dir, "folder.gguf")).unwrap();
        std::os::unix::fs::symlink("/etc/hostname", part_path(&dir, "link.gguf")).unwrap();
        let found = partials(&dir);
        let summary: Vec<_> = found
            .iter()
            .map(|p| (p.name.as_str(), p.bytes, p.total, p.repo.as_deref()))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("a-Q8.gguf", 10, 0, None),
                ("B-Q4.gguf", 300, 900, Some("owner/B-GGUF")),
            ]
        );
        assert!(partials(&dir.join("missing")).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn deleting_a_leftover_takes_its_note_and_nothing_else() {
        let dir = temp("delete");
        let file = ModelFile {
            name: "m.gguf".into(),
            size: 5,
            sha256: "a".repeat(64),
        };
        fs::write(part_path(&dir, "m.gguf"), b"12345").unwrap();
        write_sidecar(&dir, "a/b", &file).unwrap();
        fs::write(dir.join("keep.gguf"), b"model").unwrap();
        remove_partial(&dir, "m.gguf").unwrap();
        assert!(!part_path(&dir, "m.gguf").exists());
        assert!(!sidecar_path(&dir, "m.gguf").exists());
        assert!(dir.join("keep.gguf").exists());
        // Gone already is fine; a name that is a path is not.
        remove_partial(&dir, "m.gguf").unwrap();
        assert!(remove_partial(&dir, "../keep.gguf").is_err());
        assert!(dir.join("keep.gguf").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn old_leftovers_are_cleaned_up() {
        let dir = temp("stale");
        let now = SystemTime::now();
        let age = |name: &str, days: u64| {
            fs::write(part_path(&dir, name), b"x").unwrap();
            fs::write(sidecar_path(&dir, name), r#"{"repo":"a/b"}"#).unwrap();
            File::options()
                .write(true)
                .open(part_path(&dir, name))
                .unwrap()
                .set_modified(now - Duration::from_secs(days * 24 * 3600 + 60))
                .unwrap();
        };
        age("old.gguf", 31);
        age("active.gguf", 90);
        age("recent.gguf", 29);
        age("new.gguf", 0);
        // A note whose part is gone, and a model that is just old.
        fs::write(sidecar_path(&dir, "orphan.gguf"), r#"{"repo":"a/b"}"#).unwrap();
        fs::write(dir.join("model.gguf"), b"m").unwrap();
        File::options()
            .write(true)
            .open(dir.join("model.gguf"))
            .unwrap()
            .set_modified(now - Duration::from_secs(400 * 24 * 3600))
            .unwrap();

        let removed = remove_stale(&dir, STALE_AFTER, now, "active.gguf");
        assert_eq!(removed, vec!["old.gguf".to_string()]);
        let left: Vec<_> = partials(&dir).into_iter().map(|p| p.name).collect();
        assert_eq!(left, vec!["active.gguf", "new.gguf", "recent.gguf"]);
        assert!(!sidecar_path(&dir, "old.gguf").exists());
        assert!(!sidecar_path(&dir, "orphan.gguf").exists());
        assert!(sidecar_path(&dir, "recent.gguf").exists());
        assert!(dir.join("model.gguf").exists(), "models are never aged out");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_https_to_hugging_face() {
        // The real agent refuses plain http.
        assert!(https_agent().get("http://127.0.0.1:9/x").call().is_err());
        let file = ModelFile {
            name: "../x.gguf".into(),
            size: 1,
            sha256: "a".repeat(64),
        };
        assert!(
            download(
                "a/b",
                &file,
                Path::new("/tmp"),
                &AtomicBool::new(false),
                &mut |_| {}
            )
            .is_err()
        );
    }
}
