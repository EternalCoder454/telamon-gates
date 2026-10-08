//! The conversation list in the sidebar: every saved conversation, newest
//! first, each with the section it falls in ("today", "yesterday", ...).

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
        /// Each conversation's id, newest first.
        #[qproperty(QStringList, ids)]
        #[qproperty(QStringList, titles)]
        /// Each one's section: "today", "yesterday", "week" (the 7 days
        /// before), "day" (up to 30 days ago: headed by its date) or "older"
        /// (headed by its month), by when it last changed.
        #[qproperty(QStringList, sections)]
        /// When each last changed, in milliseconds since the epoch, for the
        /// date and month headings QML writes in the local time zone.
        #[qproperty(QList_f64, updates)]
        /// The list has been read once.
        #[qproperty(bool, loaded)]
        /// Where the conversations are kept, for Settings.
        #[qproperty(QString, folder)]
        #[namespace = "telamon_gates"]
        type Library = super::LibraryRust;
    }

    unsafe extern "RustQt" {
        /// Reads the list again from disk.
        #[qinvokable]
        fn reload(self: Pin<&mut Library>);

        /// The start of today, local time, in milliseconds since the epoch:
        /// QML knows the time zone, and sections count back from it.
        #[qinvokable]
        #[cxx_name = "setDayStart"]
        fn set_day_start(self: Pin<&mut Library>, ms: f64);

        /// Deletes a conversation for good: its file goes, and the chat
        /// leaves it if it is open.
        #[qinvokable]
        fn remove(self: Pin<&mut Library>, id: &QString);
    }

    impl cxx_qt::Threading for Library {}

    #[namespace = "rust::cxxqtlib1"]
    unsafe extern "C++" {
        include!("cxx-qt-lib/common.h");

        #[cxx_name = "make_unique"]
        fn library_make_unique() -> UniquePtr<Library>;
    }
}

use crate::chat;
use crate::io::Io;
use core::pin::Pin;
use cxx_qt::{CxxQtThread, CxxQtType, Threading};
use cxx_qt_lib::{QList, QString, QStringList};
use gates_core::Summary;

const DAY_MS: i64 = 86_400_000;

#[derive(Default)]
pub struct LibraryRust {
    ids: QStringList,
    titles: QStringList,
    sections: QStringList,
    updates: QList<f64>,
    loaded: bool,
    folder: QString,

    rows: Vec<Summary>,
    day_start: i64,
    pub io: Option<Io>,
    // Boxed: a thread handle is not Unpin, and the struct must be.
    pub chat: Option<Box<CxxQtThread<chat::qobject::Chat>>>,
}

/// Which section a conversation last changed at `updated` falls in, counted
/// from the start of today.
pub fn section(updated: i64, day_start: i64) -> &'static str {
    if updated >= day_start {
        "today"
    } else if updated >= day_start - DAY_MS {
        "yesterday"
    } else if updated >= day_start - 7 * DAY_MS {
        "week"
    } else if updated >= day_start - 30 * DAY_MS {
        "day"
    } else {
        "older"
    }
}

fn strings<'a>(items: impl Iterator<Item = &'a str>) -> QStringList {
    let mut list = QStringList::default();
    for s in items {
        list.append(QString::from(s));
    }
    list
}

impl qobject::Library {
    pub fn reload(self: Pin<&mut Self>) {
        let Some(io) = self.rust().io.clone() else {
            return;
        };
        let qt = self.qt_thread();
        io.run(move |store| {
            let list = store.list();
            let _ = qt.queue(move |mut lib| {
                lib.as_mut().rust_mut().rows = list;
                lib.as_mut().publish();
                lib.set_loaded(true);
            });
        });
    }

    pub fn set_day_start(mut self: Pin<&mut Self>, ms: f64) {
        // Whole milliseconds; NaN and the infinities can't be a day.
        let ms = if ms.is_finite() { ms as i64 } else { 0 };
        if ms != self.rust().day_start {
            self.as_mut().rust_mut().day_start = ms;
            self.publish();
        }
    }

    pub fn remove(mut self: Pin<&mut Self>, id: &QString) {
        let id = id.to_string();
        let before = self.rust().rows.len();
        self.as_mut().rust_mut().rows.retain(|r| r.id != id);
        if self.rust().rows.len() == before {
            return;
        }
        self.as_mut().publish();
        if let Some(chat) = &self.rust().chat {
            let id = id.clone();
            let _ = chat.queue(move |c| c.forget(&id));
        }
        if let Some(io) = &self.rust().io {
            io.run(move |store| {
                if let Err(e) = store.delete(&id) {
                    log::warn!("cannot delete conversation {id}: {e}");
                }
            });
        }
    }

    /// A conversation was saved: it goes to the top, with its new title.
    pub fn upsert(mut self: Pin<&mut Self>, summary: Summary) {
        let mut rust = self.as_mut().rust_mut();
        rust.rows.retain(|r| r.id != summary.id);
        let at = rust
            .rows
            .iter()
            .position(|r| r.updated <= summary.updated)
            .unwrap_or(rust.rows.len());
        rust.rows.insert(at, summary);
        self.publish();
    }

    fn publish(mut self: Pin<&mut Self>) {
        let day_start = self.rust().day_start;
        let rows = &self.rust().rows;
        let ids = strings(rows.iter().map(|r| r.id.as_str()));
        let titles = strings(rows.iter().map(|r| r.title.as_str()));
        let sections = strings(rows.iter().map(|r| section(r.updated, day_start)));
        let mut updates = QList::default();
        for r in rows {
            updates.append(r.updated as f64);
        }
        // Sections and times first: QML groups the ids by them.
        self.as_mut().set_sections(sections);
        self.as_mut().set_updates(updates);
        self.as_mut().set_titles(titles);
        self.set_ids(ids);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_count_back_from_today() {
        let today = 100 * DAY_MS;
        assert_eq!(section(today + 5, today), "today");
        assert_eq!(section(today - 1, today), "yesterday");
        assert_eq!(section(today - 3 * DAY_MS, today), "week");
        assert_eq!(section(today - 20 * DAY_MS, today), "day");
        assert_eq!(section(0, today), "older");
    }
}
