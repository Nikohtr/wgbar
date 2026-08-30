#!/bin/bash
# Quits and removes WGBar, its login item, its settings, and the optional sudoers rule.
set -uo pipefail
pkill -x WGBar 2>/dev/null || true
rm -rf ~/Applications/WGBar.app
defaults delete org.wgbar.WGBar 2>/dev/null || true
if [[ -e /etc/sudoers.d/wgbar ]]; then
  sudo rm -f /etc/sudoers.d/wgbar && echo "Removed /etc/sudoers.d/wgbar"
fi
echo "WGBar removed. If a stale 'WGBar' entry remains under System Settings > General > Login Items, delete it there."
