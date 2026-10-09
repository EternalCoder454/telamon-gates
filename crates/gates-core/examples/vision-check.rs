//! A picture to a real model that reads images, outside the app: the model
//! folder's model starts with its projector, and the picture goes in the
//! message as Gates sends it.
//!
//!   cargo run --example vision-check -- <llama-server> <models dir> <picture>

use gates_core::backend::{Backend, Event, Llama, Options, Request};
use gates_core::conversation::Message;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

fn main() {
    let mut args = std::env::args().skip(1);
    let binary = PathBuf::from(args.next().expect("llama-server"));
    let models = PathBuf::from(args.next().expect("models dir"));
    let picture = PathBuf::from(args.next().expect("picture"));
    let store = gates_core::store::attachments_dir();
    let attachment = gates_core::attach::read(&picture, &store).expect("a picture");
    let llama = Llama::new(
        models,
        Some(binary),
        std::env::temp_dir().join("vision-check.log"),
        Options::default(),
    );
    let request = Request {
        model: String::new(),
        system_prompt: String::new(),
        messages: vec![Message {
            attachments: vec![attachment],
            ..Message::user("What colours are in this picture? Answer in one sentence.")
        }],
        sampling: None,
        tools: Vec::new(),
        response_format: None,
        brief: false,
    };
    let result = llama.complete(&request, &AtomicBool::new(false), &mut |e| {
        if let Event::Text(t) = e {
            print!("{t}");
        }
    });
    println!("\nresult: {result:?}");
}
