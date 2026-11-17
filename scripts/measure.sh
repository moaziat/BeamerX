#!/bin/bash
# Reproducible performance measurements for BeamerX on macOS.
# Usage: scripts/measure.sh [output-dir]
# Writes one JSON file per scenario plus summary.txt. Requires a release
# build (done here) and, for equation/PDF numbers, a LaTeX engine.
set -euo pipefail
cd "$(dirname "$0")/.."
OUT=${1:-target/measurements/$(date +%Y%m%d-%H%M%S)}
mkdir -p "$OUT"
cargo build --release --target aarch64-apple-darwin --quiet
BIN=target/aarch64-apple-darwin/release/beamerx
cargo build --release --example export_timing --quiet

{
  echo "date: $(date -u +%FT%TZ)"
  echo "machine: $(sysctl -n machdep.cpu.brand_string), $(( $(sysctl -n hw.memsize) / 1073741824 )) GiB"
  echo "macOS: $(sw_vers -productVersion)"
  echo "display: $(system_profiler SPDisplaysDataType 2>/dev/null | sed -n 's/.*Resolution: //p' | head -1)"
  echo "power: $(pmset -g batt | head -1 | sed "s/.*'\(.*\)'.*/\1/")"
  echo "rustc: $(rustc --version)"
  echo "binary: $(du -h "$BIN" | cut -f1) ($(stat -f%z "$BIN") bytes)"
  echo "latex: $( (command -v tectonic && tectonic --version) | tr '\n' ' ')"
} > "$OUT/env.txt"
cat "$OUT/env.txt"

result() { grep -o 'BENCH_RESULT .*' | sed 's/^BENCH_RESULT //'; }

# cputime "M:SS.ss" -> seconds
cpu_seconds() { ps -o time= -p "$1" | awk -F: '{ if (NF==3) print $1*3600+$2*60+$3; else print $1*60+$2 }'; }

echo "== idle (deck A, 30 s window after 5 s settling) =="
LOG="$OUT/idle.log"
"$BIN" --bench idle --seconds 40 >"$LOG" 2>&1 &
PID=$!
until grep -q BENCH_READY "$LOG" 2>/dev/null; do sleep 0.2; done
sleep 5
C0=$(cpu_seconds "$PID"); T0=$(date +%s.%N 2>/dev/null || python3 -c 'import time;print(time.time())')
sleep 30
C1=$(cpu_seconds "$PID"); T1=$(python3 -c 'import time;print(time.time())')
FP=$(footprint "$PID" 2>/dev/null | sed -n 's/.*phys_footprint: *//p' | head -1 || true)
[ -z "$FP" ] && FP=$(footprint "$PID" 2>/dev/null | grep -i "footprint" | head -1 || true)
RSS=$(ps -o rss= -p "$PID")
wait "$PID" || true
python3 - "$C0" "$C1" "$RSS" "$FP" "$OUT" <<'PY'
import sys, json
c0, c1, rss, fp, out = float(sys.argv[1]), float(sys.argv[2]), int(sys.argv[3]), sys.argv[4], sys.argv[5]
r = json.loads(open(out + "/idle.log").read().split("BENCH_RESULT ")[1].splitlines()[0])
r["idle_cpu_percent_of_one_core"] = round((c1 - c0) / 30.0 * 100, 3)
r["idle_cpu_seconds_in_window"] = round(c1 - c0, 3)
r["rss_mib_external"] = round(rss / 1024, 1)
r["footprint"] = fp.strip()
json.dump(r, open(out + "/idle.json", "w"), indent=1)
print(json.dumps(r))
PY

echo "== warm start x3 =="
for i in 1 2 3; do "$BIN" --bench idle --seconds 0 2>/dev/null | result; done | tee "$OUT/startup.jsonl"

for s in drag switch images saveload; do
  echo "== $s =="
  "$BIN" --bench "$s" --out "$OUT/$s.json" 2>"$OUT/$s.log" | result
done

echo "== smoke (end-to-end workflow) =="
"$BIN" --bench smoke --out "$OUT/smoke.json" 2>"$OUT/smoke.log" | result | python3 -c 'import json,sys; r=json.load(sys.stdin); print("passed", r["passed"], "failed", r["failed"])'

echo "== export latency (deck B) =="
cargo run --release --quiet --example export_timing -- "$OUT/export" | tee "$OUT/export.json"

echo "measurements written to $OUT"
