//! The app's choices, in `~/.config/telamon-gatesrc` (the framework's
//! settings file): the model picked, the system prompt, and how the model
//! server runs.

use crate::io::Io;
use telamon_framework_ui::telamon_framework_core::settings::Settings;

const GROUP: &str = "Chat";
pub const MODEL: &str = "Model";
pub const SYSTEM_PROMPT: &str = "SystemPrompt";
/// 0 or unset: llama.cpp chooses.
pub const GPU_LAYERS: &str = "GpuLayers";
pub const CONTEXT: &str = "ContextSize";
/// A llama-server already running elsewhere; unset runs one here.
pub const SERVER_URL: &str = "ServerUrl";
/// "true": the context cache at 8 bits (less video memory, a little slower).
pub const SMALL_CACHE: &str = "SmallCache";
/// The limit on graphics memory in use, past which the model servers stop:
/// unset is 95%, "off" is no limit, else 85, 90 or 98.
pub const MEMORY_CAP: &str = "MemoryCap";
/// The model Code and Agent mode use; unset: the chat model.
pub const CODE_MODEL: &str = "CodeModel";
/// "true" lets an agent's commands use the network (off by default).
pub const AGENT_NETWORK: &str = "AgentNetwork";
/// "true" shows them the home folder, read-only (off by default).
pub const AGENT_HOME: &str = "AgentHome";
/// "false" turns SystemOne off; unset or anything else leaves it on.
pub const SYSTEM_ONE: &str = "SystemOne";
/// The decision model SystemOne uses (a file name without `.gguf`); unset
/// picks one.
pub const DECISION_MODEL: &str = "DecisionModel";

/// The Settings sections that are open, as ids joined by commas ("none" for
/// all folded); unset opens only the model's.
pub const SETTINGS_OPEN: &str = "SettingsOpen";

/// "true" lets Chat, Code and Agent replies search the web (off by default).
pub const WEB_SEARCH: &str = "WebSearch";
/// "brave", "tavily" or "searxng".
pub const WEB_PROVIDER: &str = "WebProvider";
/// A SearXNG instance's address. Not a secret.
pub const WEB_URL: &str = "WebSearxUrl";

/// "true": a key for `provider` is in the system keyring. The key is never
/// in this file; this only says there is one, so that starting the app
/// doesn't open the keyring.
pub fn web_key_flag(provider: gates_core::web::Provider) -> &'static str {
    match provider {
        gates_core::web::Provider::Brave => "WebKeyBrave",
        gates_core::web::Provider::Tavily => "WebKeyTavily",
        gates_core::web::Provider::Searxng => "WebKeySearxng",
    }
}

/// The foldable Settings sections, in page order. System Prompt is always
/// open and not among them.
pub const SECTIONS: [&str; 8] = [
    "model",
    "modes",
    "agent",
    "web",
    "systemone",
    "appearance",
    "conversations",
    "troubleshooting",
];

/// The sections open when nothing is saved.
const OPEN_BY_DEFAULT: [&str; 1] = ["model"];

/// The open sections a saved value says, in page order; ids this version
/// doesn't know are dropped.
pub fn open_sections(saved: &str) -> Vec<&'static str> {
    let saved = saved.trim();
    if saved.is_empty() {
        return OPEN_BY_DEFAULT.to_vec();
    }
    SECTIONS
        .into_iter()
        .filter(|id| saved.split(',').any(|s| s.trim() == *id))
        .collect()
}

/// What to save for the open sections.
pub fn encode_sections(open: &[&str]) -> String {
    if open.is_empty() {
        "none".into()
    } else {
        open.join(",")
    }
}

/// The sections `open` names, with `id` opened or folded; `None` when `id`
/// is no section or already is so.
pub fn with_section(open: &[String], id: &str, opened: bool) -> Option<Vec<&'static str>> {
    let id = SECTIONS.into_iter().find(|s| *s == id)?;
    let is_open = |s: &str| open.iter().any(|o| o == s);
    if is_open(id) == opened {
        return None;
    }
    Some(
        SECTIONS
            .into_iter()
            .filter(|s| if *s == id { opened } else { is_open(s) })
            .collect(),
    )
}

fn file() -> Settings {
    Settings::for_app(telamon_framework_ui::app_info())
}

/// Read once at start, before the window shows (a small file).
pub fn get(key: &str) -> String {
    file().get(GROUP, key).unwrap_or_default()
}

/// A whole number, 0 when unset or not one.
pub fn get_u32(key: &str) -> u32 {
    get(key).trim().parse().unwrap_or(0)
}

/// The graphics memory limit.
pub fn memory_cap() -> gates_core::watchdog::Cap {
    gates_core::watchdog::Cap::from_setting(&get(MEMORY_CAP))
}

/// What the settings say for the model server.
pub fn backend_options() -> gates_core::Options {
    gates_core::Options {
        gpu_layers: get_u32(GPU_LAYERS),
        context: get_u32(CONTEXT),
        server_url: get(SERVER_URL).trim().to_string(),
        // On unless turned off: the coding test scores the same with it
        // (BACKEND.md → Performance).
        small_cache: get(SMALL_CACHE) != "false",
    }
}

/// Written on the file thread; "" removes the key.
pub fn set(io: &Io, key: &'static str, value: String) {
    io.run(move |_| {
        let value = (!value.is_empty()).then_some(value.as_str());
        if let Err(e) = file().set(GROUP, key, value) {
            log::warn!("cannot save the setting {key}: {e}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(open: &[&str]) -> Vec<String> {
        open.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn unset_opens_the_model_section_only() {
        assert_eq!(open_sections(""), ["model"]);
    }

    #[test]
    fn none_is_all_folded() {
        assert!(open_sections("none").is_empty());
        assert_eq!(encode_sections(&[]), "none");
    }

    #[test]
    fn saved_ids_come_back_in_page_order_without_strangers() {
        assert_eq!(
            open_sections("web, model,gone,agent"),
            ["model", "agent", "web"]
        );
        assert_eq!(open_sections(&encode_sections(&SECTIONS)), SECTIONS);
    }

    #[test]
    fn opening_and_folding_one_section() {
        let open = names(&["model"]);
        assert_eq!(with_section(&open, "web", true).unwrap(), ["model", "web"]);
        assert!(with_section(&open, "model", false).unwrap().is_empty());
        assert_eq!(with_section(&open, "model", true), None);
        assert_eq!(with_section(&open, "web", false), None);
        assert_eq!(with_section(&open, "system-prompt", true), None);
    }
}
