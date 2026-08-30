#!/bin/bash
# Installs a sudoers rule so WGBar can run "wg-quick up/down <tunnel>" without a password prompt.
# The rule is limited to the current user and to the tunnels that exist right now;
# re-run this script after adding a new tunnel config. Remove with:  sudo rm /etc/sudoers.d/wgbar
set -euo pipefail

WG_QUICK=""
for p in /opt/homebrew /usr/local; do
  [[ -x "$p/bin/wg-quick" ]] && WG_QUICK="$p/bin/wg-quick" && break
done
[[ -n "$WG_QUICK" ]] || { echo "wg-quick not found (brew install wireguard-tools)" >&2; exit 1; }
CONF_DIR="$(dirname "$(dirname "$WG_QUICK")")/etc/wireguard"

TUNNELS=()
for f in "$CONF_DIR"/*.conf; do
  [[ -e "$f" ]] || continue
  TUNNELS+=("$(basename "${f%.conf}")")
done
[[ ${#TUNNELS[@]} -gt 0 ]] || { echo "No tunnel configs in $CONF_DIR; nothing to allow." >&2; exit 1; }

CMDS=()
for t in "${TUNNELS[@]}"; do
  CMDS+=("$WG_QUICK up $t" "$WG_QUICK down $t")
done
RULE="$(id -un) ALL=(root) NOPASSWD: $(IFS=,; printf '%s' "${CMDS[*]/#/ }" | sed 's/^ //; s/, */, /g')"

TMP="$(mktemp)"
trap 'rm -f "$TMP"' EXIT
{
  echo "# Installed by WGBar (sudoers.sh). Lets $(id -un) toggle WireGuard tunnels without a password."
  echo "$RULE"
} > "$TMP"

echo "About to install this rule to /etc/sudoers.d/wgbar:"
echo "----"
cat "$TMP"
echo "----"
sudo visudo -c -q -f "$TMP"
sudo install -m 440 -o root -g wheel "$TMP" /etc/sudoers.d/wgbar
echo "Done. WGBar will now toggle ${TUNNELS[*]} without prompting."
