#!/bin/bash
# Installs a sudoers rule so WGBar can run "wg-quick up/down <config>" without a password prompt.
# The rule is limited to the current user and to the tunnel configs that exist right now;
# re-run this script after adding a config. Remove with:  sudo rm /etc/sudoers.d/wgbar
#
# Usage: ./sudoers.sh [config-folder]
#   The folder defaults to the one WGBar uses (its "Config Folder…" setting, else the first
#   of /opt/homebrew|/usr/local|/opt/local/etc/wireguard or /etc/wireguard that has configs).
set -euo pipefail

WG_QUICK="$(defaults read org.wgbar.WGBar wgQuick 2>/dev/null || true)"
if [[ ! -x "$WG_QUICK" ]]; then
  WG_QUICK=""
  for p in /opt/homebrew/bin /usr/local/bin /opt/local/bin; do
    [[ -x "$p/wg-quick" ]] && WG_QUICK="$p/wg-quick" && break
  done
fi
[[ -n "$WG_QUICK" ]] || { echo "wg-quick not found (brew install wireguard-tools)" >&2; exit 1; }

CONF_DIR="${1:-$(defaults read org.wgbar.WGBar confDir 2>/dev/null || true)}"
if [[ -z "$CONF_DIR" ]]; then
  for d in /opt/homebrew/etc/wireguard /usr/local/etc/wireguard /opt/local/etc/wireguard /etc/wireguard; do
    ls "$d"/*.conf >/dev/null 2>&1 && CONF_DIR="$d" && break
  done
fi
CONF_DIR="${CONF_DIR%/}"
[[ -n "$CONF_DIR" && -d "$CONF_DIR" ]] || { echo "No config folder found; pass one:  ./sudoers.sh /path/to/wireguard" >&2; exit 1; }

CMDS=()
for f in "$CONF_DIR"/*.conf; do
  [[ -e "$f" ]] || continue
  CMDS+=("$WG_QUICK up $f" "$WG_QUICK down $f")
done
[[ ${#CMDS[@]} -gt 0 ]] || { echo "No .conf files in $CONF_DIR; nothing to allow." >&2; exit 1; }

RULE="$(id -un) ALL=(root) NOPASSWD: $(printf '%s, ' "${CMDS[@]}" | sed 's/, $//')"

TMP="$(mktemp)"
trap 'rm -f "$TMP"' EXIT
{
  echo "# Installed by WGBar (sudoers.sh). Lets $(id -un) toggle WireGuard tunnels in $CONF_DIR without a password."
  echo "$RULE"
} > "$TMP"

echo "About to install this rule to /etc/sudoers.d/wgbar:"
echo "----"
cat "$TMP"
echo "----"
sudo visudo -c -q -f "$TMP"
sudo install -m 440 -o root -g wheel "$TMP" /etc/sudoers.d/wgbar
echo "Done. WGBar will now toggle the tunnels in $CONF_DIR without prompting."
