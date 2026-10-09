//! Conversations on disk: one JSON file each, `<id>.json`, in
//! `$XDG_DATA_HOME/telamon-gates/conversations`. Plain files, so they can be
//! backed up, read or moved by anything.
//!
//! Every method does file I/O: call them from a worker thread, never the GUI's.

use crate::conversation::{Conversation, Summary};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Store {
    dir: PathBuf,
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

    /// Every conversation that can be read, newest first. A file that can't
    /// be read is logged and left out.
    pub fn list(&self) -> Vec<Summary> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Vec::new(),
            Err(e) => {
                log::warn!("cannot list {}: {e}", self.dir.display());
                return Vec::new();
            }
        };
        let mut list: Vec<Summary> = entries
            .filter_map(Result::ok)
            .filter_map(|e| {
                let name = e.file_name();
                let id = name.to_str()?.strip_suffix(".json")?;
                valid_id(id).then(|| id.to_string())
            })
            .filter_map(|id| match self.load(&id) {
                Ok(c) => Some(c.summary()),
                Err(e) => {
                    log::warn!("cannot read conversation {id}: {e}");
                    None
                }
            })
            .collect();
        list.sort_by(|a, b| b.updated.cmp(&a.updated).then_with(|| b.id.cmp(&a.id)));
        list
    }

    pub fn load(&self, id: &str) -> io::Result<Conversation> {
        let text = fs::read_to_string(self.path(id)?)?;
        let c: Conversation = serde_json::from_str(&text).map_err(io::Error::other)?;
        if c.id != id {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the file's id is not its name",
            ));
        }
        Ok(c)
    }

    /// Writes the conversation whole: a temporary file, then a rename, so a
    /// crash never leaves half a file.
    pub fn save(&self, c: &Conversation) -> io::Result<()> {
        use std::os::unix::fs::OpenOptionsExt;
        let path = self.path(&c.id)?;
        private_dir(&self.dir)?;
        let tmp = self.dir.join(format!(".{}.json.tmp", c.id));
        let json = serde_json::to_vec_pretty(c).map_err(io::Error::other)?;
        {
            // Yours alone: conversations can hold anything.
            let mut f = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp)?;
            f.write_all(&json)?;
            f.sync_all()?;
        }
        fs::rename(&tmp, &path)
    }

    pub fn delete(&self, id: &str) -> io::Result<()> {
        match fs::remove_file(self.path(id)?) {
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

    fn temp_store(name: &str) -> Store {
        let dir =
            std::env::temp_dir().join(format!("gates-core-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        Store::at(dir)
    }

    #[test]
    fn files_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("gates-private-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let store = Store::at(dir.join("conversations"));
        let c = Conversation::new("secret");
        store.save(&c).unwrap();
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(store.dir()), 0o700);
        assert_eq!(mode(&store.dir().join(format!("{}.json", c.id))), 0o600);
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
    fn unreadable_files_are_skipped() {
        let store = temp_store("bad");
        fs::create_dir_all(store.dir()).unwrap();
        fs::write(store.dir().join("abc.json"), "not json").unwrap();
        fs::write(store.dir().join("notes.txt"), "{}").unwrap();
        assert!(store.list().is_empty());
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
}
