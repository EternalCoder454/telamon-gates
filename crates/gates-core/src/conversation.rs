//! A conversation: who said what, in order. Kept as plain JSON (see
//! `store.rs`), so another app can read it.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::User => "user",
            Role::Assistant => "assistant",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub text: String,
    /// The reply stopped on an error; `text` is what came before it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub failed: bool,
}

impl Message {
    pub fn user(text: impl Into<String>) -> Message {
        Message {
            role: Role::User,
            text: text.into(),
            failed: false,
        }
    }

    pub fn assistant(text: impl Into<String>) -> Message {
        Message {
            role: Role::Assistant,
            text: text.into(),
            failed: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conversation {
    pub id: String,
    pub title: String,
    /// Milliseconds since the Unix epoch.
    pub created: i64,
    pub updated: i64,
    pub messages: Vec<Message>,
}

/// What the conversation list shows of one conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    pub id: String,
    pub title: String,
    pub updated: i64,
}

/// The longest title made from a first message, in characters.
const TITLE_CHARS: usize = 60;

impl Conversation {
    /// A new, empty conversation, titled after the first thing the user says.
    pub fn new(first_message: &str) -> Conversation {
        let now = now_ms();
        Conversation {
            id: new_id(now),
            title: title_from(first_message),
            created: now,
            updated: now,
            messages: Vec::new(),
        }
    }

    pub fn summary(&self) -> Summary {
        Summary {
            id: self.id.clone(),
            title: self.title.clone(),
            updated: self.updated,
        }
    }

    pub fn touch(&mut self) {
        self.updated = now_ms();
    }
}

/// The first line of `text`, trimmed, at most `TITLE_CHARS` characters (cut
/// at a word when it can be, with an ellipsis).
pub fn title_from(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let line: String = line.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= TITLE_CHARS {
        return line;
    }
    let cut: String = line.chars().take(TITLE_CHARS).collect();
    let cut = match cut.rfind(' ') {
        Some(at) if at > TITLE_CHARS / 2 => &cut[..at],
        _ => &cut,
    };
    format!("{}…", cut.trim_end())
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// Unique in this process and sortable by time: the time in milliseconds,
/// then a counter, both in hex.
fn new_id(now: i64) -> String {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed) & 0xffff;
    format!("{now:012x}-{n:04x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_is_the_first_nonblank_line() {
        assert_eq!(title_from("\n  hello   there \nsecond"), "hello there");
    }

    #[test]
    fn long_titles_are_cut_at_a_word() {
        let t = title_from(&"word ".repeat(40));
        assert!(t.ends_with('…'));
        assert!(t.chars().count() <= TITLE_CHARS + 1);
        assert!(!t.contains("wor…"));
    }

    #[test]
    fn ids_differ() {
        let a = Conversation::new("a");
        let b = Conversation::new("b");
        assert_ne!(a.id, b.id);
    }

    #[test]
    fn json_round_trip() {
        let mut c = Conversation::new("hi");
        c.messages.push(Message::user("hi"));
        c.messages.push(Message {
            failed: true,
            ..Message::assistant("partial")
        });
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains(r#""role":"assistant""#));
        let back: Conversation = serde_json::from_str(&json).unwrap();
        assert_eq!(back, c);
    }
}
