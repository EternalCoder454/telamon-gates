//! One thread for every file the app reads or writes: conversations and
//! settings. Jobs run in the order they were sent, so a conversation is never
//! read before its last save has landed, and the GUI thread never waits.

use gates_core::Store;
use std::sync::mpsc::{self, Sender};

type Job = Box<dyn FnOnce(&Store) + Send>;

#[derive(Clone)]
pub struct Io {
    tx: Sender<Job>,
}

impl Io {
    pub fn start(store: Store) -> Io {
        let (tx, rx) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("gates-io".into())
            .spawn(move || {
                for job in rx {
                    // One failed job must not take the others down with it.
                    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(&store)))
                        .is_err()
                    {
                        log::error!("a file job panicked");
                    }
                }
            })
            .expect("cannot start the file thread");
        Io { tx }
    }

    pub fn run(&self, job: impl FnOnce(&Store) + Send + 'static) {
        if self.tx.send(Box::new(job)).is_err() {
            log::error!("the file thread is gone");
        }
    }
}
