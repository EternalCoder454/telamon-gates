# Connecting a backend

Telamon Gates talks to a model through one Rust trait,
[`gates_core::Backend`](../crates/gates-core/src/backend/mod.rs). The window
knows nothing else: swap the implementation and nothing in the UI changes.
Until one is connected, the built-in `Demo` backend streams sample replies
and the chat says so in a banner.

## The trait

```rust
pub trait Backend: Send + Sync {
    fn name(&self) -> String;                 // shown in Settings
    fn is_demo(&self) -> bool { false }       // true only for the demo
    fn models(&self) -> Result<Vec<String>, BackendError>;
    fn complete(
        &self,
        request: &Request,                    // model, system prompt, messages
        cancel: &AtomicBool,                  // true when the user pressed Stop
        emit: &mut dyn FnMut(Event<'_>),      // the reply as it comes
    ) -> Result<(), BackendError>;
}

pub enum Event<'a> {
    Text(&'a str),  // the next piece of the reply: one per token
    Speed(f64),     // the server's tokens per second, if it measures it
}
```

- Both methods are called on a **worker thread** and may block: plain
  blocking HTTP is fine. The app batches what `emit` gets (about 30 times a
  second) and hands it to the window.
- Tokens per second: the window shows the count of `Text` events per second
  since the first, so emit one per token (as llama-server streams them). A
  backend that gets the server's own figure sends it with `Speed`, which
  then replaces the count. It is saved with the reply.
- `complete` returns when the reply is done, on an error, or soon after
  `cancel` turns true (then `Ok`: what was emitted stays as the reply).
  Check `cancel` between chunks.
- `models()` returns the choices for the model pickers (the chat header shows
  one when there are two or more, Settings whenever there is one). Return an
  empty list when the server has one model and nothing to pick.
- Errors: `Unreachable` (nothing answered), `Refused` (it answered with an
  error), `Other`. The text is shown to the user as it is, so make it a
  sentence.
- `Request.messages` is the conversation so far, user and assistant turns in
  order, ending with the user's message. `system_prompt` is "" for none.
- A panic in a backend is caught and shown as "The backend stopped
  unexpectedly."

## Plugging one in

1. Add a module next to `demo.rs` in `crates/gates-core/src/backend/` (say
   `llama.rs`) with a type that implements `Backend`. Put its HTTP client in
   `crates/gates-core/Cargo.toml`; nothing Qt-related belongs there.
2. Export it from `backend/mod.rs`.
3. In `apps/telamon-gates/src/lib.rs`, `fn backend()` is the one line that
   picks the backend. Return yours there.
4. Add tests beside it with recorded responses (see `demo.rs`'s tests for
   the shape: pieces in order, and cancel stops it).

## llama

Assumed: "llama.app" serves llama.cpp's `llama-server` API, which is
OpenAI-compatible. Then:

- `models()`: `GET {base}/v1/models`, the `data[].id` values.
- `complete()`: `POST {base}/v1/chat/completions` with
  `{"model", "messages": [{"role": "system"|"user"|"assistant", "content"}], "stream": true}`.
  The answer is server-sent events: each `data: {...}` line carries
  `choices[0].delta.content`; `data: [DONE]` ends it. Emit each `content`
  as `Event::Text`, and the last chunk's `timings.predicted_per_second` (when
  present) as `Event::Speed`.
- The base address (for example `http://127.0.0.1:8080`) is the backend's own
  setting. The settings file is `~/.config/telamon-gatesrc`; the app reads it
  with `settings::get` in `apps/telamon-gates/src/settings.rs`.

Every reply is untrusted text. The window already treats it so (see
`markdown.rs`: escaped, no raw HTML, no images, web links only): a backend
passes the text through as it comes and does nothing to it.
