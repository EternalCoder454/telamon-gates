//! Modes: how a reply is written. Each is a system prompt and the sampling
//! that suits it: Story runs warmer, Code cooler. A conversation is in Auto
//! (SystemOne picks one per message, else Chat) or pinned to one.
//!
//! The user can change the built-in modes' prompt and temperature, and add
//! modes of their own (`Library`, kept in `modes.json`): that is the prompt
//! library.

use serde::{Deserialize, Serialize};
use std::io;
use std::path::Path;

/// How the next token is drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sampling {
    pub temperature: f32,
    pub top_p: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mode {
    /// Stored in conversations and settings; never shown.
    pub id: &'static str,
    /// The system prompt the mode adds (before the user's own, if any).
    pub prompt: &'static str,
    /// None keeps the model's own (its recommended settings).
    pub sampling: Option<Sampling>,
}

/// The conversation's choice when SystemOne picks per message.
pub const AUTO: &str = "auto";

pub const CHAT: Mode = Mode {
    id: "chat",
    prompt: "",
    sampling: None,
};

pub const STORY: Mode = Mode {
    id: "story",
    prompt: "You are a creative writing partner. Write vivid, original prose with natural \
             dialogue. Keep characters, places and facts consistent with the story so far, \
             and follow the user's style, tense and point of view. Continue the story rather \
             than summarising or explaining it, unless asked.",
    sampling: Some(Sampling {
        temperature: 1.0,
        top_p: 0.95,
    }),
};

pub const CODE: Mode = Mode {
    id: "code",
    prompt: "You are an expert programmer. Give correct, complete code in fenced blocks \
             that name their language, then explain it briefly. Follow the user's language, \
             libraries and style, and say what you assumed and which edge cases matter.",
    sampling: Some(Sampling {
        temperature: 0.2,
        top_p: 0.9,
    }),
};

/// Works in a folder with tools (`agent.rs`, `tools.rs`). Only when the
/// user pins it: SystemOne never picks it, so no tool runs unasked for.
pub const AGENT: Mode = Mode {
    id: "agent",
    prompt: "You are an agent that gets things done in the user's workspace folder with \
             tools. Look before you change anything: list, find, search and read the files \
             that matter. Then make small, exact changes with edit_file (write_file for new \
             files), and run commands to build or test when that helps. Paths are relative \
             to the workspace. Edits and commands need the user's approval: if one is \
             declined, ask what to do instead. When you are done, say briefly what you \
             changed.",
    sampling: Some(Sampling {
        temperature: 0.3,
        top_p: 0.9,
    }),
};

/// Every mode, in the order the window offers them.
pub const MODES: [Mode; 4] = [CHAT, STORY, CODE, AGENT];

/// The modes SystemOne picks from.
pub const PICKABLE: [Mode; 3] = [CHAT, STORY, CODE];

/// The mode with `id`; Chat for one that isn't (an old or edited file).
pub fn mode(id: &str) -> Mode {
    MODES.iter().copied().find(|m| m.id == id).unwrap_or(CHAT)
}

/// Whether `id` is a conversation's valid choice: Auto or a mode.
pub fn valid_choice(id: &str) -> bool {
    id == AUTO || MODES.iter().any(|m| m.id == id)
}

/// The system prompt for `mode` with the user's own from Settings: the
/// mode's first, so the user's can refine it.
pub fn system_prompt(mode: &Mode, user: &str) -> String {
    match (mode.prompt.is_empty(), user.trim().is_empty()) {
        (true, _) => user.trim().to_string(),
        (false, true) => mode.prompt.to_string(),
        (false, false) => format!("{}\n\n{}", mode.prompt, user.trim()),
    }
}

/// A mode as the user sees and edits it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    pub id: String,
    pub name: String,
    pub prompt: String,
    /// None: the model's own (or, for a built-in, its usual one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
}

/// A mode ready to use: its prompt and sampling.
#[derive(Debug, Clone, PartialEq)]
pub struct Active {
    pub id: String,
    pub prompt: String,
    pub sampling: Option<Sampling>,
}

impl From<Mode> for Active {
    fn from(m: Mode) -> Active {
        Active {
            id: m.id.to_string(),
            prompt: m.prompt.to_string(),
            sampling: m.sampling,
        }
    }
}

/// The most modes of one's own, and the longest prompt kept.
const MAX_CUSTOM: usize = 64;
const MAX_PROMPT: usize = 16 * 1024;
/// The largest `modes.json` read.
const MAX_FILE: u64 = 1024 * 1024;

/// The user's changes to the built-in modes, and their own modes.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Library {
    /// Built-in modes as changed (only the prompt and temperature count).
    #[serde(default)]
    pub edits: Vec<Preset>,
    /// The user's own modes, in order.
    #[serde(default)]
    pub custom: Vec<Preset>,
}

fn builtin_name(id: &str) -> &'static str {
    match id {
        "story" => "Story",
        "code" => "Code",
        "agent" => "Agent",
        _ => "Chat",
    }
}

/// A preset made safe: trimmed, bounded, a sane temperature.
fn clean(mut p: Preset) -> Preset {
    p.name = p.name.trim().chars().take(40).collect();
    if p.prompt.len() > MAX_PROMPT {
        let mut cut = MAX_PROMPT;
        while !p.prompt.is_char_boundary(cut) {
            cut -= 1;
        }
        p.prompt.truncate(cut);
    }
    p.temperature = p
        .temperature
        .filter(|t| t.is_finite())
        .map(|t| t.clamp(0.0, 2.0));
    p
}

impl Library {
    /// The library in `path`; empty when there is none or it doesn't parse.
    pub fn load(path: &Path) -> Library {
        let Ok(meta) = std::fs::metadata(path) else {
            return Library::default();
        };
        if meta.len() > MAX_FILE {
            log::warn!("{} is too big; using the built-in modes", path.display());
            return Library::default();
        }
        match std::fs::read_to_string(path).map(|t| serde_json::from_str::<Library>(&t)) {
            Ok(Ok(mut lib)) => {
                lib.edits.retain(|p| MODES.iter().any(|m| m.id == p.id));
                lib.custom
                    .retain(|p| p.id.starts_with("my-") && !p.name.trim().is_empty());
                lib.custom.truncate(MAX_CUSTOM);
                lib.edits = lib.edits.into_iter().map(clean).collect();
                lib.custom = lib.custom.into_iter().map(clean).collect();
                lib
            }
            _ => {
                log::warn!("can't read {}; using the built-in modes", path.display());
                Library::default()
            }
        }
    }

    /// Writes the library to `path` whole (a file beside it, then a rename).
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        std::fs::rename(&tmp, path)
    }

    /// Every mode in the order the window shows them: the built-ins (as
    /// changed), then the user's.
    pub fn list(&self) -> Vec<Preset> {
        let mut out: Vec<Preset> = MODES
            .iter()
            .map(|m| {
                let edit = self.edits.iter().find(|e| e.id == m.id);
                Preset {
                    id: m.id.to_string(),
                    name: builtin_name(m.id).to_string(),
                    prompt: edit.map_or(m.prompt.to_string(), |e| e.prompt.clone()),
                    temperature: edit
                        .and_then(|e| e.temperature)
                        .or(m.sampling.map(|s| s.temperature)),
                }
            })
            .collect();
        out.extend(self.custom.iter().cloned());
        out
    }

    /// Whether `id` is a conversation's valid choice: Auto or a mode here.
    pub fn has(&self, id: &str) -> bool {
        valid_choice(id) || self.custom.iter().any(|p| p.id == id)
    }

    /// The mode `id`, ready to use; Chat for one that is gone.
    pub fn resolve(&self, id: &str) -> Active {
        let preset = self.list().into_iter().find(|p| p.id == id);
        let Some(preset) = preset else {
            return self.resolve(CHAT.id);
        };
        let builtin = MODES.iter().find(|m| m.id == id);
        let top_p = builtin.and_then(|m| m.sampling).map_or(0.95, |s| s.top_p);
        Active {
            id: preset.id,
            prompt: preset.prompt,
            sampling: preset
                .temperature
                .map(|temperature| Sampling { temperature, top_p }),
        }
    }

    /// Saves `preset`: a built-in's prompt and temperature, or one of the
    /// user's (a new one when its id is empty). Gives its id.
    pub fn put(&mut self, preset: Preset, new_id: impl FnOnce() -> String) -> String {
        let mut preset = clean(preset);
        if MODES.iter().any(|m| m.id == preset.id) {
            preset.name = builtin_name(&preset.id).to_string();
            self.edits.retain(|e| e.id != preset.id);
            let id = preset.id.clone();
            self.edits.push(preset);
            return id;
        }
        if preset.name.is_empty() {
            preset.name = "My Mode".into();
        }
        if let Some(existing) = self
            .custom
            .iter_mut()
            .find(|p| p.id == preset.id && !p.id.is_empty())
        {
            *existing = preset.clone();
            return preset.id;
        }
        if self.custom.len() >= MAX_CUSTOM {
            return String::new();
        }
        preset.id = new_id();
        let id = preset.id.clone();
        self.custom.push(preset);
        id
    }

    /// Deletes one of the user's modes, or puts a built-in back as it was.
    pub fn remove(&mut self, id: &str) {
        self.custom.retain(|p| p.id != id);
        self.edits.retain(|p| p.id != id);
    }
}

/// The system prompt for an active mode with the user's own from Settings.
pub fn system_prompt_for(mode: &Active, user: &str) -> String {
    match (mode.prompt.trim().is_empty(), user.trim().is_empty()) {
        (true, _) => user.trim().to_string(),
        (false, true) => mode.prompt.clone(),
        (false, false) => format!("{}\n\n{}", mode.prompt, user.trim()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_by_id() {
        assert_eq!(mode("story").id, "story");
        assert_eq!(mode("nonsense").id, "chat");
        assert!(valid_choice("auto") && valid_choice("code"));
        assert!(!valid_choice("") && !valid_choice("../x"));
        // Chat keeps the model's own sampling; Story runs warm, Code cool.
        assert_eq!(CHAT.sampling, None);
        let (story, code) = (STORY.sampling.unwrap(), CODE.sampling.unwrap());
        assert!(story.temperature > 0.8 && code.temperature < 0.5);
    }

    #[test]
    fn prompts_combine() {
        assert_eq!(system_prompt(&CHAT, "  Be brief. "), "Be brief.");
        assert_eq!(system_prompt(&CHAT, ""), "");
        assert_eq!(system_prompt(&CODE, ""), CODE.prompt);
        let both = system_prompt(&STORY, "Use British spelling.");
        assert!(both.starts_with(STORY.prompt) && both.ends_with("Use British spelling."));
    }

    #[test]
    fn the_library() {
        let mut lib = Library::default();
        assert_eq!(lib.list().len(), MODES.len());
        assert_eq!(lib.resolve("story").sampling, STORY.sampling);
        // A built-in, changed: its prompt and temperature, not its name.
        lib.put(
            Preset {
                id: "story".into(),
                name: "Renamed".into(),
                prompt: "Write noir.".into(),
                temperature: Some(1.2),
            },
            || unreachable!(),
        );
        let story = lib.resolve("story");
        assert_eq!(story.prompt, "Write noir.");
        assert_eq!(story.sampling.unwrap().temperature, 1.2);
        assert_eq!(lib.list()[1].name, "Story");
        // One of the user's own.
        let id = lib.put(
            Preset {
                id: String::new(),
                name: "  Pirate  ".into(),
                prompt: "Talk like a pirate.".into(),
                temperature: Some(9.0),
            },
            || "my-1".into(),
        );
        assert_eq!(id, "my-1");
        assert!(lib.has("my-1") && !lib.has("my-2"));
        let pirate = lib.resolve("my-1");
        assert_eq!(pirate.sampling.unwrap().temperature, 2.0);
        assert_eq!(lib.list().last().unwrap().name, "Pirate");
        // Gone: Chat; a built-in back as it was.
        lib.remove("my-1");
        assert_eq!(lib.resolve("my-1").id, "chat");
        lib.remove("story");
        assert_eq!(lib.resolve("story").prompt, STORY.prompt);
        // Kept on disk, and odd entries dropped on the way back.
        let path = std::env::temp_dir().join(format!("gates-modes-{}.json", std::process::id()));
        let mut odd = lib.clone();
        odd.custom.push(Preset {
            id: "../evil".into(),
            name: "x".into(),
            prompt: String::new(),
            temperature: None,
        });
        odd.save(&path).unwrap();
        assert_eq!(Library::load(&path), lib);
        let _ = std::fs::remove_file(&path);
        assert_eq!(
            Library::load(Path::new("/nonexistent/modes.json")),
            Library::default()
        );
    }
}
