//! A conversation as a file to keep or share: Markdown to read, or the same
//! JSON Gates keeps (`store.rs`), to open elsewhere.

use crate::conversation::{Conversation, Role};

/// The conversation as Markdown: the title, then each message under who
/// said it. A reply's own Markdown is kept as written; tool steps are one
/// quoted line each.
pub fn markdown(c: &Conversation) -> String {
    let mut out = format!("# {}\n", c.title);
    for m in &c.messages {
        match m.role {
            Role::User => {
                out.push_str("\n## You\n\n");
                out.push_str(m.text.trim_end());
                out.push('\n');
            }
            Role::Assistant => {
                if m.text.trim().is_empty() {
                    continue;
                }
                match m.mode.as_deref() {
                    Some(mode) if mode != "chat" => {
                        let mode = mode[..1].to_uppercase() + &mode[1..];
                        out.push_str(&format!("\n## Telamon Gates ({mode})\n\n"));
                    }
                    _ => out.push_str("\n## Telamon Gates\n\n"),
                }
                out.push_str(m.text.trim_end());
                out.push('\n');
                if m.failed {
                    out.push_str("\n*The reply stopped because of an error.*\n");
                }
            }
            Role::Tool => {
                let what = m.summary.as_deref().unwrap_or("A tool ran");
                let mark = if m.failed { "✗" } else { "✓" };
                out.push_str(&format!("\n> {mark} {what}\n"));
            }
        }
    }
    out
}

/// The conversation as Gates keeps it.
pub fn json(c: &Conversation) -> String {
    serde_json::to_string_pretty(c).unwrap_or_default() + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::Message;

    #[test]
    fn as_markdown() {
        let mut c = Conversation::new("Lisbon");
        c.messages.push(Message::user("Plan a weekend"));
        c.messages.push(Message {
            mode: Some("story".into()),
            ..Message::assistant("**Day one**")
        });
        c.messages.push(Message {
            summary: Some("Read a.txt".into()),
            ..Message::tool("1", "secret file text")
        });
        let md = markdown(&c);
        assert!(md.starts_with("# Lisbon\n"));
        assert!(md.contains("## You\n\nPlan a weekend\n"));
        assert!(md.contains("## Telamon Gates (Story)\n\n**Day one**\n"));
        assert!(md.contains("> ✓ Read a.txt"));
        // A tool's output stays out: it can be long, or private.
        assert!(!md.contains("secret file text"));
        let back: Conversation = serde_json::from_str(&json(&c)).unwrap();
        assert_eq!(back, c);
    }
}
