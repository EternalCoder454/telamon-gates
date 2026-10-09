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
        type QList_f64 = cxx_qt_lib::QList<f64>;
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
        /// Something the backend wants the user to know about the reply
        /// under way or just done, short of an error ("loaded with a smaller
        /// context"); plain text, "" when nothing.
        #[qproperty(QString, notice)]
        /// The backend is the built-in demo, which answers with samples.
        #[qproperty(bool, demo)]
        /// The first-run check found no model server (telamon-llama), or no
        /// graphics device for it; both false until the check is done, and
        /// when a server address is set.
        #[qproperty(bool, server_missing, cxx_name = "serverMissing")]
        #[qproperty(bool, no_gpu, cxx_name = "noGpu")]
        #[qproperty(QString, backend_name, cxx_name = "backendName")]
        /// The models to pick from, and the one picked.
        #[qproperty(QStringList, models)]
        #[qproperty(QString, model)]
        /// The model for Code and Agent mode; "" for the same as `model`.
        #[qproperty(QString, code_model, cxx_name = "codeModel")]
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
        /// The context cache at 8 bits (Settings).
        #[qproperty(bool, small_cache, cxx_name = "smallCache")]
        /// Where the backend's model files go; "" when it has no folder.
        #[qproperty(QString, models_folder, cxx_name = "modelsFolder")]
        /// The context the model ran with last, in tokens; 0 until known.
        #[qproperty(i32, active_context, cxx_name = "activeContext")]
        /// The open conversation's mode choice: "auto" or a mode's id.
        #[qproperty(QString, mode)]
        /// Files to go with the next message: their names, and for a
        /// picture where Gates keeps it ("" for text).
        #[qproperty(QStringList, pending_names, cxx_name = "pendingNames")]
        #[qproperty(QStringList, pending_images, cxx_name = "pendingImages")]
        /// Every mode, in order: the built-ins, then the user's own. Their
        /// ids, names, prompts and temperatures (-1: the model's own).
        #[qproperty(QStringList, mode_ids, cxx_name = "modeIds")]
        #[qproperty(QStringList, mode_names, cxx_name = "modeNames")]
        #[qproperty(QStringList, mode_prompts, cxx_name = "modePrompts")]
        #[qproperty(QList_f64, mode_temps, cxx_name = "modeTemps")]
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
        /// That folder is the conversation's own sandbox (Gates' folder),
        /// not one of the user's.
        #[qproperty(bool, sandboxed)]
        /// What an agent's sandboxed commands may reach (Settings), and
        /// whether they can run at all (bubblewrap is there).
        #[qproperty(bool, agent_network, cxx_name = "agentNetwork")]
        #[qproperty(bool, agent_home, cxx_name = "agentHome")]
        #[qproperty(bool, commands_available, cxx_name = "commandsAvailable")]
        /// The agent waits for the user to allow a change: what it is
        /// (a title, the text or command), and "write" or "run".
        #[qproperty(bool, approving)]
        #[qproperty(QString, approval_title, cxx_name = "approvalTitle")]
        #[qproperty(QString, approval_detail, cxx_name = "approvalDetail")]
        #[qproperty(QString, approval_kind, cxx_name = "approvalKind")]
        /// Web search (Settings → Web Search): on, the provider's id, a
        /// SearXNG address, whether a key for the provider is in the system
        /// keyring (never the key itself), whether a keyring answers and
        /// why not, and whether replies can use it now (and a sentence on
        /// what is missing when they can't).
        #[qproperty(bool, web_search, cxx_name = "webSearch")]
        #[qproperty(QString, web_provider, cxx_name = "webProvider")]
        #[qproperty(QString, web_url, cxx_name = "webUrl")]
        #[qproperty(bool, web_key_saved, cxx_name = "webKeySaved")]
        #[qproperty(bool, keyring_available, cxx_name = "keyringAvailable")]
        #[qproperty(QString, keyring_note, cxx_name = "keyringNote")]
        #[qproperty(bool, web_ready, cxx_name = "webReady")]
        #[qproperty(QString, web_note, cxx_name = "webNote")]
        /// Why Deep Research can't be used now ("" when it can).
        #[qproperty(QString, research_note, cxx_name = "researchNote")]
        /// A key is being saved or a search tried; the outcome, in words.
        #[qproperty(bool, web_testing, cxx_name = "webTesting")]
        #[qproperty(QString, web_test_result, cxx_name = "webTestResult")]
        #[qproperty(bool, web_test_ok, cxx_name = "webTestOk")]
        /// What the reply under way is doing ("Searching: …"); "" when
        /// nothing in particular.
        #[qproperty(QString, status)]
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

        /// Opens a new conversation with the messages up to `row`.
        #[qinvokable]
        #[cxx_name = "branchFrom"]
        fn branch_from(self: Pin<&mut Chat>, row: i32);

        /// Your message at `row` becomes `text`; what came after it goes,
        /// and a new reply comes.
        #[qinvokable]
        #[cxx_name = "editMessage"]
        fn edit_message(self: Pin<&mut Chat>, row: i32, text: &QString);

        /// Saves the open conversation to `path`, as "markdown" or "json".
        #[qinvokable]
        #[cxx_name = "exportTo"]
        fn export_to(self: Pin<&mut Chat>, path: &QString, format: &QString);

        #[qinvokable]
        #[cxx_name = "pickModel"]
        fn pick_model(self: Pin<&mut Chat>, name: &QString);

        #[qinvokable]
        #[cxx_name = "pickCodeModel"]
        fn pick_code_model(self: Pin<&mut Chat>, name: &QString);

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

        #[qinvokable]
        #[cxx_name = "useSmallCache"]
        fn set_small_cache_option(self: Pin<&mut Chat>, on: bool);

        /// The user is writing `text`: gets the model ready, and in Auto has
        /// SystemOne pick the mode now, so Send waits for neither.
        #[qinvokable]
        fn prepare(self: Pin<&mut Chat>, text: &QString);

        /// Asks the backend again for its models.
        #[qinvokable]
        #[cxx_name = "refreshModels"]
        fn refresh_models(self: Pin<&mut Chat>);

        #[qinvokable]
        #[cxx_name = "dismissError"]
        fn dismiss_error(self: Pin<&mut Chat>);

        #[qinvokable]
        #[cxx_name = "dismissNotice"]
        fn dismiss_notice(self: Pin<&mut Chat>);

        /// Pins the open conversation to a mode, or "auto" for SystemOne's
        /// pick per message.
        #[qinvokable]
        #[cxx_name = "chooseMode"]
        fn choose_mode(self: Pin<&mut Chat>, mode: &QString);

        /// Saves a mode: a built-in's prompt and temperature, or one of the
        /// user's ("" for a new one). `temperature` below 0: the model's own.
        #[qinvokable]
        #[cxx_name = "saveMode"]
        fn save_mode(
            self: Pin<&mut Chat>,
            id: &QString,
            name: &QString,
            prompt: &QString,
            temperature: f64,
        );

        /// Deletes one of the user's modes, or puts a built-in back.
        #[qinvokable]
        #[cxx_name = "deleteMode"]
        fn delete_mode(self: Pin<&mut Chat>, id: &QString);

        /// Reads files to send with the next message (on a worker).
        #[qinvokable]
        #[cxx_name = "attachFiles"]
        fn attach_files(self: Pin<&mut Chat>, paths: &QStringList);

        #[qinvokable]
        #[cxx_name = "removeAttachment"]
        fn remove_attachment(self: Pin<&mut Chat>, index: i32);

        #[qinvokable]
        #[cxx_name = "enableSystemOne"]
        fn enable_system_one(self: Pin<&mut Chat>, on: bool);

        #[qinvokable]
        #[cxx_name = "pickDecisionModel"]
        fn pick_decision_model(self: Pin<&mut Chat>, name: &QString);

        /// The folder Agent mode works in, for the open conversation: one
        /// of the user's, chosen on purpose.
        #[qinvokable]
        #[cxx_name = "chooseWorkspace"]
        fn choose_workspace(self: Pin<&mut Chat>, path: &QString);

        /// Back to the conversation's own sandbox folder.
        #[qinvokable]
        #[cxx_name = "useSandbox"]
        fn use_sandbox(self: Pin<&mut Chat>);

        #[qinvokable]
        #[cxx_name = "setAgentAccess"]
        fn set_agent_access(self: Pin<&mut Chat>, network: bool, home: bool);

        #[qinvokable]
        #[cxx_name = "enableWebSearch"]
        fn enable_web_search(self: Pin<&mut Chat>, on: bool);

        /// "brave", "tavily" or "searxng".
        #[qinvokable]
        #[cxx_name = "pickWebProvider"]
        fn pick_web_provider(self: Pin<&mut Chat>, id: &QString);

        #[qinvokable]
        #[cxx_name = "saveWebUrl"]
        fn save_web_url(self: Pin<&mut Chat>, url: &QString);

        /// Keeps the provider's API key in the system keyring (nowhere
        /// else); says so in `webTestResult` when there is none.
        #[qinvokable]
        #[cxx_name = "saveWebKey"]
        fn save_web_key(self: Pin<&mut Chat>, key: &QString);

        #[qinvokable]
        #[cxx_name = "removeWebKey"]
        fn remove_web_key(self: Pin<&mut Chat>);

        /// Tries one search with the provider as set.
        #[qinvokable]
        #[cxx_name = "testWebSearch"]
        fn test_web_search(self: Pin<&mut Chat>);

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
use gates_core::applog;
use gates_core::backend::llama::{LocalModel, find_server, local_models};
use gates_core::conversation::ToolCall;
use gates_core::markdown::{self, Block};
use gates_core::modes::{self, Active, Library, Preset};
use gates_core::preflight;
use gates_core::systemone::{self, SystemOne};
use gates_core::tools::Workspace;
use gates_core::web::{self, KeyStore, Provider, Setup};
use gates_core::{Backend, Conversation, Event, Message, Request, Role};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How often streamed text is handed to the window: often enough to look
/// live, seldom enough that each batch is worth a redraw.
const BATCH: Duration = Duration::from_millis(33);

const FIRST_ROLE: i32 = 0x0100; // Qt::UserRole
const ROLES: [&str; 14] = [
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
    "files",     // the names of files sent with your message
    "images",    // where Gates keeps its pictures ("" for a text file)
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
    notice: QString,
    demo: bool,
    server_missing: bool,
    no_gpu: bool,
    backend_name: QString,
    models: QStringList,
    model: QString,
    code_model: QString,
    system_prompt: QString,
    count: i32,
    retryable: bool,
    user_name: QString,
    gpu_layers: i32,
    context_size: i32,
    server_url: QString,
    small_cache: bool,
    models_folder: QString,
    active_context: i32,
    mode: QString,
    pending_names: QStringList,
    pending_images: QStringList,
    /// The files read for the next message.
    pending: Vec<gates_core::conversation::Attachment>,
    mode_ids: QStringList,
    mode_names: QStringList,
    mode_prompts: QStringList,
    mode_temps: QList<f64>,
    system_one: bool,
    system_one_ready: bool,
    decision_models: QStringList,
    decision_model: QString,
    tool_models: QStringList,
    workspace: QString,
    sandboxed: bool,
    agent_network: bool,
    agent_home: bool,
    commands_available: bool,
    approving: bool,
    approval_title: QString,
    approval_detail: QString,
    approval_kind: QString,
    web_search: bool,
    web_provider: QString,
    web_url: QString,
    web_key_saved: bool,
    keyring_available: bool,
    keyring_note: QString,
    web_ready: bool,
    web_note: QString,
    research_note: QString,
    web_testing: bool,
    web_test_result: QString,
    web_test_ok: bool,
    status: QString,

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
    /// The modes as the user has them (`modes.json`).
    modes: Arc<Library>,
    /// The decision model chosen in Settings ("" for none yet): read once,
    /// then kept here as the file is written behind.
    chosen_decision: String,
    /// Bumped by each look for decision models: an older one drops.
    looking: u64,
    /// The same for the backend's model list.
    listing: u64,
    /// Whether commands' sandbox was tried yet (`check_sandbox`).
    sandbox_checked: bool,
    /// Which model was last warmed up (`prepare`), and when.
    warmed: Option<(String, Instant)>,
    /// SystemOne's pick for the text being written: the text, the mode,
    /// and whether SystemOne picked it.
    prepick: Option<(String, Active, bool)>,
    /// Bumped by each `prepare`: an older pick drops.
    preparing: u64,
    /// Where web search keys are kept: the system keyring.
    pub web_keys: Option<Arc<dyn KeyStore>>,
    /// The key of the provider in use, once read from the keyring.
    web_cache: Arc<Mutex<Option<String>>>,
    /// Which providers have a key in the keyring (`Provider::ALL`'s order),
    /// as the settings file says.
    key_saved: [bool; 3],
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
    /// The mode of the reply, for the turns after the first.
    mode: String,
    picked: bool,
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
        let (generation, mode, picked) = (self.generation, self.mode.clone(), self.picked);
        let _ = self
            .qt
            .queue(move |chat| chat.agent_next(generation, &mode, picked));
    }

    fn status(&mut self, line: &str) {
        let (generation, line) = (self.generation, line.to_string());
        let _ = self
            .qt
            .queue(move |chat| chat.show_status(generation, &line));
    }

    fn notice(&mut self, text: &str) {
        let (generation, text) = (self.generation, text.to_string());
        let _ = self
            .qt
            .queue(move |chat| chat.show_notice(generation, &text));
    }

    fn replace(&mut self, text: &str) {
        // What came so far first, so the order of batches holds.
        self.flush();
        let (generation, text) = (self.generation, text.to_string());
        let _ = self
            .qt
            .queue(move |chat| chat.replace_reply(generation, &text));
    }
}

/// The mode for a reply: the one `pinned`, else what SystemOne (`picker`)
/// is sure `asked` wants, else the `previous` reply's (Chat to begin with).
/// True when SystemOne picked it.
fn pick_mode(
    pinned: Option<Active>,
    picker: Option<&SystemOne>,
    asked: &str,
    previous: Active,
    library: &Library,
) -> (Active, bool) {
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
                (library.resolve(&c.choice), true)
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

/// Whether `folder` is one of Gates' own sandbox folders.
fn is_sandbox(folder: &str) -> bool {
    std::path::Path::new(folder).starts_with(gates_core::store::data_dir().join("workspaces"))
}

/// Where the user's modes are kept.
fn modes_file() -> PathBuf {
    gates_core::store::data_dir().join("modes.json")
}

/// A new id for one of the user's modes.
fn new_mode_id() -> String {
    format!("my-{:x}", gates_core::conversation::now_ms())
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
            .set_code_model(QString::from(settings::get(settings::CODE_MODEL).as_str()));
        self.as_mut()
            .set_user_name(QString::from(crate::user::first_name().as_str()));
        self.as_mut().set_system_prompt(QString::from(
            settings::get(settings::SYSTEM_PROMPT).as_str(),
        ));
        let options = settings::backend_options();
        self.as_mut().set_small_cache(options.small_cache);
        self.as_mut()
            .set_gpu_layers(i32::try_from(options.gpu_layers).unwrap_or(0));
        self.as_mut()
            .set_context_size(i32::try_from(options.context).unwrap_or(0));
        self.as_mut()
            .set_server_url(QString::from(options.server_url.as_str()));
        self.as_mut().set_mode(QString::from(modes::AUTO));
        // Read once at start, like the settings (a small file).
        let library = Library::load(&modes_file());
        self.as_mut().use_modes(library);
        self.as_mut()
            .set_system_one(settings::get(settings::SYSTEM_ONE) != "false");
        self.as_mut()
            .set_agent_network(settings::get(settings::AGENT_NETWORK) == "true");
        self.as_mut()
            .set_agent_home(settings::get(settings::AGENT_HOME) == "true");
        self.as_mut().start_web();
        // Assumed until Agent mode first shows (`check_sandbox`): trying
        // bubblewrap runs it twice, which a launch needn't pay for.
        self.as_mut().set_commands_available(true);
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
        self.as_mut().first_run_check();
        self.refresh_models();
    }

    /// Looks for the model server and a graphics device on a worker, logs
    /// what it finds (`applog`), and sets `serverMissing` and `noGpu` for the
    /// banners. With a server address set, neither matters.
    fn first_run_check(self: Pin<&mut Self>) {
        let remote = !self.server_url().is_empty();
        let qt = self.qt_thread();
        std::thread::spawn(move || {
            applog::info(&format!(
                "Telamon Gates {} starting",
                env!("CARGO_PKG_VERSION")
            ));
            if remote {
                applog::info("a model server address is set: not looking for a local one");
                return;
            }
            let report = preflight::check(&preflight::Where::system());
            if report.server_missing() || report.no_gpu() {
                applog::warn(&report.summary());
            } else {
                applog::info(&report.summary());
            }
            let _ = qt.queue(move |mut chat| {
                chat.as_mut().set_server_missing(report.server_missing());
                chat.set_no_gpu(report.no_gpu());
            });
        });
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
            let (result, report) = store.load_reporting(&id);
            let _ = qt.queue(move |mut chat| {
                // A file set aside or restored on the way is said once, in
                // the banner, even if the user has moved on.
                if !report.is_empty()
                    && let Some(library) = &chat.rust().library
                {
                    let _ = library.queue(move |lib| lib.note(&report));
                }
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
        let files = !self.rust().pending.is_empty();
        if (text.is_empty() && !files) || *self.generating() || *self.loading() {
            return false;
        }
        let title = if text.is_empty() {
            self.rust()
                .pending
                .first()
                .map(|a| a.name.clone())
                .unwrap_or_default()
        } else {
            text.to_string()
        };
        let text = text.to_string();
        if self.rust().conversation.is_none() {
            let mut c = Conversation::new(&title);
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
        let attachments = std::mem::take(&mut self.as_mut().rust_mut().pending);
        self.as_mut().show_pending();
        self.as_mut().push(Message {
            attachments,
            ..Message::user(text)
        });
        self.ask();
        true
    }

    pub fn attach_files(self: Pin<&mut Self>, paths: &QStringList) {
        let room = gates_core::attach::MAX_FILES.saturating_sub(self.rust().pending.len());
        let paths: Vec<PathBuf> = paths
            .iter()
            .take(room)
            .map(|p| PathBuf::from(p.to_string()))
            .collect();
        if paths.is_empty() {
            return;
        }
        let qt = self.qt_thread();
        std::thread::spawn(move || {
            let store = gates_core::store::attachments_dir();
            let read: Vec<_> = paths
                .iter()
                .map(|p| gates_core::attach::read(p, &store))
                .collect();
            let _ = qt.queue(move |mut chat| {
                let mut errors = Vec::new();
                for r in read {
                    match r {
                        Ok(a) if chat.rust().pending.len() < gates_core::attach::MAX_FILES => {
                            chat.as_mut().rust_mut().pending.push(a);
                        }
                        Ok(_) => {}
                        Err(e) => errors.push(e),
                    }
                }
                chat.as_mut().show_pending();
                if !errors.is_empty() {
                    chat.set_error(QString::from(errors.join(" ").as_str()));
                }
            });
        });
    }

    pub fn remove_attachment(mut self: Pin<&mut Self>, index: i32) {
        if let Ok(i) = usize::try_from(index)
            && i < self.rust().pending.len()
        {
            self.as_mut().rust_mut().pending.remove(i);
            self.show_pending();
        }
    }

    fn show_pending(mut self: Pin<&mut Self>) {
        let (mut names, mut images) = (QStringList::default(), QStringList::default());
        for a in &self.rust().pending {
            names.append(QString::from(a.name.as_str()));
            images.append(QString::from(a.image.as_deref().unwrap_or("")));
        }
        self.as_mut().set_pending_names(names);
        self.set_pending_images(images);
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
        // Back to your last message: a reply, and an agent's steps with it.
        if last != Role::User {
            while self
                .rust()
                .conversation
                .as_ref()
                .and_then(|c| c.messages.last())
                .is_some_and(|m| m.role != Role::User)
            {
                self.as_mut().pop();
            }
        }
        if let Some(c) = self.as_mut().rust_mut().conversation.as_mut() {
            c.touch();
        }
        self.ask();
    }

    pub fn branch_from(mut self: Pin<&mut Self>, row: i32) {
        if *self.loading() {
            return;
        }
        let Some(branch) = usize::try_from(row).ok().and_then(|row| {
            self.rust()
                .conversation
                .as_ref()
                .filter(|c| row < c.messages.len())
                .map(|c| c.branch(row))
        }) else {
            return;
        };
        self.as_mut().leave();
        self.as_mut().rust_mut().opening += 1;
        self.as_mut().show(Some(branch));
        self.save();
    }

    pub fn edit_message(mut self: Pin<&mut Self>, row: i32, text: &QString) {
        let text = text.to_string();
        let text = text.trim();
        if text.is_empty() || *self.generating() || *self.loading() {
            return;
        }
        let Ok(row) = usize::try_from(row) else {
            return;
        };
        let mine = self
            .rust()
            .conversation
            .as_ref()
            .and_then(|c| c.messages.get(row))
            .is_some_and(|m| m.role == Role::User);
        if !mine {
            return;
        }
        // The files sent with it stay with it.
        let attachments = self
            .rust()
            .conversation
            .as_ref()
            .and_then(|c| c.messages.get(row))
            .map(|m| m.attachments.clone())
            .unwrap_or_default();
        while self.rust().rows.len() > row {
            self.as_mut().pop();
        }
        self.as_mut().set_error(QString::default());
        if let Some(c) = self.as_mut().rust_mut().conversation.as_mut() {
            c.touch();
        }
        self.as_mut().push(Message {
            attachments,
            ..Message::user(text)
        });
        self.ask();
    }

    pub fn export_to(self: Pin<&mut Self>, path: &QString, format: &QString) {
        let (Some(c), Some(io)) = (self.rust().conversation.clone(), self.rust().io.clone()) else {
            return;
        };
        let path = PathBuf::from(path.to_string());
        let text = if format.to_string() == "json" {
            gates_core::export::json(&c)
        } else {
            gates_core::export::markdown(&c)
        };
        let qt = self.qt_thread();
        io.run(move |_| {
            // Whole or not at all: a file beside it, then a rename.
            let tmp = path.with_extension("gates-export.tmp");
            let result = std::fs::write(&tmp, &text).and_then(|()| std::fs::rename(&tmp, &path));
            if let Err(e) = result {
                let _ = std::fs::remove_file(&tmp);
                let message = format!("Couldn't save {}: {e}.", path.display());
                let _ = qt.queue(move |chat| chat.set_error(QString::from(message.as_str())));
            }
        });
    }

    pub fn pick_model(mut self: Pin<&mut Self>, name: &QString) {
        if *self.model() == *name {
            return;
        }
        self.as_mut().set_model(name.clone());
        if let Some(io) = &self.rust().io {
            settings::set(io, settings::MODEL, name.to_string());
        }
        self.update_web();
    }

    pub fn pick_code_model(mut self: Pin<&mut Self>, name: &QString) {
        self.as_mut().set_code_model(name.clone());
        if let Some(io) = &self.rust().io {
            settings::set(io, settings::CODE_MODEL, name.to_string());
        }
    }

    /// The model for replies in `mode`: the code model for Code and
    /// Agent when one is set, else the chat model.
    fn model_for(&self, mode: &str) -> String {
        let code = self.code_model().to_string();
        let coding = mode == modes::CODE.id || mode == modes::AGENT.id;
        if coding
            && !code.is_empty()
            && self.models().contains(
                &QString::from(code.as_str()),
                cxx_qt_lib::CaseSensitivity::CaseSensitive,
            )
        {
            code
        } else {
            self.model().to_string()
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
            small_cache: *self.small_cache(),
        };
        self.as_mut()
            .set_gpu_layers(i32::try_from(options.gpu_layers).unwrap_or(0));
        self.as_mut()
            .set_context_size(i32::try_from(options.context).unwrap_or(0));
        self.as_mut()
            .set_server_url(QString::from(options.server_url.as_str()));
        self.as_mut().update_web();
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

    pub fn prepare(mut self: Pin<&mut Self>, text: &QString) {
        let text = text.to_string().trim().to_string();
        if text.is_empty() || *self.generating() {
            return;
        }
        // The mode's model (the code model in Code and Agent), loading while
        // the user writes: once a minute at most per model, since after that
        // it is loaded, or idle-stopped minutes later.
        let model = self.model_for(&self.mode().to_string());
        let due = self
            .rust()
            .warmed
            .as_ref()
            .is_none_or(|(warm, t)| *warm != model || t.elapsed() > Duration::from_secs(60));
        if due && let Some(backend) = self.rust().backend.clone() {
            self.as_mut().rust_mut().warmed = Some((model.clone(), Instant::now()));
            std::thread::spawn(move || backend.warm(&model));
        }
        // SystemOne's pick, made now (Auto only).
        let picker = match self.mode().to_string() == modes::AUTO {
            true => self.rust().system_one_model.clone(),
            false => None,
        };
        let Some(picker) = picker else {
            return;
        };
        let library = self.rust().modes.clone();
        let previous = self
            .rust()
            .conversation
            .as_ref()
            .and_then(|c| c.messages.iter().rev().find_map(|m| m.mode.clone()))
            .filter(|id| id != modes::AGENT.id && id != modes::DEEP_RESEARCH.id)
            .map(|id| library.resolve(&id))
            .unwrap_or_else(|| library.resolve(modes::CHAT.id));
        let preparing = {
            let mut rust = self.as_mut().rust_mut();
            rust.preparing += 1;
            rust.preparing
        };
        let qt = self.qt_thread();
        std::thread::spawn(move || {
            let (mode, picked) = pick_mode(None, Some(&picker), &text, previous, &library);
            let _ = qt.queue(move |mut chat| {
                if chat.rust().preparing == preparing {
                    chat.as_mut().rust_mut().prepick = Some((text, mode, picked));
                }
            });
        });
    }

    pub fn set_small_cache_option(mut self: Pin<&mut Self>, on: bool) {
        if *self.small_cache() == on {
            return;
        }
        self.as_mut().set_small_cache(on);
        if let Some(io) = &self.rust().io {
            settings::set(
                io,
                settings::SMALL_CACHE,
                if on { "true" } else { "false" }.into(),
            );
        }
        let (gpu, ctx, url) = (
            *self.gpu_layers(),
            *self.context_size(),
            self.server_url().clone(),
        );
        self.save_server_options(gpu, ctx, &url);
    }

    pub fn refresh_models(mut self: Pin<&mut Self>) {
        self.as_mut().refresh_decision_models();
        let Some(backend) = self.rust().backend.clone() else {
            return;
        };
        // A slow answer can't replace a newer one.
        let listing = {
            let mut rust = self.as_mut().rust_mut();
            rust.listing += 1;
            rust.listing
        };
        let qt = self.qt_thread();
        std::thread::spawn(move || {
            let result = backend.models();
            let _ = qt.queue(move |mut chat| match result {
                _ if chat.rust().listing != listing => {}
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
                        chat.as_mut().set_model(QString::from(first.as_str()));
                    }
                    chat.update_web();
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

    pub fn dismiss_notice(self: Pin<&mut Self>) {
        self.set_notice(QString::default());
    }

    /// The backend's notice for the reply of `generation` (a late one of an
    /// older reply is dropped).
    fn show_notice(self: Pin<&mut Self>, generation: u64, text: &str) {
        if self.rust().generation == generation {
            self.set_notice(QString::from(text));
        }
    }

    pub fn choose_mode(mut self: Pin<&mut Self>, mode: &QString) {
        // Not while a reply runs: an agent keeps the mode and folder it began with.
        if *self.generating() {
            return;
        }
        let mode = mode.to_string();
        if !self.rust().modes.has(&mode) || *self.mode() == QString::from(mode.as_str()) {
            return;
        }
        self.as_mut().set_mode(QString::from(mode.as_str()));
        self.as_mut().check_sandbox();
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

    pub fn save_mode(
        self: Pin<&mut Self>,
        id: &QString,
        name: &QString,
        prompt: &QString,
        temperature: f64,
    ) {
        let mut library = (*self.rust().modes).clone();
        library.put(
            Preset {
                id: id.to_string(),
                name: name.to_string(),
                prompt: prompt.to_string(),
                temperature: (temperature >= 0.0).then_some(temperature as f32),
            },
            new_mode_id,
        );
        self.keep_modes(library);
    }

    pub fn delete_mode(mut self: Pin<&mut Self>, id: &QString) {
        let id = id.to_string();
        let mut library = (*self.rust().modes).clone();
        library.remove(&id);
        // A conversation in a mode that's gone is back in Auto.
        if self.mode().to_string() == id && !library.has(&id) {
            self.as_mut().choose_mode(&QString::from(modes::AUTO));
        }
        self.keep_modes(library);
    }

    /// Uses `library` from now on, and writes it (on the file thread).
    fn keep_modes(mut self: Pin<&mut Self>, library: Library) {
        if let Some(io) = &self.rust().io {
            let copy = library.clone();
            io.run(move |_| {
                if let Err(e) = copy.save(&modes_file()) {
                    log::warn!("cannot save the modes: {e}");
                }
            });
        }
        self.as_mut().use_modes(library);
    }

    fn use_modes(mut self: Pin<&mut Self>, library: Library) {
        let (mut ids, mut names, mut prompts, mut temps) = (
            QStringList::default(),
            QStringList::default(),
            QStringList::default(),
            QList::<f64>::default(),
        );
        for p in library.list() {
            ids.append(QString::from(p.id.as_str()));
            names.append(QString::from(p.name.as_str()));
            prompts.append(QString::from(p.prompt.as_str()));
            temps.append(p.temperature.map_or(-1.0, f64::from));
        }
        self.as_mut().rust_mut().modes = Arc::new(library);
        self.as_mut().set_mode_ids(ids);
        self.as_mut().set_mode_names(names);
        self.as_mut().set_mode_prompts(prompts);
        self.set_mode_temps(temps);
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
        self.as_mut().set_sandboxed(is_sandbox(&path));
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

    /// In Agent mode, the first time: whether commands can run here (their
    /// sandbox can be made). On a worker; it runs bubblewrap.
    fn check_sandbox(mut self: Pin<&mut Self>) {
        if self.mode().to_string() != modes::AGENT.id || self.rust().sandbox_checked {
            return;
        }
        self.as_mut().rust_mut().sandbox_checked = true;
        let qt = self.qt_thread();
        std::thread::spawn(move || {
            let available = gates_core::sandbox::available();
            let _ = qt.queue(move |chat| chat.set_commands_available(available));
        });
    }

    pub fn use_sandbox(mut self: Pin<&mut Self>) {
        if *self.generating() {
            return;
        }
        self.as_mut().set_sandboxed(true);
        self.as_mut().set_workspace(QString::default());
        let saved = match self.as_mut().rust_mut().conversation.as_mut() {
            Some(c) => {
                c.workspace = None;
                true
            }
            None => false,
        };
        if saved {
            self.save();
        }
    }

    pub fn set_agent_access(mut self: Pin<&mut Self>, network: bool, home: bool) {
        self.as_mut().set_agent_network(network);
        self.as_mut().set_agent_home(home);
        if let Some(io) = &self.rust().io {
            let flag = |on: bool| {
                if on {
                    "true".to_string()
                } else {
                    String::new()
                }
            };
            settings::set(io, settings::AGENT_NETWORK, flag(network));
            settings::set(io, settings::AGENT_HOME, flag(home));
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

    /// The next turn of a reply that uses tools: a new, empty reply that
    /// streams, in the same mode.
    fn agent_next(self: Pin<&mut Self>, generation: u64, mode: &str, picked: bool) {
        if self.rust().generation != generation {
            return;
        }
        self.push(Message {
            mode: Some(mode.to_string()),
            picked,
            ..Message::assistant("")
        });
    }

    /// The text of the reply under way becomes `text` (Deep Research's
    /// finished report, its citations linked).
    fn replace_reply(mut self: Pin<&mut Self>, generation: u64, text: &str) {
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
            message.text = text.to_string();
            let row = Row::of(message);
            if let Some(last) = rust.rows.last_mut() {
                *last = row;
            }
        }
        self.last_changed();
    }

    /// What the reply under way is doing now.
    fn show_status(self: Pin<&mut Self>, generation: u64, line: &str) {
        if self.rust().generation == generation {
            self.set_status(QString::from(line));
        }
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
                    chat.as_mut().update_web();
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

    // ---- Web search

    fn provider(&self) -> Provider {
        Provider::from_id(&self.web_provider().to_string())
    }

    fn provider_slot(provider: Provider) -> usize {
        Provider::ALL
            .iter()
            .position(|p| *p == provider)
            .unwrap_or_default()
    }

    /// Whether a reply from `model` can call tools: its chat template takes
    /// them (the Models page's Tools badge). A server elsewhere and the demo
    /// can't be asked, so they count.
    fn model_can_use_tools(&self, model: &str) -> bool {
        *self.demo()
            || !self.server_url().is_empty()
            || self.tool_models().contains(
                &QString::from(model),
                cxx_qt_lib::CaseSensitivity::CaseSensitive,
            )
    }

    /// How a reply reaches the web, as Settings have it now.
    fn web_setup(&self) -> Option<Setup> {
        Some(Setup {
            provider: self.provider(),
            url: self.web_url().to_string(),
            keys: self.rust().web_keys.clone()?,
            cache: self.rust().web_cache.clone(),
            demo: *self.demo(),
        })
    }

    /// Works out `webReady` and `webNote` from the settings, the keyring
    /// and the model.
    fn update_web(mut self: Pin<&mut Self>) {
        let provider = self.provider();
        let on = *self.web_search();
        let (ready, note) = if !on {
            (false, String::new())
        } else if provider.needs_key() && !*self.keyring_available() {
            (
                false,
                "Needs an API key, and no system keyring is available to keep it.".to_string(),
            )
        } else if provider.needs_key() && !*self.web_key_saved() {
            (
                false,
                format!("Add your {} API key to search.", provider.name()),
            )
        } else if !provider.needs_key() && self.web_url().is_empty() {
            (
                false,
                "Add the address of your SearXNG instance.".to_string(),
            )
        } else if !self.model_can_use_tools(&self.model().to_string()) {
            (
                true,
                "The model in use can't call tools (see the Tools badge on the Models page), so its replies won't search."
                    .to_string(),
            )
        } else {
            (true, String::new())
        };
        // Deep Research needs the web on and ready, and a model with tools.
        let research = if !on {
            "Turn on Web Search in Settings.".to_string()
        } else if !ready {
            note.clone()
        } else if !self.model_can_use_tools(&self.model().to_string()) {
            "The model in use can't call tools (see the Tools badge on the Models page)."
                .to_string()
        } else {
            String::new()
        };
        self.as_mut().set_web_ready(ready);
        self.as_mut()
            .set_research_note(QString::from(research.as_str()));
        self.set_web_note(QString::from(note.as_str()));
    }

    pub fn enable_web_search(mut self: Pin<&mut Self>, on: bool) {
        if *self.web_search() == on {
            return;
        }
        self.as_mut().set_web_search(on);
        if let Some(io) = &self.rust().io {
            settings::set(
                io,
                settings::WEB_SEARCH,
                if on { "true".into() } else { String::new() },
            );
        }
        self.update_web();
    }

    pub fn pick_web_provider(mut self: Pin<&mut Self>, id: &QString) {
        let provider = Provider::from_id(&id.to_string());
        if self.provider() == provider && !self.web_provider().is_empty() {
            return;
        }
        self.as_mut().set_web_provider(QString::from(provider.id()));
        if let Some(io) = &self.rust().io {
            settings::set(io, settings::WEB_PROVIDER, provider.id().to_string());
        }
        // The key read was another service's.
        if let Ok(mut cache) = self.rust().web_cache.lock() {
            *cache = None;
        }
        let saved = self.rust().key_saved[Self::provider_slot(provider)];
        self.as_mut().set_web_key_saved(saved);
        self.as_mut().set_web_test_result(QString::default());
        self.as_mut().update_web();
    }

    pub fn save_web_url(mut self: Pin<&mut Self>, url: &QString) {
        let url = url.to_string().trim().trim_end_matches('/').to_string();
        if !url.is_empty()
            && let Err(e) = web::providers::instance(&url)
        {
            self.as_mut().set_web_test_ok(false);
            self.set_web_test_result(QString::from(e.to_string().as_str()));
            return;
        }
        self.as_mut().set_web_url(QString::from(url.as_str()));
        self.as_mut().set_web_test_result(QString::default());
        if let Some(io) = &self.rust().io {
            settings::set(io, settings::WEB_URL, url);
        }
        self.update_web();
    }

    pub fn save_web_key(mut self: Pin<&mut Self>, key: &QString) {
        let key = key.to_string().trim().to_string();
        let provider = self.provider();
        if key.is_empty() || !provider.needs_key() || *self.web_testing() {
            return;
        }
        let keys = match self.rust().web_keys.clone() {
            Some(keys) if *self.keyring_available() => keys,
            // No keyring: the key is kept nowhere, and the user is told.
            _ => {
                let note = format!(
                    "The key wasn't saved. {} Start one such as KWallet, then add the key.",
                    self.keyring_note()
                );
                self.as_mut().set_web_test_ok(false);
                self.set_web_test_result(QString::from(note.as_str()));
                return;
            }
        };
        self.as_mut().set_web_testing(true);
        self.as_mut().set_web_test_result(QString::default());
        let qt = self.qt_thread();
        std::thread::spawn(move || {
            let result = keys.set(&provider.key_name(), &key);
            let _ = qt.queue(move |mut chat| {
                chat.as_mut().set_web_testing(false);
                match result {
                    Ok(()) if chat.provider() == provider => {
                        if let Ok(mut cache) = chat.rust().web_cache.lock() {
                            *cache = Some(key);
                        }
                        chat.as_mut().remember_key(provider, true);
                        chat.as_mut().set_web_test_ok(true);
                        chat.as_mut().set_web_test_result(QString::from(
                            "The key is saved in the system keyring.",
                        ));
                    }
                    Ok(()) => chat.as_mut().remember_key(provider, true),
                    Err(e) => {
                        log::warn!("cannot save the web search key: {e}");
                        chat.as_mut().set_web_test_ok(false);
                        chat.as_mut().set_web_test_result(QString::from(
                            format!("The key wasn't saved. {e}").as_str(),
                        ));
                    }
                }
            });
        });
    }

    /// Notes that `provider` has (or hasn't) a key in the keyring.
    fn remember_key(mut self: Pin<&mut Self>, provider: Provider, saved: bool) {
        self.as_mut().rust_mut().key_saved[Self::provider_slot(provider)] = saved;
        if let Some(io) = &self.rust().io {
            settings::set(
                io,
                settings::web_key_flag(provider),
                if saved { "true".into() } else { String::new() },
            );
        }
        if self.provider() == provider {
            self.as_mut().set_web_key_saved(saved);
        }
        self.update_web();
    }

    pub fn remove_web_key(mut self: Pin<&mut Self>) {
        let provider = self.provider();
        if !provider.needs_key() || *self.web_testing() {
            return;
        }
        let keys = match self.rust().web_keys.clone() {
            Some(keys) if *self.keyring_available() => keys,
            _ => {
                // The key may still be in the keyring: it stays "saved".
                self.as_mut().set_web_test_ok(false);
                self.set_web_test_result(QString::from(
                    "The key can't be removed while the system keyring isn't running. Start it and try again.",
                ));
                return;
            }
        };
        // Busy while the keyring deletes, so a Save can't race it.
        self.as_mut().set_web_testing(true);
        self.as_mut().set_web_test_result(QString::default());
        let qt = self.qt_thread();
        std::thread::spawn(move || {
            let result = keys.remove(&provider.key_name());
            let _ = qt.queue(move |mut chat| {
                chat.as_mut().set_web_testing(false);
                match result {
                    Ok(()) => {
                        if chat.provider() == provider
                            && let Ok(mut cache) = chat.rust().web_cache.lock()
                        {
                            *cache = None;
                        }
                        chat.remember_key(provider, false);
                    }
                    Err(e) => {
                        // Still in the keyring, so still shown as saved.
                        log::warn!("cannot remove the web search key: {e}");
                        chat.as_mut().set_web_test_ok(false);
                        chat.set_web_test_result(QString::from(
                            format!("The key wasn't removed. {e}").as_str(),
                        ));
                    }
                }
            });
        });
    }

    pub fn test_web_search(mut self: Pin<&mut Self>) {
        if *self.web_testing() {
            return;
        }
        let Some(setup) = self.web_setup() else {
            return;
        };
        self.as_mut().set_web_testing(true);
        self.as_mut().set_web_test_result(QString::default());
        let qt = self.qt_thread();
        std::thread::spawn(move || {
            let name = setup.provider.name();
            // The real service, even when the demo backend answers replies.
            let result = setup
                .live(&AtomicBool::new(false))
                .and_then(|live| web::Web::search(&live, "telamon", 1, &AtomicBool::new(false)));
            let _ = qt.queue(move |mut chat| {
                chat.as_mut().set_web_testing(false);
                match result {
                    Ok(found) => {
                        chat.as_mut().set_web_test_ok(true);
                        chat.set_web_test_result(QString::from(
                            format!(
                                "Connected: {name} answered with {} result{}.",
                                found.len(),
                                if found.len() == 1 { "" } else { "s" }
                            )
                            .as_str(),
                        ));
                    }
                    Err(e) => {
                        chat.as_mut().set_web_test_ok(false);
                        chat.set_web_test_result(QString::from(e.to_string().as_str()));
                    }
                }
            });
        });
    }

    /// Reads the web settings and asks (on a worker) whether a keyring
    /// answers. Opening it, which may ask the user for a password, waits for
    /// the first use.
    fn start_web(mut self: Pin<&mut Self>) {
        let provider = Provider::from_id(&settings::get(settings::WEB_PROVIDER));
        let mut saved = [false; 3];
        for p in Provider::ALL {
            saved[Self::provider_slot(p)] = settings::get(settings::web_key_flag(p)) == "true";
        }
        self.as_mut().rust_mut().key_saved = saved;
        self.as_mut().set_web_provider(QString::from(provider.id()));
        self.as_mut()
            .set_web_search(settings::get(settings::WEB_SEARCH) == "true");
        self.as_mut()
            .set_web_url(QString::from(settings::get(settings::WEB_URL).trim()));
        self.as_mut()
            .set_web_key_saved(saved[Self::provider_slot(provider)]);
        let Some(keys) = self.rust().web_keys.clone() else {
            return;
        };
        // Assumed until the check answers, so Settings doesn't flash a
        // complaint while it runs.
        self.as_mut().set_keyring_available(true);
        let qt = self.qt_thread();
        std::thread::spawn(move || {
            let checked = keys.check();
            let _ = qt.queue(move |mut chat| {
                chat.as_mut().set_keyring_available(checked.is_ok());
                chat.as_mut()
                    .set_keyring_note(QString::from(checked.err().unwrap_or_default().as_str()));
                chat.update_web();
            });
        });
        self.update_web();
    }

    /// What a fleet takes from the chat: the model picked, and SystemOne
    /// while it can judge.
    pub fn fleet_setup(&self) -> (String, Option<Arc<SystemOne>>) {
        (
            self.model().to_string(),
            self.rust().system_one_model.clone(),
        )
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
            12 => {
                let mut names = QStringList::default();
                for a in &message.attachments {
                    names.append(QString::from(a.name.as_str()));
                }
                QVariant::from(&names)
            }
            13 => {
                let mut images = QStringList::default();
                for a in &message.attachments {
                    images.append(QString::from(a.image.as_deref().unwrap_or("")));
                }
                QVariant::from(&images)
            }
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
        self.as_mut().set_notice(QString::default());
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
            .set_sandboxed(workspace.is_empty() || is_sandbox(&workspace));
        self.as_mut()
            .set_workspace(QString::from(workspace.as_str()));
        let mode = conversation
            .as_ref()
            .map(|c| c.mode.clone())
            .filter(|m| self.rust().modes.has(m))
            .unwrap_or_else(|| modes::AUTO.to_string());
        self.as_mut().set_mode(QString::from(mode.as_str()));
        self.as_mut().check_sandbox();
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
        let library = self.rust().modes.clone();
        let pinned = (choice != modes::AUTO).then(|| library.resolve(&choice));
        // A pick made while the message was written, for this very text.
        let asked_now = messages
            .iter()
            .rev()
            .find(|m| m.role == Role::User)
            .map(|m| m.text.trim().to_string())
            .unwrap_or_default();
        let prepicked = match pinned {
            None => self
                .as_mut()
                .rust_mut()
                .prepick
                .take()
                .filter(|(text, _, _)| *text == asked_now)
                .map(|(_, mode, picked)| (mode, picked)),
            Some(_) => None,
        };
        let picker = match (&pinned, &prepicked) {
            (None, None) => self.rust().system_one_model.clone(),
            _ => None,
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
            .filter(|id| *id != modes::AGENT.id && *id != modes::DEEP_RESEARCH.id)
            .map(|id| library.resolve(id))
            .unwrap_or_else(|| library.resolve(modes::CHAT.id));
        let user_prompt = self.system_prompt().to_string();
        // Code and Agent replies use the code model, when one is set.
        let code_model = {
            let code = self.model_for(modes::CODE.id);
            if code == self.model().to_string() {
                String::new()
            } else {
                code
            }
        };
        // Agent mode works in the conversation's folder, which it needs
        // (opened on the worker: a folder can be on a slow disk).
        // No folder chosen: the conversation's own sandbox, made on the worker.
        let workspace = if pinned.as_ref().is_some_and(|m| m.id == modes::AGENT.id) {
            let folder = self.workspace().to_string();
            Some(if folder.is_empty() {
                gates_core::store::sandbox_dir(&self.conversation_id().to_string())
            } else {
                PathBuf::from(folder)
            })
        } else {
            None
        };
        let access = gates_core::sandbox::Access {
            network: *self.agent_network(),
            home: *self.agent_home(),
        };
        // Deep Research needs the web, and a model that can call tools.
        if pinned
            .as_ref()
            .is_some_and(|m| m.id == modes::DEEP_RESEARCH.id)
            && !self.research_note().is_empty()
        {
            let note = format!("Deep Research can't run. {}", self.research_note());
            self.as_mut().set_error(QString::from(note.as_str()));
            return;
        }
        // The web, when Settings turn it on: which models can use it is
        // known once the mode (so the model) is.
        let web_setup = if *self.web_ready() {
            self.web_setup()
        } else {
            None
        };
        let tool_models: Vec<String> = self.tool_models().iter().map(|m| m.to_string()).collect();
        let tools_unknown = *self.demo() || !self.server_url().is_empty();
        let mut request = Request {
            model: self.model().to_string(),
            system_prompt: String::new(),
            messages,
            sampling: None,
            tools: Vec::new(),
            response_format: None,
            brief: false,
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
        // An old reply's notice is not this one's.
        self.as_mut().set_notice(QString::default());
        let qt = self.qt_thread();
        std::thread::spawn(move || {
            let (mode, picked) = match prepicked {
                Some(ready) => ready,
                None => pick_mode(pinned, picker.as_deref(), &asked, previous, &library),
            };
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let id = mode.id.clone();
            let _ = qt.queue(move |chat| chat.set_reply_mode(generation, &id, picked));
            if (mode.id == modes::CODE.id || mode.id == modes::AGENT.id) && !code_model.is_empty() {
                request.model = code_model;
            }
            request.system_prompt = modes::system_prompt_for(&mode, &user_prompt);
            request.sampling = mode.sampling;
            request.brief = mode.brief;
            // The web tools: not in Story, and only for a model that can
            // call tools.
            let web: Option<Box<dyn web::Web>> = match &web_setup {
                Some(setup)
                    if mode.id != modes::STORY.id
                        && (tools_unknown || tool_models.contains(&request.model)) =>
                {
                    match setup.connect(&cancel) {
                        Ok(connection) => Some(connection),
                        Err(e) => {
                            log::warn!("web search: {e}");
                            let note =
                                format!("Web Search is on, but this answer didn't use it. {e}");
                            let _ = qt.queue(move |chat| {
                                if chat.rust().generation == generation {
                                    chat.set_error(QString::from(note.as_str()));
                                }
                            });
                            None
                        }
                    }
                }
                _ => None,
            };
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            if web.is_some() && mode.id != modes::DEEP_RESEARCH.id {
                if !request.system_prompt.is_empty() {
                    request.system_prompt.push_str("\n\n");
                }
                request.system_prompt.push_str(web::PROMPT);
            }
            let mut stream = Stream {
                qt: qt.clone(),
                generation,
                pending: String::new(),
                sent: Instant::now(),
                rate: Rate::default(),
                cancel: cancel.clone(),
                mode: mode.id.clone(),
                picked,
            };
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                // What the web may open in this reply: what it is shown.
                let session = web
                    .as_deref()
                    .map(|connection| web::Session::new(connection, &request.messages));
                if mode.id == modes::DEEP_RESEARCH.id {
                    let Some(connection) = web.as_deref() else {
                        return Err(gates_core::BackendError::Other(
                            "Deep Research needs Web Search. Check it in Settings.".into(),
                        ));
                    };
                    return gates_core::research::run(
                        backend.as_ref(),
                        connection,
                        request,
                        gates_core::research::Limits::default(),
                        &cancel,
                        &mut stream,
                    );
                }
                match &workspace {
                    Some(folder) => {
                        if folder.starts_with(gates_core::store::data_dir().join("workspaces")) {
                            let _ = gates_core::store::private_dir(folder);
                        }
                        let ws = Workspace::open(folder).map_err(|e| {
                            gates_core::BackendError::Other(format!(
                                "The agent can't work in {}: {e}.",
                                folder.display()
                            ))
                        })?;
                        request
                            .system_prompt
                            .push_str(&format!("\n\nThe workspace is {}.", ws.root().display()));
                        let ws = ws.with_access(access);
                        let tools = agent::Tools {
                            workspace: Some(&ws),
                            web: session.as_ref(),
                            max_steps: agent::MAX_STEPS,
                        };
                        agent::run_tools(backend.as_ref(), request, &tools, &cancel, &mut stream)
                    }
                    // Chat, Code and the rest: the web tools alone, in a few steps.
                    None if session.is_some() => {
                        let tools = agent::Tools {
                            workspace: None,
                            web: session.as_ref(),
                            max_steps: agent::WEB_STEPS,
                        };
                        agent::run_tools(backend.as_ref(), request, &tools, &cancel, &mut stream)
                    }
                    None => backend.complete(&request, &cancel, &mut |event| match event {
                        Event::Text(piece) => agent::Host::text(&mut stream, piece),
                        Event::Speed(s) => stream.rate.reported(s),
                        // Tools weren't offered: nothing to run.
                        Event::ToolCalls(_) => {}
                        Event::Notice(text) => agent::Host::notice(&mut stream, text),
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
        self.as_mut().set_status(QString::default());
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
