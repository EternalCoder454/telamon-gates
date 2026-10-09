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
        /// The model server's options (Settings): 0 is automatic.
        #[qproperty(i32, gpu_layers, cxx_name = "gpuLayers")]
        #[qproperty(i32, context_size, cxx_name = "contextSize")]
        /// A llama-server elsewhere; "" runs one here.
        #[qproperty(QString, server_url, cxx_name = "serverUrl")]
        /// Where the backend's model files go; "" when it has no folder.
        #[qproperty(QString, models_folder, cxx_name = "modelsFolder")]
        /// The context the model ran with last, in tokens; 0 until known.
        #[qproperty(i32, active_context, cxx_name = "activeContext")]
        /// The open conversation's mode choice: "auto" or a mode's id.
        #[qproperty(QString, mode)]
        /// SystemOne is on (Settings).
        #[qproperty(bool, system_one, cxx_name = "systemOne")]
        /// SystemOne can pick: it is on, a decision model is there and
        /// llama-server is installed.
        #[qproperty(bool, system_one_ready, cxx_name = "systemOneReady")]
        /// The decision models in the models folder, and the one in use.
        #[qproperty(QStringList, decision_models, cxx_name = "decisionModels")]
        #[qproperty(QString, decision_model, cxx_name = "decisionModel")]
        /// The models here whose chat template takes tools (Agent mode).
        #[qproperty(QStringList, tool_models, cxx_name = "toolModels")]
        /// The open conversation's folder for Agent mode; "" for none yet.
        #[qproperty(QString, workspace)]
        /// The agent waits for the user to allow a change: what it is
        /// (a title, the text or command), and "write" or "run".
        #[qproperty(bool, approving)]
        #[qproperty(QString, approval_title, cxx_name = "approvalTitle")]
        #[qproperty(QString, approval_detail, cxx_name = "approvalDetail")]
        #[qproperty(QString, approval_kind, cxx_name = "approvalKind")]
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

        /// Saves the model server's options and applies them from the next
        /// reply (a running server stops, freeing the graphics card).
        #[qinvokable]
        #[cxx_name = "saveServerOptions"]
        fn save_server_options(
            self: Pin<&mut Chat>,
            gpu_layers: i32,
            context_size: i32,
            server_url: &QString,
        );

        /// Asks the backend again for its models.
        #[qinvokable]
        #[cxx_name = "refreshModels"]
        fn refresh_models(self: Pin<&mut Chat>);

        #[qinvokable]
        #[cxx_name = "dismissError"]
        fn dismiss_error(self: Pin<&mut Chat>);

        /// Pins the open conversation to a mode, or "auto" for SystemOne's
        /// pick per message.
        #[qinvokable]
        #[cxx_name = "chooseMode"]
        fn choose_mode(self: Pin<&mut Chat>, mode: &QString);

        #[qinvokable]
        #[cxx_name = "enableSystemOne"]
        fn enable_system_one(self: Pin<&mut Chat>, on: bool);

        #[qinvokable]
        #[cxx_name = "pickDecisionModel"]
        fn pick_decision_model(self: Pin<&mut Chat>, name: &QString);

        /// The folder Agent mode works in, for the open conversation.
        #[qinvokable]
        #[cxx_name = "chooseWorkspace"]
        fn choose_workspace(self: Pin<&mut Chat>, path: &QString);

        /// The answer to the change the agent waits on: 0 deny, 1 allow,
        /// 2 allow it and the rest of this reply's edits.
        #[qinvokable]
        #[cxx_name = "answerApproval"]
        fn answer_approval(self: Pin<&mut Chat>, choice: i32);

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
use gates_core::agent::{self, Approval};
use gates_core::backend::llama::{LocalModel, find_server, local_models};
use gates_core::conversation::ToolCall;
use gates_core::markdown::{self, Block};
use gates_core::modes::{self, Mode};
use gates_core::systemone::{self, SystemOne};
use gates_core::tools::Workspace;
use gates_core::{Backend, Conversation, Event, Message, Request, Role};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

/// How often streamed text is handed to the window: often enough to look
/// live, seldom enough that each batch is worth a redraw.
const BATCH: Duration = Duration::from_millis(33);

const FIRST_ROLE: i32 = 0x0100; // Qt::UserRole
const ROLES: [&str; 12] = [
    "role",      // "user" or "assistant"
    "text",      // the message as written (Markdown for a reply)
    "kinds",     // each block's kind: "prose" or "code"
    "contents",  // each block's rich text (prose) or plain text (code)
    "langs",     // each code block's language, "" for prose
    "streaming", // the reply is still coming in
    "failed",    // the reply stopped on an error
    "speed",     // tokens per second of a reply, 0 when not known
    "mode",      // the mode that wrote a reply ("story"), "" when none
    "picked",    // SystemOne picked that mode
    "summary",   // a tool result in one line ("Read src/main.rs")
    "hasCalls",  // a reply that asked for tools (Agent mode)
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
        if message.role != Role::Assistant {
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
    gpu_layers: i32,
    context_size: i32,
    server_url: QString,
    models_folder: QString,
    active_context: i32,
    mode: QString,
    system_one: bool,
    system_one_ready: bool,
    decision_models: QStringList,
    decision_model: QString,
    tool_models: QStringList,
    workspace: QString,
    approving: bool,
    approval_title: QString,
    approval_detail: QString,
    approval_kind: QString,

    conversation: Option<Conversation>,
    /// Where the agent waits for the user's answer.
    approval: Option<Sender<Approval>>,
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
    /// The decision model in use, while SystemOne can pick.
    system_one_model: Option<Arc<SystemOne>>,
    /// llama-server, which runs the decision model too.
    server_binary: Option<PathBuf>,
    /// The decision model chosen in Settings ("" for none yet): read once,
    /// then kept here as the file is written behind.
    chosen_decision: String,
    /// Bumped by each look for decision models: an older one drops.
    looking: u64,
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

/// A reply on its way to the window: its text in batches, and in Agent
/// mode its steps, results and the questions it asks.
struct Stream {
    qt: CxxQtThread<qobject::Chat>,
    generation: u64,
    pending: String,
    sent: Instant,
    rate: Rate,
    cancel: Arc<AtomicBool>,
}

impl Stream {
    fn flush(&mut self) {
        self.sent = Instant::now();
        let text = std::mem::take(&mut self.pending);
        let speed = self.rate.speed();
        let generation = self.generation;
        let _ = self
            .qt
            .queue(move |chat| chat.append_reply(generation, &text, speed));
    }
}

impl agent::Host for Stream {
    fn text(&mut self, piece: &str) {
        self.pending.push_str(piece);
        self.rate.token();
        if self.sent.elapsed() >= BATCH {
            self.flush();
        }
    }

    fn speed(&mut self, speed: f64) {
        self.rate.reported(speed);
    }

    fn calls(&mut self, calls: &[ToolCall]) {
        self.flush();
        let (generation, calls) = (self.generation, calls.to_vec());
        let _ = self
            .qt
            .queue(move |chat| chat.agent_calls(generation, calls));
    }

    fn approve(&mut self, call: &ToolCall, title: &str, detail: &str) -> Approval {
        let (tx, rx) = mpsc::channel();
        let kind = if call.name == "run_command" {
            "run"
        } else {
            "write"
        };
        let (generation, title, detail) = (self.generation, title.to_string(), detail.to_string());
        let _ = self
            .qt
            .queue(move |chat| chat.ask_approval(generation, title, detail, kind, tx));
        // Waits for the answer; Stop (or a reply left) is a no.
        loop {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(answer) => return answer,
                Err(RecvTimeoutError::Timeout) if !self.cancel.load(Ordering::Relaxed) => {}
                Err(_) => return Approval::Deny,
            }
        }
    }

    fn result(&mut self, message: Message) {
        let generation = self.generation;
        let _ = self
            .qt
            .queue(move |chat| chat.agent_result(generation, message));
    }

    fn next_turn(&mut self) {
        self.rate = Rate::default();
        let generation = self.generation;
        let _ = self.qt.queue(move |chat| chat.agent_next(generation));
    }
}

/// The mode for a reply: the one `pinned`, else what SystemOne (`picker`)
/// is sure `asked` wants, else the `previous` reply's (Chat to begin with).
/// True when SystemOne picked it.
fn pick_mode(
    pinned: Option<Mode>,
    picker: Option<&SystemOne>,
    asked: &str,
    previous: Mode,
) -> (Mode, bool) {
    if let Some(mode) = pinned {
        return (mode, false);
    }
    let Some(picker) = picker else {
        return (previous, false);
    };
    let started = Instant::now();
    match picker.pick_mode(asked) {
        Ok(c) => {
            log::info!(
                "SystemOne: {} ({:.2}) in {} ms",
                c.choice,
                c.confidence,
                started.elapsed().as_millis()
            );
            if c.confidence >= systemone::MIN_CONFIDENCE && modes::valid_choice(&c.choice) {
                (modes::mode(&c.choice), true)
            } else {
                (previous, false)
            }
        }
        // Never in the way: the reply comes as the last one did.
        Err(e) => {
            log::warn!("SystemOne: {e}");
            (previous, false)
        }
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
        let options = settings::backend_options();
        self.as_mut()
            .set_gpu_layers(i32::try_from(options.gpu_layers).unwrap_or(0));
        self.as_mut()
            .set_context_size(i32::try_from(options.context).unwrap_or(0));
        self.as_mut()
            .set_server_url(QString::from(options.server_url.as_str()));
        self.as_mut().set_mode(QString::from(modes::AUTO));
        self.as_mut()
            .set_system_one(settings::get(settings::SYSTEM_ONE) != "false");
        {
            let mut rust = self.as_mut().rust_mut();
            rust.server_binary = find_server();
            rust.chosen_decision = settings::get(settings::DECISION_MODEL);
        }
        let folder = self.rust().backend.as_ref().and_then(|b| b.models_folder());
        if let Some(folder) = folder {
            self.as_mut()
                .set_models_folder(QString::from(folder.to_string_lossy().as_ref()));
            // There from the start, so Open Folder always opens it.
            if let Some(io) = &self.rust().io {
                io.run(move |_| {
                    if let Err(e) = std::fs::create_dir_all(&folder) {
                        log::warn!("cannot make {}: {e}", folder.display());
                    }
                });
            }
        }
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
            let mut c = Conversation::new(text);
            // The mode and folder chosen before the first message.
            c.mode = self.mode().to_string();
            let workspace = self.workspace().to_string();
            c.workspace = (!workspace.is_empty()).then_some(workspace);
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

    pub fn save_server_options(
        mut self: Pin<&mut Self>,
        gpu_layers: i32,
        context_size: i32,
        server_url: &QString,
    ) {
        let options = gates_core::Options {
            gpu_layers: u32::try_from(gpu_layers).unwrap_or(0),
            context: u32::try_from(context_size).unwrap_or(0),
            server_url: server_url
                .to_string()
                .trim()
                .trim_end_matches('/')
                .to_string(),
        };
        self.as_mut()
            .set_gpu_layers(i32::try_from(options.gpu_layers).unwrap_or(0));
        self.as_mut()
            .set_context_size(i32::try_from(options.context).unwrap_or(0));
        self.as_mut()
            .set_server_url(QString::from(options.server_url.as_str()));
        if let Some(io) = &self.rust().io {
            let number = |n: u32| if n == 0 { String::new() } else { n.to_string() };
            settings::set(io, settings::GPU_LAYERS, number(options.gpu_layers));
            settings::set(io, settings::CONTEXT, number(options.context));
            settings::set(io, settings::SERVER_URL, options.server_url.clone());
        }
        let Some(backend) = self.rust().backend.clone() else {
            return;
        };
        // Here, not on a worker, so the backend gets them in the order made.
        backend.set_options(&options);
        self.as_mut()
            .set_backend_name(QString::from(backend.name().as_str()));
        self.refresh_models();
    }

    pub fn refresh_models(mut self: Pin<&mut Self>) {
        self.as_mut().refresh_decision_models();
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

    pub fn choose_mode(mut self: Pin<&mut Self>, mode: &QString) {
        // Not while a reply runs: an agent keeps the mode and folder it began with.
        if *self.generating() {
            return;
        }
        let mode = mode.to_string();
        if !modes::valid_choice(&mode) || *self.mode() == QString::from(mode.as_str()) {
            return;
        }
        self.as_mut().set_mode(QString::from(mode.as_str()));
        // A new chat keeps it until its first message makes the file.
        let saved = match self.as_mut().rust_mut().conversation.as_mut() {
            Some(c) => {
                c.mode = mode;
                true
            }
            None => false,
        };
        if saved {
            self.save();
        }
    }

    pub fn enable_system_one(mut self: Pin<&mut Self>, on: bool) {
        if *self.system_one() == on {
            return;
        }
        self.as_mut().set_system_one(on);
        if let Some(io) = &self.rust().io {
            settings::set(
                io,
                settings::SYSTEM_ONE,
                if on { String::new() } else { "false".into() },
            );
        }
        self.refresh_decision_models();
    }

    pub fn pick_decision_model(mut self: Pin<&mut Self>, name: &QString) {
        if let Some(io) = &self.rust().io {
            settings::set(io, settings::DECISION_MODEL, name.to_string());
        }
        self.as_mut().rust_mut().chosen_decision = name.to_string();
        self.as_mut().set_decision_model(name.clone());
        self.refresh_decision_models();
    }

    pub fn choose_workspace(self: Pin<&mut Self>, path: &QString) {
        if *self.generating() {
            return;
        }
        let path = path.to_string();
        let conversation = self.conversation_id().to_string();
        // Only a real folder, and not a too wide one (checked on a worker:
        // it may be on a slow disk); the tools check every path against it.
        let qt = self.qt_thread();
        std::thread::spawn(move || {
            let checked = Workspace::open(std::path::Path::new(&path))
                .map(|ws| ws.root().to_string_lossy().into_owned());
            let _ = qt.queue(move |chat| {
                if chat.conversation_id().to_string() != conversation {
                    return;
                }
                match checked {
                    Ok(root) => chat.use_workspace(root),
                    Err(e) => chat.set_error(QString::from(
                        format!("The agent can't work in {path}: {e}.").as_str(),
                    )),
                }
            });
        });
    }

    fn use_workspace(mut self: Pin<&mut Self>, path: String) {
        self.as_mut().set_workspace(QString::from(path.as_str()));
        let saved = match self.as_mut().rust_mut().conversation.as_mut() {
            Some(c) => {
                c.workspace = Some(path);
                true
            }
            None => false,
        };
        if saved {
            self.save();
        }
    }

    pub fn answer_approval(mut self: Pin<&mut Self>, choice: i32) {
        let answer = match choice {
            1 => Approval::Allow,
            2 => Approval::AllowEdits,
            _ => Approval::Deny,
        };
        if let Some(tx) = self.as_mut().rust_mut().approval.take() {
            let _ = tx.send(answer);
        }
        self.set_approving(false);
    }

    /// The agent asks whether a change may happen (`kind`: "write" or
    /// "run"); the answer goes to `tx`.
    fn ask_approval(
        mut self: Pin<&mut Self>,
        generation: u64,
        title: String,
        detail: String,
        kind: &str,
        tx: Sender<Approval>,
    ) {
        if self.rust().generation != generation {
            return;
        }
        self.as_mut().rust_mut().approval = Some(tx);
        self.as_mut()
            .set_approval_title(QString::from(title.as_str()));
        self.as_mut()
            .set_approval_detail(QString::from(detail.as_str()));
        self.as_mut().set_approval_kind(QString::from(kind));
        self.set_approving(true);
    }

    /// The agent's turn under way ended by asking for `calls`.
    fn agent_calls(mut self: Pin<&mut Self>, generation: u64, calls: Vec<ToolCall>) {
        if self.rust().generation != generation {
            return;
        }
        if let Some(m) = self
            .as_mut()
            .rust_mut()
            .conversation
            .as_mut()
            .and_then(|c| c.messages.last_mut())
            .filter(|m| m.role == Role::Assistant)
        {
            m.tool_calls = calls;
        }
        self.as_mut().last_changed();
        self.save();
    }

    /// A tool's result, from the agent.
    fn agent_result(mut self: Pin<&mut Self>, generation: u64, message: Message) {
        if self.rust().generation != generation {
            return;
        }
        self.as_mut().push(message);
        self.save();
    }

    /// The agent's next turn: a new, empty reply that streams.
    fn agent_next(self: Pin<&mut Self>, generation: u64) {
        if self.rust().generation != generation {
            return;
        }
        self.push(Message {
            mode: Some(modes::AGENT.id.to_string()),
            ..Message::assistant("")
        });
    }

    /// Looks for decision models in the models folder (on a worker) and
    /// sets up SystemOne with the chosen one: the saved choice, else Laya,
    /// which runs on the processor, else the first.
    fn refresh_decision_models(mut self: Pin<&mut Self>) {
        let Some(dir) = self.rust().backend.as_ref().and_then(|b| b.models_folder()) else {
            return;
        };
        let looking = {
            let mut rust = self.as_mut().rust_mut();
            rust.looking += 1;
            rust.looking
        };
        let qt = self.qt_thread();
        std::thread::spawn(move || {
            let (found, chat_models): (Vec<LocalModel>, Vec<LocalModel>) = local_models(&dir)
                .into_iter()
                .partition(|m| !m.info.decision.is_empty());
            // And which chat models can work as an agent.
            let tools: Vec<String> = chat_models
                .into_iter()
                .filter(|m| m.info.tools)
                .map(|m| m.name)
                .collect();
            let _ = qt.queue(move |mut chat| {
                if chat.rust().looking == looking {
                    let mut list = QStringList::default();
                    for name in &tools {
                        list.append(QString::from(name.as_str()));
                    }
                    chat.as_mut().set_tool_models(list);
                    chat.use_decision_models(found);
                }
            });
        });
    }

    fn use_decision_models(mut self: Pin<&mut Self>, found: Vec<LocalModel>) {
        let mut names = QStringList::default();
        for m in &found {
            names.append(QString::from(m.name.as_str()));
        }
        self.as_mut().set_decision_models(names);
        let saved = self.rust().chosen_decision.clone();
        let chosen = found
            .iter()
            .find(|m| m.name == saved)
            .or_else(|| found.iter().find(|m| m.info.decision == "laya"))
            .or_else(|| found.first());
        self.as_mut()
            .set_decision_model(QString::from(chosen.map(|m| m.name.as_str()).unwrap_or("")));
        let binary = self.rust().server_binary.clone();
        let wanted = match (chosen, binary, *self.system_one()) {
            (Some(model), Some(binary), true) => Some((model.clone(), binary)),
            _ => None,
        };
        let current = self
            .rust()
            .system_one_model
            .as_ref()
            .map(|s| s.name().to_string());
        let old = match wanted {
            Some((model, _)) if current.as_deref() == Some(model.name.as_str()) => None,
            Some((model, binary)) => {
                let log = gates_core::store::state_dir().join("systemone-server.log");
                self.as_mut()
                    .rust_mut()
                    .system_one_model
                    .replace(Arc::new(SystemOne::new(&binary, &model, log)))
            }
            None => self.as_mut().rust_mut().system_one_model.take(),
        };
        // Dropping the last one stops its server, which waits for it to
        // quit: not on the GUI thread.
        if let Some(old) = old {
            std::thread::spawn(move || drop(old));
        }
        let ready = self.rust().system_one_model.is_some();
        self.set_system_one_ready(ready);
    }

    /// The reply under way is written in `mode`, which SystemOne `picked`.
    fn set_reply_mode(mut self: Pin<&mut Self>, generation: u64, mode: &str, picked: bool) {
        if self.rust().generation != generation {
            return;
        }
        {
            let mut rust = self.as_mut().rust_mut();
            let Some(message) = rust
                .conversation
                .as_mut()
                .and_then(|c| c.messages.last_mut())
                .filter(|m| m.role == Role::Assistant)
            else {
                return;
            };
            message.mode = Some(mode.to_string());
            message.picked = picked;
        }
        self.last_changed();
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
            8 => QVariant::from(&QString::from(message.mode.as_deref().unwrap_or(""))),
            9 => QVariant::from(&message.picked),
            10 => QVariant::from(&QString::from(message.summary.as_deref().unwrap_or(""))),
            11 => QVariant::from(&!message.tool_calls.is_empty()),
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
        let workspace = conversation
            .as_ref()
            .and_then(|c| c.workspace.clone())
            .unwrap_or_default();
        self.as_mut()
            .set_workspace(QString::from(workspace.as_str()));
        let mode = conversation
            .as_ref()
            .map(|c| c.mode.clone())
            .filter(|m| modes::valid_choice(m))
            .unwrap_or_else(|| modes::AUTO.to_string());
        self.as_mut().set_mode(QString::from(mode.as_str()));
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
        // The row before is no longer the last: it stops showing as
        // streaming (an agent's turn, once its results come).
        if at > 0 {
            let before = self.index(at - 1, 0, &QModelIndex::default());
            self.as_mut()
                .data_changed(&before, &before, &QList::<i32>::default());
        }
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
            // A failed or empty reply is not part of what the model said;
            // tool calls and their results are, every one answered.
            .map(|c| {
                agent::repair(
                    c.messages
                        .iter()
                        .filter(|m| {
                            m.role != Role::Assistant
                                || !m.tool_calls.is_empty()
                                || (!m.failed && !m.text.is_empty())
                        })
                        .cloned()
                        .collect::<Vec<_>>(),
                )
            })
        else {
            return;
        };
        // The mode: the one pinned, or in Auto SystemOne's pick (made on
        // the worker), else Chat.
        let choice = self.mode().to_string();
        let pinned = (choice != modes::AUTO).then(|| modes::mode(&choice));
        let picker = match pinned {
            None => self.rust().system_one_model.clone(),
            Some(_) => None,
        };
        let asked = messages
            .iter()
            .rev()
            .find(|m| m.role == Role::User)
            .map(|m| m.text.clone())
            .unwrap_or_default();
        // When SystemOne isn't sure ("continue", "make it darker"), the
        // conversation stays in the mode its last reply had.
        // Never Agent: outside Agent mode there is no folder and no tools.
        let previous = messages
            .iter()
            .rev()
            .find_map(|m| m.mode.as_deref())
            .map(modes::mode)
            .filter(|m| m.id != modes::AGENT.id)
            .unwrap_or(modes::CHAT);
        let user_prompt = self.system_prompt().to_string();
        // Agent mode works in the conversation's folder, which it needs
        // (opened on the worker: a folder can be on a slow disk).
        let workspace = if pinned.is_some_and(|m| m.id == modes::AGENT.id) {
            let folder = self.workspace().to_string();
            if folder.is_empty() {
                self.set_error(QString::from(
                    "Choose a folder for the agent to work in first.",
                ));
                return;
            }
            Some(PathBuf::from(folder))
        } else {
            None
        };
        let mut request = Request {
            model: self.model().to_string(),
            system_prompt: String::new(),
            messages,
            sampling: None,
            tools: Vec::new(),
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
            let (mode, picked) = pick_mode(pinned, picker.as_deref(), &asked, previous);
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let id = mode.id;
            let _ = qt.queue(move |chat| chat.set_reply_mode(generation, id, picked));
            request.system_prompt = modes::system_prompt(&mode, &user_prompt);
            request.sampling = mode.sampling;
            let mut stream = Stream {
                qt: qt.clone(),
                generation,
                pending: String::new(),
                sent: Instant::now(),
                rate: Rate::default(),
                cancel: cancel.clone(),
            };
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                match &workspace {
                    Some(folder) => {
                        let ws = Workspace::open(folder).map_err(|e| {
                            gates_core::BackendError::Other(format!(
                                "The agent can't work in {}: {e}.",
                                folder.display()
                            ))
                        })?;
                        request
                            .system_prompt
                            .push_str(&format!("\n\nThe workspace is {}.", ws.root().display()));
                        agent::run(backend.as_ref(), request, &ws, &cancel, &mut stream)
                    }
                    None => backend.complete(&request, &cancel, &mut |event| match event {
                        Event::Text(piece) => agent::Host::text(&mut stream, piece),
                        Event::Speed(s) => stream.rate.reported(s),
                        // Tools weren't offered: nothing to run.
                        Event::ToolCalls(_) => {}
                    }),
                }
            }));
            let pending = std::mem::take(&mut stream.pending);
            let speed = stream.rate.speed();
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
            // An agent waiting on an answer gets a no (the sender goes).
            rust.approval = None;
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
        self.as_mut().set_approving(false);
        if let Some(c) = self.as_mut().rust_mut().conversation.as_mut() {
            c.touch();
        }
        self.as_mut().update_retryable();
        let context = self
            .rust()
            .backend
            .as_ref()
            .and_then(|b| b.context_size())
            .map_or(0, |n| i32::try_from(n).unwrap_or(i32::MAX));
        self.as_mut().set_active_context(context);
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
