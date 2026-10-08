//! The app's choices, in `~/.config/telamon-gatesrc` (the framework's
//! settings file): the model picked and the system prompt.

use crate::io::Io;
use telamon_framework_ui::telamon_framework_core::settings::Settings;

const GROUP: &str = "Chat";
pub const MODEL: &str = "Model";
pub const SYSTEM_PROMPT: &str = "SystemPrompt";

fn file() -> Settings {
    Settings::for_app(telamon_framework_ui::app_info())
}

/// Read once at start, before the window shows (a small file).
pub fn get(key: &str) -> String {
    file().get(GROUP, key).unwrap_or_default()
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
