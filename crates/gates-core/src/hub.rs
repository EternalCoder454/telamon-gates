//! Models from Hugging Face: searching its public API for GGUF repositories,
//! listing a repository's model files, and downloading one into the models
//! folder.
//!
//! Everything the API says is untrusted. Only https, only plain `.gguf` file
//! names (no folders, no split files), sizes capped by what the API listed,
//! and every download checked against the sha256 Hugging Face gives for it.
//! A download goes to a hidden `.part` file and is renamed only once
//! checked; an interrupted one resumes.
//!
//! Every function blocks on the network: call them from a worker thread.

use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

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

fn https_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .https_only(true)
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

/// Downloads `file` of `repo` into `dir`, telling `progress` the bytes so
/// far (of `file.size`). Resumes a `.part` left by an earlier try. Stops,
/// keeping the part, when `cancel` turns true.
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
    let url = format!("{API}/{repo}/resolve/main/{}", file.name);
    fetch(&https_agent(), &url, file, dir, cancel, progress)
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
        .map_err(|e| format!("Couldn't write {}: {e}.", part.display()))?;
    progress(have);
    if status != 416 {
        let mut body = response.into_body();
        let mut reader = body
            .with_config()
            .limit(file.size.saturating_sub(have) + 1)
            .reader();
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err("Cancelled.".into());
            }
            let n = reader
                .read(&mut buf)
                .map_err(|e| format!("The download was cut off: {e}."))?;
            if n == 0 {
                break;
            }
            have += n as u64;
            if have > file.size {
                let _ = fs::remove_file(&part);
                return Err(format!("{} is bigger than Hugging Face said.", file.name));
            }
            out.write_all(&buf[..n])
                .map_err(|e| format!("Couldn't write {}: {e}.", part.display()))?;
            progress(have);
        }
    }
    out.sync_all()
        .map_err(|e| format!("Couldn't write {}: {e}.", part.display()))?;
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
