//! Does Stop free the model server? A long prompt (seconds of reading on
//! the processor) is stopped after half a second; then a short message is
//! timed. With the connection shut at Stop, the server drops the long one
//! and the short one answers at once.
//!
//!   cargo run --release --example stop-check -- <llama-server> <models dir>

use gates_core::backend::{Backend, Llama, Options, Request};
use gates_core::conversation::Message;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

fn main() {
    let mut args = std::env::args().skip(1);
    let binary = PathBuf::from(args.next().expect("llama-server"));
    let models = PathBuf::from(args.next().expect("models dir"));
    let llama = Arc::new(Llama::new(
        models,
        Some(binary),
        std::env::temp_dir().join("stop-check.log"),
        Options::default(),
    ));
    let ask = |text: String| Request {
        model: String::new(),
        system_prompt: String::new(),
        messages: vec![Message::user(text)],
        sampling: None,
        tools: Vec::new(),
        response_format: None,
    };
    // Warm up: the model loads.
    let _ = llama.complete(&ask("Hi".into()), &AtomicBool::new(false), &mut |_| {});

    let long = "The quick brown fox jumps over the lazy dog. ".repeat(600) + "Summarise that.";
    let cancel = Arc::new(AtomicBool::new(false));
    let (l, c) = (llama.clone(), cancel.clone());
    let started = Instant::now();
    let worker = std::thread::spawn(move || {
        let r = l.complete(&ask(long), &c, &mut |_| {});
        (r, started.elapsed())
    });
    std::thread::sleep(Duration::from_millis(500));
    cancel.store(true, Ordering::Relaxed);
    let (result, took) = worker.join().unwrap();
    println!(
        "long prompt stopped: {result:?} after {} ms",
        took.as_millis()
    );

    let t = Instant::now();
    let mut first = None;
    let _ = llama.complete(&ask("Say OK.".into()), &AtomicBool::new(false), &mut |_| {
        first.get_or_insert(t.elapsed());
    });
    println!(
        "next message: first words after {} ms",
        first.map_or(0, |d| d.as_millis())
    );
}
