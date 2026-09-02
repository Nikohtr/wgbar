#!/bin/bash
# Compiles and runs the unit tests (no XCTest needed), then the helper script tests.
set -euo pipefail
cd "$(dirname "$0")"
mkdir -p build/tests
swiftc -o build/tests/DNSGuardTests DNSGuard.swift Updater.swift OnDemand.swift tests/DNSGuardTests.swift tests/OnDemandTests.swift
build/tests/DNSGuardTests
