#!/bin/bash
# Swift baseline vs Rust port, on this Mac. Builds the Swift app from a git ref (default `main`)
# in a temporary worktree and the Rust app from this checkout, measures both the same way, and
# writes bench/results/<date>-<host>.md.
#
# Usage: bench/compare.sh [swift-ref] [idle-seconds]
#
# While it runs, your installed WGBar is quit (gracefully) and the bench copies take its place in
# the menu bar; it is relaunched at the end. Turn off Connect on Demand first: the bench stops
# copies with SIGTERM, which would leave an armed interface behind.
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"
REF="${1:-main}"; IDLE="${2:-60}"
ROOT="$PWD"
WORK="$(mktemp -d /tmp/wgbar-bench.XXXXXX)"
OUT="bench/results/$(date +%Y-%m-%d)-$(hostname -s).md"

[[ "$(uname)" == Darwin ]] || { echo "macOS only" >&2; exit 1; }
command -v swiftc >/dev/null || { echo "swiftc missing: xcode-select --install" >&2; exit 1; }
command -v cargo >/dev/null || { echo "cargo missing: https://rustup.rs" >&2; exit 1; }
if [[ "$(defaults read org.wgbar.WGBar onDemand 2>/dev/null || echo 0)" == 1 ]]; then
  echo "Connect on Demand is on. Turn it off in the WGBar menu, then re-run." >&2; exit 1
fi

relaunch=0
if pgrep -x WGBar >/dev/null; then
  relaunch=1
  osascript -e 'tell application "WGBar" to quit' || true
  for _ in 1 2 3 4 5; do pgrep -x WGBar >/dev/null || break; sleep 1; done
fi
cleanup() {
  git -C "$ROOT" worktree remove --force "$WORK/swift" 2>/dev/null || true
  rm -rf "$WORK"
  [[ $relaunch == 1 && -d ~/Applications/WGBar.app ]] && open ~/Applications/WGBar.app || true
}
trap cleanup EXIT

val() { awk -v k="$2" '$1 == k { v = $2 } END { print (v == "" ? "—" : v) }' "$1"; }
secs() { local t0 t1; t0=$(perl -MTime::HiRes=time -e 'print time'); "$@" >/dev/null 2>&1; t1=$(perl -MTime::HiRes=time -e 'print time'); awk -v a="$t0" -v b="$t1" 'BEGIN { printf "%.1f", b - a }'; }

echo "== tools"
cargo build --release --quiet --example launch_probe --example logic_bench
PROBE="$ROOT/target/release/examples/launch_probe"

echo "== Swift ($REF)"
git worktree add --quiet --detach "$WORK/swift" "$REF"
SW="$WORK/swift.txt"; RS="$WORK/rust.txt"   # `key value` lines (bash 3.2: no associative arrays)
echo "build_s $(cd "$WORK/swift" && secs ./build.sh)" >> "$SW"
echo "test_s $(cd "$WORK/swift" && secs ./test.sh)" >> "$SW"
swiftc -O -o "$WORK/LogicBench" "$ROOT/bench/LogicBench.swift" "$WORK/swift/OnDemand.swift" "$WORK/swift/DNSGuard.swift"
"$WORK/LogicBench" bench/lsof-sample.txt tests/fixtures/sample.conf >> "$SW"
bench/measure.sh "$WORK/swift/build/WGBar.app" "$PROBE" "$IDLE" >> "$SW"

echo "== Rust ($(git rev-parse --short HEAD))"
cargo clean --quiet
echo "build_s $(secs ./build.sh)" >> "$RS"
echo "test_s $(secs ./test.sh)" >> "$RS"
cargo build --release --quiet --example launch_probe --example logic_bench
target/release/examples/logic_bench bench/lsof-sample.txt tests/fixtures/sample.conf >> "$RS"
bench/measure.sh build/WGBar.app "$PROBE" "$IDLE" >> "$RS"

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
  for k in launch_ms_median launch_ms_min rss_kb_5s "rss_kb_${IDLE}s" idle_cpu_ms_per_min threads open_fds \
           binary_bytes bundle_kb build_s test_s parse_lsof_400 vpn_bound_400 poll_tick_400 parse_config; do
    echo "| $k | $(val "$SW" "$k") | $(val "$RS" "$k") |"
  done
} | tee "$OUT"
echo "Wrote $OUT"
