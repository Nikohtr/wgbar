#!/bin/bash
# Quits and removes WGBar, its login item, its settings, the helper and the optional sudoers rule.
set -uo pipefail
HELPER=/usr/local/libexec/wgbar-helper
# Quit gracefully first: applicationWillTerminate restores the DNS snapshot (still in defaults at
# this point) and takes an on-demand tunnel down. SIGTERM from pkill would skip that hook.
osascript -e 'tell application "WGBar" to quit' 2>/dev/null || true
for _ in 1 2 3 4 5; do pgrep -x WGBar >/dev/null || break; sleep 1; done
pkill -x WGBar 2>/dev/null || true
# Backstop: an on-demand ("armed") interface left behind would black-hole the VPN subnets.
if [[ -x "$HELPER" ]]; then
  for n in /var/run/wireguard/*.name; do
    [[ -e "$n" ]] || continue
    t="$(basename "$n" .name)"
    sudo "$HELPER" down "$t" && echo "Took down tunnel $t"
  done
fi
rm -rf ~/Applications/WGBar.app
defaults delete org.wgbar.WGBar 2>/dev/null || true
if [[ -e /etc/sudoers.d/wgbar || -e "$HELPER" ]]; then
  sudo rm -f /etc/sudoers.d/wgbar "$HELPER" && echo "Removed /etc/sudoers.d/wgbar and $HELPER"
fi
echo "WGBar removed. If a stale 'WGBar' entry remains under System Settings > General > Login Items, delete it there."
echo "If names still resolve oddly, check System Settings ▸ Network ▸ DNS for leftover VPN servers."
