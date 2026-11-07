#!/bin/bash
# Swift baseline vs Rust port, on this Mac. Builds the Swift app from a git ref (default `main`)
# in a temporary worktree and the Rust app from this checkout, measures both the same way, and
# writes bench/results/<date>-<host>.md.
#
# Usage: bench/compare.sh [swift-ref] [idle-seconds]
#        QUICK=1 bench/compare.sh ...   # skip the clean build_s / test_s timings
#
# An installed WGBar may keep running (even with Connect on Demand armed): the bench copies run
# under their own bundle id and defaults domain (see measure.sh), so they never touch its tunnel.
# Both copies briefly add a second icon to the menu bar.
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"
REF="${1:-main}"; IDLE="${2:-60}"
ROOT="$PWD"
WORK="$(mktemp -d /tmp/wgbar-bench.XXXXXX)"
OUT="${OUT:-bench/results/$(date +%Y-%m-%d)-$(hostname -s).md}"

[[ "$(uname)" == Darwin ]] || { echo "macOS only" >&2; exit 1; }
command -v swiftc >/dev/null || { echo "swiftc missing: xcode-select --install" >&2; exit 1; }
command -v cargo >/dev/null || { echo "cargo missing: https://rustup.rs" >&2; exit 1; }
QUICK="${QUICK:-0}"

cleanup() {
  git -C "$ROOT" worktree remove --force "$WORK/swift" 2>/dev/null || true
  rm -rf "$WORK"
}
trap cleanup EXIT

val() { awk -v k="$2" '$1 == k { v = $2 } END { print (v == "" ? "—" : v) }' "$1"; }
secs() { local t0 t1; t0=$(perl -MTime::HiRes=time -e 'print time'); "$@" >/dev/null 2>&1; t1=$(perl -MTime::HiRes=time -e 'print time'); awk -v a="$t0" -v b="$t1" 'BEGIN { printf "%.1f", b - a }'; }

echo "== tools"
TOOLS="$WORK/tools"
build_tools() {
  cargo build --release --quiet --example launch_probe --example cpu_probe --example logic_bench
  mkdir -p "$TOOLS"
  cp target/release/examples/{launch_probe,cpu_probe,logic_bench} "$TOOLS/"
}
build_tools

echo "== Swift ($REF)"
git worktree add --quiet --detach "$WORK/swift" "$REF"
SW="$WORK/swift.txt"; RS="$WORK/rust.txt"   # `key value` lines (bash 3.2: no associative arrays)
echo "build_s $(cd "$WORK/swift" && secs ./build.sh)" >> "$SW"
[[ $QUICK == 1 ]] || echo "test_s $(cd "$WORK/swift" && secs ./test.sh)" >> "$SW"
swiftc -O -o "$WORK/LogicBench" "$ROOT/bench/LogicBench.swift" "$WORK/swift/OnDemand.swift" "$WORK/swift/DNSGuard.swift"
"$WORK/LogicBench" bench/lsof-sample.txt tests/fixtures/sample.conf >> "$SW"
bench/measure.sh "$WORK/swift/build/WGBar.app" "$TOOLS" "$IDLE" >> "$SW"

echo "== Rust ($(git rev-parse --short HEAD))"
if [[ $QUICK == 1 ]]; then
  ./build.sh >/dev/null
else
  cargo clean --quiet
  echo "build_s $(secs ./build.sh)" >> "$RS"
  echo "test_s $(secs ./test.sh)" >> "$RS"
fi
"$TOOLS/logic_bench" bench/lsof-sample.txt tests/fixtures/sample.conf >> "$RS"
bench/measure.sh build/WGBar.app "$TOOLS" "$IDLE" >> "$RS"

mkdir -p bench/results
{
  echo "# WGBar benchmark — $(date '+%Y-%m-%d %H:%M')"
  echo
  echo "Mac: $(sysctl -n hw.model), $(sysctl -n machdep.cpu.brand_string), macOS $(sw_vers -productVersion)."
  echo "Swift: \`$REF\` ($(git rev-parse --short "$REF")), $(swiftc --version 2>&1 | head -1)."
  echo "Rust: \`$(git rev-parse --abbrev-ref HEAD)\` ($(git rev-parse --short HEAD)), $(rustc --version)."
  echo "Idle window: ${IDLE}s. Logic timings are ns per call (median of 7)."
  echo
  echo "| metric | Swift | Rust |"
  echo "|---|---:|---:|"
  for k in launch_ms_median launch_ms_min rss_kb_5s "rss_kb_${IDLE}s" footprint_kb_5s "footprint_kb_${IDLE}s" \
           idle_cpu_ms_per_min wakeups_per_min threads open_fds \
           binary_bytes bundle_kb build_s test_s parse_lsof_400 vpn_bound_400 poll_tick_400 parse_config \
           poll_live_ns poll_live_cpu_ns poll_lsof_ns poll_lsof_cpu_ns; do
    echo "| $k | $(val "$SW" "$k") | $(val "$RS" "$k") |"
  done
} | tee "$OUT"
echo "Wrote $OUT"
