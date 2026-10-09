//! The graphics-memory watchdog: while a model server Gates runs (the chat
//! model's or SystemOne's) is loading or running, it reads how much of the
//! card's memory is in use every 2 s. When two readings in a row are at or
//! over the limit (95% by default; Settings), it tells the window (which
//! cancels the replies and the Fleet's run), stops every server Gates runs
//! and logs it. A full card can take the whole computer down with it.
//!
//! What counts is the card's total in use, whoever uses it: the goal is
//! never to fill it, not to blame anyone.
//!
//! Off where there is nothing to read (no `mem_info_vram_*` files: NVIDIA's
//! and Intel's drivers, the dev container, Xvfb) and with a server run
//! elsewhere (Gates can't stop it; it has no `Server` here). The thread
//! lives only while a server does, and reads two small files a time.
//!
//! The restart is the user's: a stopped server is not started again by
//! itself, and the next one starts only when the card is below the limit
//! less `MARGIN` points (`Watchdog::check_start`).

use crate::applog;
use crate::backend::server::Server;
use crate::vram::{self, Vram};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

/// How often the card is read.
pub const PERIOD: Duration = Duration::from_secs(2);
/// Readings in a row at or over the limit that stop the servers: one is a
/// spike (a model loading its layers, a game starting).
pub const READINGS: u32 = 2;
/// A server starts only when the card is this many points below the limit,
/// so it doesn't start just to be stopped again.
pub const MARGIN: u8 = 5;
/// What Settings offers besides Off.
pub const CHOICES: [u8; 4] = [85, 90, 95, 98];
/// 90% would stop the recommended 30B model in normal use (about 85–90% of
/// 24 GiB with its context).
pub const DEFAULT_PERCENT: u8 = 95;

/// The limit on graphics memory in use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cap {
    Off,
    /// Of the card's total memory.
    Percent(u8),
}

impl Default for Cap {
    fn default() -> Cap {
        Cap::Percent(DEFAULT_PERCENT)
    }
}

impl Cap {
    /// From the settings file: unset (or anything unknown) is the default,
    /// "off" is Off, else one of `CHOICES`.
    pub fn from_setting(value: &str) -> Cap {
        let value = value.trim();
        if value.eq_ignore_ascii_case("off") {
            return Cap::Off;
        }
        match value.parse::<u8>() {
            Ok(n) if CHOICES.contains(&n) => Cap::Percent(n),
            _ => Cap::default(),
        }
    }

    /// For the settings file: "" for the default, so it stays unset.
    pub fn setting(self) -> String {
        match self {
            Cap::Off => "off".into(),
            Cap::Percent(DEFAULT_PERCENT) => String::new(),
            Cap::Percent(n) => n.to_string(),
        }
    }

    /// From the number the window shows: 0 for Off, else a percent (one of
    /// `CHOICES`, else the default).
    pub fn from_number(n: i32) -> Cap {
        match n {
            0 => Cap::Off,
            n => Cap::from_setting(&n.to_string()),
        }
    }

    /// The number the window shows: 0 for Off.
    pub fn number(self) -> u8 {
        match self {
            Cap::Off => 0,
            Cap::Percent(n) => n,
        }
    }

    /// `v` is at or over the limit.
    pub fn reached(self, v: Vram) -> bool {
        match self {
            Cap::Off => false,
            Cap::Percent(n) => u128::from(v.used) * 100 >= u128::from(v.total) * u128::from(n),
        }
    }

    /// A server may start at `v`: the card is below the limit less `MARGIN`
    /// points.
    pub fn allows_start(self, v: Vram) -> bool {
        match self {
            Cap::Off => true,
            Cap::Percent(n) => {
                u128::from(v.used) * 100
                    < u128::from(v.total) * u128::from(n.saturating_sub(MARGIN))
            }
        }
    }
}

/// Counts the readings in a row that are at or over the limit.
#[derive(Debug, Default)]
pub struct Tripwire {
    over: u32,
}

impl Tripwire {
    /// Takes a reading; true when `READINGS` in a row are at or over `cap`.
    pub fn observe(&mut self, cap: Cap, v: Vram) -> bool {
        if cap.reached(v) {
            self.over += 1;
        } else {
            self.over = 0;
        }
        self.over >= READINGS
    }
}

/// Memory in GiB with one decimal ("23.4").
fn gib(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / (1u64 << 30) as f64)
}

/// The card's memory in use, as a whole percent.
fn percent_used(v: Vram) -> u64 {
    if v.total == 0 {
        return 0;
    }
    ((u128::from(v.used) * 100 + u128::from(v.total) / 2) / u128::from(v.total)) as u64
}

const ADVICE: &str = "Close other programs that use the graphics card, or pick a smaller model.";

/// The limit was reached: what the readings said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Trip {
    /// The limit, in percent.
    pub percent: u8,
    pub used: u64,
    pub total: u64,
}

impl Trip {
    /// For the window, the log and an error: plain text, a few sentences.
    pub fn message(&self) -> String {
        format!(
            "Stopped the model: graphics memory reached {}% ({} of {} GiB). {ADVICE}",
            self.percent,
            gib(self.used),
            gib(self.total)
        )
    }
}

/// Why a server isn't started: the card is too full already.
fn refusal(cap_percent: u8, v: Vram) -> String {
    format!(
        "Not starting the model: graphics memory is at {}% ({} of {} GiB), too full to load a model under the {cap_percent}% limit. {ADVICE}",
        percent_used(v),
        gib(v.used),
        gib(v.total)
    )
}

type Reader = Box<dyn Fn() -> Option<Vram> + Send + Sync>;
type Hook = Arc<dyn Fn(&Trip) + Send + Sync>;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub struct Watchdog {
    cap: Mutex<Cap>,
    reader: Reader,
    period: Duration,
    /// The servers Gates runs (the chat model's, SystemOne's).
    servers: Mutex<Vec<Weak<Server>>>,
    /// Told first when the limit is reached: the window cancels what is
    /// under way with it.
    hook: Mutex<Option<Hook>>,
    /// The thread runs. Held while it decides to end, so a server that
    /// starts then is not left unwatched.
    watching: Mutex<bool>,
    last: Mutex<Option<Trip>>,
}

impl Watchdog {
    /// Reads the card's files (`vram::read`) every `PERIOD`.
    pub fn new(cap: Cap) -> Arc<Watchdog> {
        Watchdog::with_reader(cap, Box::new(vram::read), PERIOD)
    }

    /// As `new`, with another way to read the card and another period
    /// (tests).
    pub fn with_reader(cap: Cap, reader: Reader, period: Duration) -> Arc<Watchdog> {
        Arc::new(Watchdog {
            cap: Mutex::new(cap),
            reader,
            period,
            servers: Mutex::new(Vec::new()),
            hook: Mutex::new(None),
            watching: Mutex::new(false),
            last: Mutex::new(None),
        })
    }

    pub fn cap(&self) -> Cap {
        *lock(&self.cap)
    }

    /// A new limit, from now on. Never blocks.
    pub fn set_cap(self: &Arc<Self>, cap: Cap) {
        *lock(&self.cap) = cap;
        // From Off to a limit with a server up: watch it now.
        self.wake();
    }

    /// `hook` is called, on the watchdog's thread, when the limit is
    /// reached and before the servers are stopped: it cancels the replies,
    /// so they end as stopped and not as failed.
    pub fn on_trip(&self, hook: impl Fn(&Trip) + Send + Sync + 'static) {
        *lock(&self.hook) = Some(Arc::new(hook));
    }

    /// Whether the thread runs (tests).
    #[cfg(test)]
    pub(crate) fn watching(&self) -> bool {
        *lock(&self.watching)
    }

    /// The last time the limit was reached.
    pub fn last_trip(&self) -> Option<Trip> {
        *lock(&self.last)
    }

    /// `server` is one to watch and, when the limit is reached, to stop.
    pub(crate) fn register(&self, server: &Arc<Server>) {
        let mut servers = lock(&self.servers);
        servers.retain(|s| s.strong_count() > 0);
        servers.push(Arc::downgrade(server));
    }

    fn live(&self) -> Vec<Arc<Server>> {
        let mut servers = lock(&self.servers);
        servers.retain(|s| s.strong_count() > 0);
        servers.iter().filter_map(Weak::upgrade).collect()
    }

    /// A server starts, or the limit changed: starts watching if it isn't
    /// and a server is up or loading. Never blocks (a server that loads
    /// holds its lock for minutes: `Server::is_live` doesn't wait for it).
    pub(crate) fn wake(self: &Arc<Self>) {
        if self.cap() == Cap::Off || !self.live().iter().any(|s| s.is_live()) {
            return;
        }
        let mut watching = lock(&self.watching);
        if *watching {
            return;
        }
        let me = self.clone();
        let started = std::thread::Builder::new()
            .name("gates-vram-watch".into())
            .spawn(move || me.watch());
        *watching = started.is_ok();
    }

    /// Whether a server may start now: the card is below the limit less
    /// `MARGIN` points, or there is no limit, or no reading. The reason, in
    /// words, when not.
    pub(crate) fn check_start(&self) -> Result<(), String> {
        let cap = self.cap();
        let Cap::Percent(percent) = cap else {
            return Ok(());
        };
        match (self.reader)() {
            Some(v) if !cap.allows_start(v) => Err(refusal(percent, v)),
            _ => Ok(()),
        }
    }

    /// The thread: reads the card until no server is up or there is
    /// nothing to read (a trip stops the servers, so it ends then).
    fn watch(self: Arc<Self>) {
        let mut wire = Tripwire::default();
        let mut last_cap = self.cap();
        loop {
            let (cap, v, servers) = {
                let mut watching = lock(&self.watching);
                let cap = self.cap();
                let servers: Vec<Arc<Server>> =
                    self.live().into_iter().filter(|s| s.is_live()).collect();
                let reading = match cap {
                    Cap::Percent(_) if !servers.is_empty() => (self.reader)(),
                    _ => None,
                };
                let Some(v) = reading else {
                    *watching = false;
                    return;
                };
                (cap, v, servers)
            };
            if cap != last_cap {
                wire = Tripwire::default();
                last_cap = cap;
            }
            if let Cap::Percent(percent) = cap
                && wire.observe(cap, v)
            {
                self.trip(
                    Trip {
                        percent,
                        used: v.used,
                        total: v.total,
                    },
                    &servers,
                );
                // Starts over; ends at the top of the loop if nothing runs.
                wire = Tripwire::default();
            }
            std::thread::sleep(self.period);
        }
    }

    fn trip(&self, trip: Trip, servers: &[Arc<Server>]) {
        applog::warn(&trip.message());
        *lock(&self.last) = Some(trip);
        let hook = lock(&self.hook).clone();
        if let Some(hook) = hook {
            // Whatever the window does, the servers stop.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| hook(&trip)));
        }
        // Each waits up to 3 s for its server to quit, then kills it: side
        // by side.
        std::thread::scope(|scope| {
            for server in servers {
                scope.spawn(move || server.halt());
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1 << 30;

    fn card(used_gib: f64) -> Vram {
        Vram {
            used: (used_gib * GIB as f64) as u64,
            total: 24 * GIB,
        }
    }

    #[test]
    fn the_limit_is_at_or_over() {
        let cap = Cap::Percent(95);
        // 95% of 24 GiB is 22.8 GiB.
        assert!(!cap.reached(card(22.79)));
        assert!(cap.reached(card(22.81)));
        assert!(cap.reached(card(24.0)));
        let exact = Vram {
            used: 95,
            total: 100,
        };
        assert!(cap.reached(exact), "exactly at the limit counts");
        assert!(!cap.reached(Vram { used: 94, ..exact }));
        assert!(!Cap::Off.reached(card(24.0)));
        // The other choices.
        assert!(Cap::Percent(85).reached(card(20.5)));
        assert!(!Cap::Percent(98).reached(card(23.0)));
    }

    #[test]
    fn two_readings_in_a_row() {
        let cap = Cap::Percent(95);
        let mut wire = Tripwire::default();
        assert!(!wire.observe(cap, card(23.0)), "one is a spike");
        assert!(!wire.observe(cap, card(10.0)), "back under: starts over");
        assert!(!wire.observe(cap, card(23.5)));
        assert!(wire.observe(cap, card(23.9)), "two in a row");
        assert!(wire.observe(cap, card(23.9)), "and still over");
        let mut off = Tripwire::default();
        for _ in 0..5 {
            assert!(!off.observe(Cap::Off, card(24.0)));
        }
    }

    #[test]
    fn a_server_starts_below_the_limit_less_the_margin() {
        let cap = Cap::Percent(95);
        // 90% of 24 GiB is 21.6 GiB.
        assert!(cap.allows_start(card(21.59)));
        assert!(!cap.allows_start(card(21.61)));
        assert!(!cap.allows_start(card(22.9)), "over the limit too");
        // 80% of 24 GiB is 19.2 GiB.
        assert!(Cap::Percent(85).allows_start(card(19.1)));
        assert!(!Cap::Percent(85).allows_start(card(19.3)));
        assert!(Cap::Off.allows_start(card(24.0)));
    }

    #[test]
    fn the_setting() {
        assert_eq!(Cap::from_setting(""), Cap::Percent(95));
        assert_eq!(Cap::from_setting("off"), Cap::Off);
        assert_eq!(Cap::from_setting(" Off "), Cap::Off);
        for n in CHOICES {
            let cap = Cap::from_setting(&n.to_string());
            assert_eq!(cap, Cap::Percent(n));
            assert_eq!(Cap::from_setting(&cap.setting()), cap);
            assert_eq!(Cap::from_number(i32::from(cap.number())), cap);
        }
        // Not a choice: the default, never "no limit".
        assert_eq!(Cap::from_setting("100"), Cap::Percent(95));
        assert_eq!(Cap::from_setting("0"), Cap::Percent(95));
        assert_eq!(Cap::from_setting("lots"), Cap::Percent(95));
        assert_eq!(Cap::Percent(95).setting(), "", "the default stays unset");
        assert_eq!(Cap::Off.setting(), "off");
        assert_eq!(Cap::from_number(0), Cap::Off);
        assert_eq!(Cap::Off.number(), 0);
        assert_eq!(Cap::from_number(77), Cap::Percent(95));
        assert_eq!(Cap::from_number(-3), Cap::Percent(95));
    }

    #[test]
    fn what_the_user_is_told() {
        let trip = Trip {
            percent: 95,
            used: (23.4 * GIB as f64) as u64,
            total: 24 * GIB,
        };
        assert_eq!(
            trip.message(),
            "Stopped the model: graphics memory reached 95% (23.4 of 24.0 GiB). \
             Close other programs that use the graphics card, or pick a smaller model."
        );
        let no = refusal(95, card(22.0));
        assert!(
            no.starts_with("Not starting the model: graphics memory is at 92%"),
            "{no}"
        );
        assert!(no.contains("(22.0 of 24.0 GiB)"), "{no}");
        assert!(no.contains("under the 95% limit"), "{no}");
        assert!(no.ends_with("or pick a smaller model."), "{no}");
    }
}
