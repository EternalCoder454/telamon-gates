//! Telamon Gates, Rust side. `cpp/main.cpp` only starts Qt and loads the QML;
//! every QObject QML talks to is defined here. What has nothing to do with Qt
//! (conversations, their files, Markdown, the backend interface) is in the
//! `gates-core` crate.

mod chat;
mod fleet;
mod io;
mod library;
mod models;
mod settings;
mod user;
mod vram;

use cxx_qt::{CxxQtType, Threading};
use gates_core::{Backend, Store};
use std::ffi::c_void;
use std::sync::Arc;

// cxx-qt-build's generated initializer calls into cxx-qt-lib; keep the crate
// linked even while no bridge uses one of its types.
extern crate cxx_qt_lib;

// Who this app is, for the Telamon framework: `main.cpp`'s `telamon_app_init`
// and `telamon_app_ready` take the names, the logger and the crash hooks from
// it. The ID is the desktop file, the icon and the single-instance D-Bus name.
telamon_framework_ui::app! {
    name: "Telamon Gates",
    id: "net.eterneon.telamon.gates",
    repo: "telamon-gates",
    ui: "2.0.6",
}

/// The QObjects QML sees, handed to the engine as `Main.qml`'s initial
/// properties. The caller owns them; see `telamon_objects_new`.
#[repr(C)]
pub struct TelamonObjects {
    pub chat: *mut c_void,
    pub library: *mut c_void,
    pub vram: *mut c_void,
    pub models: *mut c_void,
    pub fleet: *mut c_void,
}

/// The backend replies come from: llama.cpp when telamon-llama (or a
/// `llama-server` on `$PATH`) is installed or a server address is set, else
/// the built-in demo. See `docs/BACKEND.md`.
fn backend() -> Arc<dyn Backend> {
    let options = settings::backend_options();
    let binary = gates_core::backend::llama::find_server();
    if binary.is_none() && options.server_url.is_empty() {
        log::info!("no llama-server: the demo backend answers");
        return Arc::new(gates_core::backend::Demo::default());
    }
    Arc::new(gates_core::backend::Llama::new(
        gates_core::store::data_dir().join("models"),
        binary,
        gates_core::store::state_dir().join("llama-server.log"),
        options,
    ))
}

/// Called once from `main.cpp`: makes every QObject and starts reading the
/// conversation list. Delete `chat` first: a reply under way posts to it.
#[unsafe(no_mangle)]
pub extern "C" fn telamon_objects_new() -> TelamonObjects {
    let store = Store::at(Store::default_dir());
    let folder = store.dir().to_string_lossy().into_owned();
    let io = io::Io::start(store);
    // There from the start, so Settings' Open Folder always opens it.
    // Gates' folders are the user's alone (conversations, pictures,
    // agents' work, logs).
    io.run(|store| {
        let data = gates_core::store::data_dir();
        let state = gates_core::store::state_dir();
        for dir in [data.as_path(), state.as_path(), store.dir()] {
            if let Err(e) = gates_core::store::private_dir(dir) {
                log::warn!("cannot make {} private: {e}", dir.display());
            }
        }
    });

    // One backend (and so one model server) for the chat and the fleet.
    let backend = backend();
    let mut chat = chat::qobject::chat_make_unique();
    let mut library = library::qobject::library_make_unique();

    {
        let mut rust = library.pin_mut().rust_mut();
        rust.io = Some(io.clone());
    }
    let chat_thread = chat.pin_mut().qt_thread();
    let library_thread = library.pin_mut().qt_thread();
    library.pin_mut().rust_mut().chat = Some(Box::new(chat_thread));
    library
        .pin_mut()
        .set_folder(cxx_qt_lib::QString::from(folder.as_str()));
    library.pin_mut().set_log_folder(cxx_qt_lib::QString::from(
        gates_core::store::state_dir().to_string_lossy().as_ref(),
    ));
    {
        let mut rust = chat.pin_mut().rust_mut();
        rust.io = Some(io);
        rust.backend = Some(backend.clone());
        // The web search key goes in the system keyring, nowhere else.
        rust.web_keys = Some(Arc::new(gates_core::web::SecretService));
        rust.library = Some(Box::new(library_thread));
    }
    chat.pin_mut().start();
    library.pin_mut().reload();
    let vram = vram::qobject::vram_make_unique();
    let mut models = models::qobject::model_library_make_unique();
    let models_dir = gates_core::store::data_dir().join("models");
    models.pin_mut().set_folder(cxx_qt_lib::QString::from(
        models_dir.to_string_lossy().as_ref(),
    ));
    models.pin_mut().rust_mut().dir = models_dir;
    // The free space isn't known until the Models page asks.
    models.pin_mut().set_free(-1.0);
    models.pin_mut().refresh();

    let mut fleet = fleet::qobject::fleet_make_unique();
    {
        let chat_thread = chat.pin_mut().qt_thread();
        let mut rust = fleet.pin_mut().rust_mut();
        rust.backend = Some(backend);
        rust.chat = Some(Box::new(chat_thread));
    }
    fleet.pin_mut().start_up();

    TelamonObjects {
        chat: chat.into_raw().cast(),
        library: library.into_raw().cast(),
        vram: vram.into_raw().cast(),
        models: models.into_raw().cast(),
        fleet: fleet.into_raw().cast(),
    }
}
