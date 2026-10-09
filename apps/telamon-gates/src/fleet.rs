//! The Fleet page's object: a goal, a folder, and the agents that work on it,
//! each a card (title, status, last line, steps, speed, SystemOne's belief
//! that it finished). The run is `gates_core::fleet`, on one worker thread;
//! its events come back to the Qt thread through `qt_thread().queue`, and
//! one question at a time (from the agent under way) waits for the user.

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
        include!("cxx-qt-lib/qstringlist.h");
        type QStringList = cxx_qt_lib::QStringList;
        include!("cxx-qt-lib/qlist.h");
        type QList_f64 = cxx_qt_lib::QList<f64>;
    }

    extern "RustQt" {
        #[qobject]
        /// The goal of the run under way (or the last one), and the folder
        /// the agents work in; "" for none yet.
        #[qproperty(QString, goal)]
        #[qproperty(QString, workspace)]
        /// A run is under way (the coordinator planning, or an agent working).
        #[qproperty(bool, running)]
        /// The coordinator is splitting the goal: there are no cards yet.
        #[qproperty(bool, planning)]
        /// What went wrong with the run; "" when nothing.
        #[qproperty(QString, error)]
        /// Something the model server changed to load the model, short of an
        /// error ("loaded with a smaller context"); plain text, "" for none.
        #[qproperty(QString, notice)]
        /// The built-in demo answers, which can't plan.
        #[qproperty(bool, demo)]
        /// Per agent, in the plan's order: title, status ("idle", "working",
        /// "waiting", "done", "failed", "stopped"), what it did or said last,
        /// tool steps run, tokens per second (0 when not known) and
        /// SystemOne's belief that it finished (-1 when not asked).
        #[qproperty(QStringList, titles)]
        #[qproperty(QStringList, statuses)]
        #[qproperty(QStringList, lines)]
        #[qproperty(QList_f64, steps)]
        #[qproperty(QList_f64, speeds)]
        #[qproperty(QList_f64, confidences)]
        /// An agent waits for the user to allow a change: which agent, what
        /// it is (a title, the text or command), and "write" or "run".
        #[qproperty(bool, approving)]
        #[qproperty(QString, approval_agent, cxx_name = "approvalAgent")]
        #[qproperty(QString, approval_title, cxx_name = "approvalTitle")]
        #[qproperty(QString, approval_detail, cxx_name = "approvalDetail")]
        #[qproperty(QString, approval_kind, cxx_name = "approvalKind")]
        #[namespace = "telamon_gates"]
        type Fleet = super::FleetRust;
    }

    unsafe extern "RustQt" {
        /// Plans `goal` and sets the agents to work. Nothing while a run is
        /// under way; an error for a blank goal or no folder.
        #[qinvokable]
        fn start(self: Pin<&mut Fleet>, goal: &QString);

        /// Stops the whole fleet: the agent under way and those to come.
        #[qinvokable]
        fn stop(self: Pin<&mut Fleet>);

        /// Stops agent `index` (from 0); the others go on.
        #[qinvokable]
        #[cxx_name = "stopAgent"]
        fn stop_agent(self: Pin<&mut Fleet>, index: i32);

        /// The answer to the change an agent waits on: 0 deny, 1 allow,
        /// 2 allow it and the rest of that agent's edits.
        #[qinvokable]
        #[cxx_name = "answerApproval"]
        fn answer_approval(self: Pin<&mut Fleet>, choice: i32);

        /// The folder the agents work in.
        #[qinvokable]
        #[cxx_name = "chooseWorkspace"]
        fn choose_workspace(self: Pin<&mut Fleet>, path: &QString);

        #[qinvokable]
        #[cxx_name = "dismissError"]
        fn dismiss_error(self: Pin<&mut Fleet>);

        #[qinvokable]
        #[cxx_name = "dismissNotice"]
        fn dismiss_notice(self: Pin<&mut Fleet>);
    }

    impl cxx_qt::Threading for Fleet {}

    #[namespace = "rust::cxxqtlib1"]
    unsafe extern "C++" {
        include!("cxx-qt-lib/common.h");

        #[cxx_name = "make_unique"]
        fn fleet_make_unique() -> UniquePtr<Fleet>;
    }
}

use crate::chat;
use core::pin::Pin;
use cxx_qt::{CxxQtThread, CxxQtType, Threading};
use cxx_qt_lib::{QList, QString, QStringList};
use gates_core::Backend;
use gates_core::agent::Approval;
use gates_core::conversation::ToolCall;
use gates_core::fleet::{self, Control, FleetHost, Judge, MAX_GOAL, Status, Task};
use gates_core::systemone::SystemOne;
use gates_core::tools::Workspace;
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::Duration;

/// One agent as the page shows it.
struct Card {
    title: String,
    status: Status,
    line: String,
    steps: usize,
    speed: f64,
    confidence: Option<f64>,
}

#[derive(Default)]
pub struct FleetRust {
    goal: QString,
    workspace: QString,
    running: bool,
    planning: bool,
    error: QString,
    notice: QString,
    demo: bool,
    titles: QStringList,
    statuses: QStringList,
    lines: QStringList,
    steps: QList<f64>,
    speeds: QList<f64>,
    confidences: QList<f64>,
    approving: bool,
    approval_agent: QString,
    approval_title: QString,
    approval_detail: QString,
    approval_kind: QString,

    cards: Vec<Card>,
    /// The agent whose question is on the page.
    asking: Option<usize>,
    /// Where the question's answer goes.
    approval: Option<Sender<Approval>>,
    /// Stops the run under way.
    control: Option<Arc<Control>>,
    /// Bumped by every run started: events of an older one are dropped.
    generation: u64,

    pub backend: Option<Arc<dyn Backend>>,
    /// The chat, which knows the model picked and SystemOne's decision
    /// model. Boxed: a thread handle is not Unpin, and the struct must be.
    pub chat: Option<Box<CxxQtThread<chat::qobject::Chat>>>,
}

impl Drop for FleetRust {
    /// A run under way ends with the window.
    fn drop(&mut self) {
        if let Some(control) = &self.control {
            control.stop_all();
        }
    }
}

/// The run's events, on their way to the Qt thread.
struct Events {
    qt: CxxQtThread<qobject::Fleet>,
    generation: u64,
    control: Arc<Control>,
}

impl Events {
    fn send(&self, job: impl FnOnce(Pin<&mut qobject::Fleet>, u64) + Send + 'static) {
        let generation = self.generation;
        let _ = self.qt.queue(move |fleet| job(fleet, generation));
    }
}

impl FleetHost for Events {
    fn planned(&mut self, tasks: &[Task]) {
        let titles: Vec<String> = tasks.iter().map(|t| t.title.clone()).collect();
        self.send(move |fleet, g| fleet.planned(g, titles));
    }

    fn status(&mut self, agent: usize, status: Status) {
        self.send(move |fleet, g| fleet.card_status(g, agent, status));
    }

    fn step(&mut self, agent: usize, steps: usize, line: &str) {
        let line = line.to_string();
        self.send(move |fleet, g| fleet.card_step(g, agent, steps, line));
    }

    fn speed(&mut self, agent: usize, speed: f64) {
        self.send(move |fleet, g| fleet.card_speed(g, agent, speed));
    }

    fn line(&mut self, agent: usize, line: &str) {
        let line = line.to_string();
        self.send(move |fleet, g| fleet.card_line(g, agent, line));
    }

    fn approve(&mut self, agent: usize, call: &ToolCall, title: &str, detail: &str) -> Approval {
        let (tx, rx) = mpsc::channel();
        let kind = if call.name == "run_command" {
            "run"
        } else {
            "write"
        };
        let (title, detail) = (title.to_string(), detail.to_string());
        self.send(move |fleet, g| fleet.ask_approval(g, agent, title, detail, kind, tx));
        // Waits for the answer; Stop (this agent or all) is a no.
        loop {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(answer) => return answer,
                Err(RecvTimeoutError::Timeout) if !self.control.agent_stopped(agent) => {}
                Err(_) => return Approval::Deny,
            }
        }
    }

    fn judged(&mut self, agent: usize, confidence: f64) {
        self.send(move |fleet, g| fleet.card_confidence(g, agent, confidence));
    }

    fn notice(&mut self, text: &str) {
        let text = text.to_string();
        self.send(move |fleet, g| fleet.show_notice(g, &text));
    }
}

/// The model picked in the chat, and SystemOne while it can judge: asked of
/// the chat on the Qt thread, which owns them. Without an answer in a few
/// seconds, the backend's own model and no judge.
fn from_chat(chat: Option<&CxxQtThread<chat::qobject::Chat>>) -> (String, Option<Arc<SystemOne>>) {
    let Some(chat) = chat else {
        return (String::new(), None);
    };
    let (tx, rx) = mpsc::channel();
    if chat
        .queue(move |chat| {
            let _ = tx.send(chat.fleet_setup());
        })
        .is_err()
    {
        return (String::new(), None);
    }
    rx.recv_timeout(Duration::from_secs(3)).unwrap_or_default()
}

impl qobject::Fleet {
    /// Called once by `telamon_objects_new` after the backend is set.
    pub fn start_up(mut self: Pin<&mut Self>) {
        let demo = self.rust().backend.as_ref().is_some_and(|b| b.is_demo());
        self.as_mut().set_demo(demo);
        // For screenshots only (scripts/screens.sh).
        if let Ok(seed) = std::env::var("TELAMON_GATES_SEED") {
            self.seed(&seed);
        }
    }

    /// Sample agents on the page, with no run behind them, to see it in
    /// every state: `fleet` (one done, one failed, one asks, two to come),
    /// `fleet-working` (one done, one working, one stopped, one to come)
    /// and `fleet-planning`.
    fn seed(mut self: Pin<&mut Self>, seed: &str) {
        // `workspace:<folder>` only chooses the folder, to drive a real run
        // headless.
        if let Some(path) = seed.strip_prefix("workspace:") {
            self.choose_workspace(&QString::from(path));
            return;
        }
        let card = |title: &str, status, line: &str, steps, speed, confidence| Card {
            title: title.to_string(),
            status,
            line: line.to_string(),
            steps,
            speed,
            confidence,
        };
        let cards = match seed {
            "fleet" => vec![
                card(
                    "Read the code",
                    Status::Done,
                    "Mapped the signup flow: validation lives in forms.py and nothing checks the email.",
                    4,
                    0.0,
                    Some(0.93),
                ),
                card(
                    "Add input checks",
                    Status::Failed,
                    "The model server refused the request (503): Loading model",
                    3,
                    0.0,
                    None,
                ),
                card(
                    "Write tests",
                    Status::Waiting,
                    "✓ Read tests/test_forms.py (lines 1–40 of 40)",
                    2,
                    0.0,
                    None,
                ),
                card("Update the README", Status::Idle, "", 0, 0.0, None),
                card("Check the build", Status::Idle, "", 0, 0.0, None),
            ],
            "fleet-working" => vec![
                card(
                    "Read the code",
                    Status::Done,
                    "Mapped the signup flow: validation lives in forms.py and nothing checks the email.",
                    4,
                    0.0,
                    Some(0.93),
                ),
                card(
                    "Add input checks",
                    Status::Working,
                    "✓ Edited src/forms.py (1 change)",
                    7,
                    41.3,
                    None,
                ),
                card("Write tests", Status::Stopped, "", 0, 0.0, None),
                card("Update the README", Status::Idle, "", 0, 0.0, None),
            ],
            "fleet-planning" => Vec::new(),
            _ => return,
        };
        self.as_mut().rust_mut().cards = cards;
        self.as_mut().set_goal(QString::from(
            "Add input checks to the signup form, write tests for them, and update the README.",
        ));
        self.as_mut()
            .set_workspace(QString::from("/home/user/Projects/signup"));
        self.as_mut().sync();
        self.as_mut().set_planning(seed == "fleet-planning");
        self.as_mut().set_running(true);
        if seed == "fleet" {
            self.as_mut()
                .set_approval_agent(QString::from("Write tests"));
            self.as_mut()
                .set_approval_title(QString::from("Run python3 -m pytest tests/"));
            self.as_mut().set_approval_detail(QString::from(
                "python3 -m pytest tests/ -q\n\nFolder: /home/user/Projects/signup\nTime limit: 60 s\n1 line, 28 characters",
            ));
            self.as_mut().set_approval_kind(QString::from("run"));
            self.set_approving(true);
        }
    }

    pub fn start(mut self: Pin<&mut Self>, goal: &QString) {
        if *self.running() {
            return;
        }
        let goal: String = goal.to_string().trim().chars().take(MAX_GOAL).collect();
        let folder = self.workspace().to_string();
        if goal.is_empty() {
            self.set_error(QString::from("Describe the goal first."));
            return;
        }
        if folder.is_empty() {
            self.set_error(QString::from("Choose the folder the agents work in first."));
            return;
        }
        let Some(backend) = self.rust().backend.clone() else {
            return;
        };
        let chat = self.rust().chat.as_ref().map(|c| (**c).clone());
        let control = Arc::new(Control::new());
        let generation = {
            let mut rust = self.as_mut().rust_mut();
            rust.generation += 1;
            rust.cards.clear();
            rust.control = Some(control.clone());
            rust.asking = None;
            rust.approval = None;
            rust.generation
        };
        self.as_mut().set_goal(QString::from(goal.as_str()));
        self.as_mut().set_error(QString::default());
        self.as_mut().set_notice(QString::default());
        self.as_mut().set_approving(false);
        self.as_mut().sync();
        self.as_mut().set_planning(true);
        self.as_mut().set_running(true);

        let qt = self.qt_thread();
        std::thread::spawn(move || {
            let (model, system_one) = from_chat(chat.as_ref());
            let events_qt = qt.clone();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                // Opened here, not on the Qt thread: a folder can be on a
                // slow disk.
                let workspace = Workspace::open(Path::new(&folder))
                    .map_err(|e| format!("The agents can't work in {folder}: {e}."))?;
                let mut events = Events {
                    qt: events_qt,
                    generation,
                    control: control.clone(),
                };
                let judge = system_one.as_deref().map(|s| s as &dyn Judge);
                let finished = fleet::run(
                    backend.as_ref(),
                    judge,
                    &model,
                    &goal,
                    &workspace,
                    &control,
                    &mut events,
                );
                Ok::<_, String>(finished.error)
            }));
            let error = match result {
                Ok(Ok(error)) => error,
                Ok(Err(e)) => Some(e),
                Err(_) => {
                    log::error!("the fleet panicked");
                    Some("The fleet stopped unexpectedly.".to_string())
                }
            };
            // Dropping the last SystemOne stops its server, which waits for
            // it to quit: here, not on the Qt thread.
            drop(system_one);
            let _ = qt.queue(move |fleet| fleet.finish(generation, error));
        });
    }

    pub fn stop(self: Pin<&mut Self>) {
        if let Some(control) = &self.rust().control {
            control.stop_all();
        }
    }

    pub fn stop_agent(self: Pin<&mut Self>, index: i32) {
        if let (Some(control), Ok(index)) = (&self.rust().control, usize::try_from(index)) {
            control.stop_agent(index);
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
        self.as_mut().rust_mut().asking = None;
        self.set_approving(false);
    }

    pub fn choose_workspace(self: Pin<&mut Self>, path: &QString) {
        if *self.running() {
            return;
        }
        let path = path.to_string();
        // Only a real folder, and not a too wide one (checked on a worker:
        // it may be on a slow disk); the tools check every path against it.
        let qt = self.qt_thread();
        std::thread::spawn(move || {
            let checked = Workspace::open(Path::new(&path))
                .map(|ws| ws.root().to_string_lossy().into_owned());
            let _ = qt.queue(move |mut fleet| match checked {
                Ok(root) => fleet.as_mut().set_workspace(QString::from(root.as_str())),
                Err(e) => fleet.as_mut().set_error(QString::from(
                    format!("The agents can't work in {path}: {e}.").as_str(),
                )),
            });
        });
    }

    pub fn dismiss_error(self: Pin<&mut Self>) {
        self.set_error(QString::default());
    }

    pub fn dismiss_notice(self: Pin<&mut Self>) {
        self.set_notice(QString::default());
    }

    /// The backend's notice for the run of `generation`.
    fn show_notice(self: Pin<&mut Self>, generation: u64, text: &str) {
        if self.rust().generation == generation {
            self.set_notice(QString::from(text));
        }
    }

    /// The plan is in: a card for each task.
    fn planned(mut self: Pin<&mut Self>, generation: u64, titles: Vec<String>) {
        if self.rust().generation != generation {
            return;
        }
        self.as_mut().rust_mut().cards = titles
            .into_iter()
            .map(|title| Card {
                title,
                status: Status::Idle,
                line: String::new(),
                steps: 0,
                speed: 0.0,
                confidence: None,
            })
            .collect();
        self.as_mut().sync();
        self.set_planning(false);
    }

    /// Changes agent `agent`'s card with `change`, then shows it.
    fn card(
        mut self: Pin<&mut Self>,
        generation: u64,
        agent: usize,
        change: impl FnOnce(&mut Card),
    ) {
        if self.rust().generation != generation {
            return;
        }
        match self.as_mut().rust_mut().cards.get_mut(agent) {
            Some(card) => change(card),
            None => return,
        }
        self.sync();
    }

    fn card_status(mut self: Pin<&mut Self>, generation: u64, agent: usize, status: Status) {
        if self.rust().generation != generation {
            return;
        }
        // Its question is answered (or void): off the page.
        if status != Status::Waiting && self.rust().asking == Some(agent) {
            self.as_mut().rust_mut().asking = None;
            self.as_mut().rust_mut().approval = None;
            self.as_mut().set_approving(false);
        }
        self.card(generation, agent, |card| {
            card.status = status;
            if status != Status::Working {
                card.speed = 0.0;
            }
        });
    }

    fn card_step(
        mut self: Pin<&mut Self>,
        generation: u64,
        agent: usize,
        steps: usize,
        line: String,
    ) {
        self.as_mut().card(generation, agent, |card| {
            card.steps = steps;
            card.line = line;
        });
    }

    fn card_speed(self: Pin<&mut Self>, generation: u64, agent: usize, speed: f64) {
        self.card(generation, agent, |card| card.speed = speed);
    }

    fn card_line(self: Pin<&mut Self>, generation: u64, agent: usize, line: String) {
        self.card(generation, agent, |card| card.line = line);
    }

    fn card_confidence(self: Pin<&mut Self>, generation: u64, agent: usize, confidence: f64) {
        self.card(generation, agent, |card| card.confidence = Some(confidence));
    }

    /// An agent asks whether a change may happen (`kind`: "write" or
    /// "run"); the answer goes to `tx`.
    fn ask_approval(
        mut self: Pin<&mut Self>,
        generation: u64,
        agent: usize,
        title: String,
        detail: String,
        kind: &str,
        tx: Sender<Approval>,
    ) {
        if self.rust().generation != generation {
            return;
        }
        let who = self
            .rust()
            .cards
            .get(agent)
            .map(|c| c.title.clone())
            .unwrap_or_default();
        {
            let mut rust = self.as_mut().rust_mut();
            rust.approval = Some(tx);
            rust.asking = Some(agent);
        }
        self.as_mut()
            .set_approval_agent(QString::from(who.as_str()));
        self.as_mut()
            .set_approval_title(QString::from(title.as_str()));
        self.as_mut()
            .set_approval_detail(QString::from(detail.as_str()));
        self.as_mut().set_approval_kind(QString::from(kind));
        self.set_approving(true);
    }

    /// The run ended, with `error` if it failed.
    fn finish(mut self: Pin<&mut Self>, generation: u64, error: Option<String>) {
        if self.rust().generation != generation {
            return;
        }
        {
            let mut rust = self.as_mut().rust_mut();
            rust.control = None;
            rust.asking = None;
            rust.approval = None;
        }
        if let Some(e) = &error {
            log::warn!("the fleet failed: {e}");
            self.as_mut().set_error(QString::from(e.as_str()));
        }
        self.as_mut().set_approving(false);
        self.as_mut().set_planning(false);
        self.set_running(false);
    }

    /// Shows the cards in the properties QML reads.
    fn sync(mut self: Pin<&mut Self>) {
        let mut titles = QStringList::default();
        let mut statuses = QStringList::default();
        let mut lines = QStringList::default();
        let mut steps = QList::<f64>::default();
        let mut speeds = QList::<f64>::default();
        let mut confidences = QList::<f64>::default();
        for card in &self.rust().cards {
            titles.append(QString::from(card.title.as_str()));
            statuses.append(QString::from(card.status.as_str()));
            lines.append(QString::from(card.line.as_str()));
            steps.append(card.steps as f64);
            speeds.append(card.speed);
            confidences.append(card.confidence.unwrap_or(-1.0));
        }
        self.as_mut().set_titles(titles);
        self.as_mut().set_statuses(statuses);
        self.as_mut().set_lines(lines);
        self.as_mut().set_steps(steps);
        self.as_mut().set_speeds(speeds);
        self.as_mut().set_confidences(confidences);
    }
}
