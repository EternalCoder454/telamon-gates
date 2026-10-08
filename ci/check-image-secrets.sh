#!/bin/bash
# check-image-secrets.sh <image>: fail if the CI image could carry a secret.
# GHCR makes an image pushed from this public repo public at once, so the
# `publish` job of ci.yml runs this on the loaded image before it logs in and
# pushes. Taken from atlasos-notepad (its reviewed copy); runs with docker on the runner,
# CONTAINER_ENGINE=podman to try it locally. Checks the build
# history (commands and build args), the environment, and the files a secret
# would land in: no repository or .git copied in, root's home holds only the
# skeleton files and an empty .ssh, and no token- or key-shaped string in the
# places a build writes to.
set -euo pipefail
image=${1:?usage: check-image-secrets.sh <image>}
engine=${CONTAINER_ENGINE:-docker}
fail=0
bad() { echo "::error title=Secret check::$*"; fail=1; }

# Token shapes (GitHub, AWS, Slack, private keys); strict lengths, so the
# framework's redaction tests and fixture templates do not match.
tokens='gh[pousr]_[A-Za-z0-9]{36}|github_pat_[A-Za-z0-9_]{60,}|AKIA[0-9A-Z]{16}|xox[abpr]-[0-9A-Za-z-]{10,}|-----BEGIN [A-Z ]*PRIVATE KEY-----'
names='(TOKEN|SECRET|PASSW(OR)?D|PRIVATE|CREDENTIAL|API_?KEY|AUTH)'

# Matches are reported by kind, never printed: the log of this public repo is
# public, and GitHub masks only the secrets it knows. An engine error fails.
if ! history=$("$engine" history --no-trunc --format '{{.CreatedBy}}' "$image"); then
    bad "cannot read the image history"
elif grep -qE -e "$tokens" <<<"$history"; then
    bad "the image history holds a token-shaped string"
elif found=$(grep -oiE "${names}[A-Z_]*=" <<<"$history" | sort -u | tr '\n' ' ') && [ -n "$found" ]; then
    bad "the image history sets a secret-named variable: $found"
fi

if ! env=$("$engine" image inspect --format '{{range .Config.Env}}{{println .}}{{end}}' "$image"); then
    bad "cannot read the image environment"
elif grep -qE -e "$tokens" <<<"$env"; then
    bad "the image environment holds a token-shaped string"
elif found=$(cut -d= -f1 <<<"$env" | grep -iE "$names" | tr '\n' ' ') && [ -n "$found" ]; then
    bad "the image environment has a secret-named variable: $found"
fi

# The work paths may exist as empty directories (BuildKit leaves the mount
# points of RUN --mount behind); a file in them fails. Token shapes are looked
# for where a build writes and in every file of any telamon-* RPM (binaries
# too), the only packages not from Fedora; file names only, never the matching line.
# shellcheck disable=SC2016 # expanded inside the container
if ! "$engine" run --rm --pull=never --network none --security-opt label=disable "$image" bash -c '
    rc=0
    for d in /src /workspace /github; do
        f=$(find "$d" -mindepth 1 ! -type d 2>/dev/null | head -3)
        [ -n "$f" ] && { echo "files under $d: $f"; rc=1; }
    done
    git_dirs=$(find / -xdev -name .git -not -path "/proc/*" 2>/dev/null | head -5)
    [ -n "$git_dirs" ] && { echo "git directories: $git_dirs"; rc=1; }
    extra=$(find /root -mindepth 1 -maxdepth 1 \
        ! -name .bash_logout ! -name .bash_profile ! -name .bashrc \
        ! -name .cshrc ! -name .tcshrc ! -name .ssh ! -name .cache 2>/dev/null)
    [ -n "$extra" ] && { echo "unexpected in /root: $extra"; rc=1; }
    [ -n "$(ls -A /root/.ssh 2>/dev/null)" ] && { echo "/root/.ssh is not empty"; rc=1; }
    hits=$(grep -rIlE -- "$1" /root /home /etc /opt /usr/local /tmp /var/tmp \
        /var/lib /var/log /var/cache 2>/dev/null | head -5)
    [ -n "$hits" ] && { echo "token-shaped strings in: $hits"; rc=1; }
    # Unlike Notepad'"'"'s, this image has no telamon-ui: each CI run installs
    # the framework release Cargo.toml pins. Any telamon-* package that is
    # there is scanned below all the same.
    mapfile -t pkgs < <(rpm -qa --qf "%{NAME}\n" "telamon-*")
    if [ "${#pkgs[@]}" -gt 0 ]; then
        # Binaries too (-a): these are libraries and fonts.
        hits=$(rpm -ql "${pkgs[@]}" | while IFS= read -r f; do
            [ -f "$f" ] && ! [ -L "$f" ] && printf "%s\0" "$f"
        done | LC_ALL=C xargs -0r grep -alE -- "$1" 2>/dev/null | head -5)
        [ -n "$hits" ] && { echo "token-shaped strings in: $hits"; rc=1; }
    fi
    exit $rc' _ "$tokens" >&2; then
    bad "the image files hold something that must not be published (above)"
fi

[ "$fail" = 0 ] && echo "secret check passed: $image"
exit "$fail"
