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
    /// What a tool gave back (Agent mode), for the call `tool_call_id`.
    Tool,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        }
    }
}

/// A tool the model asked to run (Agent mode).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    /// The server's id for the call; the result answers to it.
    pub id: String,
    pub name: String,
    /// The arguments, as the JSON text the model wrote.
    pub arguments: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub text: String,
    /// The reply stopped on an error; `text` is what came before it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub failed: bool,
    /// How fast the reply came, in tokens per second.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<f64>,
    /// The mode that wrote the reply (`modes.rs`), when one did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// SystemOne picked that mode (the conversation was in Auto).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub picked: bool,
    /// The tools a reply asked to run, in order (Agent mode).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// A tool result's call (`Role::Tool`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// A tool result in one line, for the window ("Read src/main.rs (40
    /// lines)"); `text` is what the model got. `failed` when it didn't work.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

impl Message {
    pub fn user(text: impl Into<String>) -> Message {
        Message {
            role: Role::User,
            text: text.into(),
            failed: false,
            speed: None,
            mode: None,
            picked: false,
            tool_calls: Vec::new(),
            tool_call_id: None,
            summary: None,
        }
    }

    pub fn assistant(text: impl Into<String>) -> Message {
        Message {
            role: Role::Assistant,
            text: text.into(),
            failed: false,
            speed: None,
            mode: None,
            picked: false,
            tool_calls: Vec::new(),
            tool_call_id: None,
            summary: None,
        }
    }

    /// What tool call `id` gave back.
    pub fn tool(id: impl Into<String>, text: impl Into<String>) -> Message {
        Message {
            role: Role::Tool,
            tool_call_id: Some(id.into()),
            ..Message::user(text)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Conversation {
    pub id: String,
    pub title: String,
    /// Milliseconds since the Unix epoch.
    pub created: i64,
    pub updated: i64,
    pub messages: Vec<Message>,
    /// "auto" (SystemOne picks per message) or a mode the user pinned.
    #[serde(default = "auto", skip_serializing_if = "is_auto")]
    pub mode: String,
    /// The folder Agent mode's tools work in; None until one is chosen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
}

fn auto() -> String {
    crate::modes::AUTO.to_string()
}

fn is_auto(mode: &String) -> bool {
    mode == crate::modes::AUTO
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
            mode: auto(),
            workspace: None,
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

    /// A new conversation with this one's messages up to and including
    /// message `upto`, its mode and folder: a way to try something else
    /// from there without losing what came after.
    pub fn branch(&self, upto: usize) -> Conversation {
        let now = now_ms();
        let end = (upto + 1).min(self.messages.len());
        Conversation {
            id: new_id(now),
            title: format!("{} (branch)", self.title.trim_end_matches(" (branch)")),
            created: now,
            updated: now,
            // Every tool call answered (a branch can cut between them).
            messages: crate::agent::repair(self.messages[..end].to_vec()),
            mode: self.mode.clone(),
            workspace: self.workspace.clone(),
        }
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
    fn branches() {
        let mut c = Conversation::new("Story");
        c.mode = "story".into();
        for t in ["one", "two", "three", "four"] {
            c.messages.push(Message::user(t));
        }
        let b = c.branch(1);
        assert_ne!(b.id, c.id);
        assert_eq!(b.title, "Story (branch)");
        assert_eq!(b.messages.len(), 2);
        assert_eq!(b.mode, "story");
        // A branch of a branch isn't "(branch) (branch)".
        assert_eq!(b.branch(0).title, "Story (branch)");
        assert_eq!(c.branch(99).messages.len(), 4);
    }

    #[test]
    fn json_round_trip() {
        let mut c = Conversation::new("hi");
        c.messages.push(Message::user("hi"));
        c.messages.push(Message {
            failed: true,
            ..Message::assistant("partial")
        });
        c.messages.push(Message {
            speed: Some(41.5),
            ..Message::assistant("fast")
        });
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains(r#""role":"assistant""#));
        // Auto and unset modes stay out of the file.
        assert!(!json.contains("mode") && !json.contains("picked"));
        let back: Conversation = serde_json::from_str(&json).unwrap();
        assert_eq!(back, c);
        c.mode = "story".into();
        c.messages.push(Message {
            mode: Some("story".into()),
            picked: true,
            ..Message::assistant("Once upon a time")
        });
        let back: Conversation = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn files_from_before_modes_open_in_auto() {
        let old = r#"{"id":"1-0","title":"t","created":1,"updated":1,"messages":[{"role":"user","text":"hi"}]}"#;
        let c: Conversation = serde_json::from_str(old).unwrap();
        assert_eq!(c.mode, "auto");
        assert_eq!(c.messages[0].mode, None);
    }
}
