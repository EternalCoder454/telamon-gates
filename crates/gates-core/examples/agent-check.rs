//! Agent mode against a real model, outside the app: a small project in a
//! temporary folder, a task, and the agent's steps as they happen. Edits
//! are allowed; commands too when `--allow-run` is given.
//!
//!   cargo run --release --example agent-check -- <llama-server> <models dir> [--allow-run]
//!
//! Prints each step, the time it took, and whether the task got done.

use gates_core::agent::{self, Approval, Host};
use gates_core::backend::{Llama, Options, Request};
use gates_core::conversation::{Message, ToolCall};
use gates_core::modes;
use gates_core::tools::Workspace;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

struct Print {
    allow_run: bool,
    started: Instant,
    steps: usize,
}

impl Host for Print {
    fn text(&mut self, piece: &str) {
        print!("{piece}");
    }
    fn speed(&mut self, s: f64) {
        println!("\n  [{s:.1} tokens/s]");
    }
    fn calls(&mut self, calls: &[ToolCall]) {
        self.steps += 1;
        for c in calls {
            println!(
                "\n→ {} ({:.1} s)",
                gates_core::tools::label(&c.name, &c.arguments),
                self.started.elapsed().as_secs_f64()
            );
        }
    }
    fn approve(&mut self, call: &ToolCall, title: &str, detail: &str) -> Approval {
        let allow = call.name != "run_command" || self.allow_run;
        println!("  ? {title}: {}", detail.lines().next().unwrap_or(""));
        println!("  {}", if allow { "allowed" } else { "declined" });
        if allow {
            Approval::Allow
        } else {
            Approval::Deny
        }
    }
    fn result(&mut self, m: Message) {
        println!(
            "  {} {}",
            if m.failed { "✗" } else { "✓" },
            m.summary.unwrap_or_default()
        );
    }
    fn next_turn(&mut self) {}
}

fn main() {
    let mut args = std::env::args().skip(1);
    let binary = PathBuf::from(args.next().expect("llama-server"));
    let models = PathBuf::from(args.next().expect("models dir"));
    let allow_run = args.any(|a| a == "--allow-run");

    let dir = std::env::temp_dir().join(format!("agent-check-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/greet.py"),
        "def greet(name):\n    return \"Hello, \" + name\n\n\nprint(greet(\"world\"))\n",
    )
    .unwrap();
    std::fs::write(dir.join("README.md"), "# Greeter\nPrints a greeting.\n").unwrap();
    let ws = Workspace::open(&dir).unwrap();

    let llama = Llama::new(
        models,
        Some(binary),
        std::env::temp_dir().join("agent-check-server.log"),
        Options::default(),
    );
    let task = "In this project, change the greeting from \"Hello\" to \"Good morning\", \
                and add a line to README.md saying how to run it (python3 src/greet.py). Then run it to check it prints Good morning.";
    let mode = modes::AGENT;
    let request = Request {
        model: String::new(),
        system_prompt: format!(
            "{}\n\nThe workspace is {}.",
            modes::system_prompt(&mode, ""),
            ws.root().display()
        ),
        messages: vec![Message::user(task)],
        sampling: mode.sampling,
        tools: Vec::new(),
        response_format: None,
    };
    let mut host = Print {
        allow_run,
        started: Instant::now(),
        steps: 0,
    };
    let result = agent::run(&llama, request, &ws, &AtomicBool::new(false), &mut host);
    println!("\n\nresult: {result:?}");
    let code = std::fs::read_to_string(dir.join("src/greet.py")).unwrap();
    let readme = std::fs::read_to_string(dir.join("README.md")).unwrap();
    println!(
        "steps: {}, total {:.1} s",
        host.steps,
        host.started.elapsed().as_secs_f64()
    );
    println!("greeting changed: {}", code.contains("Good morning"));
    println!(
        "readme has the run line: {}",
        readme.contains("python3 src/greet.py")
    );
    let _ = std::fs::remove_dir_all(&dir);
}
