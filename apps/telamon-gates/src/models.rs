//! The Models page: the GGUF models in the models folder, Hugging Face
//! search, a repository's model files, and one download at a time. All file
//! and network work runs on worker threads; results come back through
//! `qt_thread().queue`, and a newer search or repository wins over an older
//! one still answering.

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
        /// The models on this computer: file names without `.gguf`.
        #[qproperty(QStringList, names)]
        /// Their quantisation ("Q4_K_M"), size label ("8B") and size in bytes.
        #[qproperty(QStringList, quants)]
        #[qproperty(QStringList, labels)]
        #[qproperty(QList_f64, sizes)]
        /// A decision model's kind ("laya", "kev"), "" for a chat model.
        #[qproperty(QStringList, kinds)]
        /// What each can do: the context it was trained for in tokens (0 when
        /// unknown), and 1 or 0 for whether its template takes tools
        /// (`toolCapable`) and whether a projector lets it read images.
        #[qproperty(QList_f64, contexts)]
        #[qproperty(QList_f64, tool_capable, cxx_name = "toolCapable")]
        #[qproperty(QList_f64, vision)]
        /// The architecture of a model the model server can't load (the
        /// Models page marks it Unsupported); "" for one it can.
        #[qproperty(QStringList, unsupported)]
        #[qproperty(QString, folder)]
        /// The free space on the models folder's disk in bytes; -1 when not
        /// known (yet).
        #[qproperty(f64, free)]
        /// The system memory in bytes (a mixture of experts may use part of
        /// it, so the fit badges ask); -1 when not known (yet).
        #[qproperty(f64, ram_total, cxx_name = "ramTotal")]
        /// Partial downloads left in the folder: the file each would become,
        /// bytes so far, the whole size (0 when not known), and the
        /// repository it comes from ("" when not known: no Resume).
        #[qproperty(QStringList, partials)]
        #[qproperty(QList_f64, partial_sizes, cxx_name = "partialSizes")]
        #[qproperty(QList_f64, partial_totals, cxx_name = "partialTotals")]
        #[qproperty(QStringList, partial_repos, cxx_name = "partialRepos")]
        /// Hugging Face repositories found, and their downloads.
        #[qproperty(QStringList, results)]
        #[qproperty(QList_f64, downloads)]
        #[qproperty(bool, searching)]
        /// The repository open, and its model files.
        #[qproperty(QString, repo)]
        #[qproperty(QStringList, files)]
        #[qproperty(QList_f64, file_sizes, cxx_name = "fileSizes")]
        #[qproperty(bool, listing)]
        /// The file downloading ("" for none), from which repository, and how
        /// far (0 to 1).
        #[qproperty(QString, downloading)]
        #[qproperty(QString, download_repo, cxx_name = "downloadRepo")]
        #[qproperty(f64, progress)]
        /// What went wrong last (search, listing, download, delete); "" none.
        #[qproperty(QString, error)]
        /// The UGI Leaderboard (`gates_core::leaderboard`): how many open
        /// models it lists (0 until loaded), when its copy was last known to
        /// be current (milliseconds since the epoch), a note about the copy
        /// ("" none), what went wrong loading it, and whether a load is under
        /// way. `ugiVersion` changes with each load: bind the rows to it.
        #[qproperty(f64, ugi_count, cxx_name = "ugiCount")]
        #[qproperty(f64, ugi_updated, cxx_name = "ugiUpdated")]
        #[qproperty(QString, ugi_note, cxx_name = "ugiNote")]
        #[qproperty(QString, ugi_error, cxx_name = "ugiError")]
        #[qproperty(bool, ugi_loading, cxx_name = "ugiLoading")]
        #[qproperty(f64, ugi_version, cxx_name = "ugiVersion")]
        #[namespace = "telamon_gates"]
        type ModelLibrary = super::ModelLibraryRust;
    }

    unsafe extern "RustQt" {
        /// Reads the models folder again.
        #[qinvokable]
        fn refresh(self: Pin<&mut ModelLibrary>);

        /// Deletes the model `name` from the folder.
        #[qinvokable]
        fn remove(self: Pin<&mut ModelLibrary>, name: &QString);

        #[qinvokable]
        fn search(self: Pin<&mut ModelLibrary>, query: &QString);

        /// Lists the model files of a repository from the results.
        #[qinvokable]
        #[cxx_name = "openRepo"]
        fn open_repo(self: Pin<&mut ModelLibrary>, repo: &QString);

        /// Downloads one of the open repository's files.
        #[qinvokable]
        fn download(self: Pin<&mut ModelLibrary>, file: &QString);

        /// Downloads `file` of `repo` without opening it first (Settings'
        /// one-click decision models).
        #[qinvokable]
        #[cxx_name = "downloadFrom"]
        fn download_from(self: Pin<&mut ModelLibrary>, repo: &QString, file: &QString);

        /// Deletes partial downloads older than 30 days, then lists the rest
        /// and the free space (on a worker). The Models page calls it when it
        /// opens.
        #[qinvokable]
        #[cxx_name = "refreshPartials"]
        fn refresh_partials(self: Pin<&mut ModelLibrary>);

        /// Deletes the partial download of `name` (one the list shows).
        #[qinvokable]
        #[cxx_name = "removePartial"]
        fn remove_partial(self: Pin<&mut ModelLibrary>, name: &QString);

        /// Carries on with the partial download of `name`, from its repository.
        #[qinvokable]
        #[cxx_name = "resumePartial"]
        fn resume_partial(self: Pin<&mut ModelLibrary>, name: &QString);

        #[qinvokable]
        #[cxx_name = "cancelDownload"]
        fn cancel_download(self: Pin<&mut ModelLibrary>);

        #[qinvokable]
        #[cxx_name = "dismissError"]
        fn dismiss_error(self: Pin<&mut ModelLibrary>);

        /// Shows the saved copy of the leaderboard, if there is one. Never
        /// asks the network; the Models page calls it when it opens.
        #[qinvokable]
        #[cxx_name = "openLeaderboard"]
        fn open_leaderboard(self: Pin<&mut ModelLibrary>);

        /// Loads the leaderboard: the saved copy when it is under a day old,
        /// else the Hub's (`force`: always asks the Hub).
        #[qinvokable]
        #[cxx_name = "loadLeaderboard"]
        fn load_leaderboard(self: Pin<&mut ModelLibrary>, force: bool);

        /// The leaderboard's models that pass the filters in `spec` (JSON,
        /// `gates_core::leaderboard::query`), best first, as JSON:
        /// `{"total": n, "rows": [...]}`. `version` only makes QML ask again
        /// after a load.
        #[qinvokable]
        #[cxx_name = "leaderboardRows"]
        fn leaderboard_rows(self: &ModelLibrary, spec: &QString, version: f64) -> QString;

        /// The models in the folder changed (downloaded or deleted).
        #[qsignal]
        #[cxx_name = "modelsChanged"]
        fn models_changed(self: Pin<&mut ModelLibrary>);
    }

    impl cxx_qt::Threading for ModelLibrary {}

    #[namespace = "rust::cxxqtlib1"]
    unsafe extern "C++" {
        include!("cxx-qt-lib/common.h");

        #[cxx_name = "make_unique"]
        fn model_library_make_unique() -> UniquePtr<ModelLibrary>;
    }
}

use core::pin::Pin;
use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::{CaseSensitivity, QList, QString, QStringList};
use gates_core::backend::llama::{
    find_server, local_models, local_projectors, projector_for, unsupported_architecture,
};
use gates_core::hub::{self, ModelFile};
use gates_core::leaderboard::{self, Entry, Loaded};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

#[derive(Default)]
pub struct ModelLibraryRust {
    names: QStringList,
    quants: QStringList,
    labels: QStringList,
    sizes: QList<f64>,
    kinds: QStringList,
    contexts: QList<f64>,
    tool_capable: QList<f64>,
    vision: QList<f64>,
    unsupported: QStringList,
    folder: QString,
    free: f64,
    ram_total: f64,
    partials: QStringList,
    partial_sizes: QList<f64>,
    partial_totals: QList<f64>,
    partial_repos: QStringList,
    results: QStringList,
    downloads: QList<f64>,
    searching: bool,
    repo: QString,
    files: QStringList,
    file_sizes: QList<f64>,
    listing: bool,
    downloading: QString,
    download_repo: QString,
    progress: f64,
    error: QString,
    ugi_count: f64,
    ugi_updated: f64,
    ugi_note: QString,
    ugi_error: QString,
    ugi_loading: bool,
    ugi_version: f64,

    pub dir: PathBuf,
    /// The leaderboard's open models, as loaded.
    ugi: Vec<Entry>,
    /// The open repository's files, with their checksums.
    open_files: Vec<ModelFile>,
    /// The partial downloads the properties show, for Resume.
    partial_list: Vec<hub::Partial>,
    /// Bumped by each search and each repository opened: older answers drop.
    asked: u64,
    cancel: Option<Arc<AtomicBool>>,
}

fn strings<S: AsRef<str>>(items: impl IntoIterator<Item = S>) -> QStringList {
    let mut list = QStringList::default();
    for s in items {
        list.append(QString::from(s.as_ref()));
    }
    list
}

fn numbers(items: impl IntoIterator<Item = f64>) -> QList<f64> {
    let mut list = QList::default();
    for n in items {
        list.append(n);
    }
    list
}

/// A yes as 1 and a no as 0, for a list QML reads.
fn flag(yes: bool) -> f64 {
    f64::from(u8::from(yes))
}

impl qobject::ModelLibrary {
    pub fn refresh(self: Pin<&mut Self>) {
        let dir = self.rust().dir.clone();
        let qt = self.qt_thread();
        std::thread::spawn(move || {
            let _ = std::fs::create_dir_all(&dir);
            // From each one's header: quantisation, size label, kind, context
            // and tools; images from a projector file beside it.
            let found = local_models(&dir);
            let projectors = local_projectors(&dir);
            // The architecture list is telamon-llama's: another server found
            // on the path may load more.
            let server = find_server();
            let models: Vec<_> = found
                .iter()
                .map(|m| {
                    let quant = if m.info.quant.is_empty() {
                        gates_core::gguf::quant_from_name(&m.name)
                    } else {
                        m.info.quant.clone()
                    };
                    (
                        m.name.clone(),
                        quant,
                        m.info.size_label.clone(),
                        m.size as f64,
                        // "draft": a speed-up draft for another model.
                        if gates_core::backend::llama::draft_kind(&m.name).is_some() {
                            "draft".to_string()
                        } else {
                            m.info.decision.clone()
                        },
                        f64::from(m.info.context_length),
                        flag(m.info.tools),
                        flag(projector_for(m, &found, &projectors).is_some()),
                        // The architecture the server can't load, else "".
                        unsupported_architecture(&m.info, server.as_deref())
                            .unwrap_or_default()
                            .to_string(),
                    )
                })
                .collect();
            let _ = qt.queue(move |mut lib| {
                lib.as_mut()
                    .set_names(strings(models.iter().map(|m| m.0.as_str())));
                lib.as_mut()
                    .set_quants(strings(models.iter().map(|m| m.1.as_str())));
                lib.as_mut()
                    .set_labels(strings(models.iter().map(|m| m.2.as_str())));
                lib.as_mut()
                    .set_kinds(strings(models.iter().map(|m| m.4.as_str())));
                lib.as_mut()
                    .set_contexts(numbers(models.iter().map(|m| m.5)));
                lib.as_mut()
                    .set_tool_capable(numbers(models.iter().map(|m| m.6)));
                lib.as_mut().set_vision(numbers(models.iter().map(|m| m.7)));
                lib.as_mut()
                    .set_unsupported(strings(models.iter().map(|m| m.8.as_str())));
                lib.set_sizes(numbers(models.iter().map(|m| m.3)));
            });
        });
    }

    pub fn remove(mut self: Pin<&mut Self>, name: &QString) {
        let name = name.to_string();
        let path = self.rust().dir.join(format!("{name}.gguf"));
        // Only a model the list shows: no path from QML reaches the disk.
        if !self.names().contains(
            &QString::from(name.as_str()),
            CaseSensitivity::CaseSensitive,
        ) || name.contains('/')
        {
            return;
        }
        self.as_mut().set_error(QString::default());
        let qt = self.qt_thread();
        std::thread::spawn(move || {
            let result = std::fs::remove_file(&path);
            let _ = qt.queue(move |mut lib| {
                if let Err(e) = result {
                    lib.as_mut().set_error(QString::from(
                        format!("Couldn't delete {name}: {e}.").as_str(),
                    ));
                }
                lib.as_mut().refresh();
                lib.models_changed();
            });
        });
    }

    pub fn search(mut self: Pin<&mut Self>, query: &QString) {
        let query = query.to_string();
        let asked = {
            let mut rust = self.as_mut().rust_mut();
            rust.asked += 1;
            rust.asked
        };
        self.as_mut().set_repo(QString::default());
        self.as_mut().set_files(QStringList::default());
        if query.trim().is_empty() {
            self.as_mut().set_results(QStringList::default());
            self.set_searching(false);
            return;
        }
        self.as_mut().set_searching(true);
        self.as_mut().set_error(QString::default());
        let qt = self.qt_thread();
        std::thread::spawn(move || {
            let result = hub::search(&query);
            let _ = qt.queue(move |mut lib| {
                if lib.rust().asked != asked {
                    return;
                }
                lib.as_mut().set_searching(false);
                match result {
                    Ok(repos) => {
                        lib.as_mut()
                            .set_results(strings(repos.iter().map(|r| r.id.as_str())));
                        lib.set_downloads(numbers(repos.iter().map(|r| r.downloads as f64)));
                    }
                    Err(e) => lib.set_error(QString::from(e.as_str())),
                }
            });
        });
    }

    pub fn open_repo(mut self: Pin<&mut Self>, repo: &QString) {
        let repo_id = repo.to_string();
        let asked = {
            let mut rust = self.as_mut().rust_mut();
            rust.asked += 1;
            rust.open_files.clear();
            rust.asked
        };
        self.as_mut().set_repo(repo.clone());
        self.as_mut().set_files(QStringList::default());
        self.as_mut().set_file_sizes(QList::default());
        // "" closes the repository.
        if repo_id.is_empty() {
            self.set_listing(false);
            return;
        }
        self.as_mut().set_listing(true);
        self.as_mut().set_error(QString::default());
        let qt = self.qt_thread();
        std::thread::spawn(move || {
            let result = hub::files(&repo_id);
            let _ = qt.queue(move |mut lib| {
                if lib.rust().asked != asked {
                    return;
                }
                lib.as_mut().set_listing(false);
                match result {
                    Ok(files) => {
                        lib.as_mut()
                            .set_files(strings(files.iter().map(|f| f.name.as_str())));
                        lib.as_mut()
                            .set_file_sizes(numbers(files.iter().map(|f| f.size as f64)));
                        if files.is_empty() {
                            lib.as_mut().set_error(QString::from(
                                "This repository has no single-file GGUF model.",
                            ));
                        }
                        lib.rust_mut().open_files = files;
                    }
                    Err(e) => lib.set_error(QString::from(e.as_str())),
                }
            });
        });
    }

    pub fn download(self: Pin<&mut Self>, file: &QString) {
        if !self.downloading().is_empty() {
            return;
        }
        let name = file.to_string();
        // Only a file the open repository listed, with its size and checksum.
        let Some(model) = self
            .rust()
            .open_files
            .iter()
            .find(|f| f.name == name)
            .cloned()
        else {
            return;
        };
        let repo = self.repo().to_string();
        self.start_download(repo, model);
    }

    pub fn download_from(mut self: Pin<&mut Self>, repo: &QString, file: &QString) {
        if !self.downloading().is_empty() {
            return;
        }
        let (repo, name) = (repo.to_string(), file.to_string());
        // Shown at once; the size and checksum come from the listing.
        self.as_mut().set_error(QString::default());
        self.as_mut().set_progress(0.0);
        self.as_mut()
            .set_download_repo(QString::from(repo.as_str()));
        self.as_mut().set_downloading(QString::from(name.as_str()));
        // Cancel works from now, while the listing is still coming.
        let cancel = Arc::new(AtomicBool::new(false));
        self.as_mut().rust_mut().cancel = Some(cancel.clone());
        let qt = self.qt_thread();
        std::thread::spawn(move || {
            let found = hub::files(&repo).and_then(|files| {
                files
                    .into_iter()
                    .find(|f| f.name == name)
                    .ok_or_else(|| format!("{repo} has no {name}."))
            });
            let _ = qt.queue(move |mut lib| {
                lib.as_mut().set_downloading(QString::default());
                lib.as_mut().rust_mut().cancel = None;
                match found {
                    Ok(_) if cancel.load(Ordering::Relaxed) => {
                        lib.set_download_repo(QString::default());
                    }
                    Ok(model) => lib.start_download(repo, model),
                    Err(e) => {
                        lib.as_mut().set_download_repo(QString::default());
                        lib.set_error(QString::from(e.as_str()));
                    }
                }
            });
        });
    }

    fn start_download(mut self: Pin<&mut Self>, repo: String, model: ModelFile) {
        let file = QString::from(model.name.as_str());
        let dir = self.rust().dir.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        self.as_mut().rust_mut().cancel = Some(cancel.clone());
        self.as_mut().set_error(QString::default());
        self.as_mut().set_progress(0.0);
        self.as_mut()
            .set_download_repo(QString::from(repo.as_str()));
        self.as_mut().set_downloading(file.clone());
        let qt = self.qt_thread();
        std::thread::spawn(move || {
            let total = model.size.max(1) as f64;
            let mut last = Instant::now() - Duration::from_secs(1);
            let progress_qt = qt.clone();
            let result = hub::download(&repo, &model, &dir, &cancel, &mut |bytes| {
                // About ten times a second is enough for a bar.
                if last.elapsed() >= Duration::from_millis(100) {
                    last = Instant::now();
                    let done = bytes as f64 / total;
                    let _ = progress_qt.queue(move |lib| lib.set_progress(done));
                }
            });
            let cancelled = cancel.load(Ordering::Relaxed);
            let _ = qt.queue(move |mut lib| {
                lib.as_mut().rust_mut().cancel = None;
                lib.as_mut().set_downloading(QString::default());
                lib.as_mut().set_download_repo(QString::default());
                lib.as_mut().set_progress(0.0);
                // A cancelled or failed download leaves a part; a finished
                // one frees it, and the disk has less room either way.
                lib.as_mut().refresh_partials();
                match result {
                    Ok(_) => {
                        lib.as_mut().refresh();
                        lib.models_changed();
                    }
                    Err(_) if cancelled => {}
                    Err(e) => lib.set_error(QString::from(e.as_str())),
                }
            });
        });
    }

    pub fn refresh_partials(self: Pin<&mut Self>) {
        let dir = self.rust().dir.clone();
        // A download under way is not stale, whatever its part's age.
        let keep = self.downloading().to_string();
        let qt = self.qt_thread();
        std::thread::spawn(move || {
            for name in hub::remove_stale(&dir, hub::STALE_AFTER, SystemTime::now(), &keep) {
                log::info!("deleted the partial download of {name}: untouched for 30 days");
            }
            let parts = hub::partials(&dir);
            let free = hub::free_space(&dir).map_or(-1.0, |bytes| bytes as f64);
            let ram = leaderboard::system_memory().map_or(-1.0, |bytes| bytes as f64);
            let _ = qt.queue(move |mut lib| {
                lib.as_mut()
                    .set_partials(strings(parts.iter().map(|p| p.name.as_str())));
                lib.as_mut()
                    .set_partial_sizes(numbers(parts.iter().map(|p| p.bytes as f64)));
                lib.as_mut()
                    .set_partial_totals(numbers(parts.iter().map(|p| p.total as f64)));
                lib.as_mut().set_partial_repos(strings(
                    parts.iter().map(|p| p.repo.as_deref().unwrap_or("")),
                ));
                lib.as_mut().rust_mut().partial_list = parts;
                lib.as_mut().set_ram_total(ram);
                lib.set_free(free);
            });
        });
    }

    pub fn remove_partial(mut self: Pin<&mut Self>, name: &QString) {
        let name = name.to_string();
        // Only a leftover the list shows, and not the one being downloaded.
        if self.downloading().to_string() == name
            || !self.rust().partial_list.iter().any(|p| p.name == name)
        {
            return;
        }
        self.as_mut().set_error(QString::default());
        let dir = self.rust().dir.clone();
        let qt = self.qt_thread();
        std::thread::spawn(move || {
            let result = hub::remove_partial(&dir, &name);
            let _ = qt.queue(move |mut lib| {
                if let Err(e) = result {
                    lib.as_mut().set_error(QString::from(
                        format!("Couldn't delete the partial download of {name}: {e}.").as_str(),
                    ));
                }
                lib.refresh_partials();
            });
        });
    }

    pub fn resume_partial(self: Pin<&mut Self>, name: &QString) {
        let name = name.to_string();
        let repo = self
            .rust()
            .partial_list
            .iter()
            .find(|p| p.name == name)
            .and_then(|p| p.repo.clone());
        if let Some(repo) = repo {
            self.download_from(&QString::from(repo.as_str()), &QString::from(name.as_str()));
        }
    }

    pub fn cancel_download(self: Pin<&mut Self>) {
        if let Some(cancel) = &self.rust().cancel {
            cancel.store(true, Ordering::Relaxed);
        }
    }

    pub fn dismiss_error(self: Pin<&mut Self>) {
        self.set_error(QString::default());
    }

    pub fn open_leaderboard(self: Pin<&mut Self>) {
        if self.rust().ugi_loading || self.rust().ugi_count > 0.0 {
            return;
        }
        let qt = self.qt_thread();
        std::thread::spawn(move || {
            let held = leaderboard::cached(&leaderboard::cache_path());
            let _ = qt.queue(move |lib| {
                // A load the user started meanwhile wins.
                let idle = !lib.rust().ugi_loading && lib.rust().ugi_count == 0.0;
                if let (Some(held), true) = (held, idle) {
                    lib.show_leaderboard(held);
                }
            });
        });
    }

    pub fn load_leaderboard(mut self: Pin<&mut Self>, force: bool) {
        if self.rust().ugi_loading {
            return;
        }
        self.as_mut().set_ugi_loading(true);
        self.as_mut().set_ugi_error(QString::default());
        let qt = self.qt_thread();
        std::thread::spawn(move || {
            let result = leaderboard::load(force);
            let _ = qt.queue(move |mut lib| {
                lib.as_mut().set_ugi_loading(false);
                match result {
                    Ok(loaded) => lib.show_leaderboard(loaded),
                    Err(e) => lib.set_ugi_error(QString::from(e.as_str())),
                }
            });
        });
    }

    fn show_leaderboard(mut self: Pin<&mut Self>, loaded: Loaded) {
        let millis = loaded
            .fetched
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0.0, |d| d.as_millis() as f64);
        self.as_mut()
            .set_ugi_count(loaded.table.entries.len() as f64);
        self.as_mut().set_ugi_updated(millis);
        self.as_mut()
            .set_ugi_note(QString::from(loaded.note.as_deref().unwrap_or("")));
        self.as_mut().rust_mut().ugi = loaded.table.entries;
        let version = self.rust().ugi_version + 1.0;
        self.set_ugi_version(version);
    }

    pub fn leaderboard_rows(&self, spec: &QString, _version: f64) -> QString {
        QString::from(leaderboard::query(&self.rust().ugi, &spec.to_string()).as_str())
    }
}
