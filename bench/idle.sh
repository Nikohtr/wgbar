#!/bin/bash
# Idle cost of two WGBar builds measured side by side: both run at the same time, so whatever
# else the Mac is doing (mouse over the menu bar, network changes) lands on both alike.
# Prints `key value` lines per app (medians over the rounds). macOS only.
#
# Usage: bench/idle.sh <a.app> <b.app> <bin dir with cpu_probe> [seconds per round] [rounds]
# Each copy gets its own bundle id and defaults (on demand off, no login item), as in measure.sh.
set -euo pipefail
A="$1"; B="$2"; BIN="$3"; SECS="${4:-60}"; ROUNDS="${5:-3}"
CPU="$BIN/cpu_probe"
WORK="$(mktemp -d /tmp/wgbar-idle.XXXXXX)"
pids=()
cleanup() { for p in "${pids[@]:-}"; do kill "$p" 2>/dev/null || true; done; rm -rf "$WORK"; }
trap cleanup EXIT

prepare() { # <src.app> <tag> -> prints the executable path
  local app="$WORK/$2/WGBar.app" id="org.wgbar.WGBar.bench.$2"
  mkdir -p "$WORK/$2"; cp -R "$1" "$app"
  plutil -replace CFBundleIdentifier -string "$id" "$app/Contents/Info.plist"
  codesign --force --sign - "$app" 2>/dev/null
  defaults delete "$id" >/dev/null 2>&1 || true
  defaults write "$id" didRegisterLoginItem -bool true
  defaults write "$id" onDemand -bool false
  echo "$app/Contents/MacOS/WGBar"
}
probe() { "$CPU" "$1" | awk -v k="$2" '$1 == k { print $2 }'; }
median() { sort -n | awk '{ a[NR] = $1 } END { print a[int((NR + 1) / 2)] }'; }

EA="$(prepare "$A" a)"; EB="$(prepare "$B" b)"
"$EA" 2>/dev/null & pa=$!; "$EB" 2>/dev/null & pb=$!; pids=("$pa" "$pb"); disown -a
sleep 10 # past launch work (the DNS check, first refresh)

for r in $(seq "$ROUNDS"); do
  ca0=$(probe "$pa" cpu_ns); wa0=$(probe "$pa" wakeups); cb0=$(probe "$pb" cpu_ns); wb0=$(probe "$pb" wakeups)
  sleep "$SECS"
  ca1=$(probe "$pa" cpu_ns); wa1=$(probe "$pa" wakeups); cb1=$(probe "$pb" cpu_ns); wb1=$(probe "$pb" wakeups)
  awk -v a="$ca0" -v b="$ca1" -v t="$SECS" 'BEGIN { printf "%.1f\n", (b - a) / 1e6 * 60 / t }' >> "$WORK/cpu_a"
  awk -v a="$cb0" -v b="$cb1" -v t="$SECS" 'BEGIN { printf "%.1f\n", (b - a) / 1e6 * 60 / t }' >> "$WORK/cpu_b"
  awk -v a="$wa0" -v b="$wa1" -v t="$SECS" 'BEGIN { printf "%.0f\n", (b - a) * 60 / t }' >> "$WORK/wk_a"
  awk -v a="$wb0" -v b="$wb1" -v t="$SECS" 'BEGIN { printf "%.0f\n", (b - a) * 60 / t }' >> "$WORK/wk_b"
  echo "round $r: a $(tail -1 "$WORK/cpu_a") ms/min $(tail -1 "$WORK/wk_a") wk/min, b $(tail -1 "$WORK/cpu_b") ms/min $(tail -1 "$WORK/wk_b") wk/min" >&2
done
echo "a_idle_cpu_ms_per_min $(median < "$WORK/cpu_a")"
echo "a_wakeups_per_min $(median < "$WORK/wk_a")"
echo "a_footprint_kb $(probe "$pa" footprint_kb)"
echo "b_idle_cpu_ms_per_min $(median < "$WORK/cpu_b")"
echo "b_wakeups_per_min $(median < "$WORK/wk_b")"
echo "b_footprint_kb $(probe "$pb" footprint_kb)"
for t in a b; do defaults delete "org.wgbar.WGBar.bench.$t" >/dev/null 2>&1 || true; done
