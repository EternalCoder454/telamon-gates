#!/bin/bash
# A headless run of the built app, inside the dev container (or CI):
#   scripts/dev.sh scripts/smoke.sh [binary]      (default build/dev/telamon-gates)
# Xvfb and a private session bus; every XDG dir under out/smoke, with two
# saved conversations to start from. Types a message, waits for the demo
# reply, and saves screenshots to out/smoke/*.png. Never the user's display
# or files.
set -euo pipefail

repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
bin=$(realpath "${1:-$repo/build/dev/telamon-gates}")
out=$repo/out/smoke
rm -rf "$out"
mkdir -p "$out"/{config,data,cache,runtime,home,state}
chmod 700 "$out/runtime"
export HOME=$out/home XDG_CONFIG_HOME=$out/config XDG_DATA_HOME=$out/data \
    XDG_CACHE_HOME=$out/cache XDG_RUNTIME_DIR=$out/runtime XDG_STATE_HOME=$out/state
# Telamon OS's desktops run at 1.5x.
export QT_SCALE_FACTOR=${QT_SCALE_FACTOR:-1.5}

# SMOKE_DARK=1: a dark colour scheme (the container has no Plasma schemes).
if [ "${SMOKE_DARK:-0}" = 1 ]; then
    {
        for group in Window View Button Header Tooltip Complementary; do
            case $group in
                View) bg=40,38,60 ;;
                Button) bg=52,49,76 ;;
                *) bg=33,31,51 ;;
            esac
            printf '[Colors:%s]\nBackgroundNormal=%s\nBackgroundAlternate=%s\nForegroundNormal=232,230,245\nForegroundInactive=160,156,185\nDecorationFocus=138,122,244\nDecorationHover=138,122,244\n\n' "$group" "$bg" "$bg"
        done
        printf '[Colors:Selection]\nBackgroundNormal=138,122,244\nForegroundNormal=20,18,31\n'
    } >"$XDG_CONFIG_HOME/kdeglobals"
fi

# SMOKE_MODEL=<file.gguf>: put a model in the models folder, so the reply
# comes from llama.cpp (telamon-llama, or llama-server on $PATH) instead of
# the demo. The binary must be installed in the container.
if [ -n "${SMOKE_MODEL:-}" ]; then
    mkdir -p "$XDG_DATA_HOME/telamon-gates/models"
    cp "$SMOKE_MODEL" "$XDG_DATA_HOME/telamon-gates/models/"
fi

# Two saved conversations: one from today, one from last month.
now=$(date +%s%3N)
old=$((now - 20 * 86400000))
dir=$XDG_DATA_HOME/telamon-gates/conversations
mkdir -p "$dir"
cat >"$dir/000000000001-0000.json" <<EOF
{"id":"000000000001-0000","title":"Plan a weekend in Lisbon","created":$old,"updated":$old,
 "messages":[{"role":"user","text":"Plan a weekend in Lisbon"},{"role":"assistant","text":"Day one: **Alfama** and the castle.\n\n1. Tram 28\n2. Miradouro da Graça"}]}
EOF
cat >"$dir/000000000002-0000.json" <<EOF
{"id":"000000000002-0000","title":"Rust lifetimes","created":$now,"updated":$now,
 "messages":[{"role":"user","text":"What is a lifetime?"},{"role":"assistant","text":"A *lifetime* names how long a reference is valid."}]}
EOF

# One conversation file that can't be read (with text the log must never
# repeat), and one from a newer Gates than this build: the first is set aside
# in damaged/, the second is left exactly as it is.
printf 'NOT-JSON-SECRET-TEXT {' >"$dir/0000000000a0-0000.json"
printf '{"version":99,"id":"0000000000a2-0000","title":"From the future","created":1,"updated":1,"messages":[]}' >"$dir/0000000000a2-0000.json"
cp "$dir/0000000000a2-0000.json" "$out/newer.json"

shot() {
    import -window root "$out/$1.png"
    echo "saved $out/$1.png"
}

run() {
    "$bin" >"$out/app.log" 2>&1 &
    local app=$!
    sleep 4
    shot 1-start
    # One window: a second launch raises this one and exits at once (code 0),
    # and this one stays.
    "$bin" >"$out/app2.log" 2>&1 &
    local second=$!
    for _ in $(seq 50); do
        kill -0 "$second" 2>/dev/null || break
        sleep 0.1
    done
    if kill -0 "$second" 2>/dev/null; then
        echo "FAIL: a second launch is still running (a second window)" >&2
        kill "$second" "$app"
        exit 1
    fi
    wait "$second" && echo "single instance: the second launch exited with 0"
    if ! kill -0 "$app" 2>/dev/null; then
        echo "FAIL: the first instance is gone after the second launch" >&2
        exit 1
    fi
    echo "single instance: the first launch (pid $app) is still running"
    xdotool type --delay 15 "Explain Rust ownership in two lines"
    shot 2-typed
    xdotool key Return
    sleep 1
    shot 3-streaming
    sleep 8
    shot 4-replied
    # Stopped mid-reply with Escape: what came is kept.
    xdotool type --delay 15 "Second question"
    xdotool key Return
    sleep 0.6
    xdotool key Escape
    sleep 1
    shot 5-stopped
    # A saved conversation, from the sidebar (window coordinates at 1.5x).
    xdotool mousemove 138 244 click 1
    sleep 1.5
    shot 6-opened
    xdotool mousemove 115 979 click 1
    sleep 1.5
    shot 7-settings
    # Delete the oldest conversation: its menu, then the confirmation.
    xdotool mousemove 184 329 click 3
    sleep 1
    shot 8-menu
    xdotool key Down Return
    sleep 1
    shot 9-confirm
    xdotool mousemove 998 596 click 1
    sleep 1
    shot 10-deleted
    kill "$app"
    wait "$app" || true
}

export -f run shot
export bin out
xvfb-run -a -s "-screen 0 1800x1300x24" dbus-run-session -- bash -c run
echo "--- app log"
cat "$out/app.log"
if [ -n "${SMOKE_MODEL:-}" ]; then
    echo "--- llama-server log (last lines)"
    tail -5 "$out/state/telamon-gates/llama-server.log" 2>/dev/null || echo "(none)"
fi
echo "--- Gates' log"
cat "$out/state/telamon-gates/telamon-gates.log" || true
if ! grep -q ' starting$' "$out/state/telamon-gates/telamon-gates.log"; then
    echo "FAIL: no start line in the log under \$XDG_STATE_HOME/telamon-gates" >&2
    exit 1
fi
if [ "$(grep -c ' starting$' "$out/state/telamon-gates/telamon-gates.log")" != 1 ]; then
    echo "FAIL: more than one start line: the second launch ran past the single-instance check" >&2
    exit 1
fi
echo "--- conversations"
ls -la "$dir"
replied=0
for f in "$dir"/*.json; do
    case $f in */00000000000[12]-0000.json) continue ;; esac
    grep -q '"role": "assistant"' "$f" && replied=1
done
if [ "$replied" != 1 ]; then
    echo "FAIL: no new conversation with a reply was saved" >&2
    exit 1
fi
if [ -e "$dir/000000000001-0000.json" ]; then
    echo "FAIL: the deleted conversation's file is still there" >&2
    exit 1
fi
# Data safety: the unreadable file is set aside (0600) and logged without its
# text, the newer one is untouched, files are saved with a version and keep a
# backup, and the log holds events, never what was said.
glog=$out/state/telamon-gates/telamon-gates.log
if [ -e "$dir/0000000000a0-0000.json" ]; then
    echo "FAIL: the unreadable conversation file was not set aside" >&2
    exit 1
fi
set -- "$dir"/damaged/0000000000a0-0000.*.json
if [ ! -e "$1" ] || [ "$(stat -c %a "$1")" != 600 ] || [ "$(cat "$1")" != 'NOT-JSON-SECRET-TEXT {' ]; then
    echo "FAIL: the unreadable file is not in damaged/ (0600) as it was" >&2
    exit 1
fi
if ! cmp -s "$dir/0000000000a2-0000.json" "$out/newer.json"; then
    echo "FAIL: the file from a newer version was changed" >&2
    exit 1
fi
if ! grep -q ' set aside as .*0000000000a0-0000' "$glog" || ! grep -q '0000000000a2-0000 is from a newer version' "$glog"; then
    echo "FAIL: the log does not say what was done with the unreadable and newer files" >&2
    exit 1
fi
if grep -q -e 'SECRET' -e 'Alfama' -e 'lifetime' -e 'Explain Rust' "$glog"; then
    echo "FAIL: the log has conversation text in it" >&2
    exit 1
fi
if ! grep -q '"version": 1' "$dir"/*.json 2>/dev/null || ! ls "$dir"/*.json.bak >/dev/null 2>&1; then
    echo "FAIL: no saved conversation has a version and a backup" >&2
    exit 1
fi
echo "smoke: ok"
