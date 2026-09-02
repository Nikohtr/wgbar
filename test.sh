#!/bin/bash
# Compiles and runs the DNSGuard unit tests (no Xcode project needed).
set -euo pipefail
cd "$(dirname "$0")"
mkdir -p build/tests
swiftc -o build/tests/DNSGuardTests DNSGuard.swift tests/DNSGuardTests.swift
build/tests/DNSGuardTests
