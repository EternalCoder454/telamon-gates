# Telamon Gates

Rust + Qt 6.11 + Kirigami (CXX-Qt) AI chat for Telamon OS (formerly AtlasOS;
local folders still say AtlasOS). Front end only: the model is reached
through the `Backend` trait (`docs/BACKEND.md`); a demo backend answers until
llama is connected. Read `docs/DESIGN.md` first and change it with the code.

**Phase: Performant** (F.S.R.P, see `~/.claude/CLAUDE.md`): Functionable, Secure and
Reliable are done as of 1.0.0; measurements are in `docs/BACKEND.md` → Performance.

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
- **Check for running AI models and stop them before any GPU or model work.**
  This covers llama-server, a model eval, a smoke run with a model, and
  anything with `--device /dev/dri`. First run
  `pgrep -af 'llama-server|ollama|whisper|vllm|koboldcpp|text-generation'`
  and `podman ps`, and read `/sys/class/drm/card*/device/mem_info_vram_used`.
  Stop every model server found, including one Telamon Gates started, and any
  of our containers still running.
- **One heavy job at a time.** That means one container build, test suite or
  GPU job at once, never several in parallel. This PC is also the user's
  desktop: six parallel builds plus models crashed it. Cap builds with
  `CARGO_BUILD_JOBS=8`, and podman with `--memory=12g --cpus=12`. Stop
  whatever model you started when done.
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
| First run (builds the dev image) | `ATLAS_LOCAL_RPMS=<dir with telamon-ui and telamon-symbols-fonts 2.1.1 RPMs> scripts/dev.sh true` |
| Format | `scripts/dev.sh cargo fmt --all --check` |
| Lint | `scripts/dev.sh env QMAKE=/usr/bin/qmake6 cargo clippy --workspace --all-targets --locked -- -D warnings` |
| Tests | `scripts/dev.sh env QMAKE=/usr/bin/qmake6 cargo test --workspace --locked` |
| App build | `scripts/dev.sh bash -c 'cmake -S apps/telamon-gates -B build/dev -G Ninja && cmake --build build/dev'` |
| qmllint | `scripts/dev.sh cmake --build build/dev --target all_qmllint` |
| Smoke run + screenshots | `scripts/dev.sh scripts/smoke.sh` (`SMOKE_DARK=1` for a dark scheme) |
| Every page and state, light and dark | `scripts/dev.sh scripts/screens.sh` → `out/screens/{light,dark}/NN-<state>.png`; review them zoomed in, not whole |
| RPM | `podman run --rm --security-opt label=disable -v "$PWD":/src:ro -v <framework rpms>:/fw:ro -v <out>:/out -e ATLAS_LOCAL_RPMS=/fw registry.fedoraproject.org/fedora:44 /src/packaging/build-rpm.sh /out` |
| telamon-llama RPM | `podman run --rm --security-opt label=disable -v "$PWD/packaging/telamon-llama":/pkg:ro -v <out>:/out registry.fedoraproject.org/fedora:44 /pkg/build-rpm.sh /out` (about 3 min) |
| Smoke run with a real model | install the telamon-llama RPM in the dev container, then `SMOKE_MODEL=<file.gguf> scripts/smoke.sh` (also `SCREENS_MODEL` for screens.sh) |
| Real model on the CPU | install the telamon-llama RPM in the dev container (`dnf -y install <rpm>`), then `scripts/dev.sh scripts/real-model.sh [stream\|stop\|trim\|agent]`. No `--device /dev/dri`. Fetches SmolLM2-135M (105 MB, sha256-pinned) into `out/real-model`; about 30 s |
| Bundle install test | build the bundle with the framework's `tools/make-bundle.sh --app-dir apps/telamon-gates --spec packaging/telamon-gates.spec --out out/bundle --build-dir build/bundle` (in the dev container, `SOURCE_DATE_EPOCH` set: the worktree's `.git` isn't mounted), then `scripts/dev.sh scripts/bundle-install.sh out/bundle` |
| Coding eval (GPU, not CI) | `tools/eval/README.md` |
| Telamon checks | `<framework checkout>/tools/lint-app.sh apps/telamon-gates` and `tools/check-app-names.sh apps/telamon-gates` |

The framework RPMs come from the framework checkout's
`packaging/build-rpm.sh <out>` run in `registry.fedoraproject.org/fedora:44`
at the tag `Cargo.toml` pins (v2.1.1).

## CI

`.github/workflows/ci.yml`: the framework's app checks, the framework RPMs
(built once per release and Fedora Qt, cached), fmt · clippy · tests,
build · qmllint · smoke (screenshots kept as an artifact), the RPM,
`real model · CPU` (the telamon-llama RPM, from `llama.yml`, and a tiny
GGUF: `scripts/real-model.sh`) and `bundle · install · launch` (the bundle
built with the framework's tools at the pinned release, installed as Telamon
Store does and started under Xvfb: `scripts/bundle-install.sh`). Jobs
run in the public dev image `ghcr.io/eternalcoder454/telamon-gates-dev`
(`ci/Containerfile`), which a main-only job builds weekly or when the
Containerfile or spec changes, checks with `ci/check-image-secrets.sh`
before logging in, and pushes; without it they fall back to fedora:44.
`.github/workflows/bundle.yml` builds the Telamon bundle for a `v*` tag
and signs `telamon-bundle.json` with the repository secrets `MINISIGN_KEY`
and `MINISIGN_PASSWORD`. The run fails without them; the release is then
signed by hand with `minisign -Sm telamon-bundle.json` and the `.minisig` is
uploaded. The public key, which the Store's catalog pins, is in the workflow.
The secret key is the owner's alone: never create, read or store it.
Actions are pinned by commit. Moving the framework tag means changing
`Cargo.toml` and the `telamon` job (`app-checks.yml@<tag commit> # <tag>`
and `framework-ref`) together: the framework job fails when they disagree.
