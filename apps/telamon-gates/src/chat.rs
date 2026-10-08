//! The open conversation: its messages as a list model for the chat view, and
//! sending, stopping and regenerating replies. Replies stream from the
//! backend on a worker thread and come back to the Qt thread in batches.

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
        include!("cxx-qt-lib/qstringlist.h");
        type QStringList = cxx_qt_lib::QStringList;
        include!("cxx-qt-lib/qmodelindex.h");
        type QModelIndex = cxx_qt_lib::QModelIndex;
        include!("cxx-qt-lib/qvariant.h");
        type QVariant = cxx_qt_lib::QVariant;
        include!("cxx-qt-lib/qhash.h");
        type QHash_i32_QByteArray = cxx_qt_lib::QHash<cxx_qt_lib::QHashPair_i32_QByteArray>;
        include!("cxx-qt-lib/qlist.h");
        type QList_i32 = cxx_qt_lib::QList<i32>;
        include!(<QtCore/QAbstractListModel>);
        type QAbstractListModel;
    }

    extern "RustQt" {
        #[qobject]
        #[base = QAbstractListModel]
        /// The open conversation's id; "" for a new one not yet sent.
        #[qproperty(QString, conversation_id, cxx_name = "conversationId")]
        #[qproperty(QString, title)]
        /// A reply is coming in.
        #[qproperty(bool, generating)]
        /// A conversation is being read from disk.
        #[qproperty(bool, loading)]
        /// What went wrong with the last reply, to show; "" when nothing.
        #[qproperty(QString, error)]
        /// The backend is the built-in demo, which answers with samples.
        #[qproperty(bool, demo)]
        #[qproperty(QString, backend_name, cxx_name = "backendName")]
        /// The models to pick from, and the one picked.
        #[qproperty(QStringList, models)]
        #[qproperty(QString, model)]
        #[qproperty(QString, system_prompt, cxx_name = "systemPrompt")]
        #[qproperty(i32, count)]
        /// The last message is a failed reply, or a message with no reply:
        /// Try Again asks for one.
        #[qproperty(bool, retryable)]
        /// The user's first name, for the greeting; "" when unknown.
        #[qproperty(QString, user_name, cxx_name = "userName")]
        #[namespace = "telamon_gates"]
        type Chat = super::ChatRust;
    }

    unsafe extern "RustQt" {
        /// Leaves the open conversation for a new, empty one.
        #[qinvokable]
        #[cxx_name = "newChat"]
        fn new_chat(self: Pin<&mut Chat>);

        /// Opens a saved conversation.
        #[qinvokable]
        fn open(self: Pin<&mut Chat>, id: &QString);

        /// Sends `text` as the user's message and asks for a reply. False
        /// (and nothing sent) while a reply is coming in, or for blank text.
        #[qinvokable]
        fn send(self: Pin<&mut Chat>, text: &QString) -> bool;

        /// Stops the reply coming in; what came is kept.
        #[qinvokable]
        fn stop(self: Pin<&mut Chat>);

        /// Asks again for the last reply, in place of the one there, or for
        /// a reply to a last message that has none.
        #[qinvokable]
        fn regenerate(self: Pin<&mut Chat>);

        #[qinvokable]
        #[cxx_name = "pickModel"]
        fn pick_model(self: Pin<&mut Chat>, name: &QString);

        #[qinvokable]
        #[cxx_name = "saveSystemPrompt"]
        fn save_system_prompt(self: Pin<&mut Chat>, text: &QString);

        /// Asks the backend again for its models.
        #[qinvokable]
        #[cxx_name = "refreshModels"]
        fn refresh_models(self: Pin<&mut Chat>);

        #[qinvokable]
        #[cxx_name = "dismissError"]
        fn dismiss_error(self: Pin<&mut Chat>);

        #[inherit]
        #[cxx_name = "beginInsertRows"]
        fn begin_insert_rows(self: Pin<&mut Chat>, parent: &QModelIndex, first: i32, last: i32);
        #[inherit]
        #[cxx_name = "endInsertRows"]
        fn end_insert_rows(self: Pin<&mut Chat>);
        #[inherit]
        #[cxx_name = "beginRemoveRows"]
        fn begin_remove_rows(self: Pin<&mut Chat>, parent: &QModelIndex, first: i32, last: i32);
        #[inherit]
        #[cxx_name = "endRemoveRows"]
        fn end_remove_rows(self: Pin<&mut Chat>);
        #[inherit]
        #[cxx_name = "beginResetModel"]
        fn begin_reset_model(self: Pin<&mut Chat>);
        #[inherit]
        #[cxx_name = "endResetModel"]
        fn end_reset_model(self: Pin<&mut Chat>);
        #[inherit]
        fn index(self: &Chat, row: i32, column: i32, parent: &QModelIndex) -> QModelIndex;
        #[inherit]
        #[qsignal]
        #[cxx_name = "dataChanged"]
        fn data_changed(
            self: Pin<&mut Chat>,
            top_left: &QModelIndex,
            bottom_right: &QModelIndex,
            roles: &QList_i32,
        );

        #[cxx_override]
        fn data(self: &Chat, index: &QModelIndex, role: i32) -> QVariant;
        #[cxx_override]
        #[cxx_name = "roleNames"]
        fn role_names(self: &Chat) -> QHash_i32_QByteArray;
        #[cxx_override]
        #[cxx_name = "rowCount"]
        fn row_count(self: &Chat, parent: &QModelIndex) -> i32;
    }

    impl cxx_qt::Threading for Chat {}

    #[namespace = "rust::cxxqtlib1"]
    unsafe extern "C++" {
        include!("cxx-qt-lib/common.h");

        #[cxx_name = "make_unique"]
        fn chat_make_unique() -> UniquePtr<Chat>;
    }
}

use crate::io::Io;
use crate::library;
use crate::settings;
use core::pin::Pin;
use cxx_qt::{CxxQtThread, CxxQtType, Threading};
use cxx_qt_lib::{
    QByteArray, QHash, QHashPair_i32_QByteArray, QList, QModelIndex, QString, QStringList, QVariant,
};
use gates_core::markdown::{self, Block};
use gates_core::{Backend, Conversation, Event, Message, Request, Role};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// How often streamed text is handed to the window: often enough to look
/// live, seldom enough that each batch is worth a redraw.
const BATCH: Duration = Duration::from_millis(33);

const FIRST_ROLE: i32 = 0x0100; // Qt::UserRole
const ROLES: [&str; 8] = [
    "role",      // "user" or "assistant"
    "text",      // the message as written (Markdown for a reply)
    "kinds",     // each block's kind: "prose" or "code"
    "contents",  // each block's rich text (prose) or plain text (code)
    "langs",     // each code block's language, "" for prose
    "streaming", // the reply is still coming in
    "failed",    // the reply stopped on an error
    "speed",     // tokens per second of a reply, 0 when not known
];

/// A message as the view shows it, made once per change.
#[derive(Default, Clone)]
struct Row {
    kinds: QStringList,
    contents: QStringList,
    langs: QStringList,
}

impl Row {
    fn of(message: &Message) -> Row {
        let mut row = Row::default();
        if message.role == Role::User {
            return row;
        }
        for block in markdown::blocks(&message.text) {
            match block {
                Block::Prose(html) => {
                    row.kinds.append(QString::from("prose"));
                    row.contents.append(QString::from(html.as_str()));
                    row.langs.append(QString::default());
                }
                Block::Code { lang, code } => {
                    row.kinds.append(QString::from("code"));
                    row.contents.append(QString::from(code.as_str()));
                    row.langs.append(QString::from(lang.as_str()));
                }
            }
        }
        row
    }
}

#[derive(Default)]
pub struct ChatRust {
    conversation_id: QString,
    title: QString,
    generating: bool,
    loading: bool,
    error: QString,
    demo: bool,
    backend_name: QString,
    models: QStringList,
    model: QString,
    system_prompt: QString,
    count: i32,
    retryable: bool,
    user_name: QString,

    conversation: Option<Conversation>,
    rows: Vec<Row>,
    /// The reply under way, if any, stops when this turns true.
    cancel: Option<Arc<AtomicBool>>,
    /// Bumped by every reply started, stopped or left: batches from an older
    /// one are dropped.
    generation: u64,
    /// Bumped by every open: a slow read can't replace a newer choice.
    opening: u64,
    /// The last reply is coming in: it shows as streaming.
    streaming: bool,

    pub backend: Option<Arc<dyn Backend>>,
    pub io: Option<Io>,
    // Boxed: a thread handle is not Unpin, and the struct must be.
    pub library: Option<Box<CxxQtThread<library::qobject::Library>>>,
}

/// Tokens per second of a reply: the server's own figure once it sends
/// one, else the pieces counted since the first came (each is a token).
#[derive(Default)]
struct Rate {
    tokens: u64,
    first: Option<Instant>,
    reported: Option<f64>,
}

impl Rate {
    fn token(&mut self) {
        self.tokens += 1;
        self.first.get_or_insert_with(Instant::now);
    }

    fn reported(&mut self, speed: f64) {
        if speed.is_finite() && speed > 0.0 {
            self.reported = Some(speed);
        }
    }

    fn speed(&self) -> Option<f64> {
        self.reported.or_else(|| {
            let seconds = self.first?.elapsed().as_secs_f64();
            // The first token's time is the wait for it, not generation.
            (self.tokens >= 2 && seconds > 0.0).then(|| (self.tokens - 1) as f64 / seconds)
        })
    }
}

fn int(n: usize) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
}

impl qobject::Chat {
    /// Called once by `telamon_objects_new` after the backend is set.
    pub fn start(mut self: Pin<&mut Self>) {
        let (name, demo) = match &self.rust().backend {
            Some(b) => (b.name(), b.is_demo()),
            None => (String::new(), false),
        };
        self.as_mut().set_backend_name(QString::from(name.as_str()));
        self.as_mut().set_demo(demo);
        self.as_mut()
            .set_model(QString::from(settings::get(settings::MODEL).as_str()));
        self.as_mut()
            .set_user_name(QString::from(crate::user::first_name().as_str()));
        self.as_mut().set_system_prompt(QString::from(
            settings::get(settings::SYSTEM_PROMPT).as_str(),
        ));
        self.refresh_models();
    }

    pub fn new_chat(mut self: Pin<&mut Self>) {
        self.as_mut().leave();
        self.as_mut().rust_mut().opening += 1;
        self.as_mut().set_loading(false);
        self.show(None);
    }

    pub fn open(mut self: Pin<&mut Self>, id: &QString) {
        if *self.conversation_id() == *id && !*self.loading() {
            return;
        }
        let Some(io) = self.rust().io.clone() else {
            return;
        };
        self.as_mut().leave();
        self.as_mut().rust_mut().opening += 1;
        let opening = self.rust().opening;
        self.as_mut().set_loading(true);
        let id = id.to_string();
        let qt = self.qt_thread();
        io.run(move |store| {
            let result = store.load(&id);
            let _ = qt.queue(move |mut chat| {
                if chat.rust().opening != opening {
                    return;
                }
                chat.as_mut().set_loading(false);
                match result {
                    Ok(c) => chat.show(Some(c)),
                    Err(e) => {
                        log::warn!("cannot open conversation {id}: {e}");
                        chat.as_mut().set_error(QString::from(
                            format!("Couldn't open the conversation: {e}").as_str(),
                        ));
                        chat.show(None);
                    }
                }
            });
        });
    }

    pub fn send(mut self: Pin<&mut Self>, text: &QString) -> bool {
        let text = text.to_string();
        let text = text.trim();
        if text.is_empty() || *self.generating() || *self.loading() {
            return false;
        }
        if self.rust().conversation.is_none() {
            let c = Conversation::new(text);
            self.as_mut()
                .set_conversation_id(QString::from(c.id.as_str()));
            self.as_mut().set_title(QString::from(c.title.as_str()));
            self.as_mut().rust_mut().conversation = Some(c);
        }
        self.as_mut().set_error(QString::default());
        if let Some(c) = self.as_mut().rust_mut().conversation.as_mut() {
            c.touch();
        }
        self.as_mut().push(Message::user(text));
        self.ask();
        true
    }

    pub fn stop(mut self: Pin<&mut Self>) {
        if !*self.generating() {
            return;
        }
        if let Some(cancel) = self.as_mut().rust_mut().cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.as_mut().rust_mut().generation += 1;
        self.finish_reply(String::new(), None, None, true);
    }

    pub fn regenerate(mut self: Pin<&mut Self>) {
        if *self.generating() || *self.loading() {
            return;
        }
        let Some(last) = self
            .rust()
            .conversation
            .as_ref()
            .and_then(|c| c.messages.last())
            .map(|m| m.role)
        else {
            return;
        };
        self.as_mut().set_error(QString::default());
        if last == Role::Assistant {
            self.as_mut().pop();
        }
        if let Some(c) = self.as_mut().rust_mut().conversation.as_mut() {
            c.touch();
        }
        self.ask();
    }

    pub fn pick_model(mut self: Pin<&mut Self>, name: &QString) {
        if *self.model() == *name {
            return;
        }
        self.as_mut().set_model(name.clone());
        if let Some(io) = &self.rust().io {
            settings::set(io, settings::MODEL, name.to_string());
        }
    }

    pub fn save_system_prompt(mut self: Pin<&mut Self>, text: &QString) {
        if *self.system_prompt() == *text {
            return;
        }
        self.as_mut().set_system_prompt(text.clone());
        if let Some(io) = &self.rust().io {
            settings::set(io, settings::SYSTEM_PROMPT, text.to_string());
        }
    }

    pub fn refresh_models(self: Pin<&mut Self>) {
        let Some(backend) = self.rust().backend.clone() else {
            return;
        };
        let qt = self.qt_thread();
        std::thread::spawn(move || {
            let result = backend.models();
            let _ = qt.queue(move |mut chat| match result {
                Ok(models) => {
                    let mut list = QStringList::default();
                    for m in &models {
                        list.append(QString::from(m.as_str()));
                    }
                    chat.as_mut().set_models(list);
                    // The saved choice while the backend still has it, else
                    // its first.
                    let picked = chat.model().to_string();
                    if !models.contains(&picked) {
                        let first = models.first().cloned().unwrap_or_default();
                        chat.set_model(QString::from(first.as_str()));
                    }
                }
                Err(e) => {
                    log::warn!("cannot list the models: {e}");
                    chat.as_mut().set_models(QStringList::default());
                    chat.set_error(QString::from(
                        format!("Couldn't reach the model: {e}").as_str(),
                    ));
                }
            });
        });
    }

    pub fn dismiss_error(self: Pin<&mut Self>) {
        self.set_error(QString::default());
    }

    /// The conversation `id` was deleted: leave it if it is open.
    pub fn forget(mut self: Pin<&mut Self>, id: &str) {
        if self.conversation_id().to_string() == id {
            // Dropped first: stopping a reply saves the conversation, which
            // would write the deleted file back.
            self.as_mut().rust_mut().conversation = None;
            self.new_chat();
        }
    }

    // ---- The model rows ----

    pub fn data(&self, index: &QModelIndex, role: i32) -> QVariant {
        let i = index.row();
        let (Some(message), Some(row)) = (
            usize::try_from(i).ok().and_then(|i| {
                self.rust()
                    .conversation
                    .as_ref()
                    .and_then(|c| c.messages.get(i))
            }),
            usize::try_from(i)
                .ok()
                .and_then(|i| self.rust().rows.get(i)),
        ) else {
            return QVariant::default();
        };
        let last = usize::try_from(i).ok() == self.rust().rows.len().checked_sub(1);
        match role - FIRST_ROLE {
            0 => QVariant::from(&QString::from(message.role.as_str())),
            1 => QVariant::from(&QString::from(message.text.as_str())),
            2 => QVariant::from(&row.kinds),
            3 => QVariant::from(&row.contents),
            4 => QVariant::from(&row.langs),
            5 => QVariant::from(&(last && self.rust().streaming)),
            6 => QVariant::from(&message.failed),
            7 => QVariant::from(&message.speed.unwrap_or(0.0)),
            _ => QVariant::default(),
        }
    }

    pub fn role_names(&self) -> QHash<QHashPair_i32_QByteArray> {
        let mut roles = QHash::default();
        for (i, name) in ROLES.iter().enumerate() {
            roles.insert(FIRST_ROLE + int(i), QByteArray::from(*name));
        }
        roles
    }

    pub fn row_count(&self, parent: &QModelIndex) -> i32 {
        if parent.is_valid() {
            0
        } else {
            int(self.rust().rows.len())
        }
    }

    // ---- Inside ----

    /// Stops a reply under way and saves what there is.
    fn leave(mut self: Pin<&mut Self>) {
        self.as_mut().stop();
        self.set_error(QString::default());
    }

    /// Shows `conversation` (None: a new, empty one), replacing the rows.
    fn show(mut self: Pin<&mut Self>, conversation: Option<Conversation>) {
        let (id, title) = conversation
            .as_ref()
            .map(|c| (c.id.clone(), c.title.clone()))
            .unwrap_or_default();
        self.as_mut().begin_reset_model();
        let rows: Vec<Row> = conversation
            .as_ref()
            .map(|c| c.messages.iter().map(Row::of).collect())
            .unwrap_or_default();
        let count = int(rows.len());
        {
            let mut rust = self.as_mut().rust_mut();
            rust.rows = rows;
            rust.conversation = conversation;
            rust.streaming = false;
        }
        self.as_mut().end_reset_model();
        self.as_mut().set_count(count);
        self.as_mut()
            .set_conversation_id(QString::from(id.as_str()));
        self.as_mut().set_title(QString::from(title.as_str()));
        self.update_retryable();
    }

    fn push(mut self: Pin<&mut Self>, message: Message) {
        let at = int(self.rust().rows.len());
        self.as_mut()
            .begin_insert_rows(&QModelIndex::default(), at, at);
        {
            let mut rust = self.as_mut().rust_mut();
            rust.rows.push(Row::of(&message));
            if let Some(c) = rust.conversation.as_mut() {
                c.messages.push(message);
            }
        }
        self.as_mut().end_insert_rows();
        self.as_mut().set_count(at + 1);
        self.update_retryable();
    }

    fn pop(mut self: Pin<&mut Self>) {
        let Some(last) = self.rust().rows.len().checked_sub(1) else {
            return;
        };
        let at = int(last);
        self.as_mut()
            .begin_remove_rows(&QModelIndex::default(), at, at);
        {
            let mut rust = self.as_mut().rust_mut();
            rust.rows.pop();
            if let Some(c) = rust.conversation.as_mut() {
                c.messages.pop();
            }
        }
        self.as_mut().end_remove_rows();
        self.as_mut().set_count(at);
        self.update_retryable();
    }

    fn update_retryable(self: Pin<&mut Self>) {
        let retryable = self
            .rust()
            .conversation
            .as_ref()
            .and_then(|c| c.messages.last())
            .is_some_and(|m| m.role == Role::User || m.failed);
        self.set_retryable(retryable);
    }

    /// The last row changed (a reply growing, or done).
    fn last_changed(mut self: Pin<&mut Self>) {
        let Some(last) = self.rust().rows.len().checked_sub(1) else {
            return;
        };
        let index = self.index(int(last), 0, &QModelIndex::default());
        self.as_mut()
            .data_changed(&index, &index, &QList::<i32>::default());
    }

    /// Starts a reply to the conversation as it stands.
    fn ask(mut self: Pin<&mut Self>) {
        let Some(backend) = self.rust().backend.clone() else {
            return;
        };
        let Some(messages) = self
            .rust()
            .conversation
            .as_ref()
            // A failed or empty reply is not part of what the model said.
            .map(|c| {
                c.messages
                    .iter()
                    .filter(|m| m.role == Role::User || (!m.failed && !m.text.is_empty()))
                    .cloned()
                    .collect::<Vec<_>>()
            })
        else {
            return;
        };
        let request = Request {
            model: self.model().to_string(),
            system_prompt: self.system_prompt().to_string(),
            messages,
        };
        // Saved with the user's message, before the reply's row (empty
        // until its text comes) is there.
        self.as_mut().save();
        self.as_mut().rust_mut().streaming = true;
        self.as_mut().push(Message::assistant(""));

        let cancel = Arc::new(AtomicBool::new(false));
        let generation = {
            let mut rust = self.as_mut().rust_mut();
            rust.generation += 1;
            rust.cancel = Some(cancel.clone());
            rust.generation
        };
        self.as_mut().set_generating(true);
        let qt = self.qt_thread();
        std::thread::spawn(move || {
            let mut pending = String::new();
            let mut sent = Instant::now();
            let mut rate = Rate::default();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                backend.complete(&request, &cancel, &mut |event| {
                    match event {
                        Event::Text(piece) => {
                            pending.push_str(piece);
                            rate.token();
                        }
                        Event::Speed(s) => rate.reported(s),
                    }
                    if sent.elapsed() >= BATCH {
                        sent = Instant::now();
                        let text = std::mem::take(&mut pending);
                        let speed = rate.speed();
                        let _ = qt.queue(move |chat| chat.append_reply(generation, &text, speed));
                    }
                })
            }));
            let speed = rate.speed();
            let error = match result {
                Ok(Ok(())) => None,
                Ok(Err(e)) => Some(e.to_string()),
                Err(_) => {
                    log::error!("the backend panicked");
                    Some("The backend stopped unexpectedly.".to_string())
                }
            };
            let _ = qt.queue(move |chat| {
                if chat.rust().generation == generation {
                    chat.finish_reply(pending, error, speed, false);
                }
            });
        });
    }

    fn append_reply(mut self: Pin<&mut Self>, generation: u64, text: &str, speed: Option<f64>) {
        if self.rust().generation != generation || (text.is_empty() && speed.is_none()) {
            return;
        }
        {
            let mut rust = self.as_mut().rust_mut();
            let Some(message) = rust
                .conversation
                .as_mut()
                .and_then(|c| c.messages.last_mut())
            else {
                return;
            };
            message.text.push_str(text);
            if speed.is_some() {
                message.speed = speed;
            }
            let row = Row::of(message);
            if let Some(last) = rust.rows.last_mut() {
                *last = row;
            }
        }
        self.last_changed();
    }

    /// The reply ended: done, `stopped`, or failed with `error`.
    fn finish_reply(
        mut self: Pin<&mut Self>,
        rest: String,
        mut error: Option<String>,
        speed: Option<f64>,
        stopped: bool,
    ) {
        // A backend that ends without a word, and without saying why.
        let nothing = rest.is_empty()
            && self
                .rust()
                .conversation
                .as_ref()
                .and_then(|c| c.messages.last())
                .is_some_and(|m| m.role == Role::Assistant && m.text.is_empty());
        if nothing && !stopped && error.is_none() {
            error = Some("The model sent an empty reply.".to_string());
        }
        {
            let mut rust = self.as_mut().rust_mut();
            rust.cancel = None;
            rust.streaming = false;
            if let Some(message) = rust
                .conversation
                .as_mut()
                .and_then(|c| c.messages.last_mut())
                .filter(|m| m.role == Role::Assistant)
            {
                message.text.push_str(&rest);
                message.failed = error.is_some();
                if speed.is_some() {
                    message.speed = speed;
                }
                let row = Row::of(message);
                if let Some(last) = rust.rows.last_mut() {
                    *last = row;
                }
            }
        }
        // A reply stopped before any of it came is no reply.
        let empty = self
            .rust()
            .conversation
            .as_ref()
            .and_then(|c| c.messages.last())
            .is_some_and(|m| m.role == Role::Assistant && m.text.is_empty() && !m.failed);
        if empty {
            self.as_mut().pop();
        } else {
            self.as_mut().last_changed();
        }
        if let Some(e) = &error {
            log::warn!("the reply failed: {e}");
            self.as_mut().set_error(QString::from(e.as_str()));
        }
        self.as_mut().set_generating(false);
        if let Some(c) = self.as_mut().rust_mut().conversation.as_mut() {
            c.touch();
        }
        self.as_mut().update_retryable();
        self.save();
    }

    /// Saves the conversation on the file thread, and moves it to the top of
    /// the list.
    fn save(self: Pin<&mut Self>) {
        let Some(c) = self.rust().conversation.clone() else {
            return;
        };
        if let Some(library) = &self.rust().library {
            let summary = c.summary();
            let _ = library.queue(move |lib| lib.upsert(summary));
        }
        if let Some(io) = &self.rust().io {
            io.run(move |store| {
                if let Err(e) = store.save(&c) {
                    log::warn!("cannot save conversation {}: {e}", c.id);
                }
            });
        }
    }
}
