#!/bin/bash
# Installs the WGBar helper and a sudoers rule so WGBar can run "wg-quick up/down <config>" and
# "wgbar-helper <verb> <tunnel>" (Connect on Demand) without a password prompt.
# The rule is limited to the current user and to the tunnel configs that exist right now;
# re-run this script after adding a config or updating WGBar. Remove with:
#   sudo rm /etc/sudoers.d/wgbar /usr/local/libexec/wgbar-helper
#
# Security: the rule lets the user run the helper with any arguments, and its print-armed verb prints a
# config (private key included), so this script refuses config files the user cannot already read.
#
# Usage: ./sudoers.sh [config-folder]
#   The folder defaults to the one WGBar uses (its "Config Folder…" setting, else the first
#   of /opt/homebrew|/usr/local|/opt/local/etc/wireguard or /etc/wireguard that has configs).
set -euo pipefail
cd "$(dirname "$0")"

WG_QUICK="$(defaults read org.wgbar.WGBar wgQuick 2>/dev/null || true)"
if [[ ! -x "$WG_QUICK" ]]; then
  WG_QUICK=""
  for p in /opt/homebrew/bin /usr/local/bin /opt/local/bin; do
    [[ -x "$p/wg-quick" ]] && WG_QUICK="$p/wg-quick" && break
  done
fi
[[ -n "$WG_QUICK" ]] || { echo "wg-quick not found (brew install wireguard-tools)" >&2; exit 1; }
WG="${WG_QUICK%/*}/wg"
[[ -x "$WG" ]] || { echo "wg not found next to wg-quick ($WG)" >&2; exit 1; }

CONF_DIR="${1:-$(defaults read org.wgbar.WGBar confDir 2>/dev/null || true)}"
if [[ -z "$CONF_DIR" ]]; then
  for d in /opt/homebrew/etc/wireguard /usr/local/etc/wireguard /opt/local/etc/wireguard /etc/wireguard; do
    ls "$d"/*.conf >/dev/null 2>&1 && CONF_DIR="$d" && break
  done
fi
CONF_DIR="${CONF_DIR%/}"
[[ -n "$CONF_DIR" && -d "$CONF_DIR" ]] || { echo "No config folder found; pass one:  ./sudoers.sh /path/to/wireguard" >&2; exit 1; }

HELPER=/usr/local/libexec/wgbar-helper
CMDS=("$HELPER")
for f in "$CONF_DIR"/*.conf; do
  [[ -e "$f" ]] || continue
  [[ -r "$f" ]] || { echo "$f is not readable by $(id -un); the helper's print-armed must not reveal configs you cannot read. Use a folder you own." >&2; exit 1; }
  CMDS+=("$WG_QUICK up ${f// /\\ }" "$WG_QUICK down ${f// /\\ }")
done
[[ ${#CMDS[@]} -gt 1 ]] || { echo "No .conf files in $CONF_DIR; nothing to allow." >&2; exit 1; }

RULE="$(id -un) ALL=(root) NOPASSWD: $(printf '%s, ' "${CMDS[@]}" | sed 's/, $//')"

TMP="$(mktemp)"
trap 'rm -f "$TMP"' EXIT
{
  echo "# Installed by WGBar (sudoers.sh). Lets $(id -un) toggle WireGuard tunnels in $CONF_DIR without a password."
  echo "$RULE"
} > "$TMP"
# Build the helper with the three paths baked in at compile time (see build.rs).
WGBAR_BUILD_CONF_DIR="$CONF_DIR" WGBAR_BUILD_WG_QUICK="$WG_QUICK" WGBAR_BUILD_WG="$WG" \
  cargo build --release --bin wgbar-helper
BUILT=target/release/wgbar-helper

echo "About to install this rule to /etc/sudoers.d/wgbar:"
echo "----"
cat "$TMP"
echo "----"
echo "and the helper binary to $HELPER (config folder $CONF_DIR)."
sudo visudo -c -q -f "$TMP"
sudo install -d -m 755 -o root -g wheel /usr/local/libexec
sudo install -m 755 -o root -g wheel "$BUILT" "$HELPER"
sudo install -m 440 -o root -g wheel "$TMP" /etc/sudoers.d/wgbar
echo "Done. WGBar will now toggle the tunnels in $CONF_DIR without prompting, and Connect on Demand is available."
