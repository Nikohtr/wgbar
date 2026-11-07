#!/bin/bash
# Runs the Rust unit tests, then the helper tests.
set -euo pipefail
cd "$(dirname "$0")"
export PATH="$HOME/.cargo/bin:$PATH"
cargo test --quiet
tests/helper_test.sh
