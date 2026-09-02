#!/bin/bash
# Builds build/WGBar.app from main.swift + DNSGuard.swift. Run install.sh to build + install + launch.
set -euo pipefail
cd "$(dirname "$0")"
APP=build/WGBar.app
rm -rf build
mkdir -p "$APP/Contents/MacOS"
swiftc -O -framework Cocoa -framework ServiceManagement -o "$APP/Contents/MacOS/WGBar" main.swift DNSGuard.swift
cp Info.plist "$APP/Contents/Info.plist"
codesign --force --sign - "$APP"
echo "Built $APP"
