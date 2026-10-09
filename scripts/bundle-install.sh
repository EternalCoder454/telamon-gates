#!/bin/bash
# Installs a built Telamon bundle the way Telamon Store does (the framework's
# docs/BUNDLES.md, "Install layout") into a throwaway HOME, then starts it
# from the .desktop file's Exec under Xvfb and a private session bus, and
# checks: the files are the ones the manifest names, the binary finds its
# libraries, the window maps, Gates' log is under $XDG_STATE_HOME, a second
# launch raises the first and exits, and SIGTERM ends it cleanly.
#   scripts/dev.sh scripts/bundle-install.sh [bundle dir]      (default out/bundle)
# The bundle dir holds <id>-<version>-x86_64.tar.zst and telamon-bundle.json
# (the framework's tools/make-bundle.sh --out). Needs python3, tar, zstd,
# xorg-x11-server-Xvfb, dbus-daemon, xdotool and ImageMagick (screenshot).
# Everything is under out/bundle-install; never the user's files or display.
set -euo pipefail

repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
bundle=$(cd "${1:-$repo/out/bundle}" && pwd)
out=$repo/out/bundle-install
rm -rf "$out"
mkdir -p "$out"/{home,config,cache,runtime,state}
chmod 700 "$out/runtime"
export HOME=$out/home XDG_CONFIG_HOME=$out/config XDG_CACHE_HOME=$out/cache \
    XDG_RUNTIME_DIR=$out/runtime XDG_STATE_HOME=$out/state
# The Store's layout is under the data directory ($HOME/.local/share).
export XDG_DATA_HOME=$HOME/.local/share
mkdir -p "$XDG_DATA_HOME"

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

# --- the bundle: the manifest names the archive, and the archive its files
manifest=$bundle/telamon-bundle.json
read -r id version archive want_sha < <(python3 -I - "$manifest" <<'PY'
import json, sys
m = json.load(open(sys.argv[1]))
print(m["id"], m["version"], m["archive"]["name"], m["archive"]["sha256"])
PY
)
[[ $id =~ ^[A-Za-z0-9._-]+$ && $version =~ ^[0-9A-Za-z.-]+$ && $archive =~ ^[A-Za-z0-9._-]+$ ]] ||
    fail "the manifest names an id, version or archive with odd characters"
[ -f "$bundle/$archive" ] || fail "$archive is not in $bundle"
got=$(sha256sum "$bundle/$archive")
[ "${got%% *}" = "$want_sha" ] || fail "$archive is not the archive the manifest names (sha256)"
echo "bundle: $id $version, $archive matches the manifest"

# --- unpack into <apps>/<id>/<version>, and point `current` at it
apps=$XDG_DATA_HOME/telamon-apps
prefix=$apps/$id/$version
# The Store refuses an archive that leaves its folder: no absolute path, no `..`.
if tar --zstd -tf "$bundle/$archive" | grep -E '^/|(^|/)\.\.(/|$)' >/dev/null; then
    fail "the archive has an absolute or .. path"
fi
mkdir -p "$prefix"
tar --zstd -xf "$bundle/$archive" -C "$prefix" --no-same-owner --no-same-permissions
ln -sfn "$version" "$apps/$id/current"
[ "$(readlink "$apps/$id/current")" = "$version" ] || fail "current does not point at $version"

# Every file the manifest lists is there as listed (the Store checks the same).
python3 -I - "$manifest" "$prefix" <<'PY'
import hashlib, json, os, sys
m = json.load(open(sys.argv[1]))
root = sys.argv[2]
for f in m["files"]:
    p = os.path.join(root, f["path"])
    data = open(p, "rb").read()
    assert len(data) == f["size"], f"{f['path']}: size"
    assert hashlib.sha256(data).hexdigest() == f["sha256"], f"{f['path']}: sha256"
    assert bool(os.stat(p).st_mode & 0o111) == f["executable"], f"{f['path']}: executable bit"
print(f"files: {len(m['files'])} match the manifest")
PY

# --- what the Store copies out: the .desktop file with Exec at the binary
# (the first word of every Exec=, an absolute path through `current`;
# TryExec= and Path= dropped), the icons and the metainfo.
mkdir -p "$XDG_DATA_HOME/applications"
desktop=$XDG_DATA_HOME/applications/$id.desktop
sed -E -e '/^(TryExec|Path)=/d' \
    -e "s#^Exec=([^[:space:]]+)#Exec=$apps/$id/current/bin/\\1#" \
    "$prefix/share/applications/$id.desktop" >"$desktop"
for d in icons metainfo; do
    if [ -d "$prefix/share/$d" ]; then
        mkdir -p "$XDG_DATA_HOME/$d"
        cp -r "$prefix/share/$d/." "$XDG_DATA_HOME/$d/"
    fi
done
exec_line=$(sed -n 's/^Exec=//p' "$desktop" | head -n 1)
binary=${exec_line%% *}
[ "$binary" = "$apps/$id/current/bin/$(basename "$binary")" ] || fail "Exec is not at current/bin: $exec_line"
[ -x "$binary" ] || fail "Exec's binary is not executable: $binary"
if command -v desktop-file-validate >/dev/null; then
    desktop-file-validate "$desktop" || fail "desktop-file-validate"
fi
echo "installed: $desktop -> $exec_line"

# Everything the binary links is in the OS (the bundle has no lib/).
if ldd "$binary" | grep 'not found'; then
    fail "the binary is missing libraries (above)"
fi

# --- start it from the .desktop file's Exec, under Xvfb and a session bus
shot() {
    import -window root "$out/$1.png" && echo "saved $out/$1.png"
}

run() {
    # Exec has no field codes here ("%U" would be dropped, as a launcher does).
    local cmd=${exec_line//%[a-zA-Z]/}
    $cmd >"$out/app.log" 2>&1 &
    local app=$!
    if ! timeout 60 xdotool search --sync --onlyvisible --name 'Telamon Gates' >"$out/window.id"; then
        echo "FAIL: no window mapped within 60 s" >&2
        cat "$out/app.log" >&2
        kill "$app" 2>/dev/null || true
        exit 1
    fi
    echo "window: mapped (id $(head -n 1 "$out/window.id")), title: $(xdotool getwindowname "$(head -n 1 "$out/window.id")")"
    sleep 2
    shot window
    # Gates' log: written as it starts.
    log=$XDG_STATE_HOME/telamon-gates/telamon-gates.log
    for _ in $(seq 50); do
        grep -q ' starting$' "$log" 2>/dev/null && break
        sleep 0.2
    done
    grep -q ' starting$' "$log" 2>/dev/null || {
        echo "FAIL: no start line in $log" >&2
        ls -la "$XDG_STATE_HOME/telamon-gates" >&2 || true
        kill "$app" 2>/dev/null || true
        exit 1
    }
    echo "log: $log has a start line"
    # A second launch (the launcher's click) raises this window and exits at once.
    $cmd >"$out/app2.log" 2>&1 &
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
    kill -0 "$app" 2>/dev/null || {
        echo "FAIL: the first instance is gone after the second launch" >&2
        exit 1
    }
    # SIGTERM, as the session sends it at logout: it must end at once, by
    # exiting (0) or by the signal (143: Gates has no handler for it, and the
    # model server, if one runs, goes with it: PR_SET_PDEATHSIG). A crash
    # (SIGSEGV 139, SIGABRT 134) or a hang is a failure.
    kill -TERM "$app"
    local status=0 waited=0
    while kill -0 "$app" 2>/dev/null; do
        sleep 0.2
        waited=$((waited + 1))
        if [ "$waited" -gt 75 ]; then
            echo "FAIL: still running 15 s after SIGTERM" >&2
            kill -KILL "$app"
            exit 1
        fi
    done
    wait "$app" || status=$?
    echo "SIGTERM: ended after $((waited / 5)).$((waited % 5 * 2)) s with status $status (0 = exit, 143 = the signal)"
    if [ "$status" != 0 ] && [ "$status" != 143 ]; then
        echo "FAIL: SIGTERM did not end it cleanly (status $status)" >&2
        cat "$out/app.log" >&2
        exit 1
    fi
    # (procps is not in every image: /proc says which programs run.)
    local exe real
    real=$(realpath "${exec_line%% *}")
    for exe in /proc/[0-9]*/exe; do
        if [ "$(readlink "$exe" 2>/dev/null)" = "$real" ]; then
            echo "FAIL: a process of the app is still there after it exited ($exe)" >&2
            exit 1
        fi
    done
    if xdotool search --onlyvisible --name 'Telamon Gates' >/dev/null 2>&1; then
        echo "FAIL: the window is still there after it exited" >&2
        exit 1
    fi
}

export -f run shot
export out exec_line
xvfb-run -a -s "-screen 0 1800x1300x24" dbus-run-session -- bash -c run
echo "--- app log"
cat "$out/app.log"
echo "--- Gates' log"
cat "$XDG_STATE_HOME/telamon-gates/telamon-gates.log"
echo "bundle-install: ok"
