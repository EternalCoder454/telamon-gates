//! Agent mode's tools: plain Rust run inside Gates (no interpreter, no
//! server, nothing spawned except the one command `run_command` is asked to
//! run). They work inside one folder, the conversation's workspace. Every
//! path is checked to stay inside it, links included, and every result is
//! bounded, so a model can't make Gates read or send the whole disk.
//!
//! A tool's call comes from the model, which is untrusted: reading tools run
//! at once; `write_file`, `edit_file` and `run_command` change things and run
//! only after the user allows them (`Effect`).
//!
//! Blocks (files, a command): call from a worker thread.

use serde_json::{Value, json};
use std::fs;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::workbench::Sink;

/// The most a result gives the model, in bytes.
const MAX_OUTPUT: usize = 32 * 1024;
/// The largest file `read_file` and `search` look into.
const MAX_FILE: u64 = 4 * 1024 * 1024;
/// Entries `list_dir` and matches `search`/`find_files` give at most.
const MAX_ENTRIES: usize = 400;
/// Files `search` and `find_files` walk through at most.
const MAX_WALK: usize = 50_000;
/// Folders a walk skips: build output, dependencies, version control.
pub(crate) const SKIP: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    "build",
    "dist",
    ".venv",
    "__pycache__",
    ".cache",
];
/// How long a command may run, by default and at most.
const RUN_DEFAULT: Duration = Duration::from_secs(60);
const RUN_MAX: Duration = Duration::from_secs(300);

/// What running a tool does to the computer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// Only reads: runs at once.
    Read,
    /// Changes files in the workspace: asks first.
    Write,
    /// Runs a program: asks first, every time.
    Run,
}

pub struct Spec {
    pub name: &'static str,
    pub effect: Effect,
    description: &'static str,
    parameters: fn() -> Value,
}

/// Every tool, as the model is offered them.
pub const TOOLS: &[Spec] = &[
    Spec {
        name: "list_dir",
        effect: Effect::Read,
        description: "List a folder in the workspace: names, kinds and sizes.",
        parameters: || {
            json!({"type": "object", "properties": {
                "path": {"type": "string", "description": "Folder, relative to the workspace; \".\" for its top."}
            }})
        },
    },
    Spec {
        name: "read_file",
        effect: Effect::Read,
        description: "Read a text file in the workspace, with line numbers. Long files come in parts: give start_line for the next.",
        parameters: || {
            json!({"type": "object", "properties": {
                "path": {"type": "string"},
                "start_line": {"type": "integer", "description": "First line, from 1."},
                "max_lines": {"type": "integer", "description": "At most this many lines (default 400)."}
            }, "required": ["path"]})
        },
    },
    Spec {
        name: "search",
        effect: Effect::Read,
        description: "Search the workspace's text files for a string (case-insensitive unless it has capitals). Gives file:line: text.",
        parameters: || {
            json!({"type": "object", "properties": {
                "query": {"type": "string"},
                "path": {"type": "string", "description": "Folder or file to search; the whole workspace by default."}
            }, "required": ["query"]})
        },
    },
    Spec {
        name: "find_files",
        effect: Effect::Read,
        description: "Find files whose path contains a string, or matches a pattern with * and ?.",
        parameters: || {
            json!({"type": "object", "properties": {
                "pattern": {"type": "string", "description": "Such as \"main.rs\" or \"src/*.qml\"."}
            }, "required": ["pattern"]})
        },
    },
    Spec {
        name: "now",
        effect: Effect::Read,
        description: "The current local date, time, weekday and time zone. Use it for anything that depends on today's date.",
        parameters: || json!({"type": "object", "properties": {}}),
    },
    Spec {
        name: "calculate",
        effect: Effect::Read,
        description: "Evaluate an arithmetic expression exactly, such as \"(3 + 4) * 2^10 / 7\". Operators + - * / % ^ and brackets; functions sqrt, abs, round, floor, ceil, min, max, ln, log10, sin, cos, tan (radians); constants pi and e.",
        parameters: || {
            json!({"type": "object", "properties": {
                "expression": {"type": "string"}
            }, "required": ["expression"]})
        },
    },
    Spec {
        name: "write_file",
        effect: Effect::Write,
        description: "Create or replace a file in the workspace with the given text. The user is asked first.",
        parameters: || {
            json!({"type": "object", "properties": {
                "path": {"type": "string"},
                "content": {"type": "string"}
            }, "required": ["path", "content"]})
        },
    },
    Spec {
        name: "edit_file",
        effect: Effect::Write,
        description: "Replace one exact piece of text in a file with another. old_text must appear exactly once. The user is asked first.",
        parameters: || {
            json!({"type": "object", "properties": {
                "path": {"type": "string"},
                "old_text": {"type": "string"},
                "new_text": {"type": "string"}
            }, "required": ["path", "old_text", "new_text"]})
        },
    },
    Spec {
        name: "run_command",
        effect: Effect::Run,
        description: "Run a shell command in the workspace folder and get its output and exit status. The user is asked every time.",
        parameters: || {
            json!({"type": "object", "properties": {
                "command": {"type": "string"},
                "timeout_seconds": {"type": "integer", "description": "Default 60, at most 300."}
            }, "required": ["command"]})
        },
    },
];

/// The web tools: they only read, so they run at once, with no question to
/// the user. Offered alone in Chat and Code (when Settings turn the web on)
/// and beside `TOOLS` in Agent mode. What they return is untrusted
/// (`web::UNTRUSTED`).
pub const WEB_TOOLS: &[Spec] = &[
    Spec {
        name: "web_search",
        effect: Effect::Read,
        description: "Search the web: titles, addresses and snippets.",
        parameters: || {
            json!({"type": "object", "properties": {
                "query": {"type": "string", "description": "Search words."},
                "count": {"type": "integer", "description": "1 to 8, default 5."}
            }, "required": ["query"]})
        },
    },
    Spec {
        name: "fetch_page",
        effect: Effect::Read,
        description: "Read a web page as text. The address must come from a search result, a page or the user.",
        parameters: || {
            json!({"type": "object", "properties": {
                "url": {"type": "string", "description": "Full https:// address."}
            }, "required": ["url"]})
        },
    },
];

fn schema_of(specs: &[Spec]) -> Vec<Value> {
    specs
        .iter()
        .map(|t| {
            json!({"type": "function", "function": {
                "name": t.name,
                "description": t.description,
                "parameters": (t.parameters)(),
            }})
        })
        .collect()
}

/// The tools as an OpenAI `tools` array.
pub fn schema() -> Vec<Value> {
    schema_of(TOOLS)
}

/// The web tools as an OpenAI `tools` array.
pub fn web_schema() -> Vec<Value> {
    schema_of(WEB_TOOLS)
}

pub fn spec(name: &str) -> Option<&'static Spec> {
    TOOLS.iter().find(|t| t.name == name)
}

/// Whether `name` is a web tool.
pub fn is_web(name: &str) -> bool {
    WEB_TOOLS.iter().any(|t| t.name == name)
}

/// The answer for a tool that isn't offered in this reply.
pub fn unavailable(name: &str) -> Outcome {
    Outcome::err(format!("There is no tool called {name}."))
}

/// A web tool's most calls in one model turn: more are answered with a
/// refusal, so one turn can't start dozens of requests.
pub const MAX_WEB_CALLS_PER_TURN: usize = 4;

/// Runs web tool `name` with `arguments` (the model's JSON). Blocks, up to
/// the web's own time limits; stops soon after `cancel`.
pub fn run_web(
    session: &crate::web::Session<'_>,
    name: &str,
    arguments: &str,
    cancel: &AtomicBool,
) -> Outcome {
    let args: Value = match serde_json::from_str(if arguments.trim().is_empty() {
        "{}"
    } else {
        arguments
    }) {
        Ok(v @ Value::Object(_)) => v,
        _ => return Outcome::err(format!("The arguments for {name} aren't a JSON object.")),
    };
    let text = |key: &str| args.get(key).and_then(Value::as_str).map(str::trim);
    match name {
        "web_search" => match text("query").filter(|q| !q.is_empty()) {
            Some(query) => {
                let count = args
                    .get("count")
                    .and_then(Value::as_u64)
                    .map_or(crate::web::DEFAULT_RESULTS, |n| n as usize)
                    .clamp(1, crate::web::MAX_RESULTS);
                web_search(session, query, count, cancel)
            }
            None => Outcome::err("web_search needs a query."),
        },
        "fetch_page" => match text("url").filter(|u| !u.is_empty()) {
            Some(url) => fetch_page(session, url, cancel),
            None => Outcome::err("fetch_page needs a url."),
        },
        _ => unavailable(name),
    }
}

fn web_search(
    session: &crate::web::Session<'_>,
    query: &str,
    count: usize,
    cancel: &AtomicBool,
) -> Outcome {
    match session.web().search(query, count, cancel) {
        Ok(results) if results.is_empty() => Outcome::ok(
            format!("No results for \"{query}\". Try different words."),
            format!("Searched for \"{}\" (no results)", shorten(query, 60)),
        ),
        Ok(results) => {
            // The query on one line: the model wrote it, and a line break in
            // it must not make a line that reads as a result (`result_urls`).
            let mut out = format!("Web results for \"{}\"", shorten(query, 200));
            if let Some(day) = today() {
                out.push_str(&format!(" (today is {day})"));
            }
            out.push_str(":\n");
            for (i, r) in results.iter().enumerate() {
                // The address stays on the line after the title: only that
                // line is opened later (`web::session::result_urls`).
                out.push_str(&format!("\n{}. {}\n   {}\n", i + 1, r.title, r.url));
                if !r.age.is_empty() {
                    out.push_str(&format!("   Date: {}\n", r.age));
                }
                if !r.snippet.is_empty() {
                    out.push_str(&format!("   {}\n", r.snippet));
                }
            }
            out.push_str(&format!(
                "\n({} To read a page, call fetch_page with its address.)",
                crate::web::UNTRUSTED
            ));
            // The model may open the results' own addresses.
            let urls: Vec<String> = results.iter().map(|r| r.url.clone()).collect();
            session.show_results(&urls);
            let n = results.len();
            Outcome::ok(
                out,
                format!(
                    "Searched for \"{}\" ({n} result{})",
                    shorten(query, 60),
                    if n == 1 { "" } else { "s" }
                ),
            )
        }
        Err(e) => Outcome::err(format!("Search failed: {e}")),
    }
}

fn fetch_page(session: &crate::web::Session<'_>, url: &str, cancel: &AtomicBool) -> Outcome {
    if !session.allows(url) {
        return Outcome::err(
            "That address was not in a search result, on a page you read, or in the user's \
             messages, so it is not opened. Search for it first.",
        );
    }
    match session.web().fetch(url, cancel) {
        Ok(page) => {
            let mut out = format!("Page: {}\n", page.url);
            if !page.title.is_empty() {
                out.push_str(&format!("Title: {}\n", page.title));
            }
            out.push_str(&format!("({})\n\n{}", crate::web::UNTRUSTED, page.text));
            if page.truncated {
                out.push_str("\n\n(The page is longer; this is its first part.)");
            }
            // The page's own links are not added to what may be opened: a page
            // can carry any number of them, and each fetch is a covert channel
            // (`web/session.rs`).
            let kb = page.text.len().div_ceil(1024);
            Outcome::ok(out, format!("Read {} ({kb} KB)", short_url(&page.url)))
        }
        Err(e) => Outcome::err(format!("Couldn't read {}: {e}", short_url(url))),
    }
}

/// `text` cut to `max` characters, with "…" if it was.
fn shorten(text: &str, max: usize) -> String {
    let one: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= max {
        return one;
    }
    let cut: String = one.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}

/// An address in a few words for the window: "docs.rs/tokio/latest".
pub fn short_url(url: &str) -> String {
    match url::Url::parse(url.trim()) {
        Ok(u) if u.host_str().is_some() => {
            let mut s = u
                .host_str()
                .unwrap_or("")
                .trim_start_matches("www.")
                .to_string();
            s.push_str(u.path().trim_end_matches('/'));
            if u.query().is_some() {
                s.push_str("?…");
            }
            shorten(&s, 70)
        }
        _ => shorten(url, 70),
    }
}

/// What the window says while a tool runs: "Searching: rust async".
pub fn progress(name: &str, arguments: &str) -> String {
    let args: Value = serde_json::from_str(arguments).unwrap_or(Value::Null);
    let text = |key: &str| args.get(key).and_then(Value::as_str).unwrap_or("");
    match name {
        "web_search" => format!("Searching: {}", shorten(text("query"), 80)),
        "fetch_page" => format!("Reading: {}", short_url(text("url"))),
        other => label(other, arguments),
    }
}

/// What a tool gave back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub ok: bool,
    /// For the model: the result, or what went wrong.
    pub output: String,
    /// For the window, one short line: "Read src/main.rs (120 lines)".
    pub summary: String,
}

impl Outcome {
    fn ok(output: String, summary: String) -> Outcome {
        Outcome {
            ok: true,
            output: clip(output),
            summary,
        }
    }

    /// The user said no.
    pub fn declined() -> Outcome {
        Outcome {
            ok: false,
            output: "The user declined this. Ask them what to do instead.".into(),
            summary: "Declined".into(),
        }
    }

    fn err(message: impl Into<String>) -> Outcome {
        let message = message.into();
        Outcome {
            ok: false,
            summary: message.clone(),
            output: format!("Error: {message}"),
        }
    }
}

/// The folder the tools work in.
#[derive(Debug, Clone)]
pub struct Workspace {
    root: PathBuf,
    /// What its commands may reach beyond it (`sandbox.rs`).
    access: crate::sandbox::Access,
}

impl Workspace {
    /// `root`, which must be an existing folder, and not one so wide the
    /// agent could read everything: `/`, a top folder (`/etc`), or the
    /// home folder or one above it.
    pub fn open(root: &Path) -> io::Result<Workspace> {
        let root = root.canonicalize()?;
        if !root.is_dir() {
            return Err(io::Error::new(io::ErrorKind::NotADirectory, "not a folder"));
        }
        let home = std::env::var_os("HOME").and_then(|h| PathBuf::from(h).canonicalize().ok());
        if root.components().count() < 3 || home.is_some_and(|h| h.starts_with(&root)) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "too wide: choose a project folder",
            ));
        }
        Ok(Workspace {
            root,
            access: crate::sandbox::Access::default(),
        })
    }

    /// The same workspace, its commands allowed `access`.
    pub fn with_access(mut self, access: crate::sandbox::Access) -> Workspace {
        self.access = access;
        self
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `path` (relative to the workspace, or absolute inside it) as a real
    /// path inside the workspace. A path that leaves it, also through a
    /// link, is refused. The file need not exist; its folder must.
    pub fn resolve(&self, path: &str) -> Result<PathBuf, String> {
        let path = path.trim();
        let given = Path::new(if path.is_empty() { "." } else { path });
        let joined = if given.is_absolute() {
            given.to_path_buf()
        } else {
            self.root.join(given)
        };
        // `..` is resolved against the real folders, never by text alone.
        let real = match joined.canonicalize() {
            Ok(real) => real,
            Err(_) => {
                let name = joined
                    .file_name()
                    .filter(|n| {
                        !matches!(Path::new(n).components().next(), Some(Component::ParentDir))
                    })
                    .ok_or_else(|| format!("{path} is not a file name."))?;
                let parent = joined
                    .parent()
                    .ok_or_else(|| format!("{path} has no folder."))?
                    .canonicalize()
                    .map_err(|_| format!("The folder of {path} doesn't exist."))?;
                parent.join(name)
            }
        };
        if real.starts_with(&self.root) {
            Ok(real)
        } else {
            Err(format!("{path} is outside the workspace."))
        }
    }

    /// `real` as the model sees it: relative to the workspace.
    pub(crate) fn show(&self, real: &Path) -> String {
        match real.strip_prefix(&self.root) {
            Ok(rel) if rel.as_os_str().is_empty() => ".".into(),
            Ok(rel) => rel.to_string_lossy().into_owned(),
            Err(_) => real.to_string_lossy().into_owned(),
        }
    }
}

/// Runs tool `name` with `arguments` (the model's JSON). `cancel` stops a
/// command or a long walk.
pub fn run(ws: &Workspace, name: &str, arguments: &str, cancel: &AtomicBool) -> Outcome {
    run_with(ws, name, arguments, cancel, None)
}

/// `run`, with what `run_command`'s command prints sent to `sink` as it
/// prints it (the coding workspace's console).
pub fn run_with(
    ws: &Workspace,
    name: &str,
    arguments: &str,
    cancel: &AtomicBool,
    sink: Option<&Arc<dyn Sink>>,
) -> Outcome {
    let args: Value = match serde_json::from_str(if arguments.trim().is_empty() {
        "{}"
    } else {
        arguments
    }) {
        Ok(v @ Value::Object(_)) => v,
        _ => return Outcome::err(format!("The arguments for {name} aren't a JSON object.")),
    };
    let text = |key: &str| args.get(key).and_then(Value::as_str);
    let number = |key: &str| args.get(key).and_then(Value::as_u64);
    match name {
        "list_dir" => list_dir(ws, text("path").unwrap_or(".")),
        "read_file" => match text("path") {
            Some(p) => read_file(
                ws,
                p,
                number("start_line").unwrap_or(1).max(1) as usize,
                number("max_lines").unwrap_or(400).clamp(1, 2000) as usize,
            ),
            None => Outcome::err("read_file needs a path."),
        },
        "search" => match text("query").filter(|q| !q.is_empty()) {
            Some(q) => search(ws, q, text("path").unwrap_or("."), cancel),
            None => Outcome::err("search needs a query."),
        },
        "find_files" => match text("pattern").filter(|p| !p.is_empty()) {
            Some(p) => find_files(ws, p, cancel),
            None => Outcome::err("find_files needs a pattern."),
        },
        "now" => now(),
        "calculate" => match text("expression").filter(|e| !e.trim().is_empty()) {
            Some(e) => calculate(e),
            None => Outcome::err("calculate needs an expression."),
        },
        "write_file" => match (text("path"), text("content")) {
            (Some(p), Some(c)) => write_file(ws, p, c),
            _ => Outcome::err("write_file needs a path and content."),
        },
        "edit_file" => match (text("path"), text("old_text"), text("new_text")) {
            (Some(p), Some(old), Some(new)) => edit_file(ws, p, old, new),
            _ => Outcome::err("edit_file needs a path, old_text and new_text."),
        },
        "run_command" => match text("command").filter(|c| !c.trim().is_empty()) {
            Some(c) => {
                let limit = number("timeout_seconds")
                    .map(Duration::from_secs)
                    .unwrap_or(RUN_DEFAULT)
                    .min(RUN_MAX);
                run_command(ws, c, limit, cancel, sink)
            }
            None => Outcome::err("run_command needs a command."),
        },
        _ => Outcome::err(format!("There is no tool called {name}.")),
    }
}

/// A call in a few words, for the window: "Read src/main.rs".
pub fn label(name: &str, arguments: &str) -> String {
    let args: Value = serde_json::from_str(arguments).unwrap_or(Value::Null);
    let text = |key: &str| {
        let t: String = args
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .chars()
            .take(80)
            .collect();
        t
    };
    match name {
        "list_dir" => format!(
            "List {}",
            if text("path").is_empty() {
                ".".into()
            } else {
                text("path")
            }
        ),
        "read_file" => format!("Read {}", text("path")),
        "search" => format!("Search for \"{}\"", text("query")),
        "find_files" => format!("Find files \"{}\"", text("pattern")),
        "now" => "Check the date and time".into(),
        "calculate" => format!("Calculate {}", text("expression")),
        "write_file" => format!("Write {}", text("path")),
        "edit_file" => format!("Edit {}", text("path")),
        "run_command" => format!("Run {}", text("command")),
        "web_search" => format!("Search the web for \"{}\"", text("query")),
        "fetch_page" => format!("Read {}", text("url")),
        other => other.chars().take(40).collect(),
    }
}

/// Characters that can make text look like something else: controls
/// (but newlines and tabs) and the invisible or direction-changing ones.
fn hidden(c: char) -> bool {
    (c.is_control() && c != '\n' && c != '\t')
        || matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}' | '\u{00AD}')
}

/// `text` with every hidden character shown as `⟨U+202E⟩`, and whether
/// there was one.
fn reveal(text: &str) -> (String, bool) {
    let mut found = false;
    let out = text
        .chars()
        .map(|c| {
            if hidden(c) {
                found = true;
                format!("⟨U+{:04X}⟩", c as u32)
            } else {
                c.to_string()
            }
        })
        .collect();
    (out, found)
}

/// What the user is asked before a changing tool runs: a title and the
/// detail to look at (the new text, or the command), with anything hidden
/// in it shown. An Err (unusable arguments) runs nothing and asks nothing.
pub fn describe(ws: &Workspace, name: &str, arguments: &str) -> Result<(String, String), String> {
    let args: Value = match serde_json::from_str(arguments) {
        Ok(v @ Value::Object(_)) => v,
        _ => return Err(format!("The arguments for {name} aren't a JSON object.")),
    };
    let text = |key: &str| -> Result<String, String> {
        args.get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| format!("{name} needs {key}."))
    };
    let path = |p: &str| match ws.resolve(p) {
        Ok(real) => Ok(ws.show(&real)),
        Err(e) => Err(e),
    };
    // Every line marked, so a line in the old text can't pass as new.
    let mark =
        |sign: &str, t: &str| -> String { t.lines().map(|l| format!("{sign} {l}\n")).collect() };
    let (title, detail) = match name {
        "write_file" => {
            let content = text("content")?;
            (
                format!(
                    "Write {} ({} lines, {} characters)",
                    path(&text("path")?)?,
                    content.lines().count(),
                    content.chars().count()
                ),
                content,
            )
        }
        "edit_file" => {
            let (old, new) = (text("old_text")?, text("new_text")?);
            (
                format!("Edit {}", path(&text("path")?)?),
                format!("{}{}", mark("-", &old), mark("+", &new))
                    .trim_end_matches('\n')
                    .to_string(),
            )
        }
        "run_command" => {
            let command = text("command")?;
            if command.trim().is_empty() {
                return Err("run_command needs a command.".into());
            }
            let limit = args
                .get("timeout_seconds")
                .and_then(Value::as_u64)
                .map(Duration::from_secs)
                .unwrap_or(RUN_DEFAULT)
                .min(RUN_MAX);
            (
                format!(
                    "Run a command in {} (sandboxed{}{}; stopped after {} s; {} lines, {} characters)",
                    ws.root.display(),
                    if ws.access.network {
                        ", with the network"
                    } else {
                        ", no network"
                    },
                    if ws.access.home {
                        ", sees your home folder"
                    } else {
                        ""
                    },
                    limit.as_secs(),
                    command.lines().count(),
                    command.chars().count()
                ),
                command,
            )
        }
        _ => return Err(format!("There is no tool called {name} that asks.")),
    };
    let (detail, tricky) = reveal(&detail);
    let title = if tricky {
        format!("{title}. Careful: it has hidden characters, shown as ⟨U+…⟩")
    } else {
        title
    };
    Ok((title, detail))
}

/// Files whose change runs code later or elsewhere (version control, build
/// scripts, hidden settings, programs): an edit there always asks, even
/// after "Allow All Edits".
pub fn sensitive(ws: &Workspace, name: &str, arguments: &str) -> bool {
    if name != "write_file" && name != "edit_file" {
        return true;
    }
    let args: Value = serde_json::from_str(arguments).unwrap_or(Value::Null);
    let Some(path) = args.get("path").and_then(Value::as_str) else {
        return true;
    };
    let Ok(real) = ws.resolve(path) else {
        return true;
    };
    let shown = ws.show(&real);
    let rel = Path::new(&shown);
    let hidden_part = rel
        .components()
        .any(|c| c.as_os_str().to_string_lossy().starts_with('.') && c.as_os_str() != ".");
    let file = rel
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    const BUILD: &[&str] = &[
        "Makefile",
        "makefile",
        "GNUmakefile",
        "CMakeLists.txt",
        "meson.build",
        "build.rs",
        "Cargo.toml",
        "package.json",
        "pyproject.toml",
        "setup.py",
        "setup.cfg",
        "Dockerfile",
        "Containerfile",
        "justfile",
        "Justfile",
        "flake.nix",
    ];
    let script = matches!(
        rel.extension().and_then(|e| e.to_str()),
        Some("sh" | "bash" | "zsh" | "fish" | "desktop" | "service")
    );
    let executable = {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(&real).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
    };
    hidden_part || BUILD.contains(&file.as_str()) || script || executable
}

/// The file a call of `read_file`, `write_file` or `edit_file` names, as a
/// path relative to the workspace (the same spelling for any way of writing
/// it); None for any other call, and for a path outside the workspace.
pub fn target(ws: &Workspace, name: &str, arguments: &str) -> Option<String> {
    if !matches!(name, "read_file" | "write_file" | "edit_file") {
        return None;
    }
    let args: Value = serde_json::from_str(arguments).ok()?;
    let real = ws.resolve(args.get("path")?.as_str()?).ok()?;
    Some(ws.show(&real))
}

fn list_dir(ws: &Workspace, path: &str) -> Outcome {
    let dir = match ws.resolve(path) {
        Ok(d) => d,
        Err(e) => return Outcome::err(e),
    };
    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) => return Outcome::err(format!("Can't list {path}: {e}.")),
    };
    let mut rows: Vec<(bool, String, u64)> = entries
        .filter_map(Result::ok)
        .map(|e| {
            let meta = e.metadata().ok();
            let is_dir = meta.as_ref().is_some_and(|m| m.is_dir());
            let size = meta.map(|m| m.len()).unwrap_or(0);
            (is_dir, e.file_name().to_string_lossy().into_owned(), size)
        })
        .collect();
    rows.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase()))
    });
    let total = rows.len();
    let mut out = String::new();
    for (is_dir, name, size) in rows.iter().take(MAX_ENTRIES) {
        if *is_dir {
            out.push_str(&format!("{name}/\n"));
        } else {
            out.push_str(&format!("{name}  ({size} bytes)\n"));
        }
    }
    if total > MAX_ENTRIES {
        out.push_str(&format!("… and {} more\n", total - MAX_ENTRIES));
    }
    let shown = ws.show(&dir);
    Outcome::ok(out, format!("Listed {shown} ({total} entries)"))
}

/// The text of a file, or why not: too big, or not text.
pub(crate) fn text_of(path: &Path) -> Result<String, String> {
    let meta = fs::metadata(path).map_err(|e| format!("{e}"))?;
    if !meta.is_file() {
        return Err("not a file".into());
    }
    if meta.len() > MAX_FILE {
        return Err(format!("too big ({} bytes)", meta.len()));
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    fs::File::open(path)
        .and_then(|f| f.take(MAX_FILE).read_to_end(&mut bytes))
        .map_err(|e| format!("{e}"))?;
    if bytes.contains(&0) {
        return Err("not a text file".into());
    }
    String::from_utf8(bytes).map_err(|_| "not UTF-8 text".into())
}

fn read_file(ws: &Workspace, path: &str, start: usize, max: usize) -> Outcome {
    let real = match ws.resolve(path) {
        Ok(p) => p,
        Err(e) => return Outcome::err(e),
    };
    let text = match text_of(&real) {
        Ok(t) => t,
        Err(e) => return Outcome::err(format!("Can't read {path}: {e}.")),
    };
    let total = text.lines().count();
    let mut out = String::new();
    let mut last = start.saturating_sub(1);
    for (i, line) in text.lines().enumerate().skip(start - 1).take(max) {
        let row = format!("{:>5}  {line}\n", i + 1);
        if out.len() + row.len() > MAX_OUTPUT - 200 {
            break;
        }
        out.push_str(&row);
        last = i + 1;
    }
    if last < total {
        out.push_str(&format!(
            "… {} more lines: read on with start_line {}\n",
            total - last,
            last + 1
        ));
    }
    let shown = ws.show(&real);
    Outcome::ok(
        out,
        format!("Read {shown} (lines {start}–{last} of {total})"),
    )
}

/// Every file under `dir`, skipping build output and hidden folders, at
/// most `MAX_WALK`, until `cancel`.
fn walk(dir: &Path, cancel: &AtomicBool) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        if cancel.load(Ordering::Relaxed) || files.len() >= MAX_WALK {
            break;
        }
        let Ok(entries) = fs::read_dir(&d) else {
            continue;
        };
        let mut here: Vec<_> = entries.filter_map(Result::ok).collect();
        here.sort_by_key(|e| e.file_name());
        for e in here {
            let name = e.file_name();
            let name = name.to_string_lossy();
            // Links are not followed: one could lead out of the workspace.
            let Ok(kind) = e.file_type() else { continue };
            if kind.is_dir() {
                if !name.starts_with('.') && !SKIP.contains(&name.as_ref()) {
                    stack.push(e.path());
                }
            } else if kind.is_file() {
                files.push(e.path());
            }
        }
    }
    files.sort();
    files
}

fn search(ws: &Workspace, query: &str, path: &str, cancel: &AtomicBool) -> Outcome {
    let start = match ws.resolve(path) {
        Ok(p) => p,
        Err(e) => return Outcome::err(e),
    };
    let files = if start.is_file() {
        vec![start.clone()]
    } else {
        walk(&start, cancel)
    };
    // Smart case: capitals in the query make it exact.
    let exact = query.chars().any(char::is_uppercase);
    let needle = if exact {
        query.to_string()
    } else {
        query.to_lowercase()
    };
    let mut out = String::new();
    let mut hits = 0;
    let mut in_files = 0;
    'files: for file in &files {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let Ok(text) = text_of(file) else { continue };
        let mut found = false;
        for (i, line) in text.lines().enumerate() {
            let hay = if exact {
                line.to_string()
            } else {
                line.to_lowercase()
            };
            if hay.contains(&needle) {
                if !found {
                    in_files += 1;
                    found = true;
                }
                hits += 1;
                let line: String = line.trim().chars().take(200).collect();
                out.push_str(&format!("{}:{}: {line}\n", ws.show(file), i + 1));
                if hits >= MAX_ENTRIES || out.len() > MAX_OUTPUT {
                    out.push_str("… more matches: search a narrower path\n");
                    break 'files;
                }
            }
        }
    }
    if hits == 0 {
        out = format!("No match for \"{query}\" in {} files.\n", files.len());
    }
    Outcome::ok(
        out,
        format!("Searched for \"{query}\" ({hits} matches in {in_files} files)"),
    )
}

/// Whether `name` matches `pattern` with `*` (any run) and `?` (one char).
fn glob(pattern: &str, name: &str) -> bool {
    let (p, n): (Vec<char>, Vec<char>) = (pattern.chars().collect(), name.chars().collect());
    let (mut pi, mut ni, mut star, mut mark) = (0, 0, None, 0);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ni;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

fn find_files(ws: &Workspace, pattern: &str, cancel: &AtomicBool) -> Outcome {
    let files = walk(&ws.root, cancel);
    let wild = pattern.contains(['*', '?']);
    let lower = pattern.to_lowercase();
    let mut out = String::new();
    let mut n = 0;
    for f in &files {
        let rel = ws.show(f);
        let hit = if wild {
            glob(pattern, &rel)
                || f.file_name()
                    .is_some_and(|name| glob(pattern, &name.to_string_lossy()))
        } else {
            rel.to_lowercase().contains(&lower)
        };
        if hit {
            n += 1;
            if n <= MAX_ENTRIES {
                out.push_str(&rel);
                out.push('\n');
            }
        }
    }
    if n > MAX_ENTRIES {
        out.push_str(&format!("… and {} more\n", n - MAX_ENTRIES));
    }
    if n == 0 {
        out = format!("No file matches \"{pattern}\".\n");
    }
    Outcome::ok(out, format!("Found {n} files for \"{pattern}\""))
}

/// Writes `text` to `path` whole: a new temporary file beside it, then a
/// rename. The temporary name is random and made fresh (never a link a
/// repository could have planted there).
pub(crate) fn replace(path: &Path, text: &str) -> io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.gates-{}", random_hex()?));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(libc::O_NOFOLLOW)
        .mode(0o600)
        .open(&tmp)?;
    if let Err(e) = file
        .write_all(text.as_bytes())
        .and_then(|()| file.sync_all())
    {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    drop(file);
    // A new file gets the usual permissions.
    if !path.exists() {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o644));
    }
    // Keep the old file's permissions (a script stays executable).
    if let Ok(meta) = fs::metadata(path) {
        let _ = fs::set_permissions(&tmp, meta.permissions());
    }
    fs::rename(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

fn write_file(ws: &Workspace, path: &str, content: &str) -> Outcome {
    // A new file's folders are made first, inside the workspace only: the
    // nearest folder that exists must be inside it (links resolved), and
    // the rest of the path may not climb with `..`.
    let given = Path::new(path.trim());
    let target = if given.is_absolute() {
        given.to_path_buf()
    } else {
        ws.root.join(given)
    };
    if let Some(parent) = target.parent()
        && !parent.exists()
    {
        let mut existing = parent;
        while !existing.exists() {
            match existing.parent() {
                Some(up) => existing = up,
                None => break,
            }
        }
        let rest_climbs = parent
            .strip_prefix(existing)
            .map(|rest| {
                rest.components()
                    .any(|c| !matches!(c, Component::Normal(_)))
            })
            .unwrap_or(true);
        let inside = existing
            .canonicalize()
            .is_ok_and(|real| real.starts_with(&ws.root));
        if rest_climbs || !inside {
            return Outcome::err(format!("{path} is outside the workspace."));
        }
        if let Err(e) = fs::create_dir_all(parent) {
            return Outcome::err(format!("Can't make the folder for {path}: {e}."));
        }
    }
    let real = match ws.resolve(path) {
        Ok(p) => p,
        Err(e) => return Outcome::err(e),
    };
    if real.is_dir() {
        return Outcome::err(format!("{path} is a folder."));
    }
    let existed = real.exists();
    match replace(&real, content) {
        Ok(()) => {
            let shown = ws.show(&real);
            let lines = content.lines().count();
            let verb = if existed { "Replaced" } else { "Created" };
            Outcome::ok(
                format!("{verb} {shown} ({lines} lines)."),
                format!("{verb} {shown} ({lines} lines)"),
            )
        }
        Err(e) => Outcome::err(format!("Can't write {path}: {e}.")),
    }
}

fn edit_file(ws: &Workspace, path: &str, old: &str, new: &str) -> Outcome {
    let real = match ws.resolve(path) {
        Ok(p) => p,
        Err(e) => return Outcome::err(e),
    };
    let text = match text_of(&real) {
        Ok(t) => t,
        Err(e) => return Outcome::err(format!("Can't read {path}: {e}.")),
    };
    if old.trim().is_empty() {
        return Outcome::err(
            "old_text is empty: to add text, put the line it goes after in old_text, and that line with the new text in new_text.",
        );
    }
    // As written first. Not there: models copy what read_file showed, so
    // its line numbers come off (and off the new text, if it has them too),
    // then lines match whatever their indentation. Either way it must be
    // in the file once.
    let (old, new) = match text.matches(old).count() {
        0 => match unnumber(old) {
            Some(bare) => (bare, unnumber(new).unwrap_or_else(|| new.to_string())),
            None => (old.to_string(), new.to_string()),
        },
        _ => (old.to_string(), new.to_string()),
    };
    if old.trim().is_empty() {
        return Outcome::err(
            "old_text is empty: to add text, put the line it goes after in old_text, and that line with the new text in new_text.",
        );
    }
    let edited = match text.matches(old.as_str()).count() {
        1 => text.replacen(old.as_str(), &new, 1),
        0 => match loose_match(&text, &old) {
            Ok(range) => format!("{}{}{}", &text[..range.start], new, &text[range.end..]),
            Err(0) => {
                return Outcome::err(format!(
                    "old_text isn't in {path}: read the file and copy the lines exactly, without line numbers."
                ));
            }
            Err(n) => {
                return Outcome::err(format!(
                    "old_text is in {path} {n} times: include more of the lines around it."
                ));
            }
        },
        n => {
            return Outcome::err(format!(
                "old_text is in {path} {n} times: include more of the lines around it."
            ));
        }
    };
    match replace(&real, &edited) {
        Ok(()) => {
            let shown = ws.show(&real);
            let (minus, plus) = (old.lines().count(), new.lines().count());
            Outcome::ok(
                format!("Edited {shown}: {minus} lines replaced by {plus}."),
                format!("Edited {shown} (−{minus} +{plus})"),
            )
        }
        Err(e) => Outcome::err(format!("Can't write {path}: {e}.")),
    }
}

/// `text` without the line numbers `read_file` puts in front ("   12  "),
/// when every line has one and some line has text after it; else None.
fn unnumber(text: &str) -> Option<String> {
    let mut content = false;
    let mut out = Vec::new();
    for line in text.lines() {
        let rest = line.trim_start();
        let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        let after = &rest[digits..];
        if digits == 0 {
            return None;
        }
        match after.strip_prefix("  ") {
            Some(body) => {
                content |= !body.trim().is_empty();
                out.push(body.to_string());
            }
            // A blank line shows as its number alone.
            None if after.trim().is_empty() => out.push(String::new()),
            None => return None,
        }
    }
    if !content {
        return None;
    }
    let mut joined = out.join("\n");
    if text.ends_with('\n') {
        joined.push('\n');
    }
    Some(joined)
}

/// Where `old`'s lines are in `text` as a run of whole lines, comparing
/// each without the spaces at its ends: the byte range, or how many times
/// they are there when not once.
fn loose_match(text: &str, old: &str) -> Result<std::ops::Range<usize>, usize> {
    let want: Vec<&str> = old.lines().map(str::trim).collect();
    if want.is_empty() || want.iter().all(|l| l.is_empty()) {
        return Err(0);
    }
    // Each line's start and end (without its newline).
    let mut lines = Vec::new();
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        let body = line.strip_suffix('\n').unwrap_or(line);
        lines.push((at, at + body.len(), body.trim()));
        at += line.len();
    }
    let mut found = Vec::new();
    for start in 0..lines.len() {
        if start + want.len() > lines.len() {
            break;
        }
        if want
            .iter()
            .enumerate()
            .all(|(i, w)| lines[start + i].2 == *w)
        {
            found.push(lines[start].0..lines[start + want.len() - 1].1);
        }
    }
    match found.len() {
        1 => Ok(found.remove(0)),
        n => Err(n),
    }
}

/// What a command printed: its start and its end (where errors are), and
/// how much there was. Bounded however much it prints.
#[derive(Default)]
struct Printed {
    head: Vec<u8>,
    tail: std::collections::VecDeque<u8>,
    total: usize,
}

const HEAD: usize = 8 * 1024;
// With the head, the gap note and the exit line: inside MAX_OUTPUT.
const TAIL: usize = 22 * 1024;

impl Printed {
    fn add(&mut self, mut bytes: &[u8]) {
        self.total += bytes.len();
        if self.head.len() < HEAD {
            let take = bytes.len().min(HEAD - self.head.len());
            self.head.extend_from_slice(&bytes[..take]);
            bytes = &bytes[take..];
        }
        self.tail.extend(bytes);
        let over = self.tail.len().saturating_sub(TAIL);
        self.tail.drain(..over);
    }

    fn text(&self) -> String {
        let head = String::from_utf8_lossy(&self.head);
        let tail: Vec<u8> = self.tail.iter().copied().collect();
        let tail = String::from_utf8_lossy(&tail);
        let left_out = self.total - self.head.len() - self.tail.len();
        if left_out == 0 {
            format!("{head}{tail}")
        } else {
            format!("{head}\n… {left_out} bytes left out …\n{tail}")
        }
    }
}

/// How a command ended.
pub(crate) struct Executed {
    pub success: bool,
    /// What it printed, bounded (its start and its end).
    pub output: String,
    /// The exit code, or "a signal"; None when it was stopped.
    pub code: Option<String>,
    pub took: f64,
}

/// Runs `command` in the workspace's sandbox, in its own process group, so
/// a timeout or Stop (`cancel`) ends everything it started; what it prints
/// comes through a pipe, kept bounded in memory (nothing on disk), and goes
/// to `sink` as it comes, with a line on how it ended. Err: it could not be
/// started.
pub(crate) fn execute(
    ws: &Workspace,
    command: &str,
    limit: Duration,
    cancel: &AtomicBool,
    sink: Option<&Arc<dyn Sink>>,
) -> Result<Executed, String> {
    use std::os::unix::process::{CommandExt, ExitStatusExt};
    use std::sync::Mutex;
    let (mut reader, writer) = io::pipe().map_err(|e| format!("Can't run the command: {e}."))?;
    // In its sandbox, always: the workspace, the system's programs, and
    // only what the user allowed beyond.
    let mut sandboxed = crate::sandbox::command(&ws.root, ws.access, command)?;
    let child = writer.try_clone().and_then(|err| {
        let cmd = &mut sandboxed;
        cmd.current_dir(&ws.root)
            .stdin(Stdio::null())
            .stdout(writer)
            .stderr(err)
            .process_group(0);
        // Spawned, the command's copies of the pipe close with `cmd`, so
        // the read ends when the command's own do.
        cmd.spawn()
    });
    let mut child = child.map_err(|e| format!("Can't run the command: {e}."))?;
    let group = child.id() as libc::pid_t;
    let printed = Arc::new(Mutex::new(Printed::default()));
    let (done, finished) = std::sync::mpsc::channel::<()>();
    {
        let printed = printed.clone();
        let sink = sink.cloned();
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                printed
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .add(&buf[..n]);
                if let Some(sink) = &sink {
                    sink.write(&buf[..n]);
                }
            }
            let _ = done.send(());
        });
    }
    // SAFETY: signals to the group of our own child, made for it above.
    let kill_group = |signal| unsafe {
        libc::killpg(group, signal);
    };
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(_) => {
                kill_group(libc::SIGKILL);
                let _ = child.wait();
                break None;
            }
        }
        if cancel.load(Ordering::Relaxed) || started.elapsed() >= limit {
            kill_group(libc::SIGTERM);
            let grace = Instant::now();
            while grace.elapsed() < Duration::from_secs(1) {
                if matches!(child.try_wait(), Ok(Some(_))) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            kill_group(libc::SIGKILL);
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    // Anything it left running in the background ends with it.
    kill_group(libc::SIGKILL);
    // The rest of what it printed; a program that left the group can't
    // hold the answer up for long.
    let _ = finished.recv_timeout(Duration::from_secs(2));
    let output = printed.lock().unwrap_or_else(|e| e.into_inner()).text();
    let took = started.elapsed().as_secs_f64();
    let (success, code, line) = match status {
        Some(s) => {
            let code = s
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "a signal".into());
            let line = match s.code() {
                Some(c) => format!("Exited with code {c} in {took:.1} s"),
                None => format!("Ended by signal {} in {took:.1} s", s.signal().unwrap_or(0)),
            };
            (s.success(), Some(code), line)
        }
        None if cancel.load(Ordering::Relaxed) => {
            (false, None, format!("Stopped after {took:.1} s"))
        }
        None => (
            false,
            None,
            format!("Stopped after {} s (time limit)", limit.as_secs()),
        ),
    };
    if let Some(sink) = sink {
        sink.finish(&line, success);
    }
    Ok(Executed {
        success,
        output,
        code,
        took,
    })
}

/// `execute`, as the tool result the model gets.
fn run_command(
    ws: &Workspace,
    command: &str,
    limit: Duration,
    cancel: &AtomicBool,
    sink: Option<&Arc<dyn Sink>>,
) -> Outcome {
    let ran = match execute(ws, command, limit, cancel, sink) {
        Ok(ran) => ran,
        Err(e) => return Outcome::err(e),
    };
    let Executed {
        success,
        output,
        code,
        took,
    } = ran;
    match code {
        Some(code) => Outcome {
            ok: success,
            output: clip(format!("{output}\n[exit status {code}, {took:.1} s]")),
            summary: format!("Ran a command (exit {code}, {took:.1} s)"),
        },
        None if cancel.load(Ordering::Relaxed) => Outcome::err("The command was stopped."),
        None => Outcome {
            ok: false,
            output: clip(format!("{output}\n[stopped after {} s]", limit.as_secs())),
            summary: format!("Ran a command (stopped after {} s)", limit.as_secs()),
        },
    }
}

/// The local date, time and zone, in words a model can read.
/// The local time: the broken-down fields `when` takes, the offset from UTC
/// in seconds and the zone's name. None if the C library can't say.
fn local_time() -> Option<([i32; 7], i64, String)> {
    // SAFETY: `tm` is zeroed plain data that localtime_r fills; the zone
    // name it points to is libc's own string, copied at once.
    unsafe {
        let secs = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&secs, &mut tm).is_null() {
            return None;
        }
        let zone = if tm.tm_zone.is_null() {
            String::new()
        } else {
            std::ffi::CStr::from_ptr(tm.tm_zone)
                .to_string_lossy()
                .into_owned()
        };
        let fields = [
            tm.tm_wday,
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
            tm.tm_sec,
        ];
        Some((fields, tm.tm_gmtoff, zone))
    }
}

/// Today, for the model to weigh how recent a result is: "Friday, 2026-10-09".
fn today() -> Option<String> {
    let (fields, offset, zone) = local_time()?;
    let full = when(fields, offset, &zone);
    // "Friday, 2026-10-09 14:32:05 …": the weekday and the date.
    Some(full.splitn(3, ' ').take(2).collect::<Vec<_>>().join(" "))
}

fn now() -> Outcome {
    let Some((fields, offset, zone)) = local_time() else {
        return Outcome::err("The local time isn't available.");
    };
    let text = when(fields, offset, &zone);
    Outcome::ok(text.clone(), format!("Checked the time: {text}"))
}

/// "Thursday, 2026-10-08 14:32:05 UTC-04:00 (EDT)" from a broken-down time
/// (weekday with Sunday 0, year, month, day, hour, minute, second), the
/// offset from UTC in seconds and the zone's name.
fn when(f: [i32; 7], offset: i64, zone: &str) -> String {
    const DAYS: [&str; 7] = [
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
    ];
    let sign = if offset < 0 { '-' } else { '+' };
    let off = offset.unsigned_abs();
    let zone = if zone.is_empty() {
        String::new()
    } else {
        format!(" ({zone})")
    };
    format!(
        "{}, {:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC{sign}{:02}:{:02}{zone}",
        DAYS[f[0].rem_euclid(7) as usize],
        f[1],
        f[2],
        f[3],
        f[4],
        f[5],
        f[6],
        off / 3600,
        off % 3600 / 60,
    )
}

/// The longest expression `calculate` reads, in characters.
const MAX_EXPRESSION: usize = 1000;
/// How deeply brackets, signs and powers may nest.
const MAX_DEPTH: usize = 64;

/// `calculate`: evaluates `expression` and gives the number.
fn calculate(expression: &str) -> Outcome {
    let chars: Vec<char> = expression.chars().collect();
    if chars.len() > MAX_EXPRESSION {
        return Outcome::err(format!(
            "The expression is too long ({MAX_EXPRESSION} characters at most)."
        ));
    }
    let mut calc = Calc {
        chars,
        at: 0,
        depth: 0,
    };
    match calc.all() {
        Ok(value) => {
            let shown: String = expression.trim().chars().take(60).collect();
            let result = number_text(value);
            Outcome::ok(result.clone(), format!("{shown} = {result}"))
        }
        Err(e) => Outcome::err(e),
    }
}

/// `value` to 12 significant digits, as plain as it goes: "0.3" for
/// 0.1 + 0.2, "1e300" for the huge.
fn number_text(value: f64) -> String {
    if value == 0.0 {
        return "0".into();
    }
    let rounded: f64 = format!("{value:.11e}").parse().unwrap_or(value);
    if (1e-6..1e15).contains(&rounded.abs()) {
        format!("{rounded}")
    } else {
        format!("{rounded:e}")
    }
}

/// A recursive-descent parser that evaluates as it reads: sum → product →
/// sign → power → value. A power binds tighter than the sign before it
/// (`-2^2` is -4) and groups to the right (`2^3^2` is 2^9). `**` is `^`.
struct Calc {
    chars: Vec<char>,
    at: usize,
    depth: usize,
}

impl Calc {
    fn all(&mut self) -> Result<f64, String> {
        let value = self.sum()?;
        match self.peek() {
            None => Ok(value),
            Some(c) => Err(format!("Unexpected \"{c}\" at position {}.", self.at + 1)),
        }
    }

    /// The next character past any spaces.
    fn peek(&mut self) -> Option<char> {
        while self.chars.get(self.at).is_some_and(|c| c.is_whitespace()) {
            self.at += 1;
        }
        self.chars.get(self.at).copied()
    }

    /// Takes `c` if it is next.
    fn eat(&mut self, c: char) -> bool {
        let found = self.peek() == Some(c);
        if found {
            self.at += 1;
        }
        found
    }

    /// Whether a `*` is next that is not the start of `**`.
    fn times(&mut self) -> bool {
        self.peek() == Some('*') && self.chars.get(self.at + 1) != Some(&'*')
    }

    /// One level deeper, unless that is too deep.
    fn enter(&mut self) -> Result<(), String> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err("The expression is nested too deeply.".into());
        }
        Ok(())
    }

    fn sum(&mut self) -> Result<f64, String> {
        let mut value = self.product()?;
        loop {
            if self.eat('+') {
                value = check(value + self.product()?)?;
            } else if self.eat('-') {
                value = check(value - self.product()?)?;
            } else {
                return Ok(value);
            }
        }
    }

    fn product(&mut self) -> Result<f64, String> {
        let mut value = self.sign()?;
        loop {
            if self.times() {
                self.at += 1;
                value = check(value * self.sign()?)?;
            } else if matches!(self.peek(), Some('/' | '%')) {
                let divide = self.chars[self.at] == '/';
                self.at += 1;
                let by = self.sign()?;
                if by == 0.0 {
                    return Err("Division by zero.".into());
                }
                value = check(if divide { value / by } else { value % by })?;
            } else {
                return Ok(value);
            }
        }
    }

    fn sign(&mut self) -> Result<f64, String> {
        self.enter()?;
        let value = if self.eat('-') {
            -self.sign()?
        } else {
            self.eat('+');
            self.power()?
        };
        self.depth -= 1;
        Ok(value)
    }

    fn power(&mut self) -> Result<f64, String> {
        let base = self.value()?;
        if !self.eat('^') {
            if self.peek() == Some('*') && self.chars.get(self.at + 1) == Some(&'*') {
                self.at += 2;
            } else {
                return Ok(base);
            }
        }
        // The exponent may carry a sign and is a power itself: 2^-3, 2^3^2.
        let exponent = self.sign()?;
        check(base.powf(exponent))
    }

    fn value(&mut self) -> Result<f64, String> {
        let Some(c) = self.peek() else {
            return Err("The expression ends too soon.".into());
        };
        if c == '(' {
            self.at += 1;
            self.enter()?;
            let value = self.sum()?;
            self.depth -= 1;
            if !self.eat(')') {
                return Err("A bracket is not closed.".into());
            }
            return Ok(value);
        }
        if c.is_ascii_digit() || c == '.' {
            return self.number();
        }
        if !c.is_ascii_alphabetic() {
            return Err(format!("Unexpected \"{c}\" at position {}.", self.at + 1));
        }
        let start = self.at;
        while self
            .chars
            .get(self.at)
            .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_')
        {
            self.at += 1;
        }
        let name = self.chars[start..self.at]
            .iter()
            .collect::<String>()
            .to_ascii_lowercase();
        if !self.eat('(') {
            return match name.as_str() {
                "pi" => Ok(std::f64::consts::PI),
                "e" => Ok(std::f64::consts::E),
                _ => Err(format!("\"{name}\" is not a known name.")),
            };
        }
        self.enter()?;
        let mut args = vec![self.sum()?];
        while self.eat(',') {
            args.push(self.sum()?);
        }
        self.depth -= 1;
        if !self.eat(')') {
            return Err("A bracket is not closed.".into());
        }
        function(&name, &args)
    }

    /// Digits with an optional point and exponent ("1.5e3").
    fn number(&mut self) -> Result<f64, String> {
        fn digits(c: &mut Calc) {
            while c.chars.get(c.at).is_some_and(char::is_ascii_digit) {
                c.at += 1;
            }
        }
        let start = self.at;
        digits(self);
        if self.chars.get(self.at) == Some(&'.') {
            self.at += 1;
            digits(self);
        }
        if matches!(self.chars.get(self.at), Some('e' | 'E')) {
            let mut next = self.at + 1;
            if matches!(self.chars.get(next), Some('+' | '-')) {
                next += 1;
            }
            if self.chars.get(next).is_some_and(char::is_ascii_digit) {
                self.at = next;
                digits(self);
            }
        }
        let text: String = self.chars[start..self.at].iter().collect();
        match text.parse::<f64>() {
            Ok(v) if v.is_finite() => Ok(v),
            Ok(_) => Err(format!("{text} is too large.")),
            Err(_) => Err(format!("\"{text}\" is not a number.")),
        }
    }
}

/// `value` if it is a real number a computer can hold.
fn check(value: f64) -> Result<f64, String> {
    if value.is_nan() {
        Err("The result is not a real number.".into())
    } else if value.is_infinite() {
        Err("The result is too large.".into())
    } else {
        Ok(value)
    }
}

/// Function `name` of `args`.
fn function(name: &str, args: &[f64]) -> Result<f64, String> {
    let one = |f: fn(f64) -> f64| match args {
        [x] => check(f(*x)),
        _ => Err(format!("{name} takes one number.")),
    };
    match name {
        "sqrt" if args.first().is_some_and(|x| *x < 0.0) => {
            Err("sqrt of a negative number is not real.".into())
        }
        "ln" | "log10" if args.first().is_some_and(|x| *x <= 0.0) => {
            Err(format!("{name} needs a number above zero."))
        }
        "sqrt" => one(f64::sqrt),
        "abs" => one(f64::abs),
        "round" => one(f64::round),
        "floor" => one(f64::floor),
        "ceil" => one(f64::ceil),
        "ln" => one(f64::ln),
        "log10" => one(f64::log10),
        "sin" => one(f64::sin),
        "cos" => one(f64::cos),
        "tan" => one(f64::tan),
        "min" => Ok(args.iter().copied().fold(f64::INFINITY, f64::min)),
        "max" => Ok(args.iter().copied().fold(f64::NEG_INFINITY, f64::max)),
        _ => Err(format!("\"{name}\" is not a known function.")),
    }
}

/// 16 random hex characters.
fn random_hex() -> io::Result<String> {
    let mut bytes = [0u8; 8];
    fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// At most `MAX_OUTPUT` bytes of `text`, cut on a character boundary, the
/// end kept (where a command's errors are).
fn clip(text: String) -> String {
    if text.len() <= MAX_OUTPUT {
        return text;
    }
    let mut cut = text.len() - MAX_OUTPUT;
    while !text.is_char_boundary(cut) {
        cut += 1;
    }
    format!("… {cut} bytes left out …\n{}", &text[cut..])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace(name: &str) -> (PathBuf, Workspace) {
        let dir = std::env::temp_dir().join(format!("gates-tools-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(
            dir.join("src/main.rs"),
            "fn main() {\n    println!(\"Hello\");\n}\n",
        )
        .unwrap();
        fs::write(dir.join("README.md"), "# Demo\nSay hello.\n").unwrap();
        fs::create_dir_all(dir.join("target")).unwrap();
        fs::write(dir.join("target/junk.rs"), "hello from the build").unwrap();
        let ws = Workspace::open(&dir).unwrap();
        (dir, ws)
    }

    fn go(ws: &Workspace, name: &str, args: Value) -> Outcome {
        run(ws, name, &args.to_string(), &AtomicBool::new(false))
    }

    #[test]
    fn the_schema() {
        let schema = schema();
        assert_eq!(schema.len(), TOOLS.len());
        for t in &schema {
            assert_eq!(t["type"], "function");
            assert_eq!(t["function"]["parameters"]["type"], "object");
        }
        assert_eq!(spec("run_command").unwrap().effect, Effect::Run);
        assert_eq!(spec("read_file").unwrap().effect, Effect::Read);
        assert_eq!(spec("now").unwrap().effect, Effect::Read);
        assert_eq!(spec("calculate").unwrap().effect, Effect::Read);
    }

    /// `calculate`'s answer, or its error text.
    fn calc(expression: &str) -> Result<String, String> {
        let out = calculate(expression);
        if out.ok {
            Ok(out.output)
        } else {
            Err(out.output)
        }
    }

    #[test]
    fn calculates() {
        for (expression, expected) in [
            ("2 + 3 * 4", "14"),
            ("(2 + 3) * 4", "20"),
            ("10 / 4", "2.5"),
            ("17 % 5", "2"),
            ("-7 % 3", "-1"),
            ("2 ^ 10", "1024"),
            ("2 ** 10", "1024"),
            ("2 ^ 3 ^ 2", "512"),
            ("-2 ^ 2", "-4"),
            ("2 ^ -1", "0.5"),
            ("2 * -3", "-6"),
            ("--4 + +1", "5"),
            ("0.1 + 0.2", "0.3"),
            (".5 + 1.", "1.5"),
            ("1.5e3 + 2E-1", "1500.2"),
            ("sqrt(16) + abs(-3)", "7"),
            ("round(2.5) + floor(2.9) + ceil(2.1)", "8"),
            ("min(3, 1, 2) + max(3, 1, 2)", "4"),
            ("SQRT(2)^2", "2"),
            ("ln(e) + log10(1000)", "4"),
            ("sin(0) + cos(0) + tan(0)", "1"),
            ("round(pi * 1000)", "3142"),
            ("1 / 3", "0.333333333333"),
            ("2 ^ 100", "1.26765060023e30"),
            ("  7  ", "7"),
        ] {
            assert_eq!(calc(expression).as_deref(), Ok(expected), "{expression}");
        }
        let out = calculate("6 * 7");
        assert_eq!(out.summary, "6 * 7 = 42");
    }

    #[test]
    fn calculate_refuses_bad_input() {
        for expression in [
            "",
            "2 +",
            "* 2",
            "(1 + 2",
            "1 + 2)",
            "2 3",
            "abc",
            "foo(1)",
            "sqrt()",
            "sqrt(1, 2)",
            "min()",
            "1..2",
            "1 $ 2",
            "²",
            "sqrt(-1)",
            "ln(0)",
            "log10(-5)",
        ] {
            let out = calc(expression);
            assert!(
                out.as_ref().is_err_and(|e| e.starts_with("Error: ")),
                "{expression}: {out:?}"
            );
        }
    }

    #[test]
    fn calculate_is_bounded() {
        // Division by zero, however it is spelled.
        assert!(calc("1 / 0").unwrap_err().contains("Division by zero"));
        assert!(calc("1 % 0").unwrap_err().contains("Division by zero"));
        assert!(
            calc("1 / (2 - 2)")
                .unwrap_err()
                .contains("Division by zero")
        );
        // Results too big for a number.
        assert!(calc("9 ^ 9 ^ 9 ^ 9").unwrap_err().contains("too large"));
        assert!(calc("10 ^ 400").unwrap_err().contains("too large"));
        assert!(calc("1e999").unwrap_err().contains("too large"));
        assert!(calc("1e300 * 1e300").unwrap_err().contains("too large"));
        assert!(calc("0 ^ -1").unwrap_err().contains("too large"));
        assert!(
            calc("(-8) ^ 0.5")
                .unwrap_err()
                .contains("not a real number")
        );
        // Nesting: brackets, signs, powers and calls stop at a depth.
        let deep = format!("{}1{}", "(".repeat(400), ")".repeat(400));
        assert!(calc(&deep).unwrap_err().contains("nested too deeply"));
        let signs = format!("{}1", "-".repeat(900));
        assert!(calc(&signs).unwrap_err().contains("nested too deeply"));
        let powers = vec!["1"; 400].join("^");
        assert!(calc(&powers).unwrap_err().contains("nested too deeply"));
        let calls = format!("{}1{}", "abs(".repeat(150), ")".repeat(150));
        assert!(calc(&calls).unwrap_err().contains("nested too deeply"));
        let fine = format!("{}1{}", "(".repeat(30), ")".repeat(30));
        assert_eq!(calc(&fine).as_deref(), Ok("1"));
        // Length: 1,000 characters are read, 1,001 are not.
        let long = format!("1{}", "+1".repeat(499));
        assert_eq!(long.len(), 999);
        assert_eq!(calc(&long).as_deref(), Ok("500"));
        let too_long = "1".repeat(1001);
        assert!(calc(&too_long).unwrap_err().contains("too long"));
        let wide = "é".repeat(1001);
        assert!(calc(&wide).unwrap_err().contains("too long"));
    }

    #[test]
    fn calculate_through_run() {
        let (dir, ws) = workspace("calc");
        let ok = go(&ws, "calculate", json!({"expression": "2^8"}));
        assert!(ok.ok);
        assert_eq!(ok.output, "256");
        assert!(!go(&ws, "calculate", json!({})).ok);
        assert!(!go(&ws, "calculate", json!({"expression": "  "})).ok);
        assert!(!go(&ws, "calculate", json!({"expression": 5})).ok);
        assert_eq!(
            label("calculate", r#"{"expression":"1+1"}"#),
            "Calculate 1+1"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn the_time_in_words() {
        assert_eq!(
            when([4, 2026, 10, 8, 14, 3, 5], -4 * 3600, "EDT"),
            "Thursday, 2026-10-08 14:03:05 UTC-04:00 (EDT)"
        );
        assert_eq!(
            when([0, 2026, 1, 2, 0, 0, 0], 5 * 3600 + 30 * 60, ""),
            "Sunday, 2026-01-02 00:00:00 UTC+05:30"
        );
        assert!(when([0, 2026, 1, 1, 0, 0, 0], 0, "UTC").contains("UTC+00:00 (UTC)"));
    }

    #[test]
    fn now_gives_the_local_time() {
        let (dir, ws) = workspace("now");
        let out = go(&ws, "now", json!({}));
        assert!(out.ok, "{out:?}");
        // "Thursday, 2026-10-08 14:03:05 UTC+00:00 (UTC)".
        let (day, rest) = out.output.split_once(", ").unwrap();
        assert!(
            [
                "Sunday",
                "Monday",
                "Tuesday",
                "Wednesday",
                "Thursday",
                "Friday",
                "Saturday"
            ]
            .contains(&day),
            "{}",
            out.output
        );
        let b = rest.as_bytes();
        assert!(b[..4].iter().all(u8::is_ascii_digit) && b[4] == b'-' && b[10] == b' ');
        assert!(rest[11..].contains(" UTC"), "{}", out.output);
        assert!(out.summary.contains(&out.output));
        assert_eq!(label("now", "{}"), "Check the date and time");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn paths_stay_inside() {
        let (dir, ws) = workspace("paths");
        assert!(ws.resolve("src/main.rs").is_ok());
        assert!(ws.resolve("src/new.rs").is_ok());
        assert!(ws.resolve("../outside").is_err());
        assert!(ws.resolve("src/../../outside").is_err());
        assert!(ws.resolve("/etc/passwd").is_err());
        // A link that leads out is refused.
        std::os::unix::fs::symlink("/etc", dir.join("etc")).unwrap();
        assert!(ws.resolve("etc/passwd").is_err());
        assert!(
            go(&ws, "read_file", json!({"path": "etc/passwd"}))
                .output
                .contains("outside")
        );
        let w = go(
            &ws,
            "write_file",
            json!({"path": "../escape.txt", "content": "x"}),
        );
        assert!(!w.ok, "{w:?}");
        // New folders under a link that leads out aren't made.
        let out = std::env::temp_dir().join(format!("gates-tools-{}-out", std::process::id()));
        let _ = fs::remove_dir_all(&out);
        fs::create_dir_all(&out).unwrap();
        std::os::unix::fs::symlink(&out, dir.join("away")).unwrap();
        let w = go(
            &ws,
            "write_file",
            json!({"path": "away/new/file.txt", "content": "x"}),
        );
        assert!(!w.ok, "{w:?}");
        assert!(!out.join("new").exists());
        let w = go(
            &ws,
            "write_file",
            json!({"path": "deep/../../escape2.txt", "content": "x"}),
        );
        assert!(!w.ok, "{w:?}");
        let _ = fs::remove_dir_all(&out);
        assert!(!dir.parent().unwrap().join("escape.txt").exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn reading_tools() {
        let (dir, ws) = workspace("read");
        let list = go(&ws, "list_dir", json!({}));
        assert!(list.output.starts_with("src/"), "{}", list.output);
        let read = go(&ws, "read_file", json!({"path": "src/main.rs"}));
        assert!(
            read.output.contains("    2      println!"),
            "{}",
            read.output
        );
        assert_eq!(read.summary, "Read src/main.rs (lines 1–3 of 3)");
        let part = go(
            &ws,
            "read_file",
            json!({"path": "src/main.rs", "max_lines": 1}),
        );
        assert!(part.output.contains("start_line 2"));
        // Smart case, and build output skipped.
        let found = go(&ws, "search", json!({"query": "hello"}));
        assert!(found.output.contains("src/main.rs:2:"), "{}", found.output);
        assert!(found.output.contains("README.md:2:"));
        assert!(!found.output.contains("target/"));
        let exact = go(&ws, "search", json!({"query": "Hello"}));
        assert!(!exact.output.contains("README.md"));
        let files = go(&ws, "find_files", json!({"pattern": "*.rs"}));
        assert_eq!(files.output, "src/main.rs\n");
        assert!(!go(&ws, "read_file", json!({})).ok);
        assert!(!go(&ws, "nonsense", json!({})).ok);
        assert!(!run(&ws, "list_dir", "not json", &AtomicBool::new(false)).ok);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn changing_tools() {
        let (dir, ws) = workspace("write");
        let made = go(
            &ws,
            "write_file",
            json!({"path": "notes/todo.txt", "content": "one\ntwo\n"}),
        );
        assert!(made.ok, "{made:?}");
        assert_eq!(
            fs::read_to_string(dir.join("notes/todo.txt")).unwrap(),
            "one\ntwo\n"
        );
        let edit = go(
            &ws,
            "edit_file",
            json!({"path": "src/main.rs", "old_text": "Hello", "new_text": "Hi"}),
        );
        assert!(edit.ok, "{edit:?}");
        assert!(
            fs::read_to_string(dir.join("src/main.rs"))
                .unwrap()
                .contains("\"Hi\"")
        );
        let twice = go(
            &ws,
            "write_file",
            json!({"path": "x.txt", "content": "a a"}),
        );
        assert!(twice.ok);
        assert!(
            !go(
                &ws,
                "edit_file",
                json!({"path": "x.txt", "old_text": "a", "new_text": "b"})
            )
            .ok
        );
        assert!(
            !go(
                &ws,
                "edit_file",
                json!({"path": "x.txt", "old_text": "zzz", "new_text": "b"})
            )
            .ok
        );
        // As the model copies them: with read_file's line numbers, or with
        // other indentation.
        fs::write(dir.join("y.py"), "def f():\n    return 1\n").unwrap();
        let numbered = go(
            &ws,
            "edit_file",
            json!({"path": "y.py", "old_text": "    2      return 1", "new_text": "    return 2"}),
        );
        assert!(numbered.ok, "{numbered:?}");
        assert_eq!(
            fs::read_to_string(dir.join("y.py")).unwrap(),
            "def f():\n    return 2\n"
        );
        let loose = go(
            &ws,
            "edit_file",
            json!({"path": "y.py", "old_text": "def f():\nreturn 2", "new_text": "def f():\n    return 3"}),
        );
        assert!(loose.ok, "{loose:?}");
        assert_eq!(
            fs::read_to_string(dir.join("y.py")).unwrap(),
            "def f():\n    return 3\n"
        );
        let (title, detail) = describe(
            &ws,
            "edit_file",
            &json!({"path": "src/main.rs", "old_text": "a\nb", "new_text": "c"}).to_string(),
        )
        .unwrap();
        assert_eq!(title, "Edit src/main.rs");
        assert_eq!(detail, "- a\n- b\n+ c");
        // Unusable arguments ask nothing.
        assert!(describe(&ws, "write_file", "{\"path\": 3}").is_err());
        assert!(describe(&ws, "run_command", "not json").is_err());
        // Hidden characters are shown, and said.
        let (title, detail) = describe(
            &ws,
            "run_command",
            &json!({"command": "ls\u{202E}txt.sh"}).to_string(),
        )
        .unwrap();
        assert!(title.contains("hidden characters"), "{title}");
        assert_eq!(detail, "ls⟨U+202E⟩txt.sh");
        assert!(title.contains("stopped after 60 s"));
        // A bare number is text to edit, not a line number.
        fs::write(dir.join("port.txt"), "port 8080\n").unwrap();
        let port = go(
            &ws,
            "edit_file",
            json!({"path": "port.txt", "old_text": "8080", "new_text": "9090"}),
        );
        assert!(port.ok, "{port:?}");
        assert_eq!(
            fs::read_to_string(dir.join("port.txt")).unwrap(),
            "port 9090\n"
        );
        // Edits that always ask.
        let args = |p: &str| json!({"path": p, "content": "x"}).to_string();
        assert!(sensitive(&ws, "write_file", &args(".git/hooks/pre-commit")));
        assert!(sensitive(&ws, "write_file", &args("Makefile")));
        assert!(sensitive(&ws, "write_file", &args("scripts/go.sh")));
        assert!(sensitive(&ws, "write_file", &args(".envrc")));
        assert!(!sensitive(&ws, "write_file", &args("src/main.rs")));
        assert!(sensitive(&ws, "run_command", "{}"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_planted_temporary_link_is_not_followed() {
        let (dir, ws) = workspace("plant");
        // Even with a link at a name like the old temporary one, the write
        // stays in the workspace (the temporary name is random now).
        let outside =
            std::env::temp_dir().join(format!("gates-tools-{}-target", std::process::id()));
        let _ = fs::remove_file(&outside);
        std::os::unix::fs::symlink(&outside, dir.join(".README.md.gates-tmp")).unwrap();
        assert!(
            go(
                &ws,
                "write_file",
                json!({"path": "README.md", "content": "new"})
            )
            .ok
        );
        assert!(!outside.exists());
        assert_eq!(fs::read_to_string(dir.join("README.md")).unwrap(), "new");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn workspaces_that_are_too_wide() {
        assert!(Workspace::open(Path::new("/")).is_err());
        assert!(Workspace::open(Path::new("/etc")).is_err());
        if let Some(home) = std::env::var_os("HOME") {
            assert!(Workspace::open(Path::new(&home)).is_err());
        }
    }

    #[test]
    fn commands() {
        // Commands run only in their sandbox; where bubblewrap can't make
        // one (a CI runner without user namespaces), there is nothing to run.
        if !crate::sandbox::available() {
            eprintln!("no command sandbox here: skipped");
            return;
        }
        let (dir, ws) = workspace("run");
        let ok = go(
            &ws,
            "run_command",
            json!({"command": "ls src && echo done >&2"}),
        );
        assert!(ok.ok, "{ok:?}");
        assert!(ok.output.contains("main.rs") && ok.output.contains("done"));
        assert!(ok.output.contains("[exit status 0"));
        let fail = go(&ws, "run_command", json!({"command": "exit 3"}));
        assert!(!fail.ok && fail.summary.contains("exit 3"));
        // Output is bounded however much there is, start and end kept.
        let loud = go(
            &ws,
            "run_command",
            json!({"command": "echo first; head -c 5000000 /dev/zero | tr '\\0' x; echo; echo last"}),
        );
        assert!(
            loud.output.contains("first") && loud.output.contains("last"),
            "{}",
            &loud.output[..200]
        );
        assert!(loud.output.len() <= MAX_OUTPUT + 64);
        assert!(loud.output.contains("bytes left out"));
        // A timeout ends what the command started too.
        let marker = dir.join("still-running");
        let bg = go(
            &ws,
            "run_command",
            json!({"command": format!("(sleep 3; touch {}) & sleep 10", marker.display()), "timeout_seconds": 1}),
        );
        assert!(!bg.ok);
        std::thread::sleep(Duration::from_secs(4));
        assert!(!marker.exists(), "a background child outlived the timeout");
        let slow = go(
            &ws,
            "run_command",
            json!({"command": "sleep 5", "timeout_seconds": 1}),
        );
        assert!(
            !slow.ok && slow.summary.contains("stopped after 1 s"),
            "{slow:?}"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn globs_and_clipping() {
        assert!(glob("*.rs", "main.rs"));
        assert!(glob("src/*.qml", "src/Main.qml"));
        assert!(glob("a?c", "abc"));
        assert!(!glob("*.rs", "main.rst"));
        let long = "é".repeat(MAX_OUTPUT);
        let cut = clip(long);
        assert!(cut.len() <= MAX_OUTPUT + 64);
        assert!(cut.starts_with("… "));
    }

    // ---- the web tools

    use crate::conversation::Message;
    use crate::web::testing::Fake;
    use crate::web::{Page, Session};

    fn go_web(session: &Session<'_>, name: &str, args: Value) -> Outcome {
        run_web(session, name, &args.to_string(), &AtomicBool::new(false))
    }

    fn ten_results() -> Vec<crate::web::SearchResult> {
        (1..=10)
            .map(|i| {
                Fake::result(
                    &format!("Result {i}"),
                    &format!("https://example.org/r{i}"),
                    &format!("About {i}."),
                )
            })
            .collect()
    }

    #[test]
    fn only_result_addresses_are_opened_whatever_dates_snippets_and_queries_say() {
        let mut results = vec![
            Fake::result(
                "First",
                "https://good.example.org/a",
                "see https://evil.example/s",
            ),
            Fake::result("Second", "https://good.example.org/b", "x"),
        ];
        // What a service sends as a date is data too.
        results[0].age = "https://evil.example/date".into();
        results[1].age = "2026-09-30".into();
        let query = "q\n9. Fake\nhttps://evil.example/query";
        let fake = Fake::default().with_results(query, results);
        let session = Session::new(&fake, &[Message::user("search")]);
        let found = go_web(&session, "web_search", json!({ "query": query }));
        assert!(found.ok, "{found:?}");
        assert!(
            found.output.contains("   Date: 2026-09-30\n"),
            "{}",
            found.output
        );
        assert_eq!(
            crate::web::session::result_urls(&found.output),
            vec!["https://good.example.org/a", "https://good.example.org/b"]
        );
        for evil in [
            "https://evil.example/s",
            "https://evil.example/date",
            "https://evil.example/query",
        ] {
            assert!(!session.allows(evil), "{evil} was allowed");
        }
        // The header says what day it is, for judging how recent results are.
        assert!(found.output.lines().next().unwrap().contains("(today is "));
    }

    #[test]
    fn a_search_gives_titles_addresses_and_snippets() {
        let fake = Fake::default().with_results("rust async", ten_results());
        let session = Session::new(&fake, &[Message::user("tell me about rust async")]);
        let found = go_web(
            &session,
            "web_search",
            json!({"query": " rust async ", "count": 99}),
        );
        assert!(found.ok, "{found:?}");
        // 8 at most, whatever was asked.
        assert!(
            found
                .output
                .contains("8. Result 8\n   https://example.org/r8\n   About 8.")
        );
        assert!(!found.output.contains("Result 9"));
        assert!(found.output.contains(crate::web::UNTRUSTED));
        assert_eq!(found.summary, "Searched for \"rust async\" (8 results)");
        // Three without being asked for five.
        let few = go_web(
            &session,
            "web_search",
            json!({"query": "rust async", "count": 3}),
        );
        assert_eq!(few.summary, "Searched for \"rust async\" (3 results)");
        let default = go_web(&session, "web_search", json!({"query": "rust async"}));
        assert_eq!(default.summary, "Searched for \"rust async\" (5 results)");
        // Nothing found is an answer, not an error.
        let none = go_web(&session, "web_search", json!({"query": "zzzz"}));
        assert!(none.ok && none.summary.contains("no results"), "{none:?}");
        // The model's mistakes are answers too.
        assert!(!go_web(&session, "web_search", json!({})).ok);
        assert!(!go_web(&session, "web_search", json!({"query": "   "})).ok);
        let bad = run_web(&session, "web_search", "not json", &AtomicBool::new(false));
        assert!(!bad.ok && bad.output.starts_with("Error:"));
    }

    #[test]
    fn a_page_is_read_when_it_was_shown() {
        let page = Page {
            url: "https://example.org/r2".into(),
            title: "Result 2".into(),
            text: "The body.\n\nSee [next](https://example.org/deeper).".into(),
            truncated: true,
        };
        let fake = Fake::default()
            .with_results("q", ten_results())
            .with_page(page)
            .with_page(Fake::page("https://example.org/deeper", "Deeper", "More."))
            .with_failing_page("https://example.org/r3", "There is no such page (404).");
        let session = Session::new(&fake, &[Message::user("look this up")]);
        // Not shown yet: refused, and nothing was requested.
        let early = go_web(
            &session,
            "fetch_page",
            json!({"url": "https://example.org/r2"}),
        );
        assert!(
            !early.ok && early.output.contains("Search for it first"),
            "{early:?}"
        );
        assert!(fake.calls().is_empty());
        go_web(&session, "web_search", json!({"query": "q"}));
        let read = go_web(
            &session,
            "fetch_page",
            json!({"url": "https://example.org/r2"}),
        );
        assert!(read.ok, "{read:?}");
        assert!(
            read.output
                .starts_with("Page: https://example.org/r2\nTitle: Result 2\n(")
        );
        assert!(read.output.contains("The body."));
        assert!(read.output.contains("this is its first part"));
        assert_eq!(read.summary, "Read example.org/r2 (1 KB)");
        // The page's own links may not: only search results and the user's
        // words are on the list.
        let deeper = go_web(
            &session,
            "fetch_page",
            json!({"url": "https://example.org/deeper"}),
        );
        assert!(
            !deeper.ok && deeper.output.contains("Search for it first"),
            "{deeper:?}"
        );
        assert!(!fake.calls().iter().any(|c| c.contains("deeper")));
        // A failing page tells the model why.
        let gone = go_web(
            &session,
            "fetch_page",
            json!({"url": "https://example.org/r3"}),
        );
        assert!(!gone.ok);
        assert_eq!(
            gone.summary,
            "Couldn't read example.org/r3: There is no such page (404)."
        );
        // An address built for the occasion is not opened.
        let sneaky = go_web(
            &session,
            "fetch_page",
            json!({"url": "https://evil.example/?data=the+conversation"}),
        );
        assert!(!sneaky.ok);
        assert!(!fake.calls().iter().any(|c| c.contains("evil")));
        assert!(!go_web(&session, "fetch_page", json!({})).ok);
    }

    #[test]
    fn web_tools_are_known_and_read_only() {
        assert!(is_web("web_search") && is_web("fetch_page") && !is_web("read_file"));
        assert!(WEB_TOOLS.iter().all(|t| t.effect == Effect::Read));
        // Kept apart from the workspace tools, whose schema is Agent mode's.
        assert!(spec("web_search").is_none());
        let names: Vec<String> = web_schema()
            .iter()
            .map(|t| t["function"]["name"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(names, vec!["web_search", "fetch_page"]);
        assert_eq!(schema().len(), TOOLS.len());
        assert_eq!(
            label("web_search", r#"{"query":"x"}"#),
            "Search the web for \"x\""
        );
        assert_eq!(
            progress("web_search", r#"{"query":"rust  async"}"#),
            "Searching: rust async"
        );
        assert_eq!(
            progress(
                "fetch_page",
                r#"{"url":"https://www.example.org/a/b/?q=1#top"}"#
            ),
            "Reading: example.org/a/b?…"
        );
        assert_eq!(short_url("not a url"), "not a url");
        assert!(short_url(&format!("https://example.org/{}", "x".repeat(200))).ends_with('…'));
        let off = unavailable("web_search");
        assert!(!off.ok);
    }
}
