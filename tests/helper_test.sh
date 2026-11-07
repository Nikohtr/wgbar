#!/bin/bash
# Tests for the Rust wgbar-helper binary that need no root: config derivation and argument checks.
set -uo pipefail
cd "$(dirname "$0")/.."
cargo build --quiet --bin wgbar-helper || { echo "FAIL build"; exit 1; }
H=target/debug/wgbar-helper
export WGBAR_CONF_DIR="$PWD/tests/fixtures"
fail=0
check() { if [[ "$2" == "$3" ]]; then echo "ok   $1"; else echo "FAIL $1"; echo "  expected: $3"; echo "  actual:   $2"; fail=1; fi; }

check "print-armed drops DNS and PersistentKeepalive, keeps Endpoint" \
  "$($H print-armed sample 2>&1)" "$(cat tests/fixtures/sample.armed.conf)"

$H print-armed missing >/dev/null 2>&1; check "unknown tunnel exits 1" "$?" "1"
$H print-armed "../sample" >/dev/null 2>&1; check "path traversal is rejected" "$?" "2"
$H print-armed "a/b" >/dev/null 2>&1; check "slash in name is rejected" "$?" "2"
$H print-armed >/dev/null 2>&1; check "missing tunnel argument is a usage error" "$?" "2"
$H bogus sample >/dev/null 2>&1; check "unknown verb is a usage error" "$?" "2"
$H arm sample >/dev/null 2>&1; check "privileged verb refuses to run as a normal user" "$?" "1"
check "privileged verb says why" "$($H connect sample 2>&1 >/dev/null || true)" "wgbar-helper connect must run as root (via sudo)"

$H print-armed 0123456789abcdef >/dev/null 2>&1; check "16-char tunnel name is rejected" "$?" "2"
$H print-armed 0123456789abcde >/dev/null 2>&1; check "15-char tunnel name passes validation (config missing)" "$?" "1"
hash="$($H source-hash 2>&1)"; rc=$?
check "source-hash exits 0" "$rc" "0"
[[ "$hash" =~ ^[0-9a-f]{16}$ ]]; check "source-hash prints 16 hex chars" "$?" "0"

exit $fail
