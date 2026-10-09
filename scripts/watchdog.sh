#!/bin/bash
# The graphics memory limit, end to end, inside the dev container (or CI):
#   scripts/dev.sh scripts/watchdog.sh [binary]      (default build/dev/telamon-gates)
# Runs the app (a debug build) under Xvfb and a private session bus with every
# XDG dir under out/watchdog, a stand-in llama-server that streams a long
# reply (scripts/fake-llama-server.py) and a made-up card whose memory use the
# script sets (TELAMON_GATES_DRM, which only debug builds read). Checks that:
#   - a reply under way is cancelled and the server stopped when the card
#     fills, with the banner in the window and the line in the log;
#   - the next message is refused while the card is full, and starts the
#     server again once it is not;
#   - with the limit Off, the same card is left alone.
# Screenshots go to out/watchdog/*.png. Never the user's display or files, and
# no graphics card.
set -euo pipefail

repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
bin=$(realpath "${1:-$repo/build/dev/telamon-gates}")
out=$repo/out/watchdog
rm -rf "$out"
mkdir -p "$out"/{config,data,cache,runtime,home,state,fakebin}
chmod 700 "$out/runtime"
export HOME=$out/home XDG_CONFIG_HOME=$out/config XDG_DATA_HOME=$out/data \
    XDG_CACHE_HOME=$out/cache XDG_RUNTIME_DIR=$out/runtime XDG_STATE_HOME=$out/state
export QT_SCALE_FACTOR=${QT_SCALE_FACTOR:-1.5}

# A card of 24 GiB, and the stand-in server found on $PATH.
drm=$out/drm/card0/device
mkdir -p "$drm"
printf '25769803776\n' >"$drm/mem_info_vram_total"
export TELAMON_GATES_DRM=$out/drm
cp "$repo/scripts/fake-llama-server.py" "$out/fakebin/llama-server"
export PATH="$out/fakebin:$PATH"
# A model for it to "load" (the stand-in doesn't read it).
mkdir -p "$XDG_DATA_HOME/telamon-gates/models"
: >"$XDG_DATA_HOME/telamon-gates/models/test.gguf"
glog=$out/state/telamon-gates/telamon-gates.log

# The card's memory in use, in tenths of a GiB.
use() { printf '%s\n' "$(($1 * 107374182))" >"$drm/mem_info_vram_used"; }
shot() { import -window root -crop 1512x1080+0+0 +repage "$out/$1.png"; echo "saved $out/$1.png"; }
# The stand-in is up (the container has no pgrep; the brackets keep grep from
# finding itself).
server_up() {
    local f
    for f in /proc/[0-9]*/cmdline; do
        tr '\0' ' ' <"$f" 2>/dev/null | grep -q "$out/fakebin/[l]lama-server --model" && return 0
    done
    return 1
}
fail() { echo "FAIL: $*" >&2; exit 1; }
say() {
    xdotool mousemove 900 978 click 1
    xdotool type --delay 10 "$1"
    xdotool key Return
}

run() {
    # --- The limit is the default (95%): 8 GiB in use to begin with.
    use 80
    "$bin" >"$out/app.log" 2>&1 &
    app=$!
    sleep 4
    say "Tell me about lifetimes"
    sleep 4
    server_up || fail "the server didn't start for a reply"
    shot 1-streaming
    # The card fills: 23.4 of 24 GiB is 97%. Two readings 2 s apart, then
    # the stop (SIGTERM, the stand-in quits at once).
    use 234
    sleep 7
    server_up && fail "the server is still up with the card full"
    kill -0 "$app" || fail "the app is gone"
    shot 2-stopped
    # The next message: refused while the card is full.
    say "Try again"
    sleep 3
    server_up && fail "the server started with the card full"
    shot 3-refused
    # The card frees up: the same message goes through.
    use 80
    sleep 1
    say "Now?"
    sleep 4
    server_up || fail "the server didn't start with the card free"
    shot 4-restarted
    kill "$app"
    wait "$app" || true
    sleep 1
    server_up && fail "the server outlived the app"

    # --- The limit Off: the same card is left alone.
    printf '[Chat]\nMemoryCap=off\n' >>"$XDG_CONFIG_HOME/telamon-gatesrc"
    use 80
    "$bin" >"$out/app-off.log" 2>&1 &
    app=$!
    sleep 4
    say "Tell me about lifetimes"
    sleep 4
    server_up || fail "the server didn't start (limit Off)"
    use 239
    sleep 7
    server_up || fail "the server was stopped with the limit Off"
    shot 5-off
    kill "$app"
    wait "$app" || true
}

export -f run shot server_up fail say use
export bin out drm glog
if ! xvfb-run -a -s "-screen 0 1800x1300x24" dbus-run-session -- bash -c run >"$out/session.log" 2>&1; then
    tail -5 "$out/session.log" >&2
    fail "see $out/session.log and $out/app.log"
fi
echo "--- Gates' log"
cat "$glog"
count=$(grep -c 'Stopped the model: graphics memory reached 95% (23.4 of 24.0 GiB)\.' "$glog" || true)
[ "$count" = 1 ] || fail "the log should say once that the model was stopped at 95% (23.4 of 24.0 GiB), not $count times"
# Nothing in the log but the event: no conversation text.
if grep -q -e 'lifetimes' -e 'Try again' "$glog"; then
    fail "the log has conversation text in it"
fi
echo "watchdog: ok"
