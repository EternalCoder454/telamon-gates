//! The web search key's keyring (`secret-tool`) against the real Secret
//! Service, outside the app: checks it, stores a made-up key under a name of
//! its own, reads it, replaces it, forgets it, and says what each call
//! answered. Needs a session bus with a keyring that is unlocked (or that
//! asks to be).
//!
//!   cargo run --example keyring-check

use gates_core::web::{KeyStore, SecretService};

fn main() {
    let store = SecretService::default();
    let name = format!("keyring-check-{}", std::process::id());
    println!("check:   {:?}", store.check());
    println!("get:     {:?} (nothing yet)", store.get(&name));
    println!("set:     {:?}", store.set(&name, "test-key-not-real"));
    println!("get:     {:?}", store.get(&name));
    println!("replace: {:?}", store.set(&name, "second test key\nwith a line"));
    println!("get:     {:?}", store.get(&name));
    println!("remove:  {:?}", store.remove(&name));
    println!("remove:  {:?} (none left: fine)", store.remove(&name));
    println!("get:     {:?}", store.get(&name));
}
