//! A Fleet against a real model, outside the app: a small project in a
//! temporary folder, a goal, the coordinator's plan, each agent's steps as
//! they happen and, with a decision model in the models folder, SystemOne's
//! belief that each agent finished. Edits are allowed; commands too when
//! `--allow-run` is given.
//!
//!   cargo run --release --example fleet-check -- <llama-server> <models dir> [--allow-run]
//!
//! The models folder holds a chat model (one that takes tools, such as
//! Qwen3) and, optionally, a decision model (Laya or Kev).

use gates_core::agent::Approval;
use gates_core::backend::llama::local_models;
use gates_core::backend::{Llama, Options};
use gates_core::conversation::ToolCall;
use gates_core::fleet::{self, Control, FleetHost, Judge, Status, Task};
use gates_core::systemone::SystemOne;
use gates_core::tools::Workspace;
use std::path::PathBuf;
use std::time::Instant;

struct Print {
    allow_run: bool,
    started: Instant,
}

impl FleetHost for Print {
    fn planned(&mut self, tasks: &[Task]) {
        println!("plan ({:.1} s):", self.started.elapsed().as_secs_f64());
        for (i, t) in tasks.iter().enumerate() {
            println!("  {}. {}: {}", i + 1, t.title, t.instructions);
        }
    }
    fn status(&mut self, agent: usize, status: Status) {
        println!(
            "[{:5.1} s] agent {} is {}",
            self.started.elapsed().as_secs_f64(),
            agent + 1,
            status.as_str()
        );
    }
    fn step(&mut self, agent: usize, steps: usize, line: &str) {
        println!("          agent {} step {steps}: {line}", agent + 1);
    }
    fn speed(&mut self, _: usize, _: f64) {}
    fn line(&mut self, agent: usize, line: &str) {
        println!("          agent {} says: {line}", agent + 1);
    }
    fn approve(&mut self, agent: usize, call: &ToolCall, title: &str, _: &str) -> Approval {
        let allow = call.name != "run_command" || self.allow_run;
        println!(
            "          agent {} asks: {title}: {}",
            agent + 1,
            if allow { "allowed" } else { "declined" }
        );
        if allow {
            Approval::Allow
        } else {
            Approval::Deny
        }
    }
    fn judged(&mut self, agent: usize, confidence: f64) {
        println!(
            "          SystemOne: agent {} finished, {:.0}% sure",
            agent + 1,
            confidence * 100.0
        );
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let binary = PathBuf::from(args.next().expect("llama-server"));
    let models = PathBuf::from(args.next().expect("models dir"));
    let allow_run = args.any(|a| a == "--allow-run");

    let dir = std::env::temp_dir().join(format!("fleet-check-{}", std::process::id()));
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
        models.clone(),
        Some(binary.clone()),
        std::env::temp_dir().join("fleet-check-server.log"),
        Options::default(),
    );
    let system_one = local_models(&models)
        .into_iter()
        .find(|m| !m.info.decision.is_empty())
        .map(|m| {
            println!("SystemOne judges with {}", m.name);
            SystemOne::new(
                &binary,
                &m,
                std::env::temp_dir().join("fleet-check-systemone.log"),
                None,
            )
        });
    if system_one.is_none() {
        println!("no decision model: nobody judges the agents");
    }

    let goal = "In this project, change the greeting from \"Hello\" to \"Good morning\", \
                and add a line to README.md saying how to run it (python3 src/greet.py). \
                Split the work in two: the code, then the README.";
    println!("goal: {goal}\n");
    let mut host = Print {
        allow_run,
        started: Instant::now(),
    };
    let control = Control::new();
    let judge = system_one.as_ref().map(|s| s as &dyn Judge);
    let done = fleet::run(&llama, judge, "", goal, &ws, &control, &mut host);

    println!(
        "\nerror: {:?}\ntotal {:.1} s",
        done.error,
        host.started.elapsed().as_secs_f64()
    );
    for (i, r) in done.reports.iter().enumerate() {
        println!(
            "agent {}: {}, {} steps, confidence {:?}",
            i + 1,
            r.status.as_str(),
            r.steps,
            r.confidence
        );
    }
    let code = std::fs::read_to_string(dir.join("src/greet.py")).unwrap();
    let readme = std::fs::read_to_string(dir.join("README.md")).unwrap();
    println!("greeting changed: {}", code.contains("Good morning"));
    println!(
        "readme has the run line: {}",
        readme.contains("python3 src/greet.py")
    );
    let _ = std::fs::remove_dir_all(&dir);
}
