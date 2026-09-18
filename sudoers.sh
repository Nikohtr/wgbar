#!/bin/bash
# Installs the WGBar helper and a sudoers rule so WGBar can run "wg-quick up/down <config>" and
# "wgbar-helper <verb> <tunnel>" (Connect on Demand) without a password prompt.
# The rule is limited to the current user and to "<config-folder>/*.conf": sudo matches
# arguments with fnmatch, where "*" does not cross a "/", so nothing outside that folder is
# allowed and a config added later works without re-running this script. Re-run it after
# changing the config folder or updating WGBar. Note that whoever can write a config in that
# folder can run commands as root through wg-quick's PostUp, so keep the folder to yourself.
# Remove with:
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
FOUND=0
for f in "$CONF_DIR"/*.conf; do
  [[ -e "$f" ]] || continue
  [[ -r "$f" ]] || { echo "$f is not readable by $(id -un); the helper's print-armed must not reveal configs you cannot read. Use a folder you own." >&2; exit 1; }
  FOUND=1
done
[[ $FOUND -eq 1 ]] || { echo "No .conf files in $CONF_DIR; nothing to allow." >&2; exit 1; }

# One entry for the whole folder, so a config added later needs no re-run.
GLOB="${CONF_DIR// /\\ }/*.conf"
CMDS=("$HELPER" "$WG_QUICK up $GLOB" "$WG_QUICK down $GLOB")

RULE="$(id -un) ALL=(root) NOPASSWD: $(printf '%s, ' "${CMDS[@]}" | sed 's/, $//')"

TMP="$(mktemp)"; TMP_HELPER="$(mktemp)"
trap 'rm -f "$TMP" "$TMP_HELPER"' EXIT
{
  echo "# Installed by WGBar (sudoers.sh). Lets $(id -un) toggle WireGuard tunnels in $CONF_DIR without a password."
  echo "$RULE"
} > "$TMP"
CD="$(printf '%q' "$CONF_DIR")" WQ="$(printf '%q' "$WG_QUICK")" WGB="$(printf '%q' "$WG")" \
awk '/^CONF_DIR=/ { print "CONF_DIR=" ENVIRON["CD"]; next }
     /^WG_QUICK=/ { print "WG_QUICK=" ENVIRON["WQ"]; next }
     /^WG=/       { print "WG=" ENVIRON["WGB"]; next }
     { print }' helper/wgbar-helper > "$TMP_HELPER"
bash -n "$TMP_HELPER"

echo "About to install this rule to /etc/sudoers.d/wgbar:"
echo "----"
cat "$TMP"
echo "----"
echo "and the helper script to $HELPER (config folder $CONF_DIR)."
sudo visudo -c -q -f "$TMP"
sudo install -d -m 755 -o root -g wheel /usr/local/libexec
sudo install -m 755 -o root -g wheel "$TMP_HELPER" "$HELPER"
sudo install -m 440 -o root -g wheel "$TMP" /etc/sudoers.d/wgbar
echo "Done. WGBar will now toggle the tunnels in $CONF_DIR without prompting, and Connect on Demand is available."
