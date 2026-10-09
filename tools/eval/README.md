# Model evals

The tests behind `docs/BACKEND.md` → Recommended models and Performance.
They run llama-server on a model and print numbers; **they need a graphics
card to be worth running and are not part of CI** (CI has a CPU-only check of
the backend with a tiny model: `scripts/real-model.sh`).

| Script | What it measures |
|---|---|
| `codeeval.py <name> <llama-server args>` | The code test: 8 tasks (palindromes, merging intervals, Roman numerals, top-k words, durations, brackets, RPN, version sorting), each in Rust, Go and Python, 24 answers. Each answer is compiled (`rustc`, `go`) and run against fixed tests. Prints passes per language, the time for the 24, the median tokens/s and the tokens written. Failed answers are kept in `$EVAL_OUT/fail/<name>/`. |
| `bench.py <name> <llama-server args>` | Speed: three prompts (chat, rewriting a file, a story) at temperature 0. Prints generated tokens/s, how much of the draft was accepted, and the wall time. Replies are kept in `$EVAL_OUT/samples/`. |
| `brief.py <name> <llama-server args>` | Chat and story with the model's own reasoning against *brief* (`Request::brief`): tokens, time, words of reasoning. |
| `suite.sh` | `codeeval.py` and `bench.py` for each `name\|llama-server args` line on stdin. |
| `repeat.sh` | `codeeval.py` three times for each line (it varies between runs), and `bench.py` once when `BENCH=1`. |

`<name>` only labels the output and the log; **the model is whatever the
arguments say**: `-m <file.gguf>` is required, and nothing is read from the
repository but `rewrite-sample.rs` (the piece of a Rust file `bench.py` asks
to be rewritten). Every other argument goes to llama-server as it is.

## Environment

| Variable | Default | Meaning |
|---|---|---|
| `THINK` | off | `THINK=1`: the model reasons first in the code test (Qwen3 thinking on, gpt-oss at medium effort, up to 8,000 tokens a reply). Off, a reply is brief, as Gates asks for in Chat and Story. |
| `BENCH` | off | `repeat.sh` only: `BENCH=1` also runs `bench.py` after the code test. |
| `LLAMA_SERVER` | `/usr/libexec/telamon-llama/llama-server`, else `llama-server` on `$PATH` | The server binary. |
| `EVAL_OUT` | `out/eval` | Where logs, failed answers and samples go (relative to where you run it). |
| `EVAL_CTX` | `16384` | Context in tokens (`-c`). |
| `EVAL_MAX_TOKENS` | none | Caps every reply's tokens; only for trying the scripts on a tiny model on the processor. |
| `EVAL_PORT` | per script (18200, 18300, 18400) | The server's port on 127.0.0.1. |

The server is started on 127.0.0.1 with `--jinja` and the context above, then
the arguments you give, and stopped when the script ends.

## Running them in the GPU container

The code test compiles and runs what the model wrote, so run it in a
**throwaway container**, never on the host. Before any GPU run, follow
CLAUDE.md: nothing else may be using the card (`pgrep -af llama-server`,
`podman ps`, `/sys/class/drm/card*/device/mem_info_vram_used`), and only one
heavy job at a time.

```sh
# The telamon-llama RPM is built as in CLAUDE.md (packaging/telamon-llama).
podman run --rm -it --security-opt label=disable --device /dev/dri \
    --memory=12g --cpus=12 \
    -v "$PWD":/src -w /src \
    -v <folder with the .gguf files>:/models:ro \
    -v <folder with the telamon-llama RPM>:/rpm:ro \
    registry.fedoraproject.org/fedora:44 bash

# In the container:
dnf -y install /rpm/telamon-llama-*.rpm mesa-vulkan-drivers python3 golang rust

# One model, the code test:
python3 tools/eval/codeeval.py qwen3-4b -m /models/Qwen3-4B-Instruct-2507-Q4_K_M.gguf -ngl 99 --spec-default
# With reasoning:
THINK=1 python3 tools/eval/codeeval.py gpt-oss-20b -m /models/gpt-oss-20b-mxfp4.gguf -ngl 99 --spec-default

# Several models, each three times, and the speed test:
BENCH=1 tools/eval/repeat.sh <<'EOF'
qwen3-4b|-m /models/Qwen3-4B-Instruct-2507-Q4_K_M.gguf -ngl 99 --spec-default
qwen3-coder-30b|-m /models/Qwen3-Coder-30B-A3B-UD-Q4_K_XL.gguf -ngl 99 --spec-default
EOF
```

The `name|arguments` lines are split on spaces, so a path with a space needs
the script run with the arguments directly instead.

The numbers in `docs/BACKEND.md` were measured on an RX 7900 XTX (24 GiB)
with telamon-llama 0.6.0, n-gram drafting on (`--spec-default`), thinking
off, temperature 0.2, and each model run 3–4 times; the model's file name
and quantisation are in the table there. Models are compared by the same
arguments Gates starts them with (`docs/BACKEND.md` → The server Gates runs),
so add `--cache-type-k q8_0 --cache-type-v q8_0` for Smaller Context Cache.

## Without a graphics card

Everything above runs on the processor too, slowly (leave `-ngl` out, or set
`-ngl 0`, and limit `--threads`). That is only useful to check the scripts
themselves with a tiny model; its scores mean nothing. `EVAL_MAX_TOKENS=150` keeps a
rambling model short.
