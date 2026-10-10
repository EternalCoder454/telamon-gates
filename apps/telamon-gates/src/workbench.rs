//! The coding workspace beside the chat (Code and Agent mode): the files of
//! the conversation's folder, the editor's saves, the user's own commands and
//! their console. Files go through the file thread (`Io`), commands run on a
//! worker, and what they print comes back in batches; the window's thread
//! never waits for any of it. What the agent did to a file (a live edit) and
//! what its commands print arrive here from its worker too.

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    extern "RustQt" {
        #[qobject]
        /// The folder shown (its real path); "" when none is attached.
        #[qproperty(QString, root)]
        /// A folder is attached: files can be listed, opened and saved.
        #[qproperty(bool, ready)]
        /// The user's command is running.
        #[qproperty(bool, running)]
        /// Commands can run (bubblewrap is installed).
        #[qproperty(bool, commands)]
        /// Why the folder couldn't be attached; "" when it could.
        #[qproperty(QString, problem)]
        #[namespace = "telamon_gates"]
        type Workbench = super::WorkbenchRust;
    }

    unsafe extern "RustQt" {
        /// Shows the folder of a conversation: `workspace` (a folder the
        /// user chose), else the conversation's own sandbox. `network` and
        /// `home` are what its commands may reach.
        #[qinvokable]
        fn attach(
            self: Pin<&mut Workbench>,
            conversation: &QString,
            workspace: &QString,
            network: bool,
            home: bool,
        );

        /// Shows no folder; stops the user's command.
        #[qinvokable]
        fn detach(self: Pin<&mut Workbench>);

        /// What commands may reach changed (Settings).
        #[qinvokable]
        #[cxx_name = "setAccess"]
        fn set_access(self: Pin<&mut Workbench>, network: bool, home: bool);

        /// Lists folder `dir` (relative; "" for the root): `listed` or
        /// `listFailed` follows.
        #[qinvokable]
        fn list(self: Pin<&mut Workbench>, dir: &QString);

        /// Reads file `path`: `opened` or `openFailed` follows.
        #[qinvokable]
        fn open(self: Pin<&mut Workbench>, path: &QString);

        /// Writes `text` to file `path` (with CRLF lines when `crlf`):
        /// `saved` or `saveFailed` follows.
        #[qinvokable]
        fn save(self: Pin<&mut Workbench>, path: &QString, text: &QString, crlf: bool);

        /// Makes an empty file `path` (never over one that is there):
        /// `created` or `saveFailed` follows.
        #[qinvokable]
        fn create(self: Pin<&mut Workbench>, path: &QString);

        /// The editor has (or has no longer) unsaved changes in `path`.
        #[qinvokable]
        #[cxx_name = "markUnsaved"]
        fn mark_unsaved(self: Pin<&mut Workbench>, path: &QString, unsaved: bool);

        /// Starts watching folder `dir` for changes (the tree shows it).
        #[qinvokable]
        fn watch(self: Pin<&mut Workbench>, dir: &QString);

        /// Stops watching folder `dir`.
        #[qinvokable]
        fn unwatch(self: Pin<&mut Workbench>, dir: &QString);

        /// Runs `command` (the user's own, no question asked) in the
        /// sandbox, in the folder.
        #[qinvokable]
        fn run(self: Pin<&mut Workbench>, command: &QString);

        /// Stops the user's command and everything it started.
        #[qinvokable]
        fn stop(self: Pin<&mut Workbench>);

        /// What the console printed so far (kept while the panel is closed).
        #[qinvokable]
        fn backlog(self: &Workbench) -> QString;

        /// Empties the console.
        #[qinvokable]
        #[cxx_name = "clearConsole"]
        fn clear_console(self: Pin<&mut Workbench>);

        /// A folder's entries, as JSON: `[{"n": name, "d": is a folder}]`.
        /// `more` entries were left out.
        #[qsignal]
        fn listed(self: Pin<&mut Workbench>, dir: QString, json: QString, more: i32);

        #[qsignal]
        #[cxx_name = "listFailed"]
        fn list_failed(self: Pin<&mut Workbench>, dir: QString, error: QString);

        /// A file read. `path` is as the tabs keep it (relative to the
        /// folder); `readOnly`: it can't be written back as the editor has
        /// it; `bidi`: it has bidirectional control characters.
        #[qsignal]
        fn opened(
            self: Pin<&mut Workbench>,
            path: QString,
            text: QString,
            crlf: bool,
            read_only: bool,
            bidi: bool,
        );

        #[qsignal]
        #[cxx_name = "openFailed"]
        fn open_failed(self: Pin<&mut Workbench>, path: QString, error: QString);

        #[qsignal]
        fn saved(self: Pin<&mut Workbench>, path: QString);

        #[qsignal]
        fn created(self: Pin<&mut Workbench>, path: QString);

        #[qsignal]
        #[cxx_name = "saveFailed"]
        fn save_failed(self: Pin<&mut Workbench>, path: QString, error: QString);

        /// Folders (relative paths as JSON; "*" for all) where files were
        /// made, removed or moved.
        #[qsignal]
        fn changed(self: Pin<&mut Workbench>, dirs: QString);

        /// The agent changed file `path`. With `hasText`, `text` is the
        /// whole file now; else the editor reads it. `marks` is JSON,
        /// `[[first, last, added]]`, lines from 1; `scrollTo` the first
        /// change's line (0 for none).
        #[qsignal]
        #[cxx_name = "liveEdit"]
        fn live_edit(
            self: Pin<&mut Workbench>,
            path: QString,
            text: QString,
            has_text: bool,
            marks: QString,
            scroll_to: i32,
        );

        /// The agent read file `path`.
        #[qsignal]
        fn touched(self: Pin<&mut Workbench>, path: QString);

        /// Something to append to the console.
        #[qsignal]
        #[cxx_name = "consoleText"]
        fn console_text(self: Pin<&mut Workbench>, text: QString);

        #[qsignal]
        #[cxx_name = "consoleCleared"]
        fn console_cleared(self: Pin<&mut Workbench>);
    }

    impl cxx_qt::Threading for Workbench {}

    #[namespace = "rust::cxxqtlib1"]
    unsafe extern "C++" {
        include!("cxx-qt-lib/common.h");

        #[cxx_name = "make_unique"]
        fn workbench_make_unique() -> UniquePtr<Workbench>;
    }
}

use crate::io::Io;
use core::pin::Pin;
use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use gates_core::sandbox::Access;
use gates_core::tools::Workspace;
use gates_core::workbench::{self, Batcher, Change, Edits, Sink, Watcher};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// The console text kept for a panel that is opened later, in bytes.
const BACKLOG: usize = 256 * 1024;

#[derive(Default)]
pub struct WorkbenchRust {
    root: QString,
    ready: bool,
    running: bool,
    commands: bool,
    problem: QString,
    pub io: Option<Io>,
    /// Which files have unsaved changes; shared with the chat's agent.
    pub edits: Arc<Edits>,
    workspace: Option<Workspace>,
    watcher: Option<Arc<Watcher>>,
    /// Bumped by every attach and detach: answers for an older folder drop.
    generation: u64,
    /// The user's command under way stops when this turns true.
    cancel: Option<Arc<AtomicBool>>,
    console: String,
}

impl qobject::Workbench {
    pub fn attach(
        mut self: Pin<&mut Self>,
        conversation: &QString,
        workspace: &QString,
        network: bool,
        home: bool,
    ) {
        self.as_mut().reset();
        let generation = self.rust().generation;
        let (conversation, workspace) = (conversation.to_string(), workspace.to_string());
        let access = Access { network, home };
        let qt = self.qt_thread();
        // The folder may be on a slow disk, and bubblewrap is run once to
        // see whether commands can: not on the window's thread.
        std::thread::spawn(move || {
            let sandbox = workspace.is_empty();
            let folder = if sandbox {
                if conversation.is_empty() {
                    let _ = qt.queue(move |mut wb| {
                        if wb.rust().generation == generation {
                            wb.as_mut().set_problem(QString::from(
                                "Send a message first: the workspace is made with the conversation.",
                            ));
                        }
                    });
                    return;
                }
                let folder = gates_core::store::sandbox_dir(&conversation);
                let _ = gates_core::store::private_dir(&folder);
                folder
            } else {
                std::path::PathBuf::from(&workspace)
            };
            let opened = Workspace::open(&folder).map(|ws| ws.with_access(access));
            let commands = gates_core::sandbox::available();
            let _ = qt.queue(move |mut wb| {
                if wb.rust().generation != generation {
                    return;
                }
                match opened {
                    Ok(ws) => {
                        let root = ws.root().to_string_lossy().into_owned();
                        let watch_qt = wb.qt_thread();
                        let watcher = Watcher::new(move |dirs| {
                            let json = workbench::dirs_json(&dirs);
                            let _ = watch_qt.queue(move |wb| {
                                if wb.rust().generation == generation {
                                    wb.changed(QString::from(json.as_str()));
                                }
                            });
                        });
                        match watcher {
                            Ok(w) => wb.as_mut().rust_mut().watcher = Some(Arc::new(w)),
                            Err(e) => log::warn!("the folder watch can't start: {e}"),
                        }
                        wb.as_mut().rust_mut().workspace = Some(ws);
                        wb.as_mut().set_commands(commands);
                        wb.as_mut().set_root(QString::from(root.as_str()));
                        wb.as_mut().set_problem(QString::default());
                        wb.set_ready(true);
                    }
                    Err(e) => wb.set_problem(QString::from(
                        format!("The workspace can't be shown: {e}.").as_str(),
                    )),
                }
            });
        });
    }

    pub fn detach(self: Pin<&mut Self>) {
        self.reset();
    }

    /// Nothing attached: the watch, the command and what is known of the
    /// files go.
    fn reset(mut self: Pin<&mut Self>) {
        self.as_mut().stop();
        {
            let mut rust = self.as_mut().rust_mut();
            rust.generation += 1;
            rust.workspace = None;
            rust.watcher = None;
            rust.edits.reset();
        }
        self.as_mut().set_ready(false);
        self.as_mut().set_root(QString::default());
        self.as_mut().set_problem(QString::default());
    }

    pub fn set_access(mut self: Pin<&mut Self>, network: bool, home: bool) {
        let access = Access { network, home };
        let mut rust = self.as_mut().rust_mut();
        if let Some(ws) = rust.workspace.take() {
            rust.workspace = Some(ws.with_access(access));
        }
    }

    /// The workspace and file thread, when both are there.
    fn files(&self) -> Option<(Workspace, Io, u64)> {
        let rust = self.rust();
        Some((rust.workspace.clone()?, rust.io.clone()?, rust.generation))
    }

    pub fn list(self: Pin<&mut Self>, dir: &QString) {
        let Some((ws, io, generation)) = self.files() else {
            return;
        };
        let dir = dir.to_string();
        let qt = self.qt_thread();
        io.run(move |_| {
            let listing = workbench::list(&ws, &dir);
            let _ = qt.queue(move |mut wb| {
                if wb.rust().generation != generation {
                    return;
                }
                match listing {
                    Ok(l) => {
                        let json = l.json();
                        wb.as_mut().listed(
                            QString::from(dir.as_str()),
                            QString::from(json.as_str()),
                            i32::try_from(l.more).unwrap_or(i32::MAX),
                        );
                    }
                    Err(e) => {
                        wb.list_failed(QString::from(dir.as_str()), QString::from(e.as_str()))
                    }
                }
            });
        });
    }

    pub fn open(self: Pin<&mut Self>, path: &QString) {
        let Some((ws, io, generation)) = self.files() else {
            return;
        };
        let path = path.to_string();
        let qt = self.qt_thread();
        io.run(move |_| {
            let read = workbench::open(&ws, &path);
            let _ = qt.queue(move |mut wb| {
                if wb.rust().generation != generation {
                    return;
                }
                match read {
                    Ok(f) => wb.as_mut().opened(
                        QString::from(f.path.as_str()),
                        QString::from(f.text.as_str()),
                        f.crlf,
                        f.read_only,
                        f.bidi,
                    ),
                    Err(e) => {
                        wb.open_failed(QString::from(path.as_str()), QString::from(e.as_str()))
                    }
                }
            });
        });
    }

    pub fn save(self: Pin<&mut Self>, path: &QString, text: &QString, crlf: bool) {
        let Some((ws, io, generation)) = self.files() else {
            return;
        };
        let (path, text) = (path.to_string(), text.to_string());
        let edits = self.rust().edits.clone();
        let qt = self.qt_thread();
        io.run(move |_| {
            let written = workbench::save(&ws, &path, &text, crlf);
            if let Ok(shown) = &written {
                edits.note_saved(shown);
            }
            let _ = qt.queue(move |wb| {
                if wb.rust().generation != generation {
                    return;
                }
                match written {
                    Ok(shown) => wb.saved(QString::from(shown.as_str())),
                    Err(e) => {
                        wb.save_failed(QString::from(path.as_str()), QString::from(e.as_str()))
                    }
                }
            });
        });
    }

    pub fn create(self: Pin<&mut Self>, path: &QString) {
        let Some((ws, io, generation)) = self.files() else {
            return;
        };
        let path = path.to_string();
        let qt = self.qt_thread();
        io.run(move |_| {
            let made = workbench::create(&ws, &path);
            let _ = qt.queue(move |mut wb| {
                if wb.rust().generation != generation {
                    return;
                }
                match made {
                    Ok(shown) => wb.as_mut().created(QString::from(shown.as_str())),
                    Err(e) => {
                        wb.save_failed(QString::from(path.as_str()), QString::from(e.as_str()))
                    }
                }
            });
        });
    }

    pub fn mark_unsaved(self: Pin<&mut Self>, path: &QString, unsaved: bool) {
        self.rust().edits.set_unsaved(&path.to_string(), unsaved);
    }

    pub fn watch(self: Pin<&mut Self>, dir: &QString) {
        let (Some(ws), Some(watcher)) =
            (self.rust().workspace.clone(), self.rust().watcher.clone())
        else {
            return;
        };
        let dir = dir.to_string();
        let Some(io) = self.rust().io.clone() else {
            return;
        };
        io.run(move |_| {
            if let Err(e) = watcher.watch(&ws, &dir) {
                log::debug!("not watching {dir}: {e}");
            }
        });
    }

    pub fn unwatch(self: Pin<&mut Self>, dir: &QString) {
        if let Some(watcher) = self.rust().watcher.clone() {
            watcher.unwatch(&dir.to_string());
        }
    }

    pub fn run(mut self: Pin<&mut Self>, command: &QString) {
        let command = command.to_string();
        if *self.running() || command.trim().is_empty() {
            return;
        }
        let Some(ws) = self.rust().workspace.clone() else {
            return;
        };
        let generation = self.rust().generation;
        let cancel = Arc::new(AtomicBool::new(false));
        self.as_mut().rust_mut().cancel = Some(cancel.clone());
        self.as_mut().set_running(true);
        let Some(io) = self.rust().io.clone() else {
            return;
        };
        let qt = self.qt_thread();
        // After the saves the window sent just before (Run saves the files
        // it changed): the file thread reaches this job when they are done.
        io.run(move |_| {
            std::thread::spawn(move || {
                let console = qt.clone();
                let batcher = Batcher::new(move |text| {
                    let _ = console.queue(move |wb| wb.append_console(&text));
                });
                batcher.note(&format!("$ {command}"));
                let sink: Arc<dyn Sink> = batcher;
                workbench::run(&ws, &command, &cancel, &sink);
                let _ = qt.queue(move |mut wb| {
                    // The end of this run, not of a later one.
                    if wb.rust().generation == generation
                        && wb
                            .rust()
                            .cancel
                            .as_ref()
                            .is_some_and(|c| Arc::ptr_eq(c, &cancel))
                    {
                        wb.as_mut().rust_mut().cancel = None;
                        wb.as_mut().set_running(false);
                    }
                });
            });
        });
    }

    pub fn stop(mut self: Pin<&mut Self>) {
        if let Some(cancel) = self.as_mut().rust_mut().cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        // The run ends on its worker; the next run may start at once.
        self.set_running(false);
    }

    pub fn backlog(&self) -> QString {
        QString::from(self.rust().console.as_str())
    }

    pub fn clear_console(mut self: Pin<&mut Self>) {
        self.as_mut().rust_mut().console.clear();
        self.console_cleared();
    }

    /// Text for the console, from a command: kept (bounded) and sent on.
    pub fn append_console(mut self: Pin<&mut Self>, text: &str) {
        {
            let mut rust = self.as_mut().rust_mut();
            rust.console.push_str(text);
            if rust.console.len() > BACKLOG {
                let mut cut = rust.console.len() - BACKLOG / 2;
                while !rust.console.is_char_boundary(cut) {
                    cut += 1;
                }
                rust.console.drain(..cut);
            }
        }
        self.console_text(QString::from(text));
    }

    /// The agent changed (or read) a file.
    pub fn agent_touched(self: Pin<&mut Self>, touch: workbench::Touch) {
        match touch {
            workbench::Touch::Read(path) => self.touched(QString::from(path.as_str())),
            workbench::Touch::Edited(change) => self.agent_edited(change),
        }
    }

    fn agent_edited(self: Pin<&mut Self>, change: Change) {
        let marks = change.diff.marks_json();
        self.live_edit(
            QString::from(change.path.as_str()),
            QString::from(change.text.as_deref().unwrap_or("")),
            change.text.is_some(),
            QString::from(marks.as_str()),
            change
                .diff
                .first
                .map_or(0, |l| i32::try_from(l).unwrap_or(i32::MAX)),
        );
    }
}
