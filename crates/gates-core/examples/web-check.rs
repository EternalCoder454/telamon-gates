//! The web tools against the real internet, outside the app (no model):
//!
//!   cargo run --example web-check -- fetch <https address>
//!   cargo run --example web-check -- search searxng <instance address> <query>
//!   BRAVE_API_KEY=… cargo run --example web-check -- search brave <query>
//!   TAVILY_API_KEY=… cargo run --example web-check -- search tavily <query>
//!
//! The key is read from the environment, never from an argument.

use gates_core::web::{Fetcher, Live, Provider, Web};
use std::sync::atomic::AtomicBool;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let stop = AtomicBool::new(false);
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["fetch", url] => match Fetcher::new().get(url, &stop) {
            Ok(page) => {
                println!("{} ({})", page.url, page.title);
                println!(
                    "{} bytes of text{}\n",
                    page.text.len(),
                    if page.truncated { ", cut" } else { "" }
                );
                println!("{}", page.text);
            }
            Err(e) => {
                eprintln!("refused or failed: {e}");
                std::process::exit(1);
            }
        },
        ["search", "searxng", base, query @ ..] => {
            search(Provider::Searxng, "", base, &query.join(" "))
        }
        ["search", name @ ("brave" | "tavily"), query @ ..] => {
            let provider = Provider::from_id(name);
            let var = if provider == Provider::Brave {
                "BRAVE_API_KEY"
            } else {
                "TAVILY_API_KEY"
            };
            let key = std::env::var(var).unwrap_or_default();
            search(provider, &key, "", &query.join(" "));
        }
        _ => {
            eprintln!(
                "usage: web-check fetch <url> | search searxng <instance> <query> | search brave|tavily <query>"
            );
            std::process::exit(2);
        }
    }
}

fn search(provider: Provider, key: &str, base: &str, query: &str) {
    let live = match Live::new(provider, key, base) {
        Ok(live) => live,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    match live.search(query, 5, &AtomicBool::new(false)) {
        Ok(found) => {
            println!("{} answered with {} results", provider.name(), found.len());
            for r in found {
                println!("- {}\n  {}\n  {}", r.title, r.url, r.snippet);
            }
        }
        Err(e) => {
            eprintln!("failed: {e}");
            std::process::exit(1);
        }
    }
}
