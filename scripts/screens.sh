#!/bin/bash
# Screenshots of every page and state, for reviewing the look, inside the dev
# container (or CI):
#   scripts/dev.sh scripts/screens.sh [binary]     (default build/dev/telamon-gates)
# Light and dark, at 1.5x, into out/screens/<theme>/NN-<state>.png. Xvfb and a
# private session bus; every XDG dir under out/screens; never the user's
# display or files. The window sits at the screen's top left (no window
# manager), so clicks are window coordinates, in device pixels; saved
# conversations are opened through the sidebar's search, which puts the
# match first, rather than by where their rows happen to be.
set -euo pipefail

repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
bin=$(realpath "${1:-$repo/build/dev/telamon-gates}")
root=$repo/out/screens
rm -rf "$root"
export QT_SCALE_FACTOR=1.5

# A saved conversation, last changed `hours` ago: dir, id, title, hours, and
# the messages as JSON on stdin.
seed() {
    local dir=$1 id=$2 title=$3 hours=$4 when messages
    when=$(($(date +%s%3N) - hours * 3600000))
    messages=$(cat)
    printf '{"id":"%s","title":"%s","created":%s,"updated":%s,"messages":%s}\n' \
        "$id" "$title" "$when" "$when" "$messages" >"$dir/$id.json"
}

dark_scheme() {
    local group bg
    for group in Window View Button Header Tooltip Complementary; do
        case $group in
            View) bg=40,38,60 ;;
            Button) bg=52,49,76 ;;
            *) bg=33,31,51 ;;
        esac
        printf '[Colors:%s]\nBackgroundNormal=%s\nBackgroundAlternate=%s\nForegroundNormal=232,230,245\nForegroundInactive=160,156,185\nDecorationFocus=138,122,244\nDecorationHover=138,122,244\n\n' "$group" "$bg" "$bg"
    done
    printf '[Colors:Selection]\nBackgroundNormal=138,122,244\nForegroundNormal=20,18,31\n'
}

# An Agent mode conversation in its own sandbox (no folder chosen).
seed_agent() {
    local when
    when=$(($(date +%s%3N) - 3 * 3600000))
    printf '{"id":"000000000010-0000","title":"Tidy the build scripts","created":%s,"updated":%s,"mode":"agent","messages":[{"role":"user","text":"Tidy the build scripts"},{"role":"assistant","mode":"agent","text":"","tool_calls":[{"id":"a","name":"list_dir","arguments":"{}"}]},{"role":"tool","tool_call_id":"a","summary":"Listed . (3 entries)","text":"src/"},{"role":"assistant","mode":"agent","text":"The folder is empty but for src/. What should the scripts do?"}]}\n' "$when" "$when" >"$1/000000000010-0000.json"
}

seed_all() {
    local dir=$1
    seed "$dir" 000000000001-0000 "Show me what a reply can look like" 1 <<'EOF'
[{"role":"user","text":"Show me what a reply can look like"},{"role":"assistant","speed":38.6,"text":"## Getting started\n\nHere is a short tour with **bold**, *italic*, `inline code` and a [link](https://example.com).\n\n1. Install the toolchain:\n\n   ```sh\n   dnf install cargo\n   ```\n\n2. Create a project\n3. Run it\n\n> Tip: run `cargo clippy` before every commit.\n\n| Command | What it does |\n|---|---|\n| `cargo build` | Compiles |\n| `cargo test` | Runs the tests |\n\n```rust\nfn main() {\n    let names = [\"Ada\", \"Grace\"];\n    for name in names {\n        println!(\"Hello, {name}!\");\n    }\n}\n```\n\nThat is all there is to it."}]
EOF
    # A message with a picture (kept in Gates' attachments folder) and a file.
    local pics=$XDG_DATA_HOME/telamon-gates/attachments
    mkdir -p "$pics"
    magick -size 480x320 gradient:'#8a7af4'-'#f4a7c4' "$pics/0000000000000000000000aa.png"
    seed "$dir" 000000000008-0000 "What is in this picture" 2 <<EOF
[{"role":"user","text":"What is in this picture, and does the notes file match it?","attachments":[{"name":"sunset.png","image":"$pics/0000000000000000000000aa.png"},{"name":"notes.md","text":"# Notes"}]},{"role":"assistant","text":"A soft violet-to-pink gradient, like an evening sky."}]
EOF
    seed_agent "$dir"
    seed "$dir" 000000000002-0000 "Rust lifetimes" 2 <<'EOF'
[{"role":"user","text":"What is a lifetime?"},{"role":"assistant","speed":41.2,"text":"A *lifetime* names how long a reference is valid."}]
EOF
    seed "$dir" 000000000007-0000 "Long conversation about a migration plan" 3 <<'EOF'
[{"role":"user","text":"We are moving a service from one database to another.\n\nIt has about forty tables, a few of them very large, and it cannot be down for more than a minute. The team is small and has never done this before.\n\nWhere do we start, and what usually goes wrong?"},{"role":"assistant","speed":35.1,"text":"Start with a plan you can stop halfway. A migration that has to finish in one go is the one that goes wrong.\n\n### 1. Copy, then follow\n\nCopy the data once while the old database keeps serving, then follow its changes until the two agree. Only then move the reads, and last the writes.\n\n```sql\nSELECT table_name, pg_size_pretty(pg_total_relation_size(quote_ident(table_name))) AS size FROM information_schema.tables WHERE table_schema = 'public' ORDER BY pg_total_relation_size(quote_ident(table_name)) DESC;\n```\n\n### 2. What goes wrong\n\n- Sequences and auto-increment counters that were not copied\n- Time zones in timestamps\n- A long-running transaction that blocks the switch\n\nPractise the switch on a copy at least twice."},{"role":"user","text":"How do we check the two agree?"},{"role":"assistant","speed":36.4,"text":"Count rows per table, then compare checksums of the rows in batches by primary key. Any batch that differs is copied again."}]
EOF
    seed "$dir" 000000000003-0000 "Why did this fail?" 30 <<'EOF'
[{"role":"user","text":"Summarise this article"},{"role":"assistant","failed":true,"text":"The article argues that"}]
EOF
    seed "$dir" 000000000004-0000 "Plan a weekend in Lisbon" 72 <<'EOF'
[{"role":"user","text":"Plan a weekend in Lisbon"},{"role":"assistant","text":"Day one: **Alfama** and the castle."}]
EOF
    seed "$dir" 000000000005-0000 "Vegetable garden layout for a small, shady backyard" 290 <<'EOF'
[{"role":"user","text":"Garden"},{"role":"assistant","text":"Start with leafy greens."}]
EOF
    seed "$dir" 000000000006-0000 "Tax questions" 3100 <<'EOF'
[{"role":"user","text":"Tax"},{"role":"assistant","text":"Ask an accountant."}]
EOF
}

run_theme() {
    local theme=$1 out=$root/$1
    local x=$out/xdg app win
    mkdir -p "$x"/{config,data,cache,runtime,home} "$out"
    chmod 700 "$x/runtime"
    export HOME=$x/home XDG_CONFIG_HOME=$x/config XDG_DATA_HOME=$x/data \
        XDG_CACHE_HOME=$x/cache XDG_RUNTIME_DIR=$x/runtime XDG_STATE_HOME=$x/state
    if [ "$theme" = dark ]; then
        dark_scheme >"$XDG_CONFIG_HOME/kdeglobals"
    fi
    # The app's icon, as the package installs it.
    mkdir -p "$XDG_DATA_HOME/icons/hicolor/scalable/apps"
    cp "$repo/apps/telamon-gates/data/net.eterneon.telamon.gates.svg" "$XDG_DATA_HOME/icons/hicolor/scalable/apps/"
    local dir=$XDG_DATA_HOME/telamon-gates/conversations

    shot() { import -window root -crop 1512x1080+0+0 +repage "$out/$1.png"; }
    # Opens the saved conversation whose title has `text`, then clears the
    # search.
    open_chat() {
        xdotool mousemove 190 32 click 1
        xdotool type --delay 5 "$1"
        sleep 0.6
        xdotool mousemove 180 106 click 1
        sleep 1.2
        xdotool mousemove 190 32 click 1 key ctrl+a BackSpace
        sleep 0.4
    }

    # First run: nothing saved yet.
    "$bin" >"$out/app-first.log" 2>&1 &
    app=$!
    sleep 4
    shot 01-first-run
    # Dismissed with its cross.
    xdotool mousemove 1444 130 click 1
    sleep 0.8
    shot 01b-banner-dismissed
    kill "$app"
    wait "$app" || true

    # The model server is there (a stand-in on $PATH) but the container has
    # no render node: the graphics banner.
    mkdir -p "$out/fakebin"
    printf '#!/bin/sh\nexit 0\n' >"$out/fakebin/llama-server"
    chmod +x "$out/fakebin/llama-server"
    PATH="$out/fakebin:$PATH" "$bin" >"$out/app-no-gpu.log" 2>&1 &
    app=$!
    sleep 4
    shot 01c-no-graphics
    kill "$app"
    wait "$app" || true

    # Conversation files that can't be read: one cut short, and one damaged
    # whose backup is good. The first is set aside at this start and the
    # second restored from its .bak, and the banner says both. The restored
    # one is removed after, so the later screens are as they were.
    mkdir -p "$dir"
    printf '{"id":"0000000000a0-0000","title":"Cut short","created":' >"$dir/0000000000a0-0000.json"
    printf '{"id":"0000000000a1-0000","title":"Restored","created":1,"updated":1,"messages":[]}' >"$dir/0000000000a1-0000.json.bak"
    printf 'not json' >"$dir/0000000000a1-0000.json"
    "$bin" >"$out/app-damaged.log" 2>&1 &
    app=$!
    sleep 4
    shot 01d-damaged-banner
    kill "$app"
    wait "$app" || true
    rm -f "$dir"/0000000000a1-0000.json*

    seed_all "$dir"
    # One mode of the user's own, beside the built-in ones.
    printf '{"edits":[],"custom":[{"id":"my-1","name":"Pirate","prompt":"Answer like a friendly pirate.","temperature":0.9}]}' >"$XDG_DATA_HOME/telamon-gates/modes.json"
    # SCREENS_MODEL=<file.gguf>: replies from llama.cpp (installed in the
    # container) after the first run, which shows the no-model state.
    if [ -n "${SCREENS_MODEL:-}" ]; then
        mkdir -p "$XDG_DATA_HOME/telamon-gates/models"
        cp "$SCREENS_MODEL" "$XDG_DATA_HOME/telamon-gates/models/"
    fi
    # SCREENS_DECISION=<file.gguf>: a decision model, so SystemOne picks.
    if [ -n "${SCREENS_DECISION:-}" ]; then
        mkdir -p "$XDG_DATA_HOME/telamon-gates/models"
        cp "$SCREENS_DECISION" "$XDG_DATA_HOME/telamon-gates/models/"
    fi
    "$bin" >"$out/app.log" 2>&1 &
    app=$!
    sleep 4
    shot 02-new-chat
    # The mode button's menu.
    xdotool mousemove 470 1035 click 1
    sleep 0.6
    shot 02b-mode-menu
    # A click outside closes it; the field takes the typing again.
    xdotool mousemove 900 500 click 1
    sleep 0.3
    xdotool mousemove 900 978 click 1
    sleep 0.3
    xdotool type --delay 10 "Write a short story about"
    xdotool key shift+Return
    xdotool type --delay 10 "autumn in Lisbon"
    # The pointer on Send.
    xdotool mousemove 1442 991
    sleep 0.4
    shot 03-typed-hover-send
    xdotool key Return
    sleep 0.3
    shot 04-thinking
    sleep 1.2
    shot 05-streaming
    sleep 6
    xdotool mousemove 900 600
    shot 06-replied
    # The pointer on a conversation in the sidebar.
    xdotool mousemove 180 244
    sleep 0.6
    shot 07-hover-sidebar
    open_chat "Show me"
    shot 08-showcase
    # The pointer on your message: Edit and Branch From Here under it.
    xdotool mousemove 1300 193
    sleep 0.6
    shot 08b-message-actions
    xdotool mousemove 1092 197 click 1
    sleep 0.6
    shot 08c-editing
    xdotool key Escape
    sleep 0.4
    open_chat "picture"
    shot 08d-attachments
    open_chat "build scripts"
    shot 08e-agent-sandbox
    open_chat "migration"
    shot 09-long-end
    xdotool mousemove 900 500 click 4 click 4 click 4 click 4 click 4 click 4
    sleep 0.6
    shot 10-long-scrolled
    open_chat "fail"
    shot 11-failed
    xdotool mousemove 190 32 click 1
    xdotool type --delay 5 "li"
    sleep 0.8
    shot 12-search-results
    xdotool type --delay 5 "zzz"
    sleep 0.8
    shot 13-no-results
    xdotool key ctrl+a BackSpace Escape
    sleep 0.6
    xdotool mousemove 180 244 click 3
    sleep 0.8
    shot 14-menu
    xdotool key Down Return
    sleep 0.8
    shot 15-confirm
    xdotool key Escape
    sleep 0.6
    xdotool mousemove 115 979 click 1
    sleep 1.2
    shot 16-settings
    xdotool mousemove 900 600 click 5 click 5 click 5 click 5 click 5 click 5 click 5
    sleep 0.8
    shot 16b-settings-modes
    # Edit one of the user's own modes.
    xdotool mousemove 1380 227 click 1
    sleep 0.8
    shot 16c-mode-dialog
    xdotool key Escape
    sleep 0.4
    xdotool mousemove 900 600 click 4 click 4 click 4 click 4 click 4 click 4 click 4
    sleep 0.6
    # Keyboard focus, as Tab shows it.
    xdotool key Tab Tab
    sleep 0.4
    shot 17-settings-focus
    # The end of the page: the Logs row.
    xdotool mousemove 900 600 click --repeat 20 --delay 20 5
    sleep 0.6
    shot 17b-settings-logs
    xdotool mousemove 115 1039 click 1
    sleep 1.2
    shot 18-about
    xdotool mousemove 115 919 click 1
    sleep 1.5
    shot 20-models
    # SCREENS_HUB=1: search Hugging Face (needs the network) and open a result.
    if [ -n "${SCREENS_HUB:-}" ]; then
        xdotool mousemove 900 422 click 1
        xdotool type --delay 10 "SmolLM2-135M-Instruct"
        sleep 5
        shot 21-models-search
        xdotool mousemove 900 500 click 1
        sleep 4
        shot 22-models-files
        # Download the first file, look, then cancel it.
        xdotool mousemove 1357 569 click 1
        sleep 2
        shot 23-models-downloading
        xdotool mousemove 1370 427 click 1
        sleep 1
        shot 24-models-cancelled
    fi
    # The Fleet page, before any run.
    xdotool mousemove 115 859 click 1
    sleep 1.5
    shot 25-fleet-empty
    # Narrow, on a conversation: the sidebar folds to icons.
    open_chat "Show me"
    win=$(xdotool search --onlyvisible --name "Telamon Gates" | head -1)
    xdotool windowsize "$win" 640 900
    sleep 1.5
    import -window root -crop 640x900+0+0 +repage "$out/19-narrow.png"
    kill "$app"
    wait "$app" || true

    # The Fleet page with sample agents (TELAMON_GATES_SEED, src/fleet.rs):
    # one asking, one working at a narrow width, the coordinator planning.
    TELAMON_GATES_SEED=fleet "$bin" >"$out/app-fleet.log" 2>&1 &
    app=$!
    sleep 4
    # The window keeps the narrow size the last run left it at.
    win=$(xdotool search --onlyvisible --name "Telamon Gates" | head -1)
    xdotool windowsize "$win" 1512 1080
    sleep 1.5
    xdotool mousemove 115 859 click 1
    sleep 1.5
    shot 26-fleet-cards
    xdotool mousemove 900 700 click 5 click 5 click 5 click 5 click 5 click 5
    sleep 0.8
    shot 27-fleet-cards-scrolled
    kill "$app"
    wait "$app" || true
    TELAMON_GATES_SEED=fleet-working "$bin" >"$out/app-fleet-working.log" 2>&1 &
    app=$!
    sleep 4
    win=$(xdotool search --onlyvisible --name "Telamon Gates" | head -1)
    xdotool windowsize "$win" 1512 1080
    sleep 1.5
    xdotool mousemove 115 859 click 1
    sleep 1.5
    shot 28-fleet-working
    xdotool windowsize "$win" 640 900
    sleep 1.5
    import -window root -crop 640x900+0+0 +repage "$out/29-fleet-narrow.png"
    kill "$app"
    wait "$app" || true
    TELAMON_GATES_SEED=fleet-planning "$bin" >"$out/app-fleet-planning.log" 2>&1 &
    app=$!
    sleep 4
    win=$(xdotool search --onlyvisible --name "Telamon Gates" | head -1)
    xdotool windowsize "$win" 1512 1080
    sleep 1.5
    xdotool mousemove 115 859 click 1
    sleep 1.5
    shot 30-fleet-planning
    kill "$app"
    wait "$app" || true
    rm -rf "$x"
}

export -f run_theme seed seed_all seed_agent dark_scheme
export bin root repo
for theme in light dark; do
    xvfb-run -a -s "-screen 0 1600x1200x24" dbus-run-session -- bash -c "run_theme $theme" >/dev/null 2>&1
    shots=("$root/$theme"/*.png)
    echo "$root/$theme: ${#shots[@]} screenshots"
    for log in "$root/$theme"/app*.log; do
        if [ -s "$log" ]; then
            echo "--- $log"
            cat "$log"
        fi
    done
done
