#!/bin/bash
# Tests for helper/wgbar-helper that need no root: config derivation and argument checks.
set -uo pipefail
cd "$(dirname "$0")/.."
H=helper/wgbar-helper
export WGBAR_CONF_DIR="$PWD/tests/fixtures"
fail=0
check() { if [[ "$2" == "$3" ]]; then echo "ok   $1"; else echo "FAIL $1"; echo "  expected: $3"; echo "  actual:   $2"; fail=1; fi; }

check "print-armed drops DNS, Endpoint and PersistentKeepalive" \
  "$($H print-armed sample 2>&1)" "$(cat tests/fixtures/sample.armed.conf)"

check "print-public keeps what WGBar reads and drops the key material" \
  "$($H print-public sample 2>&1)" "$(cat tests/fixtures/sample.public.conf)"

check "print-public leaves no key line at all" \
  "$($H print-public sample 2>/dev/null | grep -ci 'privatekey\|presharedkey')" "0"

$H print-public "../sample" >/dev/null 2>&1; check "print-public rejects path traversal" "$?" "2"

$H print-armed missing >/dev/null 2>&1; check "unknown tunnel exits 1" "$?" "1"
$H print-armed "../sample" >/dev/null 2>&1; check "path traversal is rejected" "$?" "2"
$H print-armed "a/b" >/dev/null 2>&1; check "slash in name is rejected" "$?" "2"
$H print-armed >/dev/null 2>&1; check "missing tunnel argument is a usage error" "$?" "2"
$H bogus sample >/dev/null 2>&1; check "unknown verb is a usage error" "$?" "2"
$H arm sample >/dev/null 2>&1; check "privileged verb refuses to run as a normal user" "$?" "1"
check "privileged verb says why" "$($H connect sample 2>&1 >/dev/null || true)" "wgbar-helper connect must run as root (via sudo)"

exit $fail
