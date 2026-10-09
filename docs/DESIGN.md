# Telamon Gates: design

Telamon Gates is the AI chat of Telamon OS: a window to talk to a local model,
with conversations kept as plain files. Models run in llama.cpp's
`llama-server`, from Telamon's own Vulkan build (`telamon-llama`, packaged in
`packaging/telamon-llama/`), which Gates starts and stops itself. It is reached
through one trait (`docs/BACKEND.md`). Without the server, a demo backend
answers.

## Layout

Like Telamon Monitor: a `TelamonWindow` with a `TelamonSidebar` and the page
beside it.

- **Sidebar**: New Chat on top, then the saved conversations, newest first,
  under Today, Yesterday and Previous 7 Days, then under their date
  ("September 30, 2026") up to 30 days back, then under their month
  ("October 2025"). Above Models, Settings and About, a VRAM meter: the graphics
  card's memory used and its size (amdgpu's sysfs files, read every 3 s while
  the window shows; hidden when no card reports it). The
  built-in filter searches titles. Right click (or the Menu key) offers
  Delete, after a confirmation. Settings and About are pinned at the bottom.
  At compact widths the sidebar folds to icons.
- **Chat**: the title (and a model picker when the backend has more than one
  model) above the messages, in a centred column of at most 46 grid units.
  Your messages are accent-tinted bubbles on the trailing side; replies run
  the column's width beside a small avatar, as rich text, with each code
  block in a `TelamonCodeView` with a Copy button (a code block inside a list
  or quote stays in the text, so the list isn't cut in two). Under a finished reply:
  Copy, Regenerate on the last one, and its speed in tokens per second
  (also shown live while it comes). Beside them is the mode that wrote it
  ("Story, picked by SystemOne"), shown when SystemOne picked it or when it
  isn't Chat. While a reply comes in, the view
  follows it unless you scroll away; "Thinking…" shows until its first words.
- **Empty chat**: a greeting by the time of day with the user's first name
  (from the account's full name): morning from 5, afternoon from noon,
  evening from 5 pm, night from 8 pm, each said a few ways, one picked per
  new chat.
- **Composer**: one rounded field (a hairline border, the accent while
  writing) holding a `TelamonTextArea` that grows to about ten lines and,
  centred on its first line, a square accent Send button (faded while there
  is nothing to send; Stop while a reply comes in). Under it, on the leading
  side, a segmented Auto · Chat · Story · Code switch for the conversation
  (Auto without SystemOne answers as the last reply did, Chat at first).
  On the trailing side, the keys in small boxes (Enter Send, Shift+Enter New
  Line) and "Always double-check the answer." Escape
  stops a reply; Ctrl+N starts a new chat.
- **Banners**: an info banner while the demo backend is in use (no
  telamon-llama), one with Open Folder while the models folder is empty; an error
  banner with Try Again when a reply fails.
- **Models**: *On This Computer* lists each model in the models folder
  (`$XDG_DATA_HOME/telamon-gates/models`) with its quantisation and size label
  from the file's GGUF header (`gguf.rs`, bounded reads), its size, a badge
  against the card's VRAM (Fits: under 83 % of it, Tight: under all of it,
  Too Big: more) and Delete, after a confirmation. Vision projectors
  (`mmproj-…`) and the later parts of a split model are not listed. *Get
  Models* searches Hugging Face for GGUF repositories (`hub.rs`), opens one to
  its single-file models, and downloads one at a time with a progress bar and
  Cancel. A download goes to a hidden `.name.part`, resumes from it, is
  checked against the sha256 Hugging Face publishes, and only then is renamed
  into place; the chat's model list follows.
- **SystemOne** (in Settings): a switch, on by default; the decision model in
  use (a picker when there are several); one-click Get Laya / Get Kev when
  there's none, with the download's progress. On the Models page, a decision
  model shows "Decision model" and a SystemOne badge instead of a fit badge.
- **Settings**: the backend, the model, the system prompt (saved as you
  type), the shared transparency switch, and the folder the conversations are
  in, with Open Folder.

## Code

- `crates/gates-core` (no Qt): `Conversation`/`Message`, the `Store` (one
  JSON file per conversation), Markdown to safe rich text (`markdown.rs`),
  and the `Backend` trait with the `Demo` backend. All unit-tested.
- `apps/telamon-gates/src` (CXX-Qt):
  - `Chat` (a `QAbstractListModel` of the open conversation's messages, roles
    `role`, `text`, `kinds`, `contents`, `langs`, `streaming`, `failed`):
    `newChat`, `open`, `send`, `stop`, `regenerate`, `pickModel`,
    `saveSystemPrompt`, `refreshModels`, `dismissError`; properties
    `conversationId`, `title`, `generating`, `loading`, `error`, `demo`,
    `backendName`, `models`, `model`, `systemPrompt`, `count`, `retryable`
    (the last reply failed or never came: Try Again and Regenerate ask for
    one; a good reply is never discarded from the banner).
  - `Library` (the sidebar's list as lists `ids`, `titles`, `sections`,
    `updates`): `reload`, `setDayStart`, `remove`; `loaded`, `folder`.
  - `Vram`: `available`, `used`, `total` (bytes), `refresh`.
  - `io.rs`: the one file thread; `settings.rs`: the settings file.
- `cpp/main.cpp` only starts Qt (framework startup, single instance) and
  hands the two objects to `qml/Main.qml`.

## Threading

The GUI thread never blocks. Every file read and write runs in order on one
file thread (`Io`), so a conversation is never read before its last save
lands. Each reply streams on its own worker thread; its text comes back in
batches every 33 ms through `qt_thread().queue`. Every reply has a
generation number: Stop, New Chat and opening another conversation bump it,
so late batches of an old reply are dropped. A panicking backend is caught
and reported.

## Data

`$XDG_DATA_HOME/telamon-gates/conversations/<id>.json`, one file per
conversation (`id`, `title`, `created`, `updated` in ms since the epoch,
`messages` of `role`/`text`/`failed`/`speed`/`mode`/`picked`, and the
conversation's `mode` when it isn't Auto), written atomically (temporary file,
then rename). Ids are hex and dashes only, so no id can name a path outside
the folder. Settings are `~/.config/telamon-gatesrc`, group `[Chat]`
(`Model`, `SystemPrompt`), plus the window's size from `TelamonWindow`.

## The model server

The `telamon-llama` package: llama.cpp's `llama-server` alone, built with
Vulkan (`-DGGML_VULKAN=ON`), statically linked to llama.cpp's own libraries,
in `/usr/libexec/telamon-llama/` (nothing in /usr/bin, so it never clashes
with Fedora's `llama-cpp`).

- Left out: model downloading (`LLAMA_OPENSSL=OFF`), the embedded web UI,
  tests, examples and the other tools.
- Its CPU code is portable (`GGML_NATIVE=OFF`), since the graphics card does
  the work.
- `%prep` checks the source tarball's sha256.
- `%check` runs `--version`.

Gates runs it per `docs/BACKEND.md`: on 127.0.0.1 only, with a random port
and API key, stopped when idle, and dying with Gates.

## Privilege and attack surface

No privilege: no polkit, no system bus, no root. What comes in from outside
is the model's replies, which are untrusted: `markdown.rs` escapes every
piece of text, shows raw HTML as text, shows images as their description
(nothing is fetched), keeps link targets only for http, https and mailto,
and code blocks reach QML as plain text. Conversation files are parsed with
serde; one that doesn't parse is skipped and logged.

## Failure modes

- The backend can't be reached, or refuses: the error banner says what it
  said, with Try Again; the user's message is kept.
- A reply fails half-way: what came is kept and marked failed. Failed and
  empty replies are not sent back to the model.
- A reply ends with no text and no error: shown as "The model sent an empty
  reply.", with Try Again.
- A conversation file can't be read: it is left out of the list (logged);
  opening one that vanished shows an error and a new chat.
- A save fails: logged; the conversation stays in the window.

## Performance budget (Performant phase; not measured against yet)

Startup to first frame under 300 ms; idle CPU 0 (nothing runs while no reply
comes in); RSS under 120 MB with a long conversation open; a keystroke in the
composer under 16 ms.

Model files are trusted as much as any file the user puts in the folder: they
are parsed by llama.cpp (in its own process, not Gates'). The server's API is
reachable only from this computer and only with its key. A server address set
in Settings is used as given, over plain http.

## Phase

Functionable. From the PR #3 review, for their phases:

- Secure: the server's API key is on its command line (readable in
  `/proc` by other local users); `LLAMA_API_KEY` in its environment would not
  be. A local process could take the free port between the check and the
  server's start.
- Reliable: Stop doesn't close the connection while the server is still
  reading the prompt or loading the model, so the next message waits behind
  it; a server that hangs mid-reply holds its worker for good; a panic in a
  reply skips `release()`, so the idle stop never comes; `stop()` can signal
  a PID already reaped.
- Performant: the telamon-llama CI cache likely never hits (ccache hashes
  the random build directory); the idle thread wakes every 15 s even with
  no server.
- Thinking models stream their reasoning as `reasoning_content`, which isn't
  shown: "Thinking…" stays until the answer starts. A remote server started
  with `--api-key` can't be used yet (no field for its key).

Earlier known gaps: deleting a conversation while it
is still being opened can show it once more (rare); the file thread is not
joined on quit, so a save queued in the last instant can be lost (writes are
atomic, so never half a file). Not yet: a real backend (see `docs/BACKEND.md`), renaming a
conversation, editing a sent message, attachments, CI, the RPM's icon and
metainfo.
