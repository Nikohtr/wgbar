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
for p in /opt/homebrew/bin /usr/local/bin /opt/local/bin; do
  [[ -x "$p/wg-quick" ]] && WG_QUICK="$p/wg-quick" && break
done
if [[ -z "$WG_QUICK" ]]; then
  echo "wg-quick not found. Install WireGuard tools with:  brew install wireguard-tools" >&2
  exit 1
fi
FOUND=""
for d in /opt/homebrew/etc/wireguard /usr/local/etc/wireguard /opt/local/etc/wireguard /etc/wireguard; do
  ls "$d"/*.conf >/dev/null 2>&1 && FOUND="$d" && break
done
if [[ -z "$FOUND" ]]; then
  echo "Warning: no tunnel configs found in the usual folders — pick yours via the app's 'Config Folder…' menu item." >&2
fi

# --- build + install ---------------------------------------------------------
./build.sh
pkill -x WGBar 2>/dev/null || true
mkdir -p ~/Applications
rm -rf ~/Applications/WGBar.app
cp -R build/WGBar.app ~/Applications/WGBar.app
defaults write org.wgbar.WGBar repoDir "$PWD"   # lets "Check for Updates…" find this clone
open ~/Applications/WGBar.app
echo "Installed and launched ~/Applications/WGBar.app"
echo "Optional: run ./sudoers.sh to skip the password prompt when toggling."
