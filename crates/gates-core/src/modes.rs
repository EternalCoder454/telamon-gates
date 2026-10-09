//! Modes: how a reply is written. Each is a system prompt and the sampling
//! that suits it: Story runs warmer, Code cooler. A conversation is in Auto
//! (SystemOne picks one per message, else Chat) or pinned to one.

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
}
