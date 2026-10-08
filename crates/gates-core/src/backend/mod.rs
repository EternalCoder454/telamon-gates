//! The one place a model is reached. The window knows only [`Backend`]: give
//! the app another implementation (llama's server, anything else) and nothing
//! in the UI changes. See `docs/BACKEND.md` for how to add one.
//!
//! Both methods block and are called on a worker thread, never the GUI's, so
//! an implementation may use plain blocking I/O.

mod demo;

pub use demo::Demo;

use crate::conversation::Message;
use std::fmt;
use std::sync::atomic::AtomicBool;

/// What to answer: the conversation so far, ending with the user's message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// The model picked in the window; "" when the backend has none to pick.
    pub model: String,
    /// Sent first, as the system message; "" for none.
    pub system_prompt: String,
    pub messages: Vec<Message>,
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

    /// Streams the reply to `request`: each piece of text as it comes, in
    /// order, to `emit`. Returns when the reply is complete, when it fails, or
    /// soon after `cancel` turns true (then with `Ok`: what was emitted is
    /// kept as the reply).
    fn complete(
        &self,
        request: &Request,
        cancel: &AtomicBool,
        emit: &mut dyn FnMut(&str),
    ) -> Result<(), BackendError>;
}
