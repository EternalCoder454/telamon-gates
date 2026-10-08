//! Telamon Gates, Rust side. `cpp/main.cpp` only starts Qt and loads the QML;
//! every QObject QML talks to is defined here. What has nothing to do with Qt
//! (conversations, their files, Markdown, the backend interface) is in the
//! `gates-core` crate.

mod chat;
mod io;
mod library;
mod settings;

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
    ui: "2.0.2",
}

/// The QObjects QML sees, handed to the engine as `Main.qml`'s initial
/// properties. The caller owns them; see `telamon_objects_new`.
#[repr(C)]
pub struct TelamonObjects {
    pub chat: *mut c_void,
    pub library: *mut c_void,
}

/// The backend replies come from. This is the one line to change to connect
/// a real model: see `docs/BACKEND.md`.
fn backend() -> Arc<dyn Backend> {
    Arc::new(gates_core::backend::Demo::default())
}

/// Called once from `main.cpp`: makes every QObject and starts reading the
/// conversation list. Delete `chat` first: a reply under way posts to it.
#[unsafe(no_mangle)]
pub extern "C" fn telamon_objects_new() -> TelamonObjects {
    let store = Store::at(Store::default_dir());
    let folder = store.dir().to_string_lossy().into_owned();
    let io = io::Io::start(store);

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
    {
        let mut rust = chat.pin_mut().rust_mut();
        rust.io = Some(io);
        rust.backend = Some(backend());
        rust.library = Some(Box::new(library_thread));
    }
    chat.pin_mut().start();
    library.pin_mut().reload();

    TelamonObjects {
        chat: chat.into_raw().cast(),
        library: library.into_raw().cast(),
    }
}
