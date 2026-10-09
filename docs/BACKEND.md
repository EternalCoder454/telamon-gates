# Connecting a backend

Telamon Gates talks to a model through one Rust trait,
[`gates_core::Backend`](../crates/gates-core/src/backend/mod.rs). The window
knows nothing else: swap the implementation and nothing in the UI changes.
Until one is connected, the built-in `Demo` backend streams sample replies
and the chat says so in a banner (it says what to install; see
`docs/DESIGN.md` → Startup).

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
    API key each start, so no other program can use it. The key goes in the
    server's environment (`LLAMA_API_KEY`), never on its command line.
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
- **Batch sizes** stay at the defaults (`-b 2048 -ub 512`): measured on the
  RX 7900 with Qwen3-4B and a 13k-token prompt, `-ub 512` reads 3,060
  tokens/s, `-ub 1024` 2,452 and `-ub 2048` 1,861.
- **Context shift** stays off (the default). With `--keep 0` it can drop the
  system prompt. Gates trims long conversations instead (see below).
- **Automatic context** is Gates' own choice, not llama.cpp's fit. Left to
  itself, fit filled the RX 7900 with cache: a 129,024-token context for
  Qwen3-4B, 23.1 of 24 GiB in use, and no room for Kev. Automatic is now
  32,768 tokens, or the model's own context (`<arch>.context_length`) if
  smaller (`context_for`): 9.9 GiB in use for the same model. GPU layers are
  still fit's. Settings can set a bigger context.

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

### Model facts

What the Models page shows for each chat model, none of it loaded into the
server:

| Fact | From |
|---|---|
| Context | `<arch>.context_length` in the GGUF header (`gguf.rs`), in tokens; "32K", "128K", "1M" |
| Tools | `tokenizer.chat_template` mentions tools (`gguf.rs`); the same test Agent mode warns with |
| Images | a projector file beside the model: `projector_for` in `llama.rs` |

`projector_for(model, all, projectors)` is pure; `local_projectors(dir)`
lists the `mmproj-….gguf` files that `local_models` leaves out. A projector
belongs to a model when their names match before the quantisation
(`mmproj-gemma-3-4b-it-F16.gguf` and `gemma-3-4b-it-Q4_K_M.gguf`), or, with
no model name in it (`mmproj-model-f16.gguf`, `mmproj-F16.gguf`), when the
model is the only one in the folder. Of several precisions, F16 wins, then
BF16. Images are shown but not yet sent: attachments are not built.

`ModelLibrary` has them as lists beside `names`: `contexts` (tokens, 0 when
the header doesn't say), `toolCapable` and `vision` (1 or 0).

## Modes and SystemOne

`modes.rs` has three modes. Each is a system prompt plus sampling (temperature,
top-p), sent in the request body:

| Mode | Temperature | Top-p | Reasoning | System prompt |
|---|---|---|---|---|
| Chat | the model's own | the model's own | brief | none (only the user's own) |
| Story | 1.0 | 0.95 | brief | a creative-writing partner that keeps the story consistent |
| Code | 0.2 | 0.9 | the model's own | an expert programmer: complete fenced code, assumptions named |

Brief reasoning is explained under Performance → Recommended models.

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

## Agent mode and tools

**Agent** is a fourth mode, pinned by the user and never picked by
SystemOne, so no tool runs unless the user asked for an agent. It works in
one folder per conversation (`Conversation.workspace`), chosen above the
composer.

**The tools** (`tools.rs`) are plain Rust run inside Gates: no MCP, no
interpreter, no server.

| Tool | Does | Runs |
|---|---|---|
| `list_dir` | names, kinds and sizes in a folder | at once |
| `read_file` | text with line numbers, 400 lines a part (2,000 at most) | at once |
| `search` | a string in text files (smart case), `file:line: text` | at once |
| `find_files` | paths containing a string, or `*`/`?` patterns | at once |
| `now` | the local date, time, weekday and time zone | at once |
| `calculate` | arithmetic: `+ - * / % ^` (and `**`), brackets, unary signs, decimals, `sqrt abs round floor ceil min max ln log10 sin cos tan`, `pi` and `e` | at once |
| `write_file` | creates or replaces a file (atomic, keeps permissions) | after the user allows it |
| `edit_file` | replaces text that is in the file once | after the user allows it |
| `run_command` | `/bin/sh -c` in the folder, 60 s (300 s at most) | after the user allows it, every time |

The limits:
- **`now`** asks libc for the local time (`localtime_r`, so `TZ` and
  `/etc/localtime` apply): "Thursday, 2026-10-08 14:32:05 UTC-04:00 (EDT)".
- **`calculate`** is a small recursive-descent parser, with no `eval` and no
  dependency. It reads 1,000 characters at most and nests 64 levels at most
  (brackets, signs, powers and calls). `-2^2` is -4 and `2^3^2` is 2^9.
  Division or remainder by zero, a result too large for a number (`9^9^9^9`),
  a result that is not real (`sqrt(-1)`, `ln(0)`) and anything it can't read
  come back as errors for the model to fix. Results show 12 significant
  digits ("0.3" for 0.1 + 0.2). Angles are in radians.
- **The folder:** `/`, a top folder (`/etc`) and the home folder or one
  above it are refused as workspaces, since reading tools don't ask.
- **Paths:** every path is resolved against the real folders, links
  included. One that leaves the workspace is refused, and new folders are
  made only under a real folder inside it. Writes go through a fresh
  temporary file with a random name (`O_EXCL`, `O_NOFOLLOW`), so a link a
  repository planted can't redirect them.
- **The sandbox** (`sandbox.rs`): every command runs in bubblewrap, which
  shows it the workspace (writable), `/usr` and a few files of `/etc`
  (read-only), a private `/tmp`, `/dev` and `/run`, and new namespaces: no
  home folder, no network, no other process, no session bus. Settings →
  Agent can give it the network, or the home folder read-only (for the
  user's toolchains); both are off. Without bubblewrap commands don't run.
  Inside another container (the dev container, CI) it has no `/proc`:
  binding the host's would reach other processes' files.
- **Commands** run in their own process group: a timeout or Stop ends
  everything they started, and leftovers end when they do. Their output
  comes through a pipe, kept in memory as its first 8 KiB and last 22 KiB,
  so nothing fills the disk.
- **Asking:** the card shows the folder, the time limit, the line and
  character count, and the full text wrapped. Hidden characters (controls,
  bidi marks) show as `⟨U+202E⟩` and are called out. Arguments that can't
  run are refused without asking. "Allow All Edits in This Reply" doesn't
  cover hidden files and folders (`.git`, `.envrc`), build files (Makefile,
  package.json, Cargo.toml…), scripts and programs: those always ask.
- **Walks:** a search or find skips `.git`, `target`, `node_modules`, build
  output and hidden folders, and doesn't follow links.
- **Sizes:** a result is 32 KiB at most, and a file read is 4 MiB at most.
- **Forgiving edits:** when the text isn't in the file as written,
  `edit_file` takes off the line numbers models copy from `read_file`, then
  lets lines match whatever their indentation; it must still be there once.
- **Long runs:** to fit the context, whole turns before the task go first,
  then the oldest tools' output gives way to a note. The task and every
  call with its result stay (`trim_to_budget`).
- **One mode per run:** the mode and folder can't change while it runs,
  and Auto never continues in Agent.

**The loop** (`agent.rs`):
- **Each step:** the request carries the tools' schema. llama-server
  (`--jinja`) streams `delta.tool_calls` in pieces, and `llama.rs` hands
  them on whole (`Event::ToolCalls`) at `[DONE]`. Reads run at once; a
  write or a command waits for Allow, Deny, or "Allow All Edits in This
  Reply" (commands always ask). Each result goes back as a `tool` message,
  and the model goes on.
- **Limits:** the loop stops after 25 steps. Stop answers any waiting
  question with no.
- **Stopped runs:** `repair` gives calls left without a result "Stopped
  before it ran.", so the conversation can still be sent.
- **Cost:** llama-server's prompt cache keeps the conversation's prefix, so
  each step only reads what is new.

**Which models can:** a model's GGUF chat template has to mention tools
(`gguf.rs` reads `tokenizer.chat_template`). For others the window says the
agent can only talk.

## The Fleet

A fleet (`fleet.rs`) is several agents on one goal, in one workspace folder.
`fleet::run` blocks, so the app calls it from a worker, and it tells the
window through the `FleetHost` trait, event by event with the agent's number
(`planned`, `status`, `step`, `speed`, `line`, `approve`, `judged`).

- **The plan:** the coordinator is the chat model, asked once, with
  `response_format: {"type": "json_object", "schema": …}` (the schema asks
  for `{"tasks": [{"title", "instructions"}]}`, 1 to 6 of them), which
  llama-server turns into a grammar. `Request.response_format` carries it
  to `/v1/chat/completions`. `parse_plan` still doesn't trust the answer: it
  takes the JSON out of a fence or a sentence, keeps the first 6 tasks,
  makes a title one line of 60 characters and instructions 1,500 characters,
  skips an entry with neither, and fails with a sentence when no task is
  left. The goal is cut to 4,000 characters.
- **The agents:** each task is `agent::run` in the same workspace, one after
  another: the one server has `--parallel 1`, and agents that change the same
  files shouldn't race. An agent's system prompt is the agent mode's, the
  workspace, the goal, its own task, and one line from each agent before it
  (what it reported), so later agents know what is done. A task that fails,
  or is stopped, doesn't end the others; a server that can't be reached does.
- **Asking:** each agent's questions come through `FleetHost::approve` with
  the agent's number. The agents run one at a time, so the page has one
  question at most. While it waits the agent is "waiting for you".
- **Stopping:** `Control` stops the whole fleet (the coordinator, the agent
  under way, those to come) or agent *i* (under way, or skipped if its turn
  hasn't come); a waiting question is answered no.
- **Judging:** when an agent ends as done, `Judge::finished` asks SystemOne a
  `noul` question, "Did this agent complete its task?", about the task and the
  end of the agent's last reply (`systemone::done_question`, `done_state`).
  The answer is the probability of yes (`parse_noul`), shown as the card's
  confidence. With no decision model, or when SystemOne is resting or
  fails, nothing is shown. A model that ends a turn with "I will now
  update…" and no tool call counts as done, which is what the question is
  for.
- **Its server:** Gates doesn't change `--parallel`: with one slot, a
  request of the chat or of another fleet waits behind the agent's.

**`examples/fleet-check`** runs a real fleet on the small Python project:
`cargo run --release -p gates-core --example fleet-check -- <llama-server>
<models dir>` (a chat model that takes tools, and optionally a decision
model). Measured 2026-10-09 with Qwen3-4B-Instruct-2507 Q4_K_M and
Laya-Q8_0 on the RX 7900, for a goal that asks for two tasks:

| Run | Plan | Agents | Total | SystemOne (agent 1, 2) | Done |
|---|---|---|---|---|---|
| 1 | 2.3 s | 3 steps each | 7.3 s | 85%, 75% | both edits |
| 2 | 2.3 s | 3 and 2 steps | 6.2 s | 89%, 49% | code only |

In run 2 the second agent said what it would do and ended its turn without
the tool call; SystemOne was right to doubt it. The same fleet ran in the
app, headless, with a click on each Allow: 2 of 2 agents done, 81% and 87%.

**`examples/agent-check`** runs a real model on a small Python project (change
a greeting, add a run line to the README). Measured 2026-10-09 with
Qwen3-4B-Instruct-2507 Q4_K_M on the RX 7900, about 170 tokens/s:

| Version | Steps | Failed edits | Time | Done |
|---|---|---|---|---|
| exact `edit_file` | 11 | 4 | 10.0 s | yes |
| forgiving `edit_file` (3 runs) | 6 | 0 | 5.5 s (median) | 3 of 3 |

## Web search

Settings → Web Search turns on two more tools, `web_search(query, count ≤ 8)`
and `fetch_page(url)`. Both only read, so they run at once, with no question to
the user; each call shows as a tool row.

- **Who gets them:** Chat, Code and the user's own modes, when the model's
  chat template takes tools (`gguf::Info.tools`, the Models page's Tools
  badge; a server elsewhere and the demo backend can't be asked and count),
  through `agent::run_tools` with the web tools alone and `WEB_STEPS` (6)
  model turns, the last one asked for without tools so the answer comes. Agent
  mode has them beside its own (`MAX_STEPS`). Story never. Decision: a model
  without the Tools badge simply doesn't search; Settings says so.
- **Providers** (`web/providers.rs`; request building and parsing are pure,
  tested from captured sample responses):

  | Service | Request | Key |
  |---|---|---|
  | Brave Search | `GET api.search.brave.com/res/v1/web/search?q=…&count=…` | header `X-Subscription-Token` |
  | Tavily | `POST api.tavily.com/search` (JSON `query`, `max_results`) | header `Authorization: Bearer` |
  | SearXNG | `GET <instance>/search?q=…&format=json` | none; the instance must list `json` under `search.formats` |

  Titles and snippets are reduced to plain text, links kept only when http(s),
  duplicates dropped. The key is never in an address, so it can't reach an
  error message, and a printed request shows its header names only. Services
  with a key are reached over https only and never follow a redirect (which
  would hand the key to wherever it points); a SearXNG instance, which has no
  key and is often plain http on the user's own network, may. HTTP (`ureq`,
  native-tls) ignores proxies.
- **The key** is kept by `web::keys::KeyStore`: the system keyring through
  `oo7` (Secret Service; KWallet answers it on Plasma) for the app, `Memory`
  for tests, `Missing` for "no keyring". Without a keyring Settings says so and
  nothing is saved. Starting the app only checks that the service answers (no
  wallet is opened or unlocked); the key is read on the first reply that needs
  it, which may ask the user to unlock the wallet, and then kept in memory.
  Keys are kept per provider.
- **`fetch_page`** (`web/fetch.rs`, `web/html.rs`): https only, no sign-in in
  the address; redirects followed here, 5 at most, each checked again; a
  resolver that drops non-public addresses (`public_ip`: loopback, private,
  link-local, CGNAT, documentation, multicast, reserved, and IPv6 forms that
  carry an IPv4 address or lead to one: mapped, NAT64 (both ranges), 6to4,
  Teredo); 15 s in all (one deadline for every hop), 1.5 MB read, 20 KB of
  text kept. A panic while reading a page (a parser bug met on a hostile
  page) is caught and answered as "couldn't read the page", never mistaken
  for Stop. HTML becomes text: scripts, styles, menus and footers dropped,
  `<main>` preferred, headings as `#` lines, links as `[text](url)` with
  absolute http(s) targets, invisible and direction-changing characters
  removed. Plain text and JSON are read as they are; other types are refused.
- **What the model may open** (`web/session.rs`): the address of each result
  of a `web_search` (the line after its title, never its snippet, which
  whoever wrote the page controls), addresses the user wrote, and websites the
  user named: the host of an address they wrote (any page, any query), or a
  bare domain written as a word of its own (bounded by whitespace or
  punctuation; not part of an email or a path; not a file name: a word ending
  in `rs`, `md`, `zip`, `sh`, `py`, `go`, `json`, `toml`, `txt`, `mov` and
  the like is a file unless it starts with `www.`), which may be opened at a
  path but never with a query string. Links on pages it read do **not** count:
  a page can make as many as it likes, and each fetch is a covert channel.
  Anything else is refused ("Search for it first"), which stops an injected
  page from sending the conversation out in an address. Addresses are
  compared without their fragment, and a trailing slash only matters where
  the path is more than the root.
- **Stop:** each call runs on a thread of its own that the reply waits on, so
  Stop returns within 40 ms; the thread ends by its time limits.
- **The prompt** (`web::PROMPT`) tells the model the results are data, not
  instructions, not to put anything private in a search, and to cite what it
  used as Markdown links. Results carry the same warning.
- **Demo:** the demo backend plays a model that uses the tools (a search, the
  first page, an answer with its sources) over `web::Canned`, made-up results
  at example.org, .net and .com, so the rows and progress can be seen without a
  model or a key.
- **Cost:** the keyring (`oo7`, with `zbus` and a one-thread `tokio`) and
  `url` took the lockfile from 119 to 225 packages and the minimum Rust to
  1.92 (oo7's). A lighter way to reach the Secret Service is a Performant-phase
  question.
- **Checked live** (2026-10-09, `examples/web-check`): `fetch` of
  https://example.com and of a Wikipedia article (whose 90-language list is
  cut to a few lines and a count, so the article fits the 20 KB), and the
  refusals of `http://`, 169.254.169.254 and loopback. A search against a
  real provider was not made (no key).
- **Left:** no live check against a real provider was made (no key). The
  parsers follow the services' published response shapes; a live check is
  one Test Connection away.

## Performance (measured 2026-10-09, RX 7900 XTX, Vulkan)

Qwen3-4B-Instruct-2507 Q4_K_M unless said; temperature 0; scripts in
`out/spec` (not in the repo).

- **Speculative decoding, n-gram (on by default, `--spec-default`).** The
  server guesses the next tokens from what is already in the conversation,
  and the model checks them in one go. It costs about 16 MB and no extra
  model.

  | Prompt | Without | With |
  |---|---|---|
  | Rewrite a 6 KB Rust file (what an agent's edits are) | 178 tok/s, 9.0 s | **944 tok/s, 2.0 s** (99% accepted) |
  | Explain heat pumps (chat) | 194 tok/s | 193 tok/s |
  | A 400-word story | 192 tok/s | 193 tok/s |

- **A small draft model** (Qwen3-0.6B Q8_0 for the 4B) is slower: only
  27–36% of its tokens are accepted, and chat drops to 126 tok/s. It is not
  used.
- **Trained drafts.** A `dspark-<model>` file beside its model (as ggml-org
  publishes them) is used with n-gram drafting (`llama::draft_for`). DFlash
  drafts made chat slower and aren't used. Qwen3-8B Q8_0, tok/s:

  | Draft | Chat | Rewrite a file | Story |
  |---|---|---|---|
  | none | 86 | 82 | 85 |
  | n-gram | 86 | 726 | 86 |
  | DFlash | 70 (8% accepted) | 310 | 68 |
  | DSpark | 122 | 281 | 129 |
  | **DSpark + n-gram** | **123** | **826** | **130** |

  Drafts are kept out of the chat model list, and the Models page shows them
  as Speed-Up.
- **Context cache precision** (Settings → Smaller Context Cache, off by
  default). A 32k context costs 7,104 MiB at f16 against 4,954 MiB at q8_0
  (−30%), and generation drops from 154 to 139 tok/s (−10%).
- **Prompt cache.** Within a conversation, the next message reads only
  what's new: 16 tokens in 0.16 s after a 13k-token prompt.
- **Trimming with room to spare.** Once a conversation overflows the
  context, it is trimmed to 75% of the budget, not just under it. Dropping
  the oldest turn changes the start, and the server then reads everything
  again: 16,027 tokens in 6.4 s here, minutes on the processor.
  `--cache-reuse 256` didn't avoid that in testing, so with room to spare
  the next several messages share their start and come from the cache.
- **Warm-up while typing.** The composer calls `prepare()` 400 ms into a
  pause: the model server starts loading (once a minute at most), and in
  Auto SystemOne picks the mode for the text then. Send then waits for
  neither the load nor the pick.
- **SystemOne on the processor.** Laya runs with half the logical CPUs (up
  to 16): 307 ms a message against 606 ms with llama.cpp's 8 threads, at
  the same accuracy.
- **The binary.** HTTPS through the system's OpenSSL instead of a bundled
  rustls/ring: 8.48 → 7.14 MB stripped for the same release build
  (−1.34 MB, −16%).

### Recommended models

The Models page's *Recommended* list comes from this test (`out/spec/
codeeval.py`, `bench.py`; RX 7900 XTX, 24 GiB, telamon-llama 0.6.0, n-gram
drafting on). The code test has 8 tasks (palindromes, merging intervals,
Roman numerals, top-k words, durations, brackets, RPN, version sorting),
each written in Rust, Go and Python: 24 answers. Each answer is compiled
(`rustc`, `go`) and run against fixed tests. Thinking is off, at
temperature 0.2, and each model was run 3–4 times.

| Model (file) | Size | Code test, mean (range) | Rust | Go | Python | Time for 24 | Chat tok/s | Rewrite tok/s | Story tok/s |
|---|---|---|---|---|---|---|---|---|---|
| **Qwen3-Coder-30B-A3B** UD-Q4_K_XL | 16.5 GiB | **21.0** (21–21) | **8/8** every run | 5 | 8 | **24 s** | 192 | 674 | 192 |
| gpt-oss-20b MXFP4, low effort | 11.3 GiB | 20.6 (18–23) | 4–8 | 5–7 | 8 | 30 s | 204 | 800 | 204 |
| Qwen3-4B-Instruct-2507 Q4_K_M | 2.3 GiB | 18.4 (17–20) | 4–5 | 5–7 | 7–8 | 31 s | 196 | 966 | 195 |
| Qwen3-8B Q8_0 + DSpark | 8.1 + 1.1 GiB | 15.0 (14–16) | 4–5 | 2–3 | 8 | 19 s | 122 | 831 | 128 |
| Qwen3.5-9B Q4_K_M | 5.3 GiB | 14 | 2 | 4 | 8 | 66 s | 104 | 504 | 104 |

- **Mixture-of-experts models are the cheap way to power.** Qwen3-Coder-30B
  and gpt-oss-20b compute only about 3B parameters a token, so they answer
  as fast as the 4B and faster than the dense 8B. They know as much as
  their full size. Both fit a 24 GiB card; on a smaller one, llama.cpp's fit
  keeps experts in system memory and they still run.
- **Qwen3-Coder** has the best and steadiest code (the same 21 each run)
  and the shortest answers: 3.9k tokens for the 24, against about 5.6k for
  the others. Its Go misses are type errors (`byte` for `rune`), unused
  imports and one changed signature.
- **gpt-oss-20b** is the fastest in chat and almost as good at code.
  - Its eagle3 draft (`--spec-type draft-eagle3`) doesn't change the score,
    and it slows chat from 204 to 182 tok/s and rewrites from 800 to
    247 tok/s, so it isn't used.
  - Without `reasoning_effort` it reasons at medium, and a story request
    can go entirely to reasoning (below).
- **Qwen3-4B-2507** is the pick for cards of 8 GiB or less, and it wrote the
  most inventive story.
- **Qwen3.5-9B** with thinking off was the weakest at Rust and the slowest,
  so it isn't recommended.
- Every model got all 8 Python tasks; Rust and Go set them apart.

**Reasoning, per mode.** Reasoning gets more code right but costs a lot
more tokens.

| Code test | Score | Time for 24 | Tokens |
|---|---|---|---|
| gpt-oss-20b, low effort | 20.6 | 30 s | 5.7k |
| gpt-oss-20b, medium effort (its default) | 23 | 79 s | 15.3k |
| Qwen3-8B, thinking off | 15 | 19 s | not logged |
| Qwen3-8B, thinking on (its default) | 21 | 559 s | 90k |

In chat and stories, reasoning only costs time, and gpt-oss can loop
(`out/spec/brief.py`, temperature 0):

| Prompt | Model's own reasoning | Brief |
|---|---|---|
| gpt-oss-20b, chat | 583 tokens, 3.2 s | 724 tokens, 3.7 s |
| gpt-oss-20b, a 400-word story | **6,000 tokens, 25.8 s, all reasoning: no story** | 713 tokens, 3.7 s, 557 words |
| Qwen3-8B, chat | 1,011 tokens (494 words of thinking), 7.7 s | 442 tokens, 3.4 s |
| Qwen3-8B, story | 1,498 tokens (835 words of thinking), 13.0 s | 534 tokens, 4.0 s |
| Qwen3-Coder-30B (doesn't reason) | 2.0 s / 2.4 s | 1.0 s / 1.9 s |

So Chat and Story are *brief* (`modes::Mode::brief` → `Request::brief`).
The request carries `reasoning_effort: "low"` for gpt-oss and
`chat_template_kwargs: {"enable_thinking": false}` for Qwen3; other
templates ignore both. Code and Agent, and a user's own modes, leave
reasoning to the model. In Auto, SystemOne's pick decides: a coding
question goes to Code and reasons.

