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
  is nothing to send; Stop while a reply comes in), and at its leading end
  the paperclip, Attach Files…. Under it, on the leading side, one mode
  button showing the conversation's mode and its symbol (Auto, Chat, Story,
  Code, Agent, or one of the user's own); its menu lists them all, the
  user's own after a separator. On the trailing side, the keys in small
  boxes (Enter Send, Shift+Enter New Line) and "Always double-check the
  answer.", each hidden whole when there's no room. Escape
  stops a reply; Ctrl+N starts a new chat.
- **Banners**: an info banner while the demo backend is in use (no
  telamon-llama), one with Open Folder while the models folder is empty; an error
  banner with Try Again when a reply fails.
- **Models**: *On This Computer* lists each model in the models folder
  (`$XDG_DATA_HOME/telamon-gates/models`) with its quantisation and size label
  from the file's GGUF header (`gguf.rs`, bounded reads), its size, a badge
  against the card's VRAM (Fits: under 83 % of it, Tight: under all of it,
  Too Big: more) and Delete, after a confirmation. A chat model also shows
  its trained context ("32K context", from the header) in the subtitle and,
  beside the fit badge, an Images badge (a matching `mmproj` file is in the
  folder) and a Tools badge (its chat template takes tools). A decision model
  shows none of these. Vision projectors
  (`mmproj-…`) and the later parts of a split model are not listed. *Get
  Models* searches Hugging Face for GGUF repositories (`hub.rs`), opens one to
  its single-file models, and downloads one at a time with a progress bar and
  Cancel. A download goes to a hidden `.name.part`, resumes from it, is
  checked against the sha256 Hugging Face publishes, and only then is renamed
  into place; the chat's model list follows. *Recommended* (hidden when a
  model server URL is set) offers three tested models, one per use, from
  `docs/BACKEND.md` → Recommended models: For Coding, For Chat and Stories,
  and Small and Fast. Each shows a fit badge (never Too Big for a mixture of
  experts, which runs with part of it in system memory) and Download
  (`downloadFrom`, the same checked download). While it downloads, the row
  shows the percentage and Cancel. Once it's on the computer, Use makes the
  coding pick the Model for Code and the others the Model, and In Use marks
  the current one (the coding pick too when it is the Model and Code uses
  that).
- **Attachments**:
  - **Adding them:** Attach Files… under the message field, or drop files on
    the chat. Up to 8 a message, shown as removable chips above the field.
  - **Text files:** read whole (128 KB each, 256 KB a message) and sent as
    `<file name="…">` blocks after your text.
  - **Pictures** (PNG, JPEG, GIF, WebP, BMP, by their first bytes, 10 MB at
    most) are copied into `$XDG_DATA_HOME/telamon-gates/attachments`. They
    are sent as image parts only from there, so a conversation file can't
    point at another file.
  - **In the chat:** under your message, a picture shows as a thumbnail and
    a text file as a chip.
  - **Image models:** a model with a projector beside it starts with
    `--mmproj` and reads them.
- **Modes** (in Settings): a row per mode, with the start of its prompt and
  its temperature, and Edit…; Add a Mode. The dialog has the name (the
  user's own modes only), the system prompt, and the temperature: the
  model's own, or a slider from 0 to 2. Reset puts a built-in back; Delete
  removes one of the user's. They are kept in `$XDG_DATA_HOME/telamon-gates/
  modes.json` (bounded, validated on reading). The user's modes show in a
  "Your Modes" list beside the mode switch; SystemOne picks only Chat, Story
  and Code, as changed.
  Chat and Story (and edits of them) ask the model for brief reasoning:
  gpt-oss at low effort, Qwen3 with thinking off. Code, Agent and the user's
  own modes leave reasoning to the model (`docs/BACKEND.md` → Recommended
  models has the costs).
- **Edit, Branch, Export**:
  - **Your messages:** with the pointer on one, Edit and Branch From Here
    show beside it. Edit turns it into a text box: Send replaces what came
    after it and asks again; Escape or Cancel leaves it as it was.
  - **Replies:** Branch From Here sits beside Copy and Regenerate.
  - **Branching:** opens a new conversation, "Title (branch)", with the
    messages up to there (tool calls kept whole), and the same mode and
    folder. Regenerate in an agent run goes back to your last message.
  - **Export…** in the header saves Markdown (who said what, tool steps as
    one line, no tool output) or Gates' own JSON, through the portal's save
    dialog.
- **Agent mode**: Agent in the mode menu. Above the composer:
  - where it works: by default its **sandbox**, a folder of its own
    (`$XDG_DATA_HOME/telamon-gates/workspaces/<conversation>`), with "Use a
    Folder on This Computer…" (after a confirmation, the portal's folder
    dialog); in one of the user's folders, its path, Change… and Use the
    Sandbox;
  - a warning when the model's template takes no tools;
  - while the agent waits, a card with what it wants to do (a title, and
    the new text or command as plain text), plus Deny, Allow All Edits in
    This Reply (for edits) and Allow (Run for a command).

  Each tool result is one line under the reply (✓ or ✗ and what it did:
  "Read src/greet.py (lines 1–5 of 5)"), which opens to the output. A turn
  that only asked for tools shows nothing of its own, and Copy and
  Regenerate come only under the final answer.
- **Fleet** (above Models in the sidebar): several agents on one goal, and a
  page to watch and steer them. Top to bottom:
  - **Goal:** the title row has Start Fleet (Stop All while a run is under
    way). The goal card holds a text box and the folder the agents work in,
    with Choose Folder… (the portal's dialog; the folder is checked like an
    agent's workspace). While a run is under way the goal is only read,
    two or three lines.
  - **Question:** when an agent wants to change something, a card with the
    agent's name, the text or command as plain text, and Deny, Allow All
    Edits by This Agent (for edits), and Allow (Run for a command). The
    agents run one at a time, so there is one question at most.
  - **Figures:** agents, done of all, tool steps, and tokens per second
    now, over a progress bar of the agents that have ended.
  - **Cards:** a grid of one to three columns, one card per agent, in the
    plan's order. A card has its title, its place ("Agent 2 of 5"), a dot
    and a badge for its status (Idle, Working, Waiting for You, Done,
    Failed, Stopped), what it did or said last (two lines at most, a failure
    in the error colour), a progress bar (sliding while it works), its steps
    and tokens per second, a Stop button while it can still run, and, once
    SystemOne has judged it, "SystemOne is 93% sure it finished." A working
    agent's dot glows and its edge pulses in the accent colour; one that
    waits for you has the warning colour, a failed one the error colour.
    Motion stops under reduced motion.
  - **Empty and planning:** "No Agents Yet" with a line on what to do; a
    spinner and "Planning" while the coordinator splits the goal.
  - Every word an agent or the coordinator wrote is shown as plain text, or
    in a `TelamonCodeView`. Titles come from the model: they are cut to 60
    characters, one line.
  - A run isn't saved: closing the window ends it, and the page starts empty.
- **SystemOne** (in Settings): a switch, on by default; the decision model in
  use (a picker when there are several); one-click Get Laya / Get Kev when
  there's none, with the download's progress. On the Models page, a decision
  model shows "Decision model" and a SystemOne badge instead of a fit badge.
- **Settings**: the backend, the model, the Model for Code (shown when
  there are two or more models: Code and Agent replies, and the warm-up while
  typing in those modes, use it; "Same as Model", or a model that is gone,
  means the chat model), the system prompt (saved as you
  type), the shared transparency switch, and the folder the conversations are
  in, with Open Folder.

## Code

- `crates/gates-core` (no Qt): `Conversation`/`Message`, the `Store` (one
  JSON file per conversation), Markdown to safe rich text (`markdown.rs`),
  and the `Backend` trait with the `Demo` backend. All unit-tested.
- `apps/telamon-gates/src` (CXX-Qt):
  - `Chat` (a `QAbstractListModel` of the open conversation's messages, roles
    `role`, `text`, `kinds`, `contents`, `langs`, `streaming`, `failed`):
    `newChat`, `open`, `send`, `stop`, `regenerate`, `pickModel`, `pickCodeModel`,
    `saveSystemPrompt`, `refreshModels`, `dismissError`; properties
    `conversationId`, `title`, `generating`, `loading`, `error`, `demo`,
    `backendName`, `models`, `model`, `codeModel`, `systemPrompt`, `count`, `retryable`
    (the last reply failed or never came: Try Again and Regenerate ask for
    one; a good reply is never discarded from the banner).
  - `Fleet` (`fleet.rs`): properties `goal`, `workspace`, `running`,
    `planning`, `error`, `demo`, the per-agent lists `titles`, `statuses`,
    `lines`, `steps`, `speeds` (0 when not known), `confidences` (-1 when not
    asked), and the question `approving`, `approvalAgent`, `approvalTitle`,
    `approvalDetail`, `approvalKind`; `start(goal)`, `stop()`, `stopAgent(i)`,
    `answerApproval(choice)`, `chooseWorkspace(path)`, `dismissError`.
    `TELAMON_GATES_SEED` (`fleet`, `fleet-working`, `fleet-planning`) fills
    it with sample agents for `scripts/screens.sh`, and `workspace:<folder>`
    chooses the folder, to drive a real run headless; nothing else uses it.
  - `Library` (the sidebar's list as lists `ids`, `titles`, `sections`,
    `updates`): `reload`, `setDayStart`, `remove`; `loaded`, `folder`.
  - `Vram`: `available`, `used`, `total` (bytes), `refresh`.
  - `io.rs`: the one file thread; `settings.rs`: the settings file.
- `cpp/main.cpp` only starts Qt (framework startup, single instance) and
  hands the objects to `qml/Main.qml`.

## Threading

A fleet runs on one worker (`gates_core::fleet::run`); its events come back
one by one through `qt_thread().queue`, and a question waits there for the
user's answer (Stop answers it no). It reads the chat's model and SystemOne
once, at the start, from the Qt thread. The GUI thread never blocks. Every file read and write runs in order on one
file thread (`Io`), so a conversation is never read before its last save
lands. Each reply streams on its own worker thread; its text comes back in
batches every 33 ms through `qt_thread().queue`. Every reply has a
generation number: Stop, New Chat and opening another conversation bump it,
so late batches of an old reply are dropped. A panicking backend is caught
and reported.

## Data

`$XDG_DATA_HOME/telamon-gates/conversations/<id>.json`, one file per
conversation (`id`, `title`, `created`, `updated` in ms since the epoch,
`messages` of `role` (`user`, `assistant`, `tool`), `text`, `failed`,
`speed`, `mode`, `picked`, and in Agent mode `tool_calls` (id, name,
arguments), `tool_call_id` and `summary`; the conversation's `mode` when it
isn't Auto, and its `workspace`), written atomically (temporary file,
then rename). Ids are hex and dashes only, so no id can name a path outside
the folder. Settings are `~/.config/telamon-gatesrc`, group `[Chat]`
(`Model`, `CodeModel`, `SystemPrompt`), plus the window's size from `TelamonWindow`.

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

Performant, as of 1.0.0: Functionable, Secure and Reliable are done. What
each phase settled:

- Secure (done): the server's API key goes in its environment
  (`LLAMA_API_KEY`), never on its command line, where any local user could
  read it; Gates' folders are 0700 and conversations 0600; agents work in a
  sandbox (see Agent mode). Left: a local process could take the free port
  between the check and the server's start (it would then have to answer
  as llama-server, and never gets the key).
- Reliable (done):
  - **Stop:** the chat request goes through Gates' own small HTTP client
    (`backend/stream.rs`), so Stop shuts the connection at once. That
    includes while the server is still reading the prompt, and llama-server
    then cancels the work. On the RX 7900, the next message's first words
    came 1.75 s after a Stop in a long prompt (`examples/stop-check`).
  - **Loads and hangs:** Stop during a model load ends the load. A server
    silent for 10 minutes counts as gone.
  - **Clean-up:** a panic in a reply still releases the server, `stop()`
    never signals a process already reaped, and an older model list can't
    replace a newer one.
  - **Left:** on the processor, llama-server notices a cancel only between
    its 2,048-token prompt batches. For a 4B model on the i9 that is minutes,
    so the next message waits.
- Performant: the binary grew from 4.0 to 7.4 MB stripped (rustls, ring and
  webpki-roots, for Hugging Face downloads; Fedora's OpenSSL through ureq's
  native-tls would drop most of it). The telamon-llama CI cache likely never hits (ccache hashes
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
