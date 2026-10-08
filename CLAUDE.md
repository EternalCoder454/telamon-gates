# Telamon Gates

Rust + Qt 6.11 + Kirigami (CXX-Qt) AI chat for Telamon OS (formerly AtlasOS;
local folders still say AtlasOS). Front end only: the model is reached
through the `Backend` trait (`docs/BACKEND.md`); a demo backend answers until
llama is connected. Read `docs/DESIGN.md` first and change it with the code.

**Phase: Functionable** (F.S.R.P, see `~/.claude/CLAUDE.md`).

The stack, build and look are Telamon Monitor's
(`~/Documents/Projects/AtlasOS/AtlasOS Monitor`, github.com/EternalCoder454/atlasos-monitor).
When in doubt, do what it does.

## Hard rules

- **Build and test inside the fedora:44 dev container**, never on the host:
  `scripts/dev.sh <command>`. The repo is at `/src`.
  Set `TMPDIR` to a folder on disk before it (`podman commit` fills the
  sandbox's small tmpfs otherwise).
- **Never run the GUI on the user's display.** `scripts/smoke.sh` runs it
  under Xvfb and a private bus with every XDG dir under `out/smoke`.
- **Telamon.Ui is the installed `telamon-ui` package** (the Telamon framework,
  github.com/EternalCoder454/atlas-framework). Never copy its components here;
  ask the framework's owner for new ones.
- **The GUI thread never blocks**: files go through `Io`, replies stream on
  workers, results come back with `qt_thread().queue`.
- **Replies are untrusted.** They reach QML only through `markdown.rs`. Every
  other `Text` that shows them is `PlainText`.
- Commits are authored as
  `EternalHell <77252745+EternalCoder454@users.noreply.github.com>`.
- Licence: MIT. App ID `net.eterneon.telamon.gates`. Wording follows KDE:
  Title Case buttons and titles, US spelling.

## Commands

| Task | Command (from the repo root on the host) |
|---|---|
| First run (builds the dev image) | `ATLAS_LOCAL_RPMS=<dir with telamon-ui and telamon-symbols-fonts 2.0.2 RPMs> scripts/dev.sh true` |
| Format | `scripts/dev.sh cargo fmt --all --check` |
| Lint | `scripts/dev.sh env QMAKE=/usr/bin/qmake6 cargo clippy --workspace --all-targets --locked -- -D warnings` |
| Tests | `scripts/dev.sh env QMAKE=/usr/bin/qmake6 cargo test --workspace --locked` |
| App build | `scripts/dev.sh bash -c 'cmake -S apps/telamon-gates -B build/dev -G Ninja && cmake --build build/dev'` |
| qmllint | `scripts/dev.sh cmake --build build/dev --target all_qmllint` |
| Smoke run + screenshots | `scripts/dev.sh scripts/smoke.sh` (`SMOKE_DARK=1` for a dark scheme) |
| Telamon checks | `<framework checkout>/tools/lint-app.sh apps/telamon-gates` and `tools/check-app-names.sh apps/telamon-gates` |

The framework RPMs come from the framework checkout's
`packaging/build-rpm.sh <out>` run in `registry.fedoraproject.org/fedora:44`
at the tag `Cargo.toml` pins (v2.0.2).
