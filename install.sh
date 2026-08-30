#!/bin/bash
# Builds WGBar, installs it to ~/Applications and (re)launches it.
set -euo pipefail
cd "$(dirname "$0")"

# --- prerequisites -----------------------------------------------------------
if ! xcode-select -p >/dev/null 2>&1; then
  echo "Xcode Command Line Tools are required. Install them with:  xcode-select --install" >&2
  exit 1
fi
WG_QUICK=""
for p in /opt/homebrew /usr/local; do
  [[ -x "$p/bin/wg-quick" ]] && WG_QUICK="$p/bin/wg-quick" && break
done
if [[ -z "$WG_QUICK" ]]; then
  echo "wg-quick not found. Install WireGuard tools with:  brew install wireguard-tools" >&2
  exit 1
fi
CONF_DIR="$(dirname "$(dirname "$WG_QUICK")")/etc/wireguard"
if ! ls "$CONF_DIR"/*.conf >/dev/null 2>&1; then
  echo "Warning: no tunnel configs found in $CONF_DIR — WGBar will have nothing to toggle until you add one." >&2
fi

# --- build + install ---------------------------------------------------------
./build.sh
pkill -x WGBar 2>/dev/null || true
mkdir -p ~/Applications
rm -rf ~/Applications/WGBar.app
cp -R build/WGBar.app ~/Applications/WGBar.app
open ~/Applications/WGBar.app
echo "Installed and launched ~/Applications/WGBar.app"
echo "Optional: run ./sudoers.sh to skip the password prompt when toggling."
