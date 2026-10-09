#!/bin/bash
# The real llama backend against a real, tiny model on the processor, inside
# the dev container (or CI) with the telamon-llama RPM installed:
#   scripts/dev.sh scripts/real-model.sh [llama-server] [scenario ...]
# (no --device /dev/dri: there is no graphics card, and none is needed)
# Fetches the model (pinned by commit and sha256, kept in out/real-model),
# then runs crates/gates-core/examples/real-model-check.rs: streaming, Stop,
# trimming, and the agent's tool round trip. The server logs are kept in
# out/real-model/logs. Never the user's files or display.
set -euo pipefail

repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# SmolLM2-135M-Instruct, Q4_K_M, by bartowski (Apache-2.0): 105 MB, a chat
# template, and 8192 tokens of context. The URL names a commit, not a branch.
MODEL_FILE=SmolLM2-135M-Instruct-Q4_K_M.gguf
MODEL_URL=https://huggingface.co/bartowski/SmolLM2-135M-Instruct-GGUF/resolve/09816acd5d99df7be770d85ea30822623dab342c/$MODEL_FILE
MODEL_SHA256=2e8040ceae7815abe0dcb3540b9995eaa1fa0d2ca9e797d0a635ae4433c68c2d

out=$repo/out/real-model
mkdir -p "$out/models" "$out/logs"
rm -f "$out"/logs/*

server=
if [[ ${1:-} == /* ]]; then
    server=$1
    shift
fi
if [ -z "$server" ]; then
    if [ -x /usr/libexec/telamon-llama/llama-server ]; then
        server=/usr/libexec/telamon-llama/llama-server
    else
        server=$(command -v llama-server) || {
            echo "real-model: no llama-server: install the telamon-llama RPM" >&2
            exit 1
        }
    fi
fi

model=$out/models/$MODEL_FILE
if ! { [ -f "$model" ] && echo "$MODEL_SHA256  $model" | sha256sum -c --quiet; }; then
    echo "real-model: fetching $MODEL_FILE"
    curl -fsSL --proto '=https' --tlsv1.2 --retry 3 --max-time 600 -o "$model.part" "$MODEL_URL"
    echo "$MODEL_SHA256  $model.part" | sha256sum -c --quiet || {
        echo "real-model: $MODEL_FILE is not the file pinned in this script" >&2
        rm -f "$model.part"
        exit 1
    }
    mv "$model.part" "$model"
fi

echo "--- $("$server" --version 2>&1 | head -1)"
echo "--- devices the server finds ($(nproc) processor threads)"
"$server" --list-devices 2>&1 | grep -v '^\[Vulkan Loader\]' || true

export CHECK_LOG_DIR=$out/logs
# Everything the examples make goes in the build's own folder.
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$repo/target/dev}
cd "$repo"
status=0
cargo run -q -p gates-core --locked --example real-model-check -- "$server" "$out/models" "$@" || status=$?
echo "--- server logs (last lines)"
for f in "$out"/logs/*.log; do
    echo "$(basename "$f"): $(tail -n 1 "$f" | cut -c1-160)"
done
exit "$status"
