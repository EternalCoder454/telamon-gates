# Telamon Gates: design

Telamon Gates is the AI chat of Telamon OS: a window to talk to a local model,
with conversations kept as plain files. This repository is the front end; the
model is reached through one trait (`docs/BACKEND.md`), and until one is
connected a demo backend answers.

## Layout

Like Telamon Monitor: a `TelamonWindow` with a `TelamonSidebar` and the page
beside it.

- **Sidebar**: New Chat on top, then the saved conversations, newest first,
  under Today, Yesterday, Previous 7 Days, Previous 30 Days and Older. The
  built-in filter searches titles. Right click (or the Menu key) offers
  Delete, after a confirmation. Settings and About are pinned at the bottom.
  At compact widths the sidebar folds to icons.
- **Chat**: the title (and a model picker when the backend has more than one
  model) above the messages, in a centred column of at most 46 grid units.
  Your messages are accent-tinted bubbles on the trailing side; replies run
  the column's width beside a small avatar, as rich text, with each code
  block in a `TelamonCodeView` with a Copy button (a code block inside a list
  or quote stays in the text, so the list isn't cut in two). Under a finished reply:
  Copy, and Regenerate on the last one. While a reply comes in, the view
  follows it unless you scroll away; "Thinking…" shows until its first words.
- **Composer**: a `TelamonTextArea` that grows to about ten lines. Enter
  sends, Shift+Enter is a new line, Escape stops a reply; Send turns into
  Stop while one comes in. Ctrl+N starts a new chat.
- **Banners**: an info banner while the demo backend is in use; an error
  banner with Try Again when a reply fails.
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
  - `Library` (the sidebar's list as string lists `ids`, `titles`,
    `sections`): `reload`, `setDayStart`, `remove`; `loaded`, `folder`.
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
`messages` of `role`/`text`/`failed`), written atomically (temporary file,
then rename). Ids are hex and dashes only, so no id can name a path outside
the folder. Settings are `~/.config/telamon-gatesrc`, group `[Chat]`
(`Model`, `SystemPrompt`), plus the window's size from `TelamonWindow`.

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

## Phase

Functionable. Known gaps for later phases: deleting a conversation while it
is still being opened can show it once more (rare); the file thread is not
joined on quit, so a save queued in the last instant can be lost (writes are
atomic, so never half a file). Not yet: a real backend (see `docs/BACKEND.md`), renaming a
conversation, editing a sent message, attachments, CI, the RPM's icon and
metainfo.
