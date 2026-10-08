# Telamon Gates

The AI chat of Telamon OS: talk to a local model, with your conversations kept
as plain JSON files on your computer. Rust, Qt 6 and Kirigami (CXX-Qt), on the
Telamon framework's Telamon.Ui.

This is the front end. Replies come through one Rust trait, so connecting a
model (llama, or anything else) changes no UI code: see
[docs/BACKEND.md](docs/BACKEND.md). Until one is connected, a demo backend
answers with sample replies.

- Conversations in a sidebar, grouped by day, searchable, deletable
- Replies stream in as Markdown, with code blocks you can copy
- Stop, regenerate, pick a model, set a system prompt
- Light and dark, following the Plasma theme and Telamon's look

Design: [docs/DESIGN.md](docs/DESIGN.md). Building: [CLAUDE.md](CLAUDE.md).

## Licence

MIT.
