#!/bin/bash
# Build the Telamon Gates RPM inside a fedora:44 container, as root.
#   packaging/build-rpm.sh <out dir> [rpmbuild options]
# The binary RPM (no source, no debuginfo) is copied to <out dir>.
# Cargo needs network access (or a CARGO_HOME holding the crates).
# ATLAS_LOCAL_RPMS=<dir> installs the RPMs in <dir> first: the Telamon
# framework's (telamon-ui), which the app builds against and no repository has.
set -euo pipefail

main() {
    out=${1:?usage: build-rpm.sh <out dir> [rpmbuild options]}
    shift

    here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
    src=$(dirname "$here")
    spec=$here/telamon-gates.spec
    version=$(awk '/^Version:/ {print $2; exit}' "$spec")

    # ATLAS_LOCAL_RPMS goes to it in the environment.
    bash "$here/install-builddeps.sh"

    top=$(mktemp -d)
    trap 'rm -rf "$top"' EXIT
    mkdir -p "$top"/{SOURCES,BUILD,RPMS,SRPMS,SPECS}
    # What the checkout holds, without build output or git.
    tar -C "$src" --exclude=./.git --exclude=./target --exclude=./out --exclude=./build \
        --transform "s,^\./,telamon-gates-$version/," \
        -czf "$top/SOURCES/telamon-gates-$version.tar.gz" .

    rpmbuild -bb "$@" --define "_topdir $top" "$spec"

    mkdir -p "$out"
    find "$top/RPMS" -name '*.rpm' ! -name '*.src.rpm' ! -name '*debuginfo*' ! -name '*debugsource*' \
        -exec cp -v {} "$out"/ \;
}

main "$@"
exit $?
