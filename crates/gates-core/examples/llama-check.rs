//! Talks to a real llama-server through the Llama backend, outside the app:
//! a quick check of a telamon-llama build, a model and a graphics card.
//!
//!   cargo run -p gates-core --example llama-check -- <llama-server> <models dir> [context]
//!
//! Sends a long conversation (to see trimming at a small context), prints
//! the reply as it streams, its speed and the context the server ran with.

use gates_core::backend::{Event, Llama};
use gates_core::{Backend, Message, Options, Request};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(binary), Some(models)) = (args.next(), args.next()) else {
        eprintln!("usage: llama-check <llama-server> <models dir> [context]");
        std::process::exit(2);
    };
    let context = args.next().and_then(|c| c.parse().ok()).unwrap_or(0);
    let llama = Llama::new(
        PathBuf::from(models),
        Some(PathBuf::from(binary)),
        std::env::temp_dir().join("llama-check.log"),
        Options {
            context,
            ..Options::default()
        },
    );
    let models = llama.models().unwrap_or_default();
    println!("models: {models:?}");
    let mut messages = Vec::new();
    for i in 0..40 {
        messages.push(Message::user(format!(
            "Message {i}: tell me about the number {i} in a few sentences."
        )));
        messages.push(Message::assistant(format!(
            "The number {i} is a whole number. It comes after {} and before {}. People use it every day.",
            i.max(1) - 1,
            i + 1
        )));
    }
    messages.push(Message::user("Now say hello in five words."));
    let request = Request {
        model: models.first().cloned().unwrap_or_default(),
        system_prompt: "Be brief.".into(),
        messages,
        sampling: None,
        tools: Vec::new(),
        response_format: None,
        brief: false,
    };
    let started = std::time::Instant::now();
    let result = llama.complete(&request, &AtomicBool::new(false), &mut |e| match e {
        Event::Text(t) => {
            print!("{t}");
            let _ = std::io::stdout().flush();
        }
        Event::Speed(s) => println!("\n[speed: {s:.1} tokens/s]"),
        Event::ToolCalls(calls) => println!("\n[tool calls: {calls:?}]"),
    });
    println!(
        "\n[result: {result:?}; context: {:?} tokens; {:.1} s]",
        llama.context_size(),
        started.elapsed().as_secs_f64()
    );
}
