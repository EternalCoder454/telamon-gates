//! SystemOne's test set: messages whose mode is known, asked of a real
//! decision model, with its accuracy, confidence and speed. Run it before
//! changing the question, the threshold or the model:
//!
//!   cargo run --example systemone-check -- <decision-model.gguf> [llama-server]
//!
//! Prints each wrong pick, then the accuracy with and without the
//! confidence threshold (below it, Gates answers in Chat).

use gates_core::backend::llama::{find_server, local_models};
use gates_core::systemone::{MIN_CONFIDENCE, SystemOne};
use std::path::PathBuf;
use std::time::Instant;

const SET: &[(&str, &str)] = &[
    // Chat
    (
        "chat",
        "What's the difference between a virus and a bacterium?",
    ),
    (
        "chat",
        "How do I get better sleep when I work night shifts?",
    ),
    ("chat", "Can you explain how compound interest works?"),
    (
        "chat",
        "What should I pack for a week of hiking in Scotland in May?",
    ),
    (
        "chat",
        "Is it worth replacing my 2016 laptop battery or buying a new one?",
    ),
    (
        "chat",
        "Summarise the causes of the First World War in a few points.",
    ),
    ("chat", "Hi! How are you today?"),
    (
        "chat",
        "Translate 'where is the train station' into Portuguese.",
    ),
    (
        "chat",
        "My friend is upset with me for missing her birthday. What should I say?",
    ),
    ("chat", "Which is healthier, brown rice or quinoa?"),
    ("chat", "What does a product manager actually do all day?"),
    ("chat", "Give me a recipe for a quick vegetarian dinner."),
    // Story
    (
        "story",
        "Write a short story about a lighthouse keeper who finds a message in a bottle.",
    ),
    (
        "story",
        "Continue the scene: Mara pushed open the door and the smell of smoke hit her.",
    ),
    ("story", "Write a poem about autumn rain on a city street."),
    (
        "story",
        "Let's roleplay. You are a grumpy dwarf blacksmith and I walk into your shop.",
    ),
    (
        "story",
        "Describe the villain of my fantasy novel: a queen who stole the moon.",
    ),
    (
        "story",
        "Write the opening paragraph of a noir detective novel set in 2080 Tokyo.",
    ),
    (
        "story",
        "Give me three plot twists for a murder mystery on a cruise ship.",
    ),
    ("story", "Write song lyrics about leaving your hometown."),
    (
        "story",
        "Rewrite this dialogue so it sounds more tense: 'Hi.' 'Hello.' 'You came.'",
    ),
    (
        "story",
        "Tell me a bedtime story about a dragon who is afraid of the dark.",
    ),
    ("story", "Write a haiku about the first snow."),
    (
        "story",
        "Create a backstory for my D&D character, an elven rogue named Sylas.",
    ),
    // Code
    (
        "code",
        "Write a Python function that checks whether a string is a palindrome.",
    ),
    (
        "code",
        "Why does this Rust code say 'borrowed value does not live long enough'?",
    ),
    ("code", "How do I reverse a linked list in C?"),
    (
        "code",
        "Fix this SQL: SELECT name, COUNT(*) FROM users GROUP BY;",
    ),
    ("code", "What does `git rebase -i HEAD~3` do?"),
    (
        "code",
        "Write a bash script that renames every .jpeg file in a folder to .jpg.",
    ),
    (
        "code",
        "Explain what this regex matches: ^[a-z0-9._%+-]+@[a-z0-9.-]+\\.[a-z]{2,}$",
    ),
    (
        "code",
        "Convert this JavaScript callback code to async/await.",
    ),
    ("code", "How do I center a div with CSS grid?"),
    (
        "code",
        "Review my QML: Rectangle { width: parent.width; anchors.fill: parent }",
    ),
    (
        "code",
        "My Docker container exits immediately with code 137. Why?",
    ),
    (
        "code",
        "Write a unit test for a function that adds two numbers in Go.",
    ),
];

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(model) = args.next().map(PathBuf::from) else {
        eprintln!("usage: systemone-check <decision-model.gguf> [llama-server]");
        std::process::exit(2);
    };
    let binary = args
        .next()
        .map(PathBuf::from)
        .or_else(find_server)
        .expect("no llama-server");
    let dir = model.parent().expect("a file").to_path_buf();
    let name = model.file_stem().unwrap().to_string_lossy().to_string();
    let found = local_models(&dir)
        .into_iter()
        .find(|m| m.name == name)
        .expect("the model is in its folder");
    assert!(!found.info.decision.is_empty(), "not a decision model");
    let log = std::env::temp_dir().join("systemone-check.log");
    let one = SystemOne::new(&binary, &found, log.clone(), None);
    println!("{} ({}), log in {}", one.name(), one.kind(), log.display());

    let started = Instant::now();
    let _ = one.pick_mode("Hello");
    println!("first answer (load): {} ms", started.elapsed().as_millis());

    let mut right = 0;
    // What Gates does: below the threshold it answers in Chat.
    let mut used_right = 0;
    let mut confident = 0;
    let mut confident_right = 0;
    let mut times = Vec::new();
    let mut by_mode: Vec<(&str, usize, usize)> =
        vec![("chat", 0, 0), ("story", 0, 0), ("code", 0, 0)];
    for (want, text) in SET {
        let t = Instant::now();
        let pick = one.pick_mode(text).expect("an answer");
        times.push(t.elapsed().as_secs_f64() * 1000.0);
        let ok = pick.choice == *want;
        let row = by_mode.iter_mut().find(|r| r.0 == *want).unwrap();
        row.2 += 1;
        if ok {
            right += 1;
            row.1 += 1;
        }
        let used = if pick.confidence >= MIN_CONFIDENCE {
            pick.choice.as_str()
        } else {
            "chat"
        };
        if used == *want {
            used_right += 1;
        }
        if pick.confidence >= MIN_CONFIDENCE {
            confident += 1;
            if ok {
                confident_right += 1;
            }
        }
        println!(
            "{} want {:5} got {:5} conf {:.2}  {}",
            if ok { "  " } else { "✗ " },
            want,
            pick.choice,
            pick.confidence,
            text.chars().take(60).collect::<String>()
        );
    }
    times.sort_by(f64::total_cmp);
    let n = SET.len();
    println!(
        "\naccuracy: {right}/{n} ({:.0}%)",
        100.0 * right as f64 / n as f64
    );
    for (mode, ok, all) in &by_mode {
        println!("  {mode}: {ok}/{all}");
    }
    println!(
        "as used (Chat below {MIN_CONFIDENCE}): {used_right}/{n} ({:.0}%)",
        100.0 * used_right as f64 / n as f64
    );
    println!(
        "confident (>= {MIN_CONFIDENCE}): {confident}/{n}, of which right {confident_right} ({:.0}%)",
        if confident > 0 {
            100.0 * confident_right as f64 / confident as f64
        } else {
            0.0
        }
    );
    println!(
        "latency: median {:.0} ms, max {:.0} ms",
        times[times.len() / 2],
        times[times.len() - 1]
    );
}
