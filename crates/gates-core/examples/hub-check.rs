//! Asks Hugging Face's live API what the Models page would show:
//!   cargo run -p gates-core --example hub-check -- <search> [repo]

fn main() {
    let mut args = std::env::args().skip(1);
    let query = args
        .next()
        .unwrap_or_else(|| "SmolLM2-135M-Instruct".into());
    match gates_core::hub::search(&query) {
        Ok(repos) => {
            for r in repos.iter().take(5) {
                println!("{:>10}  {}", r.downloads, r.id);
            }
            let repo = args.next().or_else(|| repos.first().map(|r| r.id.clone()));
            if let Some(repo) = repo {
                match gates_core::hub::files(&repo) {
                    Ok(files) => {
                        println!("{repo}:");
                        for f in files.iter().take(8) {
                            println!("  {:>12}  {}  {}…", f.size, f.name, &f.sha256[..12]);
                        }
                    }
                    Err(e) => println!("files: {e}"),
                }
            }
        }
        Err(e) => println!("search: {e}"),
    }
}
