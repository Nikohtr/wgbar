#!/bin/bash
# Measures one built WGBar.app: size, launch latency (until its menu bar icon exists), memory,
# threads and idle CPU. Prints `key value` lines. macOS only; used by bench/compare.sh.
#
# Usage: bench/measure.sh <path/to/WGBar.app> <path/to/launch_probe> [idle-seconds]
set -euo pipefail
APP="$1"; PROBE="$2"; IDLE="${3:-60}"
EXE="$APP/Contents/MacOS/WGBar"
LAUNCHES=7

# ps prints cputime as [[hh:]mm:]ss.cc; convert to seconds.
cpu_seconds() { ps -o time= -p "$1" | awk -F: '{ s = 0; for (i = 1; i <= NF; i++) s = s * 60 + $i; print s }'; }
rss_kb() { ps -o rss= -p "$1" | tr -d ' '; }
quit() { kill "$1" 2>/dev/null || true; for _ in $(seq 50); do kill -0 "$1" 2>/dev/null || return 0; sleep 0.1; done; kill -9 "$1" 2>/dev/null || true; }

echo "binary_bytes $(stat -f %z "$EXE")"
echo "bundle_kb $(du -sk "$APP" | cut -f1)"

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

# Steady state: one more launch, left idle.
read -r pid _ < <("$PROBE" "$EXE")
sleep 5
echo "rss_kb_5s $(rss_kb "$pid")"
cpu0="$(cpu_seconds "$pid")"
sleep "$IDLE"
cpu1="$(cpu_seconds "$pid")"
echo "rss_kb_${IDLE}s $(rss_kb "$pid")"
echo "idle_cpu_ms_per_min $(awk -v a="$cpu0" -v b="$cpu1" -v t="$IDLE" 'BEGIN { printf "%.0f", (b - a) * 1000 * 60 / t }')"
echo "threads $(($(ps -M -p "$pid" | wc -l) - 1))"
echo "open_fds $(lsof -p "$pid" 2>/dev/null | tail -n +2 | wc -l | tr -d ' ')"
quit "$pid"
