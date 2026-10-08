//! The graphics card's video memory, for the meter in the sidebar: read
//! from sysfs on a worker thread whenever QML asks (it asks every few
//! seconds while the window can be seen).

#[cxx_qt::bridge]
pub mod qobject {
    extern "RustQt" {
        #[qobject]
        /// A card reports its video memory (amdgpu); false hides the meter.
        #[qproperty(bool, available)]
        /// Bytes; `f64` so QML gets them whole past 4 GiB.
        #[qproperty(f64, used)]
        #[qproperty(f64, total)]
        #[namespace = "telamon_gates"]
        type Vram = super::VramRust;
    }

    unsafe extern "RustQt" {
        /// Reads the figures again.
        #[qinvokable]
        fn refresh(self: Pin<&mut Vram>);
    }

    impl cxx_qt::Threading for Vram {}

    #[namespace = "rust::cxxqtlib1"]
    unsafe extern "C++" {
        include!("cxx-qt-lib/common.h");

        #[cxx_name = "make_unique"]
        fn vram_make_unique() -> UniquePtr<Vram>;
    }
}

use core::pin::Pin;
use cxx_qt::{CxxQtType, Threading};

#[derive(Default)]
pub struct VramRust {
    available: bool,
    used: f64,
    total: f64,
    /// A read is under way: the next ask waits for it.
    reading: bool,
}

impl qobject::Vram {
    pub fn refresh(mut self: Pin<&mut Self>) {
        if self.rust().reading {
            return;
        }
        self.as_mut().rust_mut().reading = true;
        let qt = self.qt_thread();
        std::thread::spawn(move || {
            let read = gates_core::vram::read();
            let _ = qt.queue(move |mut vram| {
                vram.as_mut().rust_mut().reading = false;
                match read {
                    Some(v) => {
                        vram.as_mut().set_total(v.total as f64);
                        vram.as_mut().set_used(v.used as f64);
                        vram.set_available(true);
                    }
                    None => vram.set_available(false),
                }
            });
        });
    }
}
