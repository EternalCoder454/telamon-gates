//! The Fleet: several agents working on one goal, and a way to watch and
//! steer them.
//!
//! A run has a goal and a workspace folder. The chat model, asked once for a
//! JSON plan (llama-server turns `response_format` into a grammar), splits
//! the goal into a few subtasks (`parse_plan` checks and bounds what comes
//! back). Each subtask then runs as an agent (`agent::run`) in the same
//! folder, one after another: with `--parallel 1` that is the only way the
//! one server serves them, and agents that change the same files shouldn't
//! race anyway. An agent that finishes can be judged by SystemOne (`Judge`):
//! "Did this agent complete its task?", answered as a probability.
//!
//! Everything the window shows comes through `FleetHost`, event by event
//! with the agent's number. Stopping is `Control`: one agent, or the whole
//! fleet.
//!
//! Blocks (the model, the tools, the user's answers): call it from a worker.

use crate::agent::{self, Approval};
use crate::backend::{Backend, BackendError, Event, Request};
use crate::conversation::{Message, ToolCall};
use crate::modes;
use crate::systemone::SystemOne;
use crate::tools::Workspace;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Agents in a fleet at most: more is more than the window and one server
/// can serve.
pub const MAX_TASKS: usize = 6;
/// The longest goal, in characters.
pub const MAX_GOAL: usize = 4000;
/// The longest title and instructions of a subtask, in characters.
pub const MAX_TITLE: usize = 60;
pub const MAX_INSTRUCTIONS: usize = 1500;
/// The longest line a card shows (what an agent did or said last).
const MAX_LINE: usize = 200;
/// How much of an agent's report the agents after it are told.
const MAX_BRIEF: usize = 400;
/// How often an agent's speed is handed to the window.
const SPEED_EVERY: Duration = Duration::from_millis(250);

/// One subtask: what the coordinator made of a part of the goal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub title: String,
    pub instructions: String,
}

/// Where an agent is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Waits for its turn.
    Idle,
    Working,
    /// Asked to change something, and waits for the user.
    Waiting,
    Done,
    Failed,
    Stopped,
}

impl Status {
    /// The name QML switches on.
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Idle => "idle",
            Status::Working => "working",
            Status::Waiting => "waiting",
            Status::Done => "done",
            Status::Failed => "failed",
            Status::Stopped => "stopped",
        }
    }
}

// ---------------------------------------------------------------- the plan

/// The shape the coordinator must answer in.
pub fn plan_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "tasks": {
                "type": "array",
                "minItems": 1,
                "maxItems": MAX_TASKS,
                "items": {
                    "type": "object",
                    "properties": {
                        "title": {"type": "string"},
                        "instructions": {"type": "string"}
                    },
                    "required": ["title", "instructions"]
                }
            }
        },
        "required": ["tasks"]
    })
}

/// The request that asks the chat model (`model`) to split `goal` for the
/// team.
pub fn plan_request(model: &str, goal: &str) -> Request {
    Request {
        model: model.to_string(),
        system_prompt: format!(
            "You coordinate a team of agents. They work one after another in the same project \
             folder, with tools to read, search, write and edit files and to run commands. \
             They know the folder, so don't name it. Split the \
             user's goal into 2 to {MAX_TASKS} subtasks, in the order they should be done, \
             each a clear piece of work one agent can finish alone. Give each a short title \
             (a few words) and complete instructions: an agent sees only its own, plus the \
             goal. Answer with JSON only: {{\"tasks\": [{{\"title\": …, \"instructions\": …}}]}}."
        ),
        messages: vec![Message::user(format!(
            "Goal: {}",
            clip(goal.trim(), MAX_GOAL)
        ))],
        sampling: Some(modes::Sampling {
            temperature: 0.3,
            top_p: 0.9,
        }),
        tools: Vec::new(),
        response_format: Some(json!({"type": "json_object", "schema": plan_schema()})),
    }
}

/// The subtasks in the coordinator's answer. At most `MAX_TASKS` (the rest
/// are left out), each with a one-line title and instructions cut to their
/// caps; an entry with neither is skipped. An answer with no usable task, or
/// no JSON, is an error that says so.
pub fn parse_plan(text: &str) -> Result<Vec<Task>, String> {
    let value = json_in(text)
        .ok_or_else(|| "The coordinator's plan wasn't understood: it sent no JSON.".to_string())?;
    let list = value
        .get("tasks")
        .and_then(Value::as_array)
        .or_else(|| value.as_array())
        .ok_or_else(|| "The coordinator's plan had no tasks.".to_string())?;
    let tasks: Vec<Task> = list
        .iter()
        .filter_map(|entry| {
            let field = |name: &str| entry.get(name).and_then(Value::as_str).unwrap_or("");
            let instructions = clean(field("instructions"), MAX_INSTRUCTIONS);
            let mut title = one_line(field("title"), MAX_TITLE);
            if title.is_empty() {
                title = one_line(&instructions, MAX_TITLE);
            }
            if title.is_empty() {
                return None;
            }
            let instructions = if instructions.is_empty() {
                title.clone()
            } else {
                instructions
            };
            Some(Task {
                title,
                instructions,
            })
        })
        .take(MAX_TASKS)
        .collect();
    if tasks.is_empty() {
        return Err("The coordinator's plan had no tasks.".to_string());
    }
    Ok(tasks)
}

/// The JSON object in `text`: all of it, or from its first `{` to its last
/// `}` (a model may wrap it in a code fence or a sentence).
fn json_in(text: &str) -> Option<Value> {
    let text = text.trim();
    if let Ok(value) = serde_json::from_str(text) {
        return Some(value);
    }
    let (start, end) = (text.find('{')?, text.rfind('}')?);
    if start >= end {
        return None;
    }
    serde_json::from_str(&text[start..=end]).ok()
}

/// The first `max` characters of `text`.
fn clip(text: &str, max: usize) -> &str {
    match text.char_indices().nth(max) {
        Some((i, _)) => &text[..i],
        None => text,
    }
}

/// `text` on one line, cut to `max` characters (with "…" when it was).
fn one_line(text: &str, max: usize) -> String {
    let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let joined: String = joined.chars().filter(|c| !c.is_control()).collect();
    if joined.chars().count() > max {
        format!("{}…", clip(&joined, max.saturating_sub(1)).trim_end())
    } else {
        joined
    }
}

/// `text` without control characters (but with its line breaks and tabs),
/// trimmed, cut to `max` characters.
fn clean(text: &str, max: usize) -> String {
    let kept: String = text
        .chars()
        .filter(|c| matches!(c, '\n' | '\t') || !c.is_control())
        .collect();
    clip(kept.trim(), max).trim_end().to_string()
}

// ------------------------------------------------------- stopping, judging

/// Stops one agent or the whole fleet, from any thread.
#[derive(Default)]
pub struct Control {
    stopped: AtomicBool,
    agents: Mutex<Vec<Arc<AtomicBool>>>,
}

impl Control {
    pub fn new() -> Control {
        Control::default()
    }

    /// Stops everything: the coordinator, the agent under way (it ends
    /// soon, a waiting question is answered no) and every agent still to
    /// come.
    pub fn stop_all(&self) {
        let agents = self.agents.lock().unwrap_or_else(|e| e.into_inner());
        self.stopped.store(true, Ordering::Relaxed);
        for flag in agents.iter() {
            flag.store(true, Ordering::Relaxed);
        }
    }

    /// Stops agent `index` (from 0): under way, it ends soon; yet to come,
    /// it is skipped. Any other index is ignored.
    pub fn stop_agent(&self, index: usize) {
        let agents = self.agents.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(flag) = agents.get(index) {
            flag.store(true, Ordering::Relaxed);
        }
    }

    /// The whole fleet was stopped.
    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::Relaxed)
    }

    /// Agent `index` (or the whole fleet) was stopped.
    pub fn agent_stopped(&self, index: usize) -> bool {
        self.is_stopped()
            || self
                .agents
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(index)
                .is_some_and(|f| f.load(Ordering::Relaxed))
    }

    /// The flag the coordinator watches.
    fn whole(&self) -> &AtomicBool {
        &self.stopped
    }

    /// One flag per agent of a plan of `n`; already set if the fleet was
    /// stopped meanwhile.
    fn begin(&self, n: usize) -> Vec<Arc<AtomicBool>> {
        let mut agents = self.agents.lock().unwrap_or_else(|e| e.into_inner());
        let stopped = self.stopped.load(Ordering::Relaxed);
        *agents = (0..n).map(|_| Arc::new(AtomicBool::new(stopped))).collect();
        agents.clone()
    }
}

/// Judges whether an agent finished its task.
pub trait Judge: Send + Sync {
    /// How likely it is (0 to 1) that the agent given `task` completed it,
    /// going by its last `reply`; or why there's no answer.
    fn finished(&self, task: &Task, reply: &str) -> Result<f64, String>;
}

/// SystemOne, with the `noul` question "Did this agent complete its task?".
impl Judge for SystemOne {
    fn finished(&self, task: &Task, reply: &str) -> Result<f64, String> {
        self.judge_done(&task.instructions, reply)
    }
}

// ------------------------------------------------------------- the running

/// What the fleet tells the window, in order, with the agent's number (from
/// 0, in the plan's order).
pub trait FleetHost {
    /// The plan: one agent for each task, all `Idle`.
    fn planned(&mut self, tasks: &[Task]);
    fn status(&mut self, agent: usize, status: Status);
    /// A tool ran (or didn't): `line` says what ("✓ Read src/a.py (lines
    /// 1–5 of 5)"), and `steps` is how many have run.
    fn step(&mut self, agent: usize, steps: usize, line: &str);
    /// The agent's tokens per second now.
    fn speed(&mut self, agent: usize, speed: f64);
    /// What the agent said last, or why it failed: one line.
    fn line(&mut self, agent: usize, line: &str);
    /// May `call` run? `title` and `detail` say what it does. Blocks until
    /// the user answers; a stopped agent (`Control`) gets a no.
    fn approve(&mut self, agent: usize, call: &ToolCall, title: &str, detail: &str) -> Approval;
    /// SystemOne's belief that the agent finished (0 to 1).
    fn judged(&mut self, agent: usize, confidence: f64);
}

/// How an agent ended.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub status: Status,
    pub steps: usize,
    /// The agent's last reply, or why it failed.
    pub reply: String,
    /// SystemOne's belief that it finished.
    pub confidence: Option<f64>,
}

/// How a run ended: each agent's report (none if there was no plan), and
/// what went wrong, if the run itself failed.
#[derive(Debug, Clone, PartialEq)]
pub struct Finished {
    pub reports: Vec<Report>,
    pub error: Option<String>,
}

/// Tokens per second of an agent's turn: the server's own figure once it
/// sends one, else the pieces counted since the first came.
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

    fn speed(&self) -> Option<f64> {
        self.reported.or_else(|| {
            let seconds = self.first?.elapsed().as_secs_f64();
            // The first token's time is the wait for it, not generation.
            (self.tokens >= 2 && seconds > 0.0).then(|| (self.tokens - 1) as f64 / seconds)
        })
    }
}

/// One agent's `agent::Host`, turning its events into the fleet's.
struct AgentHost<'a> {
    index: usize,
    host: &'a mut dyn FleetHost,
    steps: usize,
    /// The text of the turn under way; the last turn's is the report.
    reply: String,
    rate: Rate,
    sent: Instant,
}

impl AgentHost<'_> {
    fn send_speed(&mut self) {
        self.sent = Instant::now();
        if let Some(speed) = self.rate.speed() {
            self.host.speed(self.index, speed);
        }
    }
}

impl agent::Host for AgentHost<'_> {
    fn text(&mut self, piece: &str) {
        self.reply.push_str(piece);
        self.rate.token();
        if self.sent.elapsed() >= SPEED_EVERY {
            self.send_speed();
        }
    }

    fn speed(&mut self, speed: f64) {
        if speed.is_finite() && speed > 0.0 {
            self.rate.reported = Some(speed);
            self.send_speed();
        }
    }

    fn calls(&mut self, _: &[ToolCall]) {}

    fn approve(&mut self, call: &ToolCall, title: &str, detail: &str) -> Approval {
        self.host.status(self.index, Status::Waiting);
        let answer = self.host.approve(self.index, call, title, detail);
        self.host.status(self.index, Status::Working);
        answer
    }

    fn result(&mut self, message: Message) {
        self.steps += 1;
        let line = format!(
            "{} {}",
            if message.failed { "✗" } else { "✓" },
            message.summary.as_deref().unwrap_or("Ran a tool")
        );
        self.host
            .step(self.index, self.steps, &one_line(&line, MAX_LINE));
    }

    fn next_turn(&mut self) {
        self.rate = Rate::default();
        self.reply.clear();
    }
}

/// The request for agent `index` of `tasks`: the agent mode's prompt, the
/// workspace, the goal and its part in it, and what the agents before it
/// reported.
fn agent_request(
    model: &str,
    goal: &str,
    tasks: &[Task],
    index: usize,
    before: &[String],
    workspace: &Workspace,
) -> Request {
    let task = &tasks[index];
    let mut system = format!(
        "{}\n\nThe workspace is {}.\n\nYou are agent {} of {} in a team working on one goal, \
         one agent after another, in this same folder.\nThe goal: {}\nYour task: {}\nDo your \
         task and nothing else: the other agents do the rest.",
        modes::system_prompt(&modes::AGENT, ""),
        workspace.root().display(),
        index + 1,
        tasks.len(),
        clip(goal.trim(), MAX_GOAL),
        task.title,
    );
    if !before.is_empty() {
        system.push_str("\n\nWhat the agents before you reported:");
        for line in before {
            system.push_str("\n- ");
            system.push_str(line);
        }
    }
    Request {
        model: model.to_string(),
        system_prompt: system,
        messages: vec![Message::user(task.instructions.clone())],
        sampling: modes::AGENT.sampling,
        tools: Vec::new(),
        response_format: None,
    }
}

/// Runs a fleet on `goal` in `workspace`: the plan, then each agent in turn
/// (a stopped or failed one doesn't end the others), each judged by `judge`
/// when it finishes. `model` is the chat model ("" for the backend's own).
/// Returns when every agent has ended.
pub fn run(
    backend: &dyn Backend,
    judge: Option<&dyn Judge>,
    model: &str,
    goal: &str,
    workspace: &Workspace,
    control: &Control,
    host: &mut dyn FleetHost,
) -> Finished {
    let fail = |error: String| Finished {
        reports: Vec::new(),
        error: Some(error),
    };
    // The coordinator, asked once.
    let mut answer = String::new();
    let asked = backend.complete(&plan_request(model, goal), control.whole(), &mut |event| {
        if let Event::Text(piece) = event {
            answer.push_str(piece);
        }
    });
    if let Err(e) = asked {
        return fail(e.to_string());
    }
    if control.is_stopped() {
        return Finished {
            reports: Vec::new(),
            error: None,
        };
    }
    let tasks = match parse_plan(&answer) {
        Ok(tasks) => tasks,
        Err(e) => return fail(e),
    };
    let flags = control.begin(tasks.len());
    host.planned(&tasks);

    let mut reports: Vec<Report> = Vec::new();
    let mut error = None;
    let mut before: Vec<String> = Vec::new();
    for (index, task) in tasks.iter().enumerate() {
        let flag = &flags[index];
        // Stopped before its turn (itself, or the whole fleet, or an
        // earlier agent found the server gone).
        if flag.load(Ordering::Relaxed) || error.is_some() {
            host.status(index, Status::Stopped);
            reports.push(Report {
                status: Status::Stopped,
                steps: 0,
                reply: String::new(),
                confidence: None,
            });
            continue;
        }
        host.status(index, Status::Working);
        let request = agent_request(model, goal, &tasks, index, &before, workspace);
        let mut agent_host = AgentHost {
            index,
            host: &mut *host,
            steps: 0,
            reply: String::new(),
            rate: Rate::default(),
            sent: Instant::now(),
        };
        let result = agent::run(backend, request, workspace, flag, &mut agent_host);
        let (steps, reply) = (agent_host.steps, agent_host.reply.trim().to_string());
        let stopped = flag.load(Ordering::Relaxed);
        let (status, said) = match result {
            _ if stopped => (Status::Stopped, reply.clone()),
            Ok(()) if reply.is_empty() && steps == 0 => {
                (Status::Failed, "The agent sent an empty reply.".to_string())
            }
            Ok(()) => (Status::Done, reply.clone()),
            Err(e) => {
                if matches!(e, BackendError::Unreachable(_)) {
                    error = Some(e.to_string());
                }
                (Status::Failed, e.to_string())
            }
        };
        if !said.is_empty() {
            host.line(index, &one_line(&said, MAX_LINE));
        }
        host.status(index, status);
        let mut confidence = None;
        if status == Status::Done
            && let Some(judge) = judge
        {
            match judge.finished(task, &reply) {
                Ok(p) => {
                    confidence = Some(p);
                    host.judged(index, p);
                }
                Err(e) => log::warn!("SystemOne couldn't judge agent {}: {e}", index + 1),
            }
        }
        before.push(format!(
            "{}: {}",
            task.title,
            if status == Status::Done && !said.is_empty() {
                one_line(&said, MAX_BRIEF)
            } else {
                format!("{} without a report", status.as_str())
            }
        ));
        reports.push(Report {
            status,
            steps,
            reply: said,
            confidence,
        });
    }
    Finished { reports, error }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::path::PathBuf;

    // ---- the plan

    #[test]
    fn a_plan_is_read_and_bounded() {
        let plan = r#"{"tasks":[
            {"title":"  Read\nthe code ","instructions":"Look at src/.\u0007\nThen report."},
            {"title":"","instructions":"Add a test for the parser"},
            {"title":"Docs"},
            {"title":"","instructions":""}
        ]}"#;
        let tasks = parse_plan(plan).unwrap();
        assert_eq!(tasks.len(), 3, "an empty entry is skipped: {tasks:?}");
        assert_eq!(tasks[0].title, "Read the code");
        // Line breaks stay in the instructions; control characters don't.
        assert_eq!(tasks[0].instructions, "Look at src/.\nThen report.");
        // A missing title comes from the instructions, and the other way.
        assert_eq!(tasks[1].title, "Add a test for the parser");
        assert_eq!(tasks[2].title, "Docs");
        assert_eq!(tasks[2].instructions, "Docs");
    }

    #[test]
    fn a_plan_in_a_fence_or_a_sentence() {
        let one = r#"{"tasks":[{"title":"A","instructions":"do a"}]}"#;
        for wrapped in [
            format!("```json\n{one}\n```"),
            format!("Here is the plan: {one} Good luck."),
            one.to_string(),
            r#"[{"title":"A","instructions":"do a"}]"#.to_string(),
        ] {
            assert_eq!(parse_plan(&wrapped).unwrap().len(), 1, "{wrapped}");
        }
    }

    #[test]
    fn a_plan_that_is_too_big_or_not_one() {
        // Seven tasks: the first six. Long text: cut.
        let many: Vec<Value> = (0..7)
            .map(|i| json!({"title": format!("T{i}"), "instructions": "é".repeat(5000)}))
            .collect();
        let tasks = parse_plan(&json!({ "tasks": many }).to_string()).unwrap();
        assert_eq!(tasks.len(), MAX_TASKS);
        assert_eq!(tasks[5].title, "T5");
        assert_eq!(tasks[0].instructions.chars().count(), MAX_INSTRUCTIONS);
        let long = parse_plan(
            &json!({"tasks": [{"title": "w ".repeat(100), "instructions": "x"}]}).to_string(),
        )
        .unwrap();
        assert_eq!(long[0].title.chars().count(), MAX_TITLE);
        assert!(long[0].title.ends_with('…'));

        for bad in [
            "",
            "I cannot do that.",
            "{}",
            r#"{"tasks": []}"#,
            r#"{"tasks": "none"}"#,
            r#"{"tasks": [{"x": 1}, 5, null]}"#,
            "} {",
        ] {
            assert!(parse_plan(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn the_plan_request() {
        let request = plan_request("qwen", &"g".repeat(MAX_GOAL + 100));
        assert_eq!(request.model, "qwen");
        assert!(request.tools.is_empty());
        let format = request.response_format.unwrap();
        assert_eq!(format["type"], "json_object");
        assert_eq!(
            format["schema"]["properties"]["tasks"]["maxItems"],
            MAX_TASKS
        );
        assert!(request.system_prompt.contains("2 to 6"));
        let text = &request.messages[0].text;
        assert!(text.chars().count() < MAX_GOAL + 100, "the goal is cut");
    }

    // ---- a scripted team

    /// What the model does next: answer with text and tool calls, or fail.
    enum Turn {
        Say(&'static str, Vec<ToolCall>),
        Fail(BackendError),
    }

    /// A backend that gives the coordinator's plan, then plays its turns in
    /// order to whichever agent asks (they run one at a time); with none
    /// left it says "All done.".
    struct Team {
        plan: String,
        turns: Mutex<VecDeque<Turn>>,
        seen: Mutex<Vec<Request>>,
    }

    impl Team {
        fn new(plan: &str, turns: Vec<Turn>) -> Team {
            Team {
                plan: plan.to_string(),
                turns: Mutex::new(turns.into()),
                seen: Mutex::new(Vec::new()),
            }
        }
    }

    impl Backend for Team {
        fn name(&self) -> String {
            "team".into()
        }
        fn models(&self) -> Result<Vec<String>, BackendError> {
            Ok(Vec::new())
        }
        fn complete(
            &self,
            request: &Request,
            _cancel: &AtomicBool,
            emit: &mut dyn FnMut(Event<'_>),
        ) -> Result<(), BackendError> {
            self.seen.lock().unwrap().push(request.clone());
            if request.response_format.is_some() {
                emit(Event::Text(&self.plan));
                return Ok(());
            }
            match self.turns.lock().unwrap().pop_front() {
                None => emit(Event::Text("All done.")),
                Some(Turn::Fail(e)) => return Err(e),
                Some(Turn::Say(text, calls)) => {
                    emit(Event::Text(text));
                    if !calls.is_empty() {
                        emit(Event::ToolCalls(&calls));
                    }
                }
            }
            Ok(())
        }
    }

    const TWO: &str = r#"{"tasks":[
        {"title":"Look","instructions":"Read a.txt"},
        {"title":"Write","instructions":"Write b.txt"}]}"#;

    fn call(id: &str, name: &str, args: &str) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: name.into(),
            arguments: args.into(),
        }
    }

    fn workspace(name: &str) -> (PathBuf, Workspace) {
        let dir = std::env::temp_dir().join(format!("gates-fleet-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "alpha\n").unwrap();
        let ws = Workspace::open(&dir).unwrap();
        (dir, ws)
    }

    /// Records every event; answers approvals with `answer`; may stop things
    /// when the plan lands.
    struct Log<'a> {
        events: Vec<String>,
        answer: Approval,
        control: &'a Control,
        on_plan: fn(&Control),
    }

    impl<'a> Log<'a> {
        fn new(control: &'a Control) -> Log<'a> {
            Log {
                events: Vec::new(),
                answer: Approval::Deny,
                control,
                on_plan: |_| {},
            }
        }
    }

    impl FleetHost for Log<'_> {
        fn planned(&mut self, tasks: &[Task]) {
            let titles: Vec<&str> = tasks.iter().map(|t| t.title.as_str()).collect();
            self.events.push(format!("plan {}", titles.join(",")));
            (self.on_plan)(self.control);
        }
        fn status(&mut self, agent: usize, status: Status) {
            self.events.push(format!("{agent} {}", status.as_str()));
        }
        fn step(&mut self, agent: usize, steps: usize, line: &str) {
            self.events.push(format!("{agent} step {steps} {line}"));
        }
        fn speed(&mut self, _: usize, _: f64) {}
        fn line(&mut self, agent: usize, line: &str) {
            self.events.push(format!("{agent} line {line}"));
        }
        fn approve(&mut self, agent: usize, _: &ToolCall, title: &str, _: &str) -> Approval {
            self.events.push(format!("{agent} ask {title}"));
            self.answer
        }
        fn judged(&mut self, agent: usize, confidence: f64) {
            self.events.push(format!("{agent} judged {confidence:.2}"));
        }
    }

    /// Says 0.9 for a task whose reply says "All done", else it can't tell.
    struct Judging;
    impl Judge for Judging {
        fn finished(&self, task: &Task, reply: &str) -> Result<f64, String> {
            assert!(!task.instructions.is_empty());
            if reply.contains("All done") {
                Ok(0.9)
            } else {
                Err("SystemOne is resting after a failed start.".into())
            }
        }
    }

    #[test]
    fn agents_run_in_turn_and_are_judged() {
        let (dir, ws) = workspace("turn");
        let backend = Team::new(
            TWO,
            vec![
                Turn::Say(
                    "Looking.",
                    vec![call("1", "read_file", r#"{"path":"a.txt"}"#)],
                ),
                Turn::Say("The file says alpha.", vec![]),
                // The second agent's only turn: no judge answer (no "All done").
                Turn::Say("Wrote it up.", vec![]),
            ],
        );
        let control = Control::new();
        let mut host = Log::new(&control);
        let done = run(
            &backend,
            Some(&Judging),
            "qwen",
            "Tidy up",
            &ws,
            &control,
            &mut host,
        );
        assert_eq!(done.error, None);
        assert_eq!(
            host.events,
            vec![
                "plan Look,Write",
                "0 working",
                "0 step 1 ✓ Read a.txt (lines 1–1 of 1)",
                "0 line The file says alpha.",
                "0 done",
                "1 working",
                "1 line Wrote it up.",
                "1 done",
            ]
        );
        assert_eq!(done.reports.len(), 2);
        assert_eq!(done.reports[0].steps, 1);
        assert_eq!(done.reports[0].confidence, None, "the judge was unsure");
        assert_eq!(done.reports[1].status, Status::Done);

        let seen = backend.seen.lock().unwrap();
        // The coordinator: JSON asked for, no tools. Then agents, with tools.
        assert!(seen[0].response_format.is_some() && seen[0].tools.is_empty());
        assert!(seen[1].response_format.is_none());
        assert_eq!(seen[1].tools.len(), crate::tools::TOOLS.len());
        assert_eq!(seen[1].model, "qwen");
        assert_eq!(seen[1].messages[0].text, "Read a.txt");
        assert!(seen[1].system_prompt.contains("agent 1 of 2"));
        assert!(seen[1].system_prompt.contains("Tidy up"));
        assert!(!seen[1].system_prompt.contains("agents before you"));
        // The second agent is told what the first reported.
        let last = seen.last().unwrap();
        assert_eq!(last.messages[0].text, "Write b.txt");
        assert!(
            last.system_prompt.contains("- Look: The file says alpha."),
            "{}",
            last.system_prompt
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_finished_agent_gets_its_confidence() {
        let (dir, ws) = workspace("judge");
        let backend = Team::new(TWO, vec![Turn::Say("All done.", vec![])]);
        let control = Control::new();
        let mut host = Log::new(&control);
        let done = run(&backend, Some(&Judging), "", "g", &ws, &control, &mut host);
        assert!(host.events.contains(&"0 judged 0.90".to_string()));
        assert_eq!(done.reports[0].confidence, Some(0.9));
        // Without SystemOne, nothing is asked.
        let backend = Team::new(TWO, vec![]);
        let mut host = Log::new(&control);
        let done = run(&backend, None, "", "g", &ws, &Control::new(), &mut host);
        assert!(!host.events.iter().any(|e| e.contains("judged")));
        assert!(done.reports.iter().all(|r| r.confidence.is_none()));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn approvals_name_their_agent() {
        let (dir, ws) = workspace("ask");
        let write = call("1", "write_file", r#"{"path":"b.txt","content":"beta"}"#);
        let backend = Team::new(
            TWO,
            vec![
                Turn::Say("Skip.", vec![]),
                Turn::Say("Writing.", vec![write.clone()]),
            ],
        );
        let control = Control::new();
        let mut host = Log::new(&control);
        run(&backend, None, "", "g", &ws, &control, &mut host);
        let asked: Vec<&String> = host.events.iter().filter(|e| e.contains("ask")).collect();
        assert_eq!(asked, vec!["1 ask Write b.txt (1 lines, 4 characters)"]);
        // Waiting while it asks, working again after.
        let i = host
            .events
            .iter()
            .position(|e| e.starts_with("1 ask"))
            .unwrap();
        assert_eq!(host.events[i - 1], "1 waiting");
        assert_eq!(host.events[i + 1], "1 working");
        // Denied: nothing written.
        assert!(!dir.join("b.txt").exists());

        // Allowed: written.
        let backend = Team::new(
            TWO,
            vec![
                Turn::Say("Skip.", vec![]),
                Turn::Say("Writing.", vec![write]),
            ],
        );
        let mut host = Log::new(&control);
        host.answer = Approval::Allow;
        run(&backend, None, "", "g", &ws, &Control::new(), &mut host);
        assert_eq!(std::fs::read_to_string(dir.join("b.txt")).unwrap(), "beta");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn one_agent_stopped_is_skipped() {
        let (dir, ws) = workspace("one");
        let backend = Team::new(TWO, vec![]);
        let control = Control::new();
        let mut host = Log::new(&control);
        host.on_plan = |c| c.stop_agent(1);
        let done = run(&backend, None, "", "g", &ws, &control, &mut host);
        assert_eq!(done.reports[0].status, Status::Done);
        assert_eq!(done.reports[1].status, Status::Stopped);
        assert!(host.events.contains(&"1 stopped".to_string()));
        // Only the coordinator and agent 0 asked the model.
        assert_eq!(backend.seen.lock().unwrap().len(), 2);
        assert!(control.agent_stopped(1) && !control.agent_stopped(0) && !control.is_stopped());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_whole_fleet_stopped() {
        let (dir, ws) = workspace("all");
        let backend = Team::new(TWO, vec![]);
        let control = Control::new();
        let mut host = Log::new(&control);
        host.on_plan = |c| c.stop_all();
        let done = run(&backend, None, "", "g", &ws, &control, &mut host);
        assert!(done.reports.iter().all(|r| r.status == Status::Stopped));
        assert_eq!(done.error, None);
        assert_eq!(backend.seen.lock().unwrap().len(), 1, "no agent ran");

        // Stopped while the coordinator thinks: no plan is acted on.
        let backend = Team::new(TWO, vec![]);
        let control = Control::new();
        control.stop_all();
        let mut host = Log::new(&control);
        let done = run(&backend, None, "", "g", &ws, &control, &mut host);
        assert!(done.reports.is_empty() && done.error.is_none());
        assert!(host.events.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn failures_are_reported_and_the_others_go_on() {
        let (dir, ws) = workspace("fail");
        let backend = Team::new(
            TWO,
            vec![Turn::Fail(BackendError::Refused(
                "Model overloaded.".into(),
            ))],
        );
        let control = Control::new();
        let mut host = Log::new(&control);
        let done = run(&backend, Some(&Judging), "", "g", &ws, &control, &mut host);
        assert_eq!(done.reports[0].status, Status::Failed);
        assert_eq!(done.reports[0].reply, "Model overloaded.");
        assert!(
            host.events
                .contains(&"0 line Model overloaded.".to_string())
        );
        assert_eq!(done.reports[1].status, Status::Done);
        // A failed agent isn't judged.
        assert!(!host.events.iter().any(|e| e.starts_with("0 judged")));
        assert_eq!(done.error, None);

        // The server gone: the rest are not tried.
        let backend = Team::new(
            TWO,
            vec![Turn::Fail(BackendError::Unreachable(
                "Nothing answered.".into(),
            ))],
        );
        let mut host = Log::new(&control);
        let done = run(&backend, None, "", "g", &ws, &Control::new(), &mut host);
        assert_eq!(done.reports[0].status, Status::Failed);
        assert_eq!(done.reports[1].status, Status::Stopped);
        assert_eq!(done.error.as_deref(), Some("Nothing answered."));

        // A silent model is a failure, not a success.
        let backend = Team::new(TWO, vec![Turn::Say("", vec![])]);
        let mut host = Log::new(&control);
        let done = run(&backend, None, "", "g", &ws, &Control::new(), &mut host);
        assert_eq!(done.reports[0].status, Status::Failed);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_bad_plan_is_an_error() {
        let (dir, ws) = workspace("bad");
        let backend = Team::new("Sorry, I can't.", vec![]);
        let control = Control::new();
        let mut host = Log::new(&control);
        let done = run(&backend, None, "", "g", &ws, &control, &mut host);
        assert!(done.reports.is_empty());
        assert!(done.error.unwrap().contains("plan"));
        assert!(host.events.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_status_names() {
        let all = [
            Status::Idle,
            Status::Working,
            Status::Waiting,
            Status::Done,
            Status::Failed,
            Status::Stopped,
        ];
        let names: Vec<&str> = all.iter().map(|s| s.as_str()).collect();
        assert_eq!(
            names,
            ["idle", "working", "waiting", "done", "failed", "stopped"]
        );
    }
}
