#!/usr/bin/env python3
"""Speculative decoding on llama-server: one config per run, three prompts,
temperature 0. Prints generated tokens/s, the draft acceptance and the wall
time per prompt; the replies are kept in $EVAL_OUT/samples/.

Usage: bench.py <name> <llama-server args, including -m <model.gguf>>
(Add --spec-default for n-gram drafting, or --model-draft for a draft model.)"""
import os, sys

from common import chat, max_tokens, out_path, reasoning, server

PORT = int(os.environ.get("EVAL_PORT", "18200"))
HERE = os.path.dirname(os.path.abspath(__file__))
# A fixed 6,000-byte piece of a Rust file (crates/gates-core/src/attach.rs as
# it was on 2026-10-08), for the rewrite prompt: an agent's edit repeats most
# of the file, which is where n-gram drafting pays.
CODE = open(os.path.join(HERE, "rewrite-sample.rs")).read()
PROMPTS = {
    "chat": "Explain in about 300 words how a heat pump works and when it beats a gas boiler.",
    "rewrite": "Return this Rust file unchanged except: rename the function `read` to `read_attachment`. "
               "Output only the whole file in one code block.\n\n```rust\n" + CODE + "\n```",
    "story": "Write a 400-word story about a lighthouse keeper who finds a message in a bottle.",
}


def main():
    if len(sys.argv) < 3:
        sys.exit(__doc__)
    name, args = sys.argv[1], sys.argv[2:]
    with server(f"b-{name}", args, PORT):
        for label, prompt in PROMPTS.items():
            r, wall = chat(PORT, {"messages": [{"role": "user", "content": prompt}],
                                  "temperature": 0, "max_tokens": max_tokens(1500), "seed": 1, **reasoning(False)}, timeout=600)
            with open(out_path("samples", f"{name}-{label}.txt"), "w") as f:
                f.write(r["choices"][0]["message"].get("content") or "")
            t = r.get("timings", {})
            drafted, accepted = t.get("draft_n", 0), t.get("draft_n_accepted", 0)
            acc = f"{100 * accepted / drafted:.0f}%" if drafted else "-"
            print(f"{name:12} {label:8} gen {t.get('predicted_per_second', 0):6.1f} tok/s  "
                  f"tokens {t.get('predicted_n', 0):5}  accepted {acc:>4}  wall {wall:5.1f} s", flush=True)


if __name__ == "__main__":
    main()
