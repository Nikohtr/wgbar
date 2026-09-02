#!/bin/bash
# Quits and removes WGBar, its login item, its settings, the helper and the optional sudoers rule.
set -uo pipefail
HELPER=/usr/local/libexec/wgbar-helper
# An on-demand ("armed") interface left behind would black-hole the VPN subnets: take it down first.
if [[ -x "$HELPER" ]]; then
  for n in /var/run/wireguard/*.name; do
    [[ -e "$n" ]] || continue
    t="$(basename "$n" .name)"
    sudo "$HELPER" down "$t" && echo "Took down tunnel $t"
  done
fi
pkill -x WGBar 2>/dev/null || true
rm -rf ~/Applications/WGBar.app
defaults delete org.wgbar.WGBar 2>/dev/null || true
if [[ -e /etc/sudoers.d/wgbar || -e "$HELPER" ]]; then
  sudo rm -f /etc/sudoers.d/wgbar "$HELPER" && echo "Removed /etc/sudoers.d/wgbar and $HELPER"
fi
echo "WGBar removed. If a stale 'WGBar' entry remains under System Settings > General > Login Items, delete it there."
