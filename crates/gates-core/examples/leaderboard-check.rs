//! Loads the live UGI Leaderboard the way the Models page does, into a cache
//! file of its own (not the user's), three times: no copy (a download), a
//! fresh copy (no network), and a forced refresh (a conditional request,
//! which the Hub answers 304 to), then lists the top models:
//!   cargo run -p gates-core --example leaderboard-check -- [cache-file] [search]

use gates_core::leaderboard::{Filter, Reply, fetch_from_hub, load_with, select};
use std::cell::Cell;
use std::time::{Instant, SystemTime};

fn main() {
    let mut args = std::env::args().skip(1);
    let cache = args
        .next()
        .unwrap_or_else(|| "/tmp/telamon-gates-ugi-check/ugi.csv".into());
    let search = args.next().unwrap_or_default();
    let cache = std::path::PathBuf::from(cache);
    let _ = std::fs::remove_file(&cache);
    let mut table = None;
    for (round, force) in [(1, false), (2, false), (3, true)] {
        let started = Instant::now();
        let asked = Cell::new(false);
        let loaded = load_with(&cache, force, SystemTime::now(), &|etag| {
            asked.set(true);
            let reply = fetch_from_hub(etag)?;
            match &reply {
                Reply::NotModified => {
                    println!("round {round}: 304 Not Modified (etag held: {etag:?})")
                }
                Reply::Body { text, etag } => {
                    println!("round {round}: 200, {} bytes, etag {etag:?}", text.len());
                }
            }
            Ok(reply)
        });
        match loaded {
            Ok(l) => {
                println!(
                    "round {round}: asked the Hub: {}, {} open models, {} rows skipped, {:?}",
                    asked.get(),
                    l.table.entries.len(),
                    l.table.skipped,
                    started.elapsed()
                );
                table = Some(l.table);
            }
            Err(e) => println!("round {round}: {e}"),
        }
    }
    if let Some(table) = table {
        let filter = Filter {
            search,
            ..Filter::default()
        };
        for e in select(&table.entries, &filter).iter().take(5) {
            println!(
                "{:>6.2}  W/10 {:>4}  {:>6}B  {:<8}  {}  ->  {}",
                e.ugi.unwrap_or(f64::NAN),
                e.willingness.map_or("NA".into(), |w| format!("{w:.1}")),
                e.total,
                e.kind.label(),
                e.name,
                e.gguf_query()
            );
        }
    }
}
