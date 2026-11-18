#!/bin/sh
# Package one already-built target into the archive consumed by install.sh.
set -eu

cd "$(dirname "$0")/.."
target=${1:?usage: scripts/package-release.sh TARGET}
profile=${PROFILE:-dist}
binary=target/${target}/${profile}/beamerx
[ -x "$binary" ] || {
    printf 'missing executable: %s\n' "$binary" >&2
    exit 1
}

stage=$(mktemp -d "${TMPDIR:-/tmp}/beamerx-package.XXXXXX")
cleanup() {
    rm -rf "$stage"
}
trap cleanup EXIT HUP INT TERM

name=beamerx-${target}
mkdir -p "$stage/$name" dist
cp "$binary" "$stage/$name/beamerx"
cp README.md "$stage/$name/README.md"
tar -C "$stage" -czf "dist/$name.tar.gz" "$name"
printf 'built dist/%s.tar.gz\n' "$name"
