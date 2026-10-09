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

/// What the settings say for the model server.
pub fn backend_options() -> gates_core::Options {
    gates_core::Options {
        gpu_layers: get_u32(GPU_LAYERS),
        context: get_u32(CONTEXT),
        server_url: get(SERVER_URL).trim().to_string(),
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
