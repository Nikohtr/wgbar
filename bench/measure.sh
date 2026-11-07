#!/bin/bash
# Measures one built WGBar.app: size, launch latency, memory, threads, idle CPU and wakeups.
# Prints `key value` lines. macOS only; used by bench/compare.sh.
#
# Usage: bench/measure.sh <path/to/WGBar.app> <bin dir with launch_probe and cpu_probe> [idle-seconds]
#
# The app is measured as a copy with its own bundle id (org.wgbar.WGBar.bench), so it has its own
# defaults domain: Connect on Demand off, no login item registration. That keeps it from arming
# (or fighting over) the tunnel of an installed WGBar, which may stay running during the bench.
set -euo pipefail
SRC="$1"; BIN="$2"; IDLE="${3:-60}"
PROBE="$BIN/launch_probe"; CPU="$BIN/cpu_probe"
LAUNCHES=7
BENCH_ID=org.wgbar.WGBar.bench

WORK="$(mktemp -d /tmp/wgbar-measure.XXXXXX)"
trap 'rm -rf "$WORK"' EXIT
APP="$WORK/WGBar.app"
cp -R "$SRC" "$APP"
plutil -replace CFBundleIdentifier -string "$BENCH_ID" "$APP/Contents/Info.plist"
codesign --force --sign - "$APP" 2>/dev/null
defaults delete "$BENCH_ID" >/dev/null 2>&1 || true
defaults write "$BENCH_ID" didRegisterLoginItem -bool true
defaults write "$BENCH_ID" onDemand -bool false
EXE="$APP/Contents/MacOS/WGBar"

rss_kb() { ps -o rss= -p "$1" | tr -d ' '; }
probe() { "$CPU" "$1" | awk -v k="$2" '$1 == k { print $2 }'; }
quit() { kill "$1" 2>/dev/null || true; for _ in $(seq 50); do kill -0 "$1" 2>/dev/null || return 0; sleep 0.1; done; kill -9 "$1" 2>/dev/null || true; }

echo "binary_bytes $(stat -f %z "$SRC/Contents/MacOS/WGBar")"
echo "bundle_kb $(du -sk "$SRC" | cut -f1)"

# Launch latency: median of $LAUNCHES cold-ish starts (first one discarded as a cache warm-up).
times=()
for i in $(seq 0 "$LAUNCHES"); do
  read -r pid ms < <("$PROBE" "$EXE")
  quit "$pid"
  sleep 1
  [[ $i -eq 0 || "$ms" == timeout ]] && continue
  times+=("$ms")
done
if [[ ${#times[@]} -gt 0 ]]; then
  echo "launch_ms_median $(printf '%s\n' "${times[@]}" | sort -n | awk '{ a[NR] = $1 } END { print a[int((NR + 1) / 2)] }')"
  echo "launch_ms_min $(printf '%s\n' "${times[@]}" | sort -n | head -1)"
else
  echo "launch_ms_median timeout"
fi

# Steady state: one more launch, left idle. CPU and wakeups count only the window after 5 s, so
# launch work is excluded.
read -r pid _ < <("$PROBE" "$EXE")
sleep 5
echo "rss_kb_5s $(rss_kb "$pid")"
echo "footprint_kb_5s $(probe "$pid" footprint_kb)"
cpu0="$(probe "$pid" cpu_ns)"; wk0="$(probe "$pid" wakeups)"
sleep "$IDLE"
cpu1="$(probe "$pid" cpu_ns)"; wk1="$(probe "$pid" wakeups)"
echo "rss_kb_${IDLE}s $(rss_kb "$pid")"
echo "footprint_kb_${IDLE}s $(probe "$pid" footprint_kb)"
echo "idle_cpu_ms_per_min $(awk -v a="$cpu0" -v b="$cpu1" -v t="$IDLE" 'BEGIN { printf "%.1f", (b - a) / 1e6 * 60 / t }')"
echo "wakeups_per_min $(awk -v a="$wk0" -v b="$wk1" -v t="$IDLE" 'BEGIN { printf "%.0f", (b - a) * 60 / t }')"
echo "threads $(($(ps -M -p "$pid" | wc -l) - 1))"
echo "open_fds $(lsof -p "$pid" 2>/dev/null | tail -n +2 | wc -l | tr -d ' ')"
quit "$pid"
defaults delete "$BENCH_ID" >/dev/null 2>&1 || true
