//! Files sent with a message. A text file goes in as its text, a picture as
//! a copy in Gates' own folder (sent to a model that reads images). Every
//! file is bounded, and nothing but the chosen file is read.
//!
//! Reads files: call from a worker thread.

use crate::conversation::Attachment;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

/// The largest text file, and all text in one message.
pub const MAX_TEXT: u64 = 128 * 1024;
pub const MAX_TEXT_TOTAL: usize = 256 * 1024;
/// The largest picture.
pub const MAX_IMAGE: u64 = 10 * 1024 * 1024;
/// Files in one message at most.
pub const MAX_FILES: usize = 8;

/// Pictures by their first bytes: the formats llama.cpp reads.
fn image_kind(head: &[u8]) -> Option<&'static str> {
    if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if head.starts_with(b"\xFF\xD8\xFF") {
        Some("jpeg")
    } else if head.starts_with(b"GIF8") {
        Some("gif")
    } else if head.starts_with(b"BM") {
        Some("bmp")
    } else if head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        Some("webp")
    } else {
        None
    }
}

/// Reads `path` as an attachment; a picture is copied into `store` (Gates'
/// attachments folder) under a new name.
pub fn read(path: &Path, store: &Path) -> Result<Attachment, String> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());
    let meta = fs::metadata(path).map_err(|e| format!("Can't open {name}: {e}."))?;
    if !meta.is_file() {
        return Err(format!("{name} isn't a file."));
    }
    let mut file = fs::File::open(path).map_err(|e| format!("Can't open {name}: {e}."))?;
    let mut head = [0u8; 12];
    let n = file.read(&mut head).unwrap_or(0);
    if let Some(kind) = image_kind(&head[..n]) {
        if meta.len() > MAX_IMAGE {
            return Err(format!("{name} is too big (10 MB at most)."));
        }
        fs::create_dir_all(store).map_err(|e| format!("Can't keep {name}: {e}."))?;
        let kept = store.join(format!("{}.{kind}", random_name()?));
        copy_bounded(path, &kept, MAX_IMAGE).map_err(|e| format!("Can't keep {name}: {e}."))?;
        return Ok(Attachment {
            name,
            text: None,
            image: Some(kept.to_string_lossy().into_owned()),
        });
    }
    if meta.len() > MAX_TEXT {
        return Err(format!(
            "{name} is too big to send as text (128 KB at most)."
        ));
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .and_then(|f| f.take(MAX_TEXT).read_to_end(&mut bytes))
        .map_err(|e| format!("Can't read {name}: {e}."))?;
    if bytes.contains(&0) {
        return Err(format!("{name} isn't a text file or a picture."));
    }
    let text = String::from_utf8(bytes).map_err(|_| format!("{name} isn't UTF-8 text."))?;
    Ok(Attachment {
        name,
        text: Some(text),
        image: None,
    })
}

/// Copies at most `max` bytes, into a new file only.
fn copy_bounded(from: &Path, to: &Path, max: u64) -> io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut out = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(to)?;
    io::copy(&mut fs::File::open(from)?.take(max), &mut out)?;
    Ok(())
}

fn random_name() -> Result<String, String> {
    let mut bytes = [0u8; 12];
    fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .map_err(|e| format!("No random name: {e}."))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// The message as the model reads it: your text, then each text file in a
/// `<file>` block. The total is bounded; past it, a file is left out with
/// a note.
pub fn text_for_model(text: &str, attachments: &[Attachment]) -> String {
    let mut out = text.to_string();
    let mut used = 0;
    for a in attachments {
        let Some(body) = &a.text else { continue };
        if used + body.len() > MAX_TEXT_TOTAL {
            out.push_str(&format!(
                "\n\n(The file {} was left out: too much text.)",
                a.name
            ));
            continue;
        }
        used += body.len();
        let name = a.name.replace(['"', '<', '>'], "_");
        out.push_str(&format!("\n\n<file name=\"{name}\">\n{body}\n</file>"));
    }
    out
}

/// The kept pictures of `attachments`, as data URLs for the model; a
/// picture that can't be read is left out.
pub fn image_urls(attachments: &[Attachment], store: &Path) -> Vec<String> {
    attachments
        .iter()
        .filter_map(|a| a.image.as_deref())
        .map(PathBuf::from)
        // Only Gates' own copies: a conversation file can't point elsewhere.
        .filter(|p| p.parent() == Some(store))
        .filter_map(|p| {
            let mut bytes = Vec::new();
            fs::File::open(&p)
                .and_then(|f| f.take(MAX_IMAGE).read_to_end(&mut bytes))
                .ok()?;
            let kind = image_kind(&bytes)?;
            Some(format!("data:image/{kind};base64,{}", base64(&bytes)))
        })
        .collect()
}

/// Standard base64, with padding.
pub fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(ABC[(n >> 18) as usize & 63] as char);
        out.push(ABC[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ABC[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ABC[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("gates-attach-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn base64_as_everyone_writes_it() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn text_and_pictures() {
        let d = dir("read");
        let store = d.join("store");
        fs::write(d.join("notes.md"), "# Notes\nhello").unwrap();
        let a = read(&d.join("notes.md"), &store).unwrap();
        assert_eq!(a.text.as_deref(), Some("# Notes\nhello"));
        let png = b"\x89PNG\r\n\x1a\n rest of a picture";
        fs::write(d.join("pic.png"), png).unwrap();
        let p = read(&d.join("pic.png"), &store).unwrap();
        let kept = PathBuf::from(p.image.clone().unwrap());
        assert_eq!(kept.parent(), Some(store.as_path()));
        assert_eq!(fs::read(&kept).unwrap(), png);
        let urls = image_urls(&[p.clone()], &store);
        assert!(urls[0].starts_with("data:image/png;base64,iVBORw0KGgo"));
        // A picture outside Gates' folder is never read.
        let elsewhere = Attachment {
            image: Some(d.join("pic.png").to_string_lossy().into_owned()),
            ..p
        };
        assert!(image_urls(&[elsewhere], &store).is_empty());
        // Binary that isn't a picture, and too much text, are refused.
        fs::write(d.join("blob.bin"), b"\x00\x01\x02").unwrap();
        assert!(read(&d.join("blob.bin"), &store).is_err());
        fs::write(d.join("big.txt"), "x".repeat(MAX_TEXT as usize + 1)).unwrap();
        assert!(read(&d.join("big.txt"), &store).is_err());
        assert!(read(&d, &store).is_err());
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn what_the_model_reads() {
        let a = Attachment {
            name: "a\"b.rs".into(),
            text: Some("fn main() {}".into()),
            image: None,
        };
        let t = text_for_model("Review this", &[a]);
        assert_eq!(
            t,
            "Review this\n\n<file name=\"a_b.rs\">\nfn main() {}\n</file>"
        );
        let big = Attachment {
            name: "big".into(),
            text: Some("x".repeat(MAX_TEXT_TOTAL)),
            image: None,
        };
        let t = text_for_model("", &[big.clone(), big]);
        assert!(t.contains("(The file big was left out: too much text.)"));
    }
}
