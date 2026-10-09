//! The one place a model is reached. The window knows only [`Backend`]: give
//! the app another implementation (llama's server, anything else) and nothing
//! in the UI changes. See `docs/BACKEND.md` for how to add one.
//!
//! Both methods block and are called on a worker thread, never the GUI's, so
//! an implementation may use plain blocking I/O.

mod demo;
pub mod llama;
pub mod server;
pub mod sse;

pub use demo::Demo;
pub use llama::Llama;

use crate::conversation::Message;
use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

/// The user's choices for the model server (Settings). The default leaves
/// everything to llama.cpp and runs the server here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Options {
    /// Model layers on the graphics card; 0 lets llama.cpp choose from the
    /// free video memory.
    pub gpu_layers: u32,
    /// The context, in tokens (how much of the conversation the model
    /// sees); 0 lets llama.cpp choose.
    pub context: u32,
    /// A llama-server already running elsewhere (`http://host:port`); empty
    /// for the one Gates runs itself.
    pub server_url: String,
}

/// What to answer: the conversation so far, ending with the user's message.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    /// The model picked in the window; "" when the backend has none to pick.
    pub model: String,
    /// Sent first, as the system message; "" for none.
    pub system_prompt: String,
    pub messages: Vec<Message>,
}

/// What a backend streams while it answers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Event<'a> {
    /// The next piece of the reply, in order. Send one per token, as
    /// servers stream them: the window counts these for its tokens per
    /// second until the backend says otherwise with `Speed`.
    Text(&'a str),
    /// The server's own measure of how fast it generates, in tokens per
    /// second (llama-server's `timings.predicted_per_second`). Once sent, it
    /// is shown in place of the window's count.
    Speed(f64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendError {
    /// Nothing answered at the address (not running, wrong address).
    Unreachable(String),
    /// It answered, with an error (no such model, bad request, overloaded).
    Refused(String),
    /// Anything else; the text is shown to the user as it is.
    Other(String),
}

impl fmt::Display for BackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BackendError::Unreachable(s) | BackendError::Refused(s) | BackendError::Other(s) => {
                f.write_str(s)
            }
        }
    }
}

impl std::error::Error for BackendError {}

pub trait Backend: Send + Sync {
    /// Shown in Settings ("llama.cpp at http://127.0.0.1:8080").
    fn name(&self) -> String;

    /// True for the built-in demo, which answers with samples: the window
    /// says so.
    fn is_demo(&self) -> bool {
        false
    }

    /// The models the user can pick from, in the order to show them. Empty
    /// when the backend serves one model and there is nothing to pick.
    fn models(&self) -> Result<Vec<String>, BackendError>;

    /// New choices from Settings; they apply from the next reply.
    fn set_options(&self, _options: &Options) {}

    /// Where the backend's model files are, if it has a folder of them.
    fn models_folder(&self) -> Option<PathBuf> {
        None
    }

    /// Streams the reply to `request`: each piece of text as it comes, in
    /// order, to `emit` (and, if the server measures it, its speed).
    /// Returns when the reply is complete, when it fails, or soon after
    /// `cancel` turns true (then with `Ok`: what was emitted is kept as the
    /// reply).
    fn complete(
        &self,
        request: &Request,
        cancel: &AtomicBool,
        emit: &mut dyn FnMut(Event<'_>),
    ) -> Result<(), BackendError>;
}
