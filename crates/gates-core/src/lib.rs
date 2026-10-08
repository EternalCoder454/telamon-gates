//! Telamon Gates without Qt: what a conversation is, how it is kept on disk,
//! how a reply's Markdown becomes what the window shows, and the interface a
//! chat backend (llama, or anything else) implements. The app crate
//! (`apps/telamon-gates`) only moves these to and from QML.

pub mod backend;
pub mod conversation;
pub mod markdown;
pub mod store;

pub use backend::{Backend, BackendError, Request};
pub use conversation::{Conversation, Message, Role, Summary};
pub use store::Store;
