#!/bin/bash
# Builds build/WGBar.app from the Rust crate (and target/release/wgbar-helper, which sudoers.sh
# installs). Run install.sh to build + install + launch.
set -euo pipefail
cd "$(dirname "$0")"
export PATH="$HOME/.cargo/bin:$PATH"
command -v cargo >/dev/null || { echo "Rust is required: https://rustup.rs (curl ... | sh), then re-run." >&2; exit 1; }
APP=build/WGBar.app
cargo build --release --bins
rm -rf build
mkdir -p "$APP/Contents/MacOS"
cp target/release/WGBar "$APP/Contents/MacOS/WGBar"
cp Info.plist "$APP/Contents/Info.plist"
codesign --force --sign - "$APP"
echo "Built $APP"
