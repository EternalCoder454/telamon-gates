#!/bin/bash
# The code test and the speed test once for each "name|llama-server args"
# line on stdin, e.g.
#   echo 'qwen3-4b|-m /models/Qwen3-4B-Instruct-2507-Q4_K_M.gguf -ngl 99 --spec-default' | tools/eval/suite.sh
# THINK=1 lets the models reason in the code test. Output goes to $EVAL_OUT
# (default out/eval); see README.md. The container needs llama-server, python3,
# rustc and go.
set -uo pipefail
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
while IFS='|' read -r name args; do
    [ -z "$name" ] && continue
    # shellcheck disable=SC2086  # the arguments are split on purpose
    python3 "$here/codeeval.py" "$name" $args </dev/null
    # shellcheck disable=SC2086
    python3 "$here/bench.py" "$name" $args </dev/null
done
