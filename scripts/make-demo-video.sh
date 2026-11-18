#!/usr/bin/env bash
# Record the scripted BeamerX demo and encode it as an MP4.
#
#   scripts/make-demo-video.sh REFERENCE.beamerx OUTPUT.mp4 [ORIGINAL ...]
#
# Opens one BeamerX window for a few minutes: starting from a blank slide,
# the app rebuilds REFERENCE.beamerx through its own UI (`--bench demo`),
# presents it, saves the Beamer source and scrolls through its main.tex,
# capturing its own frames (the screen is not recorded). Don't touch the window while it runs; real mouse input over it
# would mix with the scripted input. Optional ORIGINAL images or PDF pages
# (`deck.pdf#73`), e.g. the slides being reproduced, are shown first. The
# Beamer folder the demo saves (its main.tex is shown in the video) is kept
# next to OUTPUT as OUTPUT-beamer/. Encoding uses AVFoundation (scripts/frames-to-mp4.swift).
set -euo pipefail

usage="usage: make-demo-video.sh REFERENCE.beamerx OUTPUT.mp4 [ORIGINAL ...]"
deck=${1:?$usage}
out=${2:?$usage}
shift 2
root=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d "${TMPDIR:-/tmp}/beamerx-demo.XXXXXX")
trap 'echo "frames kept in $work" >&2' ERR

cargo build --release --quiet --manifest-path "$root/Cargo.toml"
"$root/target/release/beamerx" --bench demo --out "$work" "$deck" | grep BENCH_RESULT || true
[ -s "$work/frames.txt" ] || { echo "no frames recorded (kept $work)" >&2; exit 1; }

manifest="$work/video.txt"
: >"$manifest"
n=0
for img in "$@"; do
    n=$((n + 1))
    abs="$(cd "$(dirname "$img")" && pwd)/$(basename "$img")"   # keeps a #page suffix
    label=$n
    case $img in *.pdf\#[0-9]*) label=${img##*#} ;; esac
    printf '%s\t2.5\tThe original slide %s\n' "$abs" "$label" >>"$manifest"
done
cat "$work/frames.txt" >>"$manifest"
swift "$root/scripts/frames-to-mp4.swift" "$manifest" "$out"
# Keep the Beamer source the demo saved (the main.tex shown in the video).
for dir in "$work"/*-beamer; do
    [ -d "$dir" ] || continue
    keep="$(dirname "$out")/$(basename "$out" .mp4)-beamer"
    rm -rf "$keep" && cp -R "$dir" "$keep" && echo "Beamer source: $keep/main.tex"
done
rm -rf "$work"
