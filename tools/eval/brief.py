#!/usr/bin/env python3
"""Chat and story with the model's own reasoning against brief
(Request::brief): tokens, time, and how many words went to reasoning.

Usage: brief.py <name> <llama-server args, including -m <model.gguf>>"""
import os, sys

from common import chat, max_tokens, reasoning, server

PORT = int(os.environ.get("EVAL_PORT", "18400"))
PROMPTS = {"chat": "Explain in about 300 words how a heat pump works and when it beats a gas boiler.",
           "story": "Write a 400-word story about a lighthouse keeper who finds a message in a bottle."}


def main():
    if len(sys.argv) < 3:
        sys.exit(__doc__)
    name, args = sys.argv[1], sys.argv[2:]
    with server(f"br-{name}", args, PORT):
        for label, p in PROMPTS.items():
            for brief in (False, True):
                body = {"messages": [{"role": "user", "content": p}], "temperature": 0, "seed": 1, "max_tokens": max_tokens(6000)}
                if brief:
                    body.update(reasoning(False))
                r, wall = chat(PORT, body)
                msg, t = r["choices"][0]["message"], r["timings"]
                think = len((msg.get("reasoning_content") or "").split())
                words = len((msg.get("content") or "").split())
                print(f"{name:12} {label:6} {'brief' if brief else 'own  '}  tokens {t['predicted_n']:5}  wall {wall:5.1f} s  "
                      f"reasoning words {think:5}  answer words {words:4}", flush=True)


if __name__ == "__main__":
    main()
