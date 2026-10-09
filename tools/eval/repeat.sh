#!/bin/bash
# The code test 3 times for each "name|llama-server args" line on stdin (it
# varies from run to run), then, with BENCH=1, the speed test once. THINK=1
# lets the models reason in the code test. See README.md.
set -uo pipefail
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
while IFS='|' read -r name args; do
    [ -z "$name" ] && continue
    for _ in 1 2 3; do
        # shellcheck disable=SC2086  # the arguments are split on purpose
        python3 "$here/codeeval.py" "$name" $args </dev/null
    done
    # shellcheck disable=SC2086
    [ -n "${BENCH:-}" ] && python3 "$here/bench.py" "$name" $args </dev/null
done
exit 0
