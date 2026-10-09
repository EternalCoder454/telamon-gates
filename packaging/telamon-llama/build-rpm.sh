#!/bin/bash
# Build the telamon-llama RPM inside a fedora:44 container, as root.
#   packaging/telamon-llama/build-rpm.sh <out dir> [rpmbuild options]
# Downloads the llama.cpp release the spec names (its sha256 is checked in
# %prep) unless SOURCES=<dir> already holds it. The binary RPM is copied to
# <out dir>. CCACHE_DIR, when set, keeps compiled objects between builds.
set -euo pipefail

main() {
    out=${1:?usage: build-rpm.sh <out dir> [rpmbuild options]}
    shift

    here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
    spec=$here/telamon-llama.spec

    dnf -y install rpm-build dnf5-plugins curl ccache >&2
    dnf -y builddep "$spec" >&2

    top=$(mktemp -d)
    trap 'rm -rf "$top"' EXIT
    mkdir -p "$top"/{SOURCES,BUILD,RPMS,SRPMS,SPECS}
    # awk reads to the end: an early exit would kill rpmspec with SIGPIPE.
    url=$(rpmspec -P "$spec" | awk '/^Source0:/ && !u {u = $2} END {print u}')
    name=${url##*/}
    if [ -n "${SOURCES:-}" ] && [ -f "$SOURCES/$name" ]; then
        cp "$SOURCES/$name" "$top/SOURCES/"
    else
        curl -fsSL --proto '=https' --max-time 600 -o "$top/SOURCES/$name" "$url"
    fi

    # ccache for the C++ (the Vulkan shaders are most of the build).
    # The build tree is a new random folder each time: paths relative to it,
    # and no hashing of the working directory, so the cache can hit.
    if [ -n "${CCACHE_DIR:-}" ]; then
        export CMAKE_C_COMPILER_LAUNCHER=ccache CMAKE_CXX_COMPILER_LAUNCHER=ccache
        export CCACHE_BASEDIR="$top" CCACHE_NOHASHDIR=1
    fi
    rpmbuild -bb "$@" --define "_topdir $top" "$spec"

    mkdir -p "$out"
    find "$top/RPMS" -name '*.rpm' ! -name '*debuginfo*' ! -name '*debugsource*' -exec cp -v {} "$out"/ \;
}

main "$@"
