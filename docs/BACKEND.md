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
  (`local_models`); the picker's name is the file name without `.gguf`.
  Vision projectors (`mmproj-…`) and the later parts of a split model
  (`…-00002-of-00003`) are not models to pick. The Models page fills the
  folder (see `docs/DESIGN.md`). With
  a server address, `GET /v1/models` lists them instead.
- **The server Gates runs** (`backend/server.rs`): started on the first reply
  with the chosen model, stopped after 5 idle minutes (the model leaves the
  graphics card's memory), restarted when the model or an option changes.
  New options don't cut a reply off: the server goes once the last reply on
  it ends (`retire`), and Settings hands them over on the GUI thread, in the
  order they were made.
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
  - `data: [DONE]` ends it; a stream that ends without it was cut off, and
    fails with what came kept;
  - an `error` object becomes a `BackendError` with its message.

  Stop drops the connection, and llama-server stops generating.
- **The HTTP client** is `ureq`: plain http to 127.0.0.1, or to a server
  address on the LAN (Settings checks the address is `http://`). It ignores
  `HTTP_PROXY` and the like, which would send the server's key to the proxy.
  Only `hub.rs` uses TLS (rustls, https only) to reach Hugging Face.

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

## Modes and SystemOne

`modes.rs` has three modes. Each is a system prompt plus sampling (temperature,
top-p), sent in the request body:

| Mode | Temperature | Top-p | System prompt |
|---|---|---|---|
| Chat | the model's own | the model's own | none (only the user's own) |
| Story | 1.0 | 0.95 | a creative-writing partner that keeps the story consistent |
| Code | 0.2 | 0.9 | an expert programmer: complete fenced code, assumptions named |

A user's own system prompt (Settings) follows the mode's. A conversation is in
Auto or pinned to a mode (`Conversation.mode`). Each reply records the mode
that wrote it, and whether SystemOne picked it.

**SystemOne** (`systemone.rs`) picks the mode in Auto.
- **How it asks:** before the chat model answers, a decision model gets one
  `choice` question about the user's message through llama.cpp's
  `/v1/systemone` (TypeSafe-compatible, in telamon-llama 0.6.0).
- **What it acts on:** an answer below `MIN_CONFIDENCE` (0.5), an error or a
  timeout means the mode of the conversation's last reply (Chat at first), so
  "continue" stays in a story. It never fails a reply. A question may take
  15 s, and its server 30 s to start. After a failed start SystemOne steps
  aside for 5 minutes, so a broken setup doesn't slow every message. Its answer is
  a label from a fixed set; no text from it reaches the window.
- **Which models are decision models:** they are recognised by the GGUF key
  `<arch>.decision.type` (`gguf.rs`). They are kept out of the chat model
  list, since they write no text.
- **Which one it uses:** the one saved in Settings, else Laya, else the first.
  Laya runs with `--n-gpu-layers 0` on the processor. Kev fits itself to the
  graphics card.
- **Its server:** its own llama-server (`systemone-server.log`), with a
  2048-token context and batch, because a decision model reads each prompt in
  one micro-batch. It stops after 5 idle minutes, like the chat server.
- **Getting a model:** Settings offers both from ggml-org's official
  conversions (`Laya-GGUF` Q8_0, `Kev-4B-GGUF` Q4_K_M), downloaded and checked
  like any other model.

### The test set

`cargo run --release -p gates-core --example systemone-check --
<decision-model.gguf>` asks 36 labelled messages (12 per mode, some
deliberately hard). "As used" counts Chat for answers below the threshold,
which is what Gates does. Measured 2026-10-09, telamon-llama 0.6.0:

| Model | Runs on | Accuracy | As used | Confident and right | Per message (median) | Load |
|---|---|---|---|---|---|---|
| Laya-Q8_0 (421M) | processor (i9-14900KF) | 29/36 (81%) | 32/36 (89%) | 22/24 (92%) | 606 ms | 0.8 s |
| Kev-4B-Q4_K_M | RX 7900 (Vulkan) | 34/36 (94%) | 34/36 (94%) | 34/34 (100%) | 47 ms | 1.9 s |
| Kev-4B-Q4_K_M | processor | 34/36 (94%) | 34/36 (94%) | 34/34 (100%) | 7.4 s | 7.4 s |

- **Laya** is sure about Story and Code. It is weak on Chat, the "anything
  else" option: it often guesses Story or Code with low confidence. The
  threshold turns those guesses into Chat. Laya's own model card says it is a
  base to fine-tune and close to chance zero-shot; with this question it does
  better than that.
- **Kev** is accurate and well calibrated. It needs the graphics card (3 GB of
  video memory) to be fast.
- **The wording:** a "Chat: anything else: …" description raised Laya's Chat
  accuracy from 3/12 to 6/12.
- **The threshold:** anywhere from 0.5 to 0.65 scored the same on this set, so
  it stays at 0.5 rather than being tuned to 36 messages.

