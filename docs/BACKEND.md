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

## Plugging in another one

1. Add a module next to `demo.rs` and `llama.rs` in
   `crates/gates-core/src/backend/` with a type that implements `Backend`. Put its HTTP client in
   `crates/gates-core/Cargo.toml`; nothing Qt-related belongs there.
2. Export it from `backend/mod.rs`.
3. In `apps/telamon-gates/src/lib.rs`, `fn backend()` picks the backend.
   Return yours there.
4. Add tests beside it with recorded responses (see `demo.rs`'s tests for
   the shape: pieces in order, and cancel stops it).

## llama (built in)

`backend/llama.rs` is the llama.cpp backend; `lib.rs` picks it when
telamon-llama is installed (`/usr/libexec/telamon-llama/llama-server`, else a
`llama-server` on `$PATH`) or a server address is set in Settings, and keeps
the demo otherwise.

- **Models** are the `.gguf` files in `$XDG_DATA_HOME/telamon-gates/models`
  (`local_models`); the picker's name is the file name without `.gguf`. With
  a server address, `GET /v1/models` lists them instead.
- **The server Gates runs** (`backend/server.rs`): started on the first reply
  with the chosen model, stopped after 5 idle minutes (the model leaves the
  graphics card's memory), restarted when the model or an option changes.
  - It listens on 127.0.0.1 only, on a free port, and wants a fresh random
    `--api-key` each start, so no other program can use it.
  - It is started from one long-lived thread and dies with Gates
    (PR_SET_PDEATHSIG). That signal fires when the *thread* that started the
    child ends, so starting it from a reply's worker killed it after every
    reply; `the_server_outlives_the_thread_that_asked_for_it` tests this.
  - Flags: `--parallel 1 --jinja --no-webui --offline`. GPU layers
    (`--n-gpu-layers`) and context (`--ctx-size`) are passed only when set in
    Settings. Left out, llama.cpp's own fit (`--fit`, on by default since
    v0.6) chooses both from the free video memory, which a number would turn
    off.
  - Its output goes to `$XDG_STATE_HOME/telamon-gates/llama-server.log`; a
    failed start quotes the log's last line.
- **A reply** is `POST /v1/chat/completions` with `"stream": true` (the
  system prompt first). `backend/sse.rs` reads the server-sent events:
  - `choices[0].delta.content` becomes `Event::Text`;
  - the last chunk's `timings.predicted_per_second` becomes `Event::Speed`;
  - `data: [DONE]` ends it;
  - an `error` object becomes a `BackendError` with its message.

  Stop drops the connection, and llama-server stops generating.
- **The HTTP client** is `ureq` without TLS: plain http to 127.0.0.1, or to a
  server address on the LAN (Settings checks the address is `http://`).

### Tuning, and what was left at llama.cpp's defaults

Checked against the v0.6.0 source and a web search (2026-10-08), not taken
from advice on trust:

- **Prompt caching** is on by default (`cache_prompt`): a new message
  reuses the conversation already in the cache.
- **Flash attention** is `auto`. Vulkan supports it, and it is kept only
  where the model's layers can use it.
- **KV cache** stays f16. q8_0 halves it at little quality cost, but a
  quantised V cache needs flash attention on. This is a Performant-phase
  option.
- **Batch sizes** stay at the defaults (`-b 2048 -ub 512`). There is no
  Vulkan/RDNA3 evidence for a larger `-ub`; benchmark before changing.
- **Context shift** stays off (the default). With `--keep 0` it can drop the
  system prompt. Gates trims long conversations instead (see below).

### Long conversations

Before each reply, Gates checks the conversation fits the server's context.

- It reads the context the server actually runs with (`/v1/models`:
  `meta.n_ctx`, once per server), so Automatic shows its real size in
  Settings.
- It counts the conversation's real tokens with the server's own chat
  template and tokenizer (`/apply-template`, then `/tokenize`).
- If they exceed the context minus room for the reply (a quarter of it, at
  most 2048 tokens), the oldest turns go first. The system prompt and the
  last message always stay; `trim_to_budget` is the pure, tested rule.
- A server without those endpoints gets the whole conversation.

`cargo run -p gates-core --example llama-check -- <llama-server> <models dir>
[context]` checks a build, a model and a graphics card outside the app. With
a context of 512 it shows trimming.
- **Speculative decoding** is not used: there are no gains measured on AMD,
  and it needs a matching draft model.

Every reply is untrusted text. The window already treats it so (see
`markdown.rs`: escaped, no raw HTML, no images, web links only): a backend
passes the text through as it comes and does nothing to it.
