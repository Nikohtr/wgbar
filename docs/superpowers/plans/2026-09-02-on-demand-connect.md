# On-Demand Connect Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Keep the selected WireGuard tunnel "armed" (interface and routes up, peer without endpoint) and connect it automatically when TCP traffic to its `AllowedIPs` appears, disconnecting after the traffic stops, so an RDP session through the Windows App never sees a failed first attempt.

**Architecture:** A root-owned bash helper (`wgbar-helper`) does the privileged `wg-quick`/`wg` work behind a sudoers rule. A new `OnDemand.swift` holds pure parsing (netstat, CIDR, AllowedIPs), a pure state-decision function, and an `OnDemandController` that polls `netstat` once a second and calls the helper. `main.swift` grows a "Connect on Demand" menu checkbox, a third icon state, and lifecycle hooks (launch, wake, quit, tunnel change). DNS is applied and restored by WGBar itself via the existing `DNSGuard`.

**Tech Stack:** Swift 5 (Cocoa, Foundation, `inet_pton`), bash, `wg-quick`/`wg` from Homebrew `wireguard-tools`, `netstat`, `networksetup`, sudoers. No Xcode project: `swiftc` via `build.sh` / `test.sh`.

**Spec:** `docs/superpowers/specs/2026-09-02-on-demand-design.md`

## Global Constraints

- macOS 13+ (`LSMinimumSystemVersion` 13.0); no dependencies beyond Cocoa.
- All Swift goes through `swiftc` in `build.sh` and `test.sh`; every new `.swift` file must be added to both.
- Tests use the existing harness in `tests/DNSGuardTests.swift`: global `expect(actual, expected, name)`, one `@main`, exit code 1 on any failure. New test files define a `func run<Area>Tests()` called from that `main`.
- Helper path is exactly `/usr/local/libexec/wgbar-helper`; helper verbs are exactly `arm`, `connect`, `disconnect`, `down`, `status`, `print-armed`, each taking a tunnel name.
- Defaults keys: `onDemand` (bool), `onDemandIdle` (seconds, default 30). Existing keys (`tunnel`, `confDir`, `wgQuick`, `dnsSnapshot`, `repoDir`) unchanged.
- Armed config = real config minus lines whose key is `DNS`, `Endpoint`, or `PersistentKeepalive` (case-insensitive).
- Menu copy: checkbox title `Connect on Demand`; full-tunnel tooltip `Not available for full-tunnel configs (AllowedIPs includes 0.0.0.0/0)`; missing-helper alert `On-Demand needs the WGBar helper.\nRun ./sudoers.sh in the WGBar folder and try again.`
- Icon symbols: connected `shield.fill`, armed/paused `shield`, off `shield.slash`, busy `shield.lefthalf.filled`, DNS issue `exclamationmark.shield`.
- Commit messages: short imperative title like the existing history, body optional, and end with:
  ```
  Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01FPsguthtoFfkwpurrc56gm
  ```
- Never run `sudo` non-interactively in tests. Helper tests exercise only `print-armed` and argument validation as the normal user.

## File structure

| File | Responsibility |
|---|---|
| `OnDemand.swift` (new) | `TCPSocket`, `parseNetstat`, `CIDR`, `ipBytes`, `parseAllowedIPs`, `isFullTunnel`, `vpnBound`, `OnDemandState`, `OnDemandAction`, `decide`, `OnDemandController` |
| `DNSGuard.swift` | add `configDNSSearch(_:)` and `DNSGuard.apply(servers:search:)` |
| `main.swift` | menu item, icon/status mapping, timers, lifecycle glue, `anyTunnelConnected` |
| `helper/wgbar-helper` (new) | privileged verbs; installed by `sudoers.sh` |
| `sudoers.sh`, `install.sh`, `uninstall.sh` | install/refresh/remove the helper and its rule |
| `build.sh`, `test.sh` | compile the new file; run helper tests |
| `tests/OnDemandTests.swift` (new), `tests/DNSGuardTests.swift` | unit tests |
| `tests/helper_test.sh` (new), `tests/fixtures/sample.conf`, `tests/fixtures/sample.armed.conf` (new) | helper tests |
| `README.md` | "Connect on demand" section |

---

### Task 1: Socket and CIDR parsing

**Files:**
- Create: `OnDemand.swift`
- Create: `tests/OnDemandTests.swift`
- Modify: `tests/DNSGuardTests.swift` (call the new test function before the summary line)
- Modify: `test.sh`, `build.sh` (add `OnDemand.swift`)

**Interfaces:**
- Produces:
  ```swift
  struct TCPSocket: Equatable { let state: String; let remoteIP: String; let remotePort: Int }
  func parseNetstat(_ output: String) -> [TCPSocket]
  func ipBytes(_ s: String) -> [UInt8]?            // 4 bytes for IPv4, 16 for IPv6, nil otherwise
  struct CIDR: Equatable { let network: [UInt8]; let prefix: Int; init?(_ text: String); func contains(_ ip: String) -> Bool }
  func parseAllowedIPs(_ configText: String) -> [CIDR]
  func isFullTunnel(_ cidrs: [CIDR]) -> Bool
  func vpnBound(_ sockets: [TCPSocket], _ allowed: [CIDR]) -> [TCPSocket]   // SYN_SENT/ESTABLISHED inside allowed
  ```

- [ ] **Step 1: Wire the new files into the scripts**

`test.sh` becomes:

```bash
#!/bin/bash
# Compiles and runs the unit tests (no XCTest needed), then the helper script tests.
set -euo pipefail
cd "$(dirname "$0")"
mkdir -p build/tests
swiftc -o build/tests/DNSGuardTests DNSGuard.swift Updater.swift OnDemand.swift tests/DNSGuardTests.swift tests/OnDemandTests.swift
build/tests/DNSGuardTests
```

(Task 4 appends the helper test line.) In `build.sh` change the `swiftc` line to:

```bash
swiftc -O -framework Cocoa -framework ServiceManagement -o "$APP/Contents/MacOS/WGBar" main.swift DNSGuard.swift Updater.swift OnDemand.swift
```

and its header comment to mention `OnDemand.swift`.

- [ ] **Step 2: Write the failing tests**

Create `tests/OnDemandTests.swift`:

```swift
// Tests for OnDemand.swift. Called from tests/DNSGuardTests.swift's main (shared `expect` harness).
import Foundation

func runOnDemandTests() {
    // --- netstat -----------------------------------------------------------
    let netstat = """
    Active Internet connections
    Proto Recv-Q Send-Q  Local Address          Foreign Address        (state)
    tcp4       0      0  10.42.66.9.60655       10.42.1.9.3389         ESTABLISHED
    tcp4       0      0  10.5.102.162.60773     3.233.158.111.443      ESTABLISHED
    tcp4       0      0  10.42.66.9.60700       10.42.1.63.3389        SYN_SENT
    tcp4       0      0  10.5.102.162.60760     160.79.104.10.443      TIME_WAIT
    tcp6       0      0  fe80::1%lo0.1024       fe80::1%lo0.1025       ESTABLISHED
    tcp4       0      0  *.5000                 *.*                    LISTEN
    tcp46      0      0  *.7000                 *.*                    LISTEN
    """
    let sockets = parseNetstat(netstat)
    expect(sockets.count, 5, "netstat: header and LISTEN lines skipped")
    expect(sockets.first, TCPSocket(state: "ESTABLISHED", remoteIP: "10.42.1.9", remotePort: 3389), "netstat: ip and port split at the last dot")
    expect(sockets[2], TCPSocket(state: "SYN_SENT", remoteIP: "10.42.1.63", remotePort: 3389), "netstat: SYN_SENT parsed")
    expect(sockets[4], TCPSocket(state: "ESTABLISHED", remoteIP: "fe80::1", remotePort: 1025), "netstat: IPv6 scope id stripped")
    expect(parseNetstat(""), [], "netstat: empty output")

    // --- CIDR ----------------------------------------------------------------
    expect(ipBytes("10.0.0.22"), [10, 0, 0, 22], "ip: v4 bytes")
    expect(ipBytes("::1")?.count, 16, "ip: v6 is 16 bytes")
    expect(ipBytes("office.example"), nil, "ip: hostname is not an address")
    let lan = CIDR("10.0.0.0/24")!, wide = CIDR("10.42.0.0/16")!
    expect(lan.contains("10.0.0.1"), true, "cidr: first host in /24")
    expect(lan.contains("10.0.0.255"), true, "cidr: last address in /24")
    expect(lan.contains("10.0.1.0"), false, "cidr: next network is outside /24")
    expect(wide.contains("10.42.1.9"), true, "cidr: /16 match")
    expect(wide.contains("10.43.0.1"), false, "cidr: /16 miss")
    expect(CIDR("10.42.1.9")!.contains("10.42.1.9"), true, "cidr: bare address is a /32")
    expect(CIDR("10.42.1.9")!.contains("10.42.1.8"), false, "cidr: /32 excludes neighbours")
    expect(CIDR("fd00::/8")!.contains("fd00:1::5"), true, "cidr: v6 match")
    expect(CIDR("fd00::/8")!.contains("10.0.0.1"), false, "cidr: v4 never matches a v6 network")
    expect(CIDR("10.0.0.0/33"), nil, "cidr: prefix too long is rejected")
    expect(CIDR("nonsense/8"), nil, "cidr: bad address is rejected")

    // --- AllowedIPs ------------------------------------------------------------
    let conf = "[Interface]\nAddress = 10.42.66.9/24\nDNS = 10.0.0.22\n\n[Peer]\nAllowedIPs = 10.0.0.0/24,10.42.0.0/16\nallowedips = fd00::/8\n"
    expect(parseAllowedIPs(conf), [CIDR("10.0.0.0/24")!, CIDR("10.42.0.0/16")!, CIDR("fd00::/8")!], "allowedips: comma list and repeated key, case-insensitive")
    expect(parseAllowedIPs("[Interface]\nAddress = 10.0.0.2/32\n"), [], "allowedips: none")
    expect(isFullTunnel([CIDR("10.0.0.0/24")!]), false, "full tunnel: split config is not full")
    expect(isFullTunnel([CIDR("0.0.0.0/0")!]), true, "full tunnel: default route")
    expect(isFullTunnel([CIDR("::/0")!]), true, "full tunnel: v6 default route")
    expect(isFullTunnel([CIDR("0.0.0.0/1")!, CIDR("128.0.0.0/1")!]), true, "full tunnel: two halves count as full")

    // --- filter ------------------------------------------------------------
    let bound = vpnBound(sockets, [lan, wide])
    expect(bound.map(\.remoteIP), ["10.42.1.9", "10.42.1.63"], "vpnBound: only SYN_SENT/ESTABLISHED inside AllowedIPs")
    expect(vpnBound(sockets, [CIDR("192.168.0.0/16")!]), [], "vpnBound: nothing inside")
}
```

In `tests/DNSGuardTests.swift`, insert `runOnDemandTests()` on the line before `print("\(passes) passed, \(failures) failed")`.

- [ ] **Step 3: Run tests to verify they fail**

Run: `./test.sh`
Expected: compile error, `cannot find 'parseNetstat' in scope` (and friends).

- [ ] **Step 4: Implement `OnDemand.swift` parsing**

Create `OnDemand.swift`:

```swift
// OnDemand — keeps a split-tunnel WireGuard interface "armed" (up, routes installed, peer without
// endpoint) and connects it when TCP traffic to its AllowedIPs appears, disconnecting when the
// traffic stops. Design: docs/superpowers/specs/2026-09-02-on-demand-design.md
//
// Pure functions here are covered by tests/OnDemandTests.swift (./test.sh).

import Foundation

// MARK: Sockets

/// One line of `netstat -n -p tcp`: the connection state and the remote end.
struct TCPSocket: Equatable {
    let state: String
    let remoteIP: String
    let remotePort: Int
}

/// Parses `netstat -n -p tcp` output. Header, LISTEN (`*.*`) and malformed lines are skipped.
func parseNetstat(_ output: String) -> [TCPSocket] {
    var result: [TCPSocket] = []
    for line in output.split(separator: "\n") {
        let f = line.split(separator: " ", omittingEmptySubsequences: true).map(String.init)
        guard f.count >= 6, f[0].hasPrefix("tcp") else { continue }
        let remote = f[4]
        guard let dot = remote.lastIndex(of: "."), let port = Int(remote[remote.index(after: dot)...]) else { continue }
        var ip = String(remote[..<dot])
        if let pct = ip.firstIndex(of: "%") { ip = String(ip[..<pct]) }   // fe80::1%lo0
        result.append(TCPSocket(state: f[5], remoteIP: ip, remotePort: port))
    }
    return result
}

// MARK: Addresses

/// Raw bytes of an IPv4 (4) or IPv6 (16) address; nil for anything else (hostnames included).
func ipBytes(_ s: String) -> [UInt8]? {
    var v4 = in_addr()
    if inet_pton(AF_INET, s, &v4) == 1 { return withUnsafeBytes(of: &v4) { Array($0) } }
    var v6 = in6_addr()
    if inet_pton(AF_INET6, s, &v6) == 1 { return withUnsafeBytes(of: &v6) { Array($0) } }
    return nil
}

private func masked(_ bytes: [UInt8], prefix: Int) -> [UInt8] {
    bytes.enumerated().map { i, byte in
        let bits = max(0, min(8, prefix - i * 8))
        return bits == 0 ? 0 : byte & UInt8(truncatingIfNeeded: 0xFF << (8 - bits))
    }
}

/// A network in CIDR notation (`10.0.0.0/24`, `fd00::/8`); a bare address is a single host.
struct CIDR: Equatable {
    let network: [UInt8]
    let prefix: Int

    init?(_ text: String) {
        let parts = text.split(separator: "/", maxSplits: 1).map(String.init)
        guard let addr = ipBytes(parts[0]) else { return nil }
        let maxBits = addr.count * 8
        guard let prefix = parts.count == 2 ? Int(parts[1]) : maxBits, (0...maxBits).contains(prefix) else { return nil }
        self.network = masked(addr, prefix: prefix)
        self.prefix = prefix
    }

    func contains(_ ip: String) -> Bool {
        guard let bytes = ipBytes(ip), bytes.count == network.count else { return false }
        return masked(bytes, prefix: prefix) == network
    }
}

/// Every network listed on `AllowedIPs =` lines of a wg-quick config.
func parseAllowedIPs(_ configText: String) -> [CIDR] {
    var result: [CIDR] = []
    for line in configText.split(separator: "\n") {
        let parts = line.split(separator: "=", maxSplits: 1).map { $0.trimmingCharacters(in: .whitespaces) }
        guard parts.count == 2, parts[0].lowercased() == "allowedips" else { continue }
        for entry in parts[1].split(separator: ",") {
            if let cidr = CIDR(entry.trimmingCharacters(in: .whitespaces)) { result.append(cidr) }
        }
    }
    return result
}

/// A config that routes (nearly) everything: arming it would black-hole all traffic.
func isFullTunnel(_ cidrs: [CIDR]) -> Bool { cidrs.contains { $0.prefix <= 1 } }

/// Connections that are being opened or are open to an address inside the tunnel.
func vpnBound(_ sockets: [TCPSocket], _ allowed: [CIDR]) -> [TCPSocket] {
    sockets.filter { s in
        (s.state == "SYN_SENT" || s.state == "ESTABLISHED") && allowed.contains { $0.contains(s.remoteIP) }
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `./test.sh`
Expected: last line `N passed, 0 failed` with N larger than before (the DNSGuard and Updater tests still pass).

- [ ] **Step 6: Confirm the app still builds**

Run: `./build.sh`
Expected: `Built build/WGBar.app`.

- [ ] **Step 7: Commit**

```bash
git add OnDemand.swift tests/OnDemandTests.swift tests/DNSGuardTests.swift test.sh build.sh
git commit -m "Parse netstat sockets and AllowedIPs networks for on-demand connect"
```

---

### Task 2: State decision function

**Files:**
- Modify: `OnDemand.swift` (append)
- Modify: `tests/OnDemandTests.swift` (append inside `runOnDemandTests`)

**Interfaces:**
- Produces:
  ```swift
  enum OnDemandState: String { case off, armed, connected, paused }
  enum OnDemandAction: Equatable { case connect, disconnect, resumeArmed }
  func decide(state: OnDemandState, sticky: Bool, hasSockets: Bool, lastSeen: Date, now: Date, idle: TimeInterval) -> OnDemandAction?
  ```
  `lastSeen` is the last moment VPN-bound sockets were observed, or the moment of the last connect/disconnect if none were seen since.

- [ ] **Step 1: Write the failing tests**

Append to `runOnDemandTests()` before its closing brace:

```swift
    // --- decide --------------------------------------------------------------
    let t0 = Date(timeIntervalSince1970: 1_000_000)
    func at(_ s: TimeInterval) -> Date { t0.addingTimeInterval(s) }
    expect(decide(state: .armed, sticky: false, hasSockets: true, lastSeen: at(0), now: at(0), idle: 30), .connect, "decide: armed + traffic → connect")
    expect(decide(state: .armed, sticky: false, hasSockets: false, lastSeen: at(0), now: at(100), idle: 30), nil, "decide: armed + quiet → nothing")
    expect(decide(state: .connected, sticky: false, hasSockets: true, lastSeen: at(50), now: at(50), idle: 30), nil, "decide: connected + traffic → stay")
    expect(decide(state: .connected, sticky: false, hasSockets: false, lastSeen: at(50), now: at(70), idle: 30), nil, "decide: connected + quiet but not idle yet → stay")
    expect(decide(state: .connected, sticky: false, hasSockets: false, lastSeen: at(50), now: at(80), idle: 30), .disconnect, "decide: connected + idle reached → disconnect")
    expect(decide(state: .connected, sticky: true, hasSockets: false, lastSeen: at(50), now: at(5000), idle: 30), nil, "decide: sticky never idles out")
    expect(decide(state: .paused, sticky: false, hasSockets: true, lastSeen: at(0), now: at(0), idle: 30), nil, "decide: paused + traffic → stay paused")
    expect(decide(state: .paused, sticky: false, hasSockets: false, lastSeen: at(0), now: at(10), idle: 30), nil, "decide: paused + quiet but not idle yet → stay")
    expect(decide(state: .paused, sticky: false, hasSockets: false, lastSeen: at(0), now: at(30), idle: 30), .resumeArmed, "decide: paused + idle reached → armed")
    expect(decide(state: .off, sticky: false, hasSockets: true, lastSeen: at(0), now: at(0), idle: 30), nil, "decide: off never acts")
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `./test.sh`
Expected: compile error `cannot find 'decide' in scope`.

- [ ] **Step 3: Implement**

Append to `OnDemand.swift`:

```swift
// MARK: State machine

enum OnDemandState: String { case off, armed, connected, paused }

enum OnDemandAction: Equatable { case connect, disconnect, resumeArmed }

/// What to do on a poll tick. `lastSeen` is when VPN-bound sockets were last observed (or the last
/// connect/disconnect, whichever is later); `idle` is how long to wait without traffic.
func decide(state: OnDemandState, sticky: Bool, hasSockets: Bool, lastSeen: Date, now: Date, idle: TimeInterval) -> OnDemandAction? {
    let quietFor = now.timeIntervalSince(lastSeen)
    switch state {
    case .armed:     return hasSockets ? .connect : nil
    case .connected: return !sticky && !hasSockets && quietFor >= idle ? .disconnect : nil
    case .paused:    return !hasSockets && quietFor >= idle ? .resumeArmed : nil
    case .off:       return nil
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `./test.sh`
Expected: `N passed, 0 failed`.

- [ ] **Step 5: Commit**

```bash
git add OnDemand.swift tests/OnDemandTests.swift
git commit -m "Add on-demand state decision function"
```

---

### Task 3: Applying VPN DNS from WGBar

**Files:**
- Modify: `DNSGuard.swift`
- Modify: `tests/DNSGuardTests.swift` (add a block after the "repair reports networksetup errors" block; the `Fake` class defined there is reused)

**Interfaces:**
- Produces:
  ```swift
  func configDNSSearch(_ text: String) -> [String]                   // non-IP entries of DNS = lines
  extension DNSGuard { func apply(servers: [String], search: [String]) -> [String] }   // per-service error strings
  ```

- [ ] **Step 1: Write the failing tests**

Insert after the `do { // repair reports networksetup errors ... }` block:

```swift
        // --- apply (on-demand connect sets DNS itself, like wg-quick would) ------------
        expect(configDNSSearch("DNS = 10.0.0.22, corp.example, 10.0.0.4\nDNS = office.lan\n"),
               ["corp.example", "office.lan"], "config: search domains are the non-IP DNS entries")
        expect(configDNSSearch("DNS = 10.0.0.22\n"), [], "config: no search domains")
        do {
            let f = Fake(dns: ["Wi-Fi": ["1.1.1.1"], "Bridge": []])
            let g = DNSGuard(vpnDNS: vpn2, run: f.run)
            expect(g.apply(servers: ["10.0.0.22", "10.0.0.4"], search: []), [], "apply: no errors")
            expect(f.calls.contains(["-setdnsservers", "Wi-Fi", "10.0.0.22", "10.0.0.4"]), true, "apply: Wi-Fi gets both servers")
            expect(f.calls.contains(["-setdnsservers", "Bridge", "10.0.0.22", "10.0.0.4"]), true, "apply: every service gets them")
            expect(f.calls.contains(["-setsearchdomains", "Wi-Fi", "Empty"]), true, "apply: no search domains → Empty")
        }
        do {
            let f = Fake(dns: ["Wi-Fi": []])
            let g = DNSGuard(vpnDNS: vpn2, run: f.run)
            _ = g.apply(servers: ["10.0.0.22"], search: ["corp.example"])
            expect(f.calls.contains(["-setsearchdomains", "Wi-Fi", "corp.example"]), true, "apply: search domains applied")
        }
        do {
            let f = Fake(dns: ["Wi-Fi": []])
            let failing: (String, [String]) -> CmdResult = { exe, args in
                args.first == "-setdnsservers" ? CmdResult(status: 1, output: "** Error: nope") : f.run(exe, args)
            }
            expect(DNSGuard(vpnDNS: vpn2, run: failing).apply(servers: ["10.0.0.22"], search: []),
                   ["Wi-Fi: ** Error: nope"], "apply error: surfaced per service")
        }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `./test.sh`
Expected: compile error `cannot find 'configDNSSearch' in scope` / `value of type 'DNSGuard' has no member 'apply'`.

- [ ] **Step 3: Implement**

In `DNSGuard.swift`, after `configDNSServers` add:

```swift
/// The search domains declared by a wg-quick config: `DNS =` entries that are not IP addresses.
func configDNSSearch(_ text: String) -> [String] {
    var domains: [String] = []
    for line in text.split(separator: "\n") {
        let parts = line.split(separator: "=", maxSplits: 1).map { $0.trimmingCharacters(in: .whitespaces) }
        guard parts.count == 2, parts[0].lowercased() == "dns" else { continue }
        for entry in parts[1].split(separator: ",").map({ $0.trimmingCharacters(in: .whitespaces) }) {
            if !entry.isEmpty, !isIPAddress(entry) { domains.append(entry) }
        }
    }
    return domains
}
```

Inside `struct DNSGuard`, after `repair(snapshot:)` add:

```swift
    /// Point every network service at the VPN DNS servers, exactly as wg-quick does on `up`.
    /// Used by on-demand connect, whose armed config carries no DNS line. Returns per-service errors.
    func apply(servers: [String], search: [String]) -> [String] {
        var errors: [String] = []
        for s in services() {
            let r1 = run(networksetup, ["-setdnsservers", s] + servers)
            let r2 = run(networksetup, ["-setsearchdomains", s] + (search.isEmpty ? ["Empty"] : search))
            let failed = [r1, r2].filter { $0.status != 0 || $0.output.contains("Error") }
            if !failed.isEmpty { errors.append("\(s): " + failed.map(\.output).joined(separator: "; ")) }
        }
        return errors
    }
```

Update the file's header comment: after the sentence ending "no tunnel to reach it." add "On-demand mode (OnDemand.swift) applies and restores DNS through this file instead of wg-quick."

- [ ] **Step 4: Run tests to verify they pass**

Run: `./test.sh`
Expected: `N passed, 0 failed`.

- [ ] **Step 5: Commit**

```bash
git add DNSGuard.swift tests/DNSGuardTests.swift
git commit -m "DNSGuard: apply VPN DNS to all services"
```

---

### Task 4: The privileged helper script

**Files:**
- Create: `helper/wgbar-helper`
- Create: `tests/fixtures/sample.conf`, `tests/fixtures/sample.armed.conf`
- Create: `tests/helper_test.sh`
- Modify: `test.sh` (run the helper tests)

**Interfaces:**
- Produces: `/usr/local/libexec/wgbar-helper <verb> <tunnel>` with verbs `arm`, `connect`, `disconnect`, `down`, `status`, `print-armed`. Exit 0 on success. `status` prints one of `off`, `armed`, `connected` on stdout. Errors: one line on stderr, exit 1 (exit 2 for usage errors). Lines starting with `CONF_DIR=`, `WG_QUICK=`, `WG=` are rewritten at install time by `sudoers.sh` (Task 5). When not root, `WGBAR_CONF_DIR` in the environment overrides `CONF_DIR` (for tests).

- [ ] **Step 1: Write the fixtures and the failing test**

`tests/fixtures/sample.conf`:

```
# Sample tunnel used by tests/helper_test.sh
[Interface]
PrivateKey = cGxhY2Vob2xkZXItcHJpdmF0ZS1rZXktZm9yLXRlc3Rz=
Address = 10.42.66.9/24
dns = 10.0.0.22
DNS = 10.0.0.4, corp.example
MTU = 1380

[Peer]
PublicKey = cGxhY2Vob2xkZXItcHVibGljLWtleS1mb3ItdGVzdHM=
PresharedKey = cGxhY2Vob2xkZXItcHJlc2hhcmVkLWtleS10ZXN0cw=
Endpoint = office.example:42660
AllowedIPs = 10.0.0.0/24,10.42.0.0/16
persistentkeepalive = 25
```

`tests/fixtures/sample.armed.conf` (exactly the lines that must survive, in order):

```
# Sample tunnel used by tests/helper_test.sh
[Interface]
PrivateKey = cGxhY2Vob2xkZXItcHJpdmF0ZS1rZXktZm9yLXRlc3Rz=
Address = 10.42.66.9/24
MTU = 1380

[Peer]
PublicKey = cGxhY2Vob2xkZXItcHVibGljLWtleS1mb3ItdGVzdHM=
PresharedKey = cGxhY2Vob2xkZXItcHJlc2hhcmVkLWtleS10ZXN0cw=
AllowedIPs = 10.0.0.0/24,10.42.0.0/16
```

`tests/helper_test.sh`:

```bash
#!/bin/bash
# Tests for helper/wgbar-helper that need no root: config derivation and argument checks.
set -uo pipefail
cd "$(dirname "$0")/.."
H=helper/wgbar-helper
export WGBAR_CONF_DIR="$PWD/tests/fixtures"
fail=0
check() { if [[ "$2" == "$3" ]]; then echo "ok   $1"; else echo "FAIL $1"; echo "  expected: $3"; echo "  actual:   $2"; fail=1; fi; }

check "print-armed drops DNS, Endpoint and PersistentKeepalive" \
  "$($H print-armed sample 2>&1)" "$(cat tests/fixtures/sample.armed.conf)"

$H print-armed missing >/dev/null 2>&1; check "unknown tunnel exits 1" "$?" "1"
$H print-armed "../sample" >/dev/null 2>&1; check "path traversal is rejected" "$?" "2"
$H print-armed "a/b" >/dev/null 2>&1; check "slash in name is rejected" "$?" "2"
$H print-armed >/dev/null 2>&1; check "missing tunnel argument is a usage error" "$?" "2"
$H bogus sample >/dev/null 2>&1; check "unknown verb is a usage error" "$?" "2"
$H arm sample >/dev/null 2>&1; check "privileged verb refuses to run as a normal user" "$?" "1"
check "privileged verb says why" "$($H connect sample 2>&1 >/dev/null || true)" "wgbar-helper connect must run as root (via sudo)"

exit $fail
```

Make it executable: `chmod +x tests/helper_test.sh`. Append to `test.sh`:

```bash
tests/helper_test.sh
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `./test.sh`
Expected: the Swift tests pass, then `helper_test.sh` fails with `helper/wgbar-helper: No such file or directory` (several FAIL lines) and `./test.sh` exits non-zero.

- [ ] **Step 3: Write the helper**

Create `helper/wgbar-helper` and `chmod +x` it:

```bash
#!/bin/bash
# wgbar-helper — the one privileged command WGBar's "Connect on Demand" mode needs.
# Installed to /usr/local/libexec/wgbar-helper by sudoers.sh, which rewrites the three
# paths below and allows the current user to run this file (and nothing else) via sudo.
# Design: docs/superpowers/specs/2026-09-02-on-demand-design.md
#
# Usage: wgbar-helper arm|connect|disconnect|down|status|print-armed <tunnel>
#   arm          bring the interface up from the "armed" config (no DNS, no Endpoint)
#   connect      give the peer its Endpoint (and PersistentKeepalive) → traffic flows
#   disconnect   take the Endpoint away again; routes stay, packets are held
#   down         wg-quick down (never touches DNS, since the armed config has none)
#   status       print off | armed | connected
#   print-armed  print the derived armed config (works without root; for tests)
set -euo pipefail

CONF_DIR=/opt/homebrew/etc/wireguard
WG_QUICK=/opt/homebrew/bin/wg-quick
WG=/opt/homebrew/bin/wg
export PATH="${WG_QUICK%/*}:/usr/bin:/bin:/usr/sbin:/sbin"
[[ $EUID -ne 0 && -n "${WGBAR_CONF_DIR:-}" ]] && CONF_DIR="$WGBAR_CONF_DIR"

usage() { echo "usage: wgbar-helper arm|connect|disconnect|down|status|print-armed <tunnel>" >&2; exit 2; }
die() { echo "$*" >&2; exit 1; }

verb="${1:-}"; tunnel="${2:-}"
case "$verb" in arm|connect|disconnect|down|status|print-armed) ;; *) usage ;; esac
[[ -n "$tunnel" ]] || usage
# Same character set wg-quick accepts for interface names; in particular no "/" or "..".
[[ "$tunnel" =~ ^[a-zA-Z0-9_=+.-]{1,15}$ && "$tunnel" != *..* ]] || usage
conf="$CONF_DIR/$tunnel.conf"
[[ -f "$conf" ]] || die "no such config: $conf"
namefile="/var/run/wireguard/$tunnel.name"

lower() { tr '[:upper:]' '[:lower:]' <<<"$1"; }
trim() { local v="$1"; v="${v#"${v%%[![:space:]]*}"}"; printf '%s' "${v%"${v##*[![:space:]]}"}"; }

# The armed variant of the real config: every line except DNS, Endpoint and PersistentKeepalive.
armed_config() {
  local line key
  while IFS= read -r line || [[ -n "$line" ]]; do
    if [[ "$line" == *=* ]]; then
      key="$(lower "$(trim "${line%%=*}")")"
      case "$key" in dns|endpoint|persistentkeepalive) continue ;; esac
    fi
    printf '%s\n' "$line"
  done < "$conf"
}

# First value of key $1 (case-insensitive) in the real config; comments stripped. Exit 1 if absent.
field() {
  local want line key
  want="$(lower "$1")"
  while IFS= read -r line || [[ -n "$line" ]]; do
    line="${line%%\#*}"
    [[ "$line" == *=* ]] || continue
    key="$(lower "$(trim "${line%%=*}")")"
    [[ "$key" == "$want" ]] || continue
    trim "${line#*=}"; echo; return 0
  done < "$conf"
  return 1
}

need_root() { [[ $EUID -eq 0 ]] || die "wgbar-helper $verb must run as root (via sudo)"; }
iface() { [[ -f "$namefile" ]] || die "$tunnel is not up"; cat "$namefile"; }

# Writes the armed config (0600, in a root-only temp dir removed on exit) and sets $ARMED to its path.
make_armed() {
  local dir; dir="$(mktemp -d /tmp/wgbar.XXXXXX)"; chmod 700 "$dir"
  trap 'rm -rf "$dir"' EXIT
  ARMED="$dir/$tunnel.conf"
  (umask 077; armed_config > "$ARMED")
}

case "$verb" in
  print-armed)
    armed_config ;;
  status)
    if [[ ! -f "$namefile" ]]; then echo off; exit 0; fi
    need_root
    if "$WG" show "$(cat "$namefile")" endpoints | awk '{print $2}' | grep -qv '^(none)$'; then echo connected; else echo armed; fi ;;
  arm)
    need_root
    [[ -f "$namefile" ]] && exit 0
    make_armed
    "$WG_QUICK" up "$ARMED" ;;
  connect)
    need_root
    i="$(iface)"
    pk="$(field PublicKey)" || die "no PublicKey in $conf"
    ep="$(field Endpoint)"  || die "no Endpoint in $conf"
    ka="$(field PersistentKeepalive || true)"
    if [[ -n "$ka" ]]; then "$WG" set "$i" peer "$pk" endpoint "$ep" persistent-keepalive "$ka"
    else "$WG" set "$i" peer "$pk" endpoint "$ep"; fi ;;
  disconnect)
    need_root
    i="$(iface)"
    pk="$(field PublicKey)" || die "no PublicKey in $conf"
    make_armed
    "$WG_QUICK" strip "$ARMED" > "${ARMED%/*}/stripped.conf"
    "$WG" set "$i" peer "$pk" remove
    "$WG" addconf "$i" "${ARMED%/*}/stripped.conf" ;;
  down)
    need_root
    [[ -f "$namefile" ]] || exit 0
    make_armed
    "$WG_QUICK" down "$ARMED" ;;
esac
```

Notes for the implementer:
- `wg-quick` derives the tunnel name from the config file's basename, so the armed copy is named `<tunnel>.conf` and produces the same `/var/run/wireguard/<tunnel>.name` as a classic `up`.
- `wg set` cannot clear an endpoint, hence remove + `addconf` in `disconnect`. `wg-quick strip` turns the armed config into `wg`-only syntax (drops `Address`, `MTU`, hooks). Kernel routes were installed by `wg-quick up` and are untouched by peer changes.
- `wg-quick down` restores DNS from its background monitor's memory, and the armed monitor recorded none, so `down` never touches DNS.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `./test.sh`
Expected: Swift tests `N passed, 0 failed`, then eight `ok` lines from `helper_test.sh`, exit 0.

Also run `bash -n helper/wgbar-helper` (syntax) and, if `shellcheck` is installed, `shellcheck helper/wgbar-helper` (no errors; warnings about `$?` in the test are fine).

- [ ] **Step 5: Commit**

```bash
git add helper/wgbar-helper tests/helper_test.sh tests/fixtures/sample.conf tests/fixtures/sample.armed.conf test.sh
git commit -m "Add wgbar-helper: privileged arm/connect/disconnect for on-demand mode"
```

---

### Task 5: Installing the helper (sudoers.sh, install.sh, uninstall.sh)

**Files:**
- Modify: `sudoers.sh`
- Modify: `install.sh`
- Modify: `uninstall.sh`

**Interfaces:**
- Consumes: `helper/wgbar-helper` from Task 4.
- Produces: `/usr/local/libexec/wgbar-helper` (root:wheel 755) with `CONF_DIR`, `WG_QUICK`, `WG` rewritten; sudoers rule that includes the helper path.

- [ ] **Step 1: Update `sudoers.sh`**

Replace the header comment and everything from `CMDS=()` onward so the script reads:

```bash
#!/bin/bash
# Installs the WGBar helper and a sudoers rule so WGBar can run "wg-quick up/down <config>" and
# "wgbar-helper <verb> <tunnel>" (Connect on Demand) without a password prompt.
# The rule is limited to the current user and to the tunnel configs that exist right now;
# re-run this script after adding a config or updating WGBar. Remove with:
#   sudo rm /etc/sudoers.d/wgbar /usr/local/libexec/wgbar-helper
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
  CMDS+=("$WG_QUICK up $f" "$WG_QUICK down $f")
done
[[ ${#CMDS[@]} -gt 1 ]] || { echo "No .conf files in $CONF_DIR; nothing to allow." >&2; exit 1; }

RULE="$(id -un) ALL=(root) NOPASSWD: $(printf '%s, ' "${CMDS[@]}" | sed 's/, $//')"

TMP="$(mktemp)"; TMP_HELPER="$(mktemp)"
trap 'rm -f "$TMP" "$TMP_HELPER"' EXIT
{
  echo "# Installed by WGBar (sudoers.sh). Lets $(id -un) toggle WireGuard tunnels in $CONF_DIR without a password."
  echo "$RULE"
} > "$TMP"
sed -e "s|^CONF_DIR=.*|CONF_DIR=$CONF_DIR|" -e "s|^WG_QUICK=.*|WG_QUICK=$WG_QUICK|" -e "s|^WG=.*|WG=$WG|" \
  helper/wgbar-helper > "$TMP_HELPER"
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
```

- [ ] **Step 2: Update `install.sh`**

Before the final `echo "Optional: run ./sudoers.sh ..."` line, add:

```bash
HELPER=/usr/local/libexec/wgbar-helper
if [[ -e "$HELPER" ]] && ! diff -q <(grep -vE '^(CONF_DIR|WG_QUICK|WG)=' "$HELPER") <(grep -vE '^(CONF_DIR|WG_QUICK|WG)=' helper/wgbar-helper) >/dev/null; then
  echo "Note: the installed WGBar helper is out of date. Run ./sudoers.sh to update it."
fi
```

and change the last line to:

```bash
echo "Optional: run ./sudoers.sh to skip the password prompt when toggling (required for Connect on Demand)."
```

- [ ] **Step 3: Update `uninstall.sh`**

Replace the file with:

```bash
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
```

- [ ] **Step 4: Verify the scripts parse and the rule is valid**

Run:

```bash
bash -n sudoers.sh && bash -n install.sh && bash -n uninstall.sh && echo syntax ok
TMP=$(mktemp); printf '%s\n' "$(id -un) ALL=(root) NOPASSWD: /usr/local/libexec/wgbar-helper, /opt/homebrew/bin/wg-quick up /opt/homebrew/etc/wireguard/x.conf" > "$TMP"; visudo -c -q -f "$TMP" && echo rule ok; rm -f "$TMP"
```

Expected: `syntax ok` and `rule ok` (`visudo -c -f` on a temp file does not need root).

Do NOT run `./sudoers.sh` here; the user runs it during manual acceptance (Task 8).

- [ ] **Step 5: Commit**

```bash
git add sudoers.sh install.sh uninstall.sh
git commit -m "Install wgbar-helper from sudoers.sh; remove it on uninstall"
```

---

### Task 6: OnDemandController

**Files:**
- Modify: `OnDemand.swift` (append)
- Modify: `tests/OnDemandTests.swift` (append)

**Interfaces:**
- Consumes: `parseNetstat`, `vpnBound`, `CIDR`, `decide`, `OnDemandState`, `CmdResult` (from `DNSGuard.swift`).
- Produces:
  ```swift
  final class OnDemandController {
      static let helperPath: String            // "/usr/local/libexec/wgbar-helper"
      static let needsHelperMessage: String
      static func helperInstalled() -> Bool
      let tunnel: String
      init(tunnel: String, allowed: [CIDR], idle: TimeInterval, kick: String?,
           run: @escaping (String, [String]) -> CmdResult, now: @escaping () -> Date)
      var onConnected: () -> Void              // apply VPN DNS (called on the caller's thread)
      var onDisconnected: () -> Void           // restore DNS
      var onError: (String) -> Void
      var onChange: (OnDemandState) -> Void    // every state change, with the new state
      private(set) var state: OnDemandState
      @discardableResult func arm() -> Bool
      func reconcile()
      func tick()
      func manualToggle()
      func shutdown()
  }
  ```
  All methods are synchronous and must be called from one serial queue; callbacks fire on that queue.

- [ ] **Step 1: Write the failing tests**

Append to `runOnDemandTests()`:

```swift
    // --- controller (fake sudo/netstat/ping) ----------------------------------------
    /// Counts the controller's callbacks.
    final class Events { var connected = 0, disconnected = 0; var errors: [String] = []; var states: [OnDemandState] = [] }
    /// Fakes every command the controller runs and records the helper verbs it asked for.
    final class FakeSystem {
        var netstat = ""
        var helperFails: Set<String> = []          // verbs that fail
        var sudoNeedsPassword = false
        var statusOutput = "armed"
        var verbs: [String] = []
        var pings: [String] = []
        var clock = Date(timeIntervalSince1970: 1_000_000)
        func advance(_ s: TimeInterval) { clock = clock.addingTimeInterval(s) }
        func run(_ exe: String, _ args: [String]) -> CmdResult {
            switch exe {
            case "/usr/sbin/netstat": return CmdResult(status: 0, output: netstat)
            case "/sbin/ping": pings.append(args.last ?? ""); return CmdResult(status: 0, output: "")
            case "/usr/bin/sudo":
                expect(Array(args.prefix(2)), ["-n", OnDemandController.helperPath], "controller: sudo -n helper")
                expect(args.last, "ileasing", "controller: tunnel name passed")
                if sudoNeedsPassword { return CmdResult(status: 1, output: "sudo: a password is required") }
                let verb = args[2]; verbs.append(verb)
                if helperFails.contains(verb) { return CmdResult(status: 1, output: "wg-quick: boom") }
                return CmdResult(status: 0, output: verb == "status" ? statusOutput : "")
            default: return CmdResult(status: 1, output: "unexpected \(exe)")
            }
        }
        func controller(kick: String? = "10.0.0.22") -> (OnDemandController, Events) {
            let ev = Events()
            let c = OnDemandController(tunnel: "ileasing", allowed: [CIDR("10.0.0.0/24")!, CIDR("10.42.0.0/16")!],
                                       idle: 30, kick: kick, run: run, now: { self.clock })
            c.onConnected = { ev.connected += 1 }
            c.onDisconnected = { ev.disconnected += 1 }
            c.onError = { ev.errors.append($0) }
            c.onChange = { ev.states.append($0) }
            return (c, ev)
        }
    }
    let rdp = "tcp4 0 0 10.42.66.9.60655 10.42.1.9.3389 ESTABLISHED\n"
    let syn = "tcp4 0 0 10.42.66.9.60700 10.42.1.63.3389 SYN_SENT\n"
    let web = "tcp4 0 0 10.5.102.162.60773 3.233.158.111.443 ESTABLISHED\n"

    do {   // arm
        let sys = FakeSystem(); let (c, ev) = sys.controller()
        expect(c.arm(), true, "arm: succeeds")
        expect(sys.verbs, ["arm"], "arm: calls helper arm")
        expect(c.state, .armed, "arm: state armed")
        expect(ev.states, [.armed], "arm: change reported")
    }
    do {   // arm failure
        let sys = FakeSystem(); sys.helperFails = ["arm"]; let (c, ev) = sys.controller()
        expect(c.arm(), false, "arm failure: returns false")
        expect(c.state, .off, "arm failure: state off")
        expect(ev.errors, ["wg-quick: boom"], "arm failure: helper output surfaced")
    }
    do {   // missing sudoers rule
        let sys = FakeSystem(); sys.sudoNeedsPassword = true; let (c, ev) = sys.controller()
        c.arm()
        expect(ev.errors, [OnDemandController.needsHelperMessage], "no sudoers: friendly message")
    }
    do {   // full cycle: traffic → connect → quiet → idle → disconnect
        let sys = FakeSystem(); let (c, ev) = sys.controller()
        c.arm()
        sys.netstat = web; c.tick()
        expect(c.state, .armed, "cycle: unrelated traffic does not connect")
        sys.netstat = web + syn; c.tick()
        expect(c.state, .connected, "cycle: SYN_SENT into the tunnel connects")
        expect(sys.verbs, ["arm", "connect"], "cycle: helper connect called once")
        expect(sys.pings, ["10.0.0.22"], "cycle: handshake kicked with a ping")
        expect(ev.connected, 1, "cycle: onConnected fired (DNS applied)")
        sys.netstat = rdp; sys.advance(600); c.tick()
        expect(c.state, .connected, "cycle: established session keeps it up")
        sys.netstat = web; sys.advance(20); c.tick()
        expect(c.state, .connected, "cycle: 20 s quiet is not idle yet")
        sys.advance(10); c.tick()
        expect(c.state, .armed, "cycle: 30 s quiet disconnects back to armed")
        expect(sys.verbs, ["arm", "connect", "disconnect"], "cycle: helper disconnect called")
        expect(ev.disconnected, 1, "cycle: onDisconnected fired (DNS restored)")
        expect(ev.states, [.armed, .connected, .armed], "cycle: state changes reported")
    }
    do {   // connect failure → off, no retry loop
        let sys = FakeSystem(); sys.helperFails = ["connect"]; let (c, ev) = sys.controller()
        c.arm(); sys.netstat = syn; c.tick(); c.tick()
        expect(c.state, .off, "connect failure: state off")
        expect(sys.verbs, ["arm", "connect"], "connect failure: not retried every tick")
        expect(ev.errors.count, 1, "connect failure: one error")
        expect(ev.connected, 0, "connect failure: DNS untouched")
    }
    do {   // manual connect is sticky; manual disconnect with live sockets pauses
        let sys = FakeSystem(); let (c, ev) = sys.controller(kick: nil)
        c.arm()
        c.manualToggle()
        expect(c.state, .connected, "manual: click connects")
        expect(sys.pings, [], "manual: no kick address → no ping")
        sys.netstat = ""; sys.advance(3600); c.tick()
        expect(c.state, .connected, "manual: sticky ignores idle")
        sys.netstat = rdp; c.manualToggle()
        expect(c.state, .paused, "manual: disconnect while a session exists → paused")
        expect(ev.disconnected, 1, "manual: DNS restored on manual disconnect")
        sys.netstat = syn; c.tick(); sys.advance(10); c.tick()
        expect(c.state, .paused, "manual: reconnect attempts keep it paused")
        expect(sys.verbs, ["arm", "connect", "disconnect"], "manual: paused does not reconnect")
        sys.netstat = ""; c.tick(); sys.advance(30); c.tick()
        expect(c.state, .armed, "manual: quiet for idle → armed again")
        sys.netstat = syn; c.tick()
        expect(c.state, .connected, "manual: armed again reacts to new traffic")
        expect(c.state == .connected && sys.verbs.last == "connect", true, "manual: auto connect after pause")
    }
    do {   // manual disconnect with no sockets goes straight to armed
        let sys = FakeSystem(); let (c, _) = sys.controller()
        c.arm(); c.manualToggle(); sys.netstat = ""; c.manualToggle()
        expect(c.state, .armed, "manual: disconnect without sessions → armed")
    }
    do {   // reconcile at launch / wake
        let sys = FakeSystem(); let (c, _) = sys.controller()
        sys.statusOutput = "off"; c.reconcile(); expect(c.state, .off, "reconcile: off")
        sys.statusOutput = "armed"; c.reconcile(); expect(c.state, .armed, "reconcile: armed")
        sys.statusOutput = "Warning: something\nconnected"; c.reconcile(); expect(c.state, .connected, "reconcile: last line wins")
        sys.netstat = ""; sys.advance(30); c.tick()
        expect(c.state, .armed, "reconcile: adopted connection is not sticky and idles out")
    }
    do {   // shutdown
        let sys = FakeSystem(); let (c, ev) = sys.controller()
        c.arm(); sys.netstat = syn; c.tick()
        c.shutdown()
        expect(sys.verbs, ["arm", "connect", "down"], "shutdown: connected → down (helper's down handles the peer)")
        expect(ev.disconnected, 1, "shutdown: DNS restored first")
        expect(c.state, .off, "shutdown: off")
        let sys2 = FakeSystem(); let (c2, ev2) = sys2.controller()
        c2.shutdown()
        expect(sys2.verbs, [], "shutdown: nothing to do when off")
        expect(ev2.disconnected, 0, "shutdown: no DNS restore when off")
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `./test.sh`
Expected: compile error `cannot find 'OnDemandController' in scope`.

- [ ] **Step 3: Implement the controller**

Append to `OnDemand.swift`:

```swift
// MARK: Controller

/// Drives one tunnel through armed ⇄ connected by polling `netstat` and calling the root helper.
/// Synchronous; call every method from a single serial queue. Callbacks fire on that queue.
final class OnDemandController {
    static let helperPath = "/usr/local/libexec/wgbar-helper"
    static let needsHelperMessage = "On-Demand needs the WGBar helper.\nRun ./sudoers.sh in the WGBar folder and try again."
    static func helperInstalled() -> Bool { FileManager.default.isExecutableFile(atPath: helperPath) }

    let tunnel: String
    let allowed: [CIDR]
    let idle: TimeInterval
    /// Pinged right after connect so the handshake starts now instead of at the next TCP retransmit.
    let kick: String?
    let run: (String, [String]) -> CmdResult
    let now: () -> Date

    private(set) var state: OnDemandState = .off
    private(set) var sticky = false
    private var lastSeen: Date

    var onConnected: () -> Void = {}
    var onDisconnected: () -> Void = {}
    var onError: (String) -> Void = { _ in }
    var onChange: (OnDemandState) -> Void = { _ in }

    init(tunnel: String, allowed: [CIDR], idle: TimeInterval, kick: String?,
         run: @escaping (String, [String]) -> CmdResult, now: @escaping () -> Date) {
        self.tunnel = tunnel; self.allowed = allowed; self.idle = idle; self.kick = kick
        self.run = run; self.now = now
        lastSeen = now()
    }

    private func set(_ s: OnDemandState) {
        guard s != state else { return }
        state = s
        onChange(s)
    }

    /// `sudo -n wgbar-helper <verb> <tunnel>`. A password prompt means the sudoers rule is missing.
    private func helper(_ verb: String) -> (ok: Bool, output: String) {
        let r = run("/usr/bin/sudo", ["-n", Self.helperPath, verb, tunnel])
        if r.status == 0 { return (true, r.output) }
        if r.output.contains("password") { return (false, Self.needsHelperMessage) }
        return (false, r.output.isEmpty ? "wgbar-helper \(verb) failed" : r.output)
    }

    private func sockets() -> [TCPSocket] {
        vpnBound(parseNetstat(run("/usr/sbin/netstat", ["-n", "-p", "tcp"]).output), allowed)
    }

    /// Bring the interface up without an endpoint. False (and `onError`) if the helper failed.
    @discardableResult func arm() -> Bool {
        let r = helper("arm")
        if r.ok { set(.armed) } else { set(.off); onError(r.output) }
        return r.ok
    }

    /// Adopt whatever state the interface is actually in (launch, wake).
    func reconcile() {
        let r = helper("status")
        guard r.ok else { set(.off); onError(r.output); return }
        switch r.output.split(separator: "\n").last.map(String.init) ?? "" {
        case "connected": sticky = false; lastSeen = now(); set(.connected)
        case "armed":     set(.armed)
        default:          set(.off)
        }
    }

    /// One poll: look at the sockets, act on `decide`.
    func tick() {
        guard state != .off else { return }
        let has = !sockets().isEmpty
        let t = now()
        if has { lastSeen = t }
        switch decide(state: state, sticky: sticky, hasSockets: has, lastSeen: lastSeen, now: t, idle: idle) {
        case .connect?:     connect(sticky: false)
        case .disconnect?:  disconnect()
        case .resumeArmed?: set(.armed)
        case nil:           break
        }
    }

    /// Left-click: connect (sticky) from armed/paused, disconnect from connected, re-arm from off.
    func manualToggle() {
        switch state {
        case .armed, .paused: connect(sticky: true)
        case .connected:      disconnect()
        case .off:            arm()
        }
    }

    /// Restore DNS if needed and take the interface down (quit, disable, tunnel change).
    func shutdown() {
        guard state != .off else { return }
        if state == .connected { onDisconnected() }
        let r = helper("down")
        if !r.ok { onError(r.output) }
        set(.off)
    }

    private func connect(sticky: Bool) {
        let r = helper("connect")
        guard r.ok else { set(.off); onError(r.output); return }   // .off: no retry storm
        self.sticky = sticky
        lastSeen = now()
        if let kick { _ = run("/sbin/ping", ["-c", "1", "-W", "1000", kick]) }
        set(.connected)
        onConnected()
    }

    private func disconnect() {
        onDisconnected()   // put DNS back before the tunnel stops answering
        let r = helper("disconnect")
        sticky = false
        lastSeen = now()
        guard r.ok else { set(.off); onError(r.output); return }
        set(sockets().isEmpty ? .armed : .paused)
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `./test.sh`
Expected: `N passed, 0 failed`, helper tests `ok`.

- [ ] **Step 5: Commit**

```bash
git add OnDemand.swift tests/OnDemandTests.swift
git commit -m "Add OnDemandController: poll sockets, drive the helper"
```

---

### Task 7: Wire On-Demand into the app

**Files:**
- Modify: `main.swift`

**Interfaces:**
- Consumes: `OnDemandController`, `OnDemandState`, `parseAllowedIPs`, `isFullTunnel`, `configDNSServers`, `configDNSSearch`, `DNSGuard.apply`, `DNSGuard.snapshot`, `DNSGuard.repair`.
- Produces: user-visible behaviour only. New top-level function `anyTunnelConnected(ignoring:)`.

This task has no unit tests (all Cocoa glue); verify by building and by the manual checklist in Task 8. Make the edits below in order, building after each group with `./build.sh`.

- [ ] **Step 1: Add `anyTunnelConnected` next to `anyTunnelUp`**

After `func anyTunnelUp()` add:

```swift
/// Like `anyTunnelUp`, but an on-demand interface that is merely armed (no endpoint, no VPN DNS)
/// does not count: its name file exists while DNS must still be the user's own.
func anyTunnelConnected(ignoring armedTunnel: String?) -> Bool {
    let names = (try? FileManager.default.contentsOfDirectory(atPath: "/var/run/wireguard")) ?? []
    return names.contains { $0.hasSuffix(".name") && $0 != armedTunnel.map { "\($0).name" } }
}
```

- [ ] **Step 2: Add state and menu item to `AppDelegate`**

After `private let updateItem = ...` add:

```swift
    private let onDemandItem = NSMenuItem(title: "Connect on Demand", action: #selector(toggleOnDemand), keyEquivalent: "")
    /// Present while "Connect on Demand" is enabled. Only ever touched on `odQueue`.
    private var onDemand: OnDemandController?
    /// Mirror of the controller's state for the main thread (updated via onChange).
    private var odState: OnDemandState = .off
    private var odTunnel: String?
    private var odInFlight = false
    private let odQueue = DispatchQueue(label: "org.wgbar.ondemand")
```

In `applicationDidFinishLaunching`, after `updateItem.target = self` add `onDemandItem.target = self`, and add `menu.addItem(onDemandItem)` right after `menu.addItem(tunnelItem)`.

After the existing 2-second `Timer.scheduledTimer(...)` line add:

```swift
        Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in self?.onDemandTick() }
        if defaults.bool(forKey: "onDemand") { startOnDemand(atLaunch: true) }
```

Inside the `didWakeNotification` observer closure, before the DNS `asyncAfter`, add:

```swift
            self?.odQueue.async { self?.onDemand?.reconcile() }
```

- [ ] **Step 3: Teach `refresh`, `updateIcon` and `checkDNS` about the third state**

In `refresh()` replace `isUp = !tunnel.isEmpty && FileManager.default.fileExists(atPath: runFile(tunnel))` with:

```swift
        isUp = odTunnel != nil ? odState == .connected
             : !tunnel.isEmpty && FileManager.default.fileExists(atPath: runFile(tunnel))
```

In `updateIcon()` replace the `let name = ...` expression with:

```swift
        let armed = odTunnel != nil && (odState == .armed || odState == .paused)
        let name = busy ? "shield.lefthalf.filled"
            : isUp ? "shield.fill"
            : armed ? "shield"
            : dnsIssue.isEmpty ? "shield.slash" : "exclamationmark.shield"
        let desc = busy ? "WireGuard switching" : isUp ? "WireGuard on" : armed ? "WireGuard armed" : "WireGuard off"
```

and replace the tooltip's last branch (`"\(tunnel): \(isUp ? "connected" : "disconnected") — click to toggle"`) with `statusText() + " — click to toggle"`. Add this helper to the class:

```swift
    /// "ileasing: Connected" etc., shared by the menu status line and the icon tooltip.
    private func statusText() -> String {
        let addr = tunnelAddress(tunnel).map { " — \($0)" } ?? ""
        guard odTunnel != nil else { return "\(tunnel): " + (isUp ? "Connected\(addr)" : "Disconnected") }
        switch odState {
        case .connected: return "\(tunnel): Connected (on demand)\(addr)"
        case .armed:     return "\(tunnel): Armed, connects on demand"
        case .paused:    return "\(tunnel): Paused"
        case .off:       return "\(tunnel): Off (on demand not armed)"
        }
    }
```

In `checkDNS(interactive:)` replace `guard !busy, !dnsChecking, !anyTunnelUp() else { return }` with:

```swift
        let armed = odTunnel != nil && odState != .connected ? odTunnel : nil
        guard !busy, !dnsChecking, !anyTunnelConnected(ignoring: armed) else { return }
```

- [ ] **Step 4: Menu**

In `showMenu()` replace the `statusLine.title = isUp ? ... : ...` assignment with `statusLine.title = statusText()`, and after `rebuildTunnelSubmenu()` add:

```swift
        let confText = tunnel.isEmpty ? "" : (try? String(contentsOfFile: confPath(tunnel), encoding: .utf8)) ?? ""
        let full = isFullTunnel(parseAllowedIPs(confText))
        onDemandItem.state = odTunnel != nil ? .on : .off
        onDemandItem.isEnabled = !busy && !tunnel.isEmpty && (odTunnel != nil || !full)
        onDemandItem.toolTip = full ? "Not available for full-tunnel configs (AllowedIPs includes 0.0.0.0/0)"
                                    : "Keep the tunnel armed and connect it when traffic to its AllowedIPs appears"
```

- [ ] **Step 5: Route the left-click and the poll through the controller**

At the top of `toggle()`, after `guard !busy else { return }`, add:

```swift
        if odTunnel != nil {
            busy = true; updateIcon()
            odQueue.async {
                self.onDemand?.manualToggle()
                DispatchQueue.main.async { self.busy = false; self.refresh() }
            }
            return
        }
```

Add the tick method:

```swift
    // MARK: On demand

    private func onDemandTick() {
        guard odTunnel != nil, !odInFlight, !busy else { return }
        odInFlight = true
        odQueue.async {
            self.onDemand?.tick()
            DispatchQueue.main.async { self.odInFlight = false }
        }
    }
```

- [ ] **Step 6: Controller construction, DNS callbacks, enable/disable/switch**

Add to the class:

```swift
    private func makeController(_ tunnel: String, _ cidrs: [CIDR]) -> OnDemandController {
        let idle = (defaults.object(forKey: "onDemandIdle") as? NSNumber)?.doubleValue ?? 30
        let text = (try? String(contentsOfFile: confPath(tunnel), encoding: .utf8)) ?? ""
        let od = OnDemandController(tunnel: tunnel, allowed: cidrs, idle: idle,
                                    kick: configDNSServers(text).first, run: run, now: Date.init)
        od.onConnected = { [weak self] in self?.applyVPNDNS(for: tunnel) }
        od.onDisconnected = { [weak self] in self?.restoreDNS() }
        od.onError = { [weak self] msg in DispatchQueue.main.async { self?.showError("", msg) } }
        od.onChange = { [weak self] s in DispatchQueue.main.async { self?.odState = s; self?.refresh() } }
        return od
    }

    /// Runs on odQueue. Remember the current DNS, then set the config's servers everywhere (as wg-quick would).
    private func applyVPNDNS(for tunnel: String) {
        guard let text = try? String(contentsOfFile: confPath(tunnel), encoding: .utf8) else { return }
        let servers = configDNSServers(text)
        guard !servers.isEmpty else { return }
        let guardian = DNSGuard(vpnDNS: allConfigDNS(), run: run)
        defaults.set(guardian.snapshot(), forKey: "dnsSnapshot")
        let errors = guardian.apply(servers: servers, search: configDNSSearch(text))
        DispatchQueue.main.async {
            self.prefsSeen = networkPrefsModified()
            if !errors.isEmpty { self.showError("", "Could not set VPN DNS:\n" + errors.joined(separator: "\n")) }
        }
    }

    /// Runs on odQueue. Put back whatever DNS the services had before connect.
    private func restoreDNS() {
        let snapshot = defaults.dictionary(forKey: "dnsSnapshot") as? DNSSnapshot ?? [:]
        let result = DNSGuard(vpnDNS: allConfigDNS(), run: run).repair(snapshot: snapshot)
        DispatchQueue.main.async {
            self.prefsSeen = networkPrefsModified()
            if !result.errors.isEmpty { self.showError("", "Could not restore DNS:\n" + result.errors.joined(separator: "\n")) }
        }
    }

    /// Menu checkbox: enable or disable Connect on Demand for the selected tunnel.
    @objc private func toggleOnDemand() {
        guard !busy else { return }
        if odTunnel != nil { stopOnDemand(); return }
        startOnDemand(atLaunch: false)
    }

    /// Arm the selected tunnel. At launch, adopt the interface's real state first (it may still be up).
    private func startOnDemand(atLaunch: Bool) {
        guard !tunnel.isEmpty else { return }
        guard OnDemandController.helperInstalled() else {
            defaults.set(false, forKey: "onDemand")
            showError("", OnDemandController.needsHelperMessage)
            return
        }
        guard let text = try? String(contentsOfFile: confPath(tunnel), encoding: .utf8) else { return }
        let cidrs = parseAllowedIPs(text)
        guard !cidrs.isEmpty, !isFullTunnel(cidrs) else {
            defaults.set(false, forKey: "onDemand")
            showError("", "Connect on Demand needs a split-tunnel config (specific AllowedIPs, not 0.0.0.0/0).")
            return
        }
        busy = true; updateIcon()
        let tunnel = self.tunnel
        let od = makeController(tunnel, cidrs)
        odQueue.async {
            // A classic `wg-quick up` interface carries VPN DNS and a monitor that re-applies it: replace it.
            if !atLaunch, FileManager.default.fileExists(atPath: runFile(tunnel)) {
                if let error = wgQuick("down", tunnel) {
                    DispatchQueue.main.async { self.busy = false; self.refresh(); self.showError("down", error) }
                    return
                }
            }
            if atLaunch { od.reconcile() }
            let ok = od.state != .off || od.arm()
            DispatchQueue.main.async {
                self.busy = false
                if ok {
                    self.onDemand = od; self.odTunnel = tunnel; self.odState = od.state
                    defaults.set(true, forKey: "onDemand")
                } else {
                    defaults.set(false, forKey: "onDemand")
                }
                self.refresh()
            }
        }
    }

    /// Take the on-demand interface down and return to classic mode.
    private func stopOnDemand(then completion: (() -> Void)? = nil) {
        guard let od = onDemand else { completion?(); return }
        busy = true; updateIcon()
        onDemand = nil; odTunnel = nil; odState = .off
        defaults.set(false, forKey: "onDemand")
        odQueue.async {
            od.shutdown()
            DispatchQueue.main.async { self.busy = false; self.refresh(); completion?() }
        }
    }

    /// The tunnel or config folder changed while On-Demand is on: re-arm for the new selection.
    private func restartOnDemand() {
        guard odTunnel != nil else { return }
        stopOnDemand { [weak self] in
            self?.resolveTunnel()
            self?.startOnDemand(atLaunch: false)
        }
    }
```

Note `stopOnDemand` clears `defaults onDemand`, and `startOnDemand` sets it again on success, so a failed re-arm leaves the checkbox off rather than half-on.

- [ ] **Step 7: Tunnel change, folder change, quit**

In `selectTunnel(_:)`, replace the trailing `refresh()` with:

```swift
        if odTunnel != nil { restartOnDemand() } else { refresh() }
```

In `chooseFolder()`, replace the trailing `tunnel = ""` / `refresh()` pair with:

```swift
        tunnel = ""
        resolveTunnel()
        if odTunnel != nil { restartOnDemand() } else { refresh() }
```

Add the terminate hook to the class:

```swift
    /// Quit: an armed interface with nobody watching would black-hole the VPN subnets. Take it down.
    func applicationWillTerminate(_ notification: Notification) {
        guard let od = onDemand else { return }
        odQueue.sync { od.shutdown() }
    }
```

- [ ] **Step 8: Build and run the full test suite**

Run: `./build.sh && ./test.sh`
Expected: `Built build/WGBar.app`, all tests pass. Fix any compiler errors (typical: missing `self.` in closures, `odState` used before declaration).

- [ ] **Step 9: Smoke test without root**

Run: `./install.sh` (this relaunches WGBar). Right-click the icon: the menu shows **Connect on Demand** unticked. Click it: since the helper is not installed yet, the alert "On-Demand needs the WGBar helper…" appears and the checkbox stays off. Classic left-click toggling still works as before.

- [ ] **Step 10: Commit**

```bash
git add main.swift
git commit -m "Add Connect on Demand menu item and lifecycle"
```

---

### Task 8: README, manual acceptance, and finish

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Document the feature**

In `README.md`, in the bullet list under the title, change the right-click bullet to end with `..., pick a tunnel (when you have more than one), connect on demand, launch at login, check for updates, quit.` and the icon bullet to: ``Icon: `shield.fill` = connected, `shield` = armed (on demand), `shield.slash` = disconnected.``

Insert a new section before `## DNS left behind (and the fix)`:

````markdown
## Connect on demand

Right-click → **Connect on Demand** keeps the selected tunnel *armed*: the WireGuard
interface and its routes stay up, but the peer has no endpoint and your DNS is untouched.
The moment something opens a TCP connection to an address inside the tunnel's `AllowedIPs`
(an RDP session in the Windows App, ssh to an office box, an internal website), WGBar gives
the peer its endpoint, applies the config's DNS servers, and the connection goes through.
Thirty seconds after the last such connection closes, the endpoint is removed and DNS is
restored. Left-click still works: a manual connect stays up until you click again; a manual
disconnect pauses auto-connect until the current sessions have gone away.

Why armed rather than down: a socket picks its source address when it connects. If the
tunnel came up only afterwards, the first attempt would leave with your Wi-Fi address and be
dropped by the WireGuard server. With the interface already present the socket uses the
tunnel address, wireguard-go holds the packets until the handshake, and nothing fails; the
session just opens a second or two later.

Requirements:

- A split-tunnel config (specific `AllowedIPs`; the item is disabled for `0.0.0.0/0`).
- `./sudoers.sh`, which also installs the small root helper WGBar uses for this
  (`/usr/local/libexec/wgbar-helper`, source in `helper/`). Re-run it after updating WGBar
  if `install.sh` says the helper is out of date.
- Remote machines addressed by IP, or by names your normal DNS resolves to their tunnel
  address. A name that only VPN DNS knows will not trigger the connection yet.

The idle time is configurable:

```sh
defaults write org.wgbar.WGBar onDemandIdle 60   # seconds without traffic before disconnecting
```

Quitting WGBar takes the armed interface down again; relaunching (or login) re-arms it.
````

In the `## How it works` section add a bullet:

```markdown
- Connect on Demand polls `netstat -n -p tcp` once a second and calls
  `sudo -n /usr/local/libexec/wgbar-helper arm|connect|disconnect|down|status <tunnel>`;
  the helper derives an "armed" config (no `DNS`, `Endpoint`, `PersistentKeepalive`) from
  yours for `wg-quick up`, and uses `wg set` for connect/disconnect. WGBar applies and
  restores DNS itself via `networksetup`.
```

Update the `## Hacking` section's first sentence to: "The app is `main.swift`; `DNSGuard.swift`, `Updater.swift` and `OnDemand.swift` hold the testable parts; `helper/wgbar-helper` is the root helper."

Also fix the `sudoers.sh` paragraph in **Optional: toggle without a password prompt** to mention that it installs the helper too: change "This writes `/etc/sudoers.d/wgbar` allowing **only your user** to run" to "This installs the WGBar helper and writes `/etc/sudoers.d/wgbar` allowing **only your user** to run the helper and".

- [ ] **Step 2: Commit the docs**

```bash
git add README.md
git commit -m "README: Connect on Demand"
```

- [ ] **Step 3: Manual acceptance (needs the user's password once for sudoers.sh)**

Run through and record the result of each item; report any deviation rather than working around it.

1. `./sudoers.sh` (shows the rule and the helper path, asks for the password once). Then
   `sudo -n /usr/local/libexec/wgbar-helper status ileasing` prints `off` or `armed`/`connected` without a prompt.
2. `./install.sh`. If the tunnel is currently up classically, left-click to bring it down.
3. Right-click → **Connect on Demand**. Icon becomes `shield` (outline). Check:
   ```sh
   netstat -rn -f inet | grep -E '^10(\.42)?/'          # two routes on a utun
   scutil --dns | grep nameserver | sort -u             # your home DNS, not 10.0.0.22
   sudo -n /usr/local/libexec/wgbar-helper status ileasing   # armed
   ```
4. In the Windows App open the PC saved as `10.42.1.63` (if `nws.office.ileasing.eu` is the
   one you use, save it by its IP 10.42.1.9 first; see the spec's phase 2 note). Time from
   click to desktop; expected within ~3 s of the usual time, no error dialog. Icon is `shield.fill`;
   `scutil --dns` now lists 10.0.0.22 and 10.0.0.4.
5. Close the VM window. Within ~30 s the icon returns to `shield` and `scutil --dns` shows home DNS.
6. Left-click (manual connect). Wait 45 s: still `shield.fill` (sticky). Left-click again: `shield`.
7. Open the VM, then left-click while the session is up: session drops, status line says
   `Paused`; the Windows App's reconnect attempts do not bring the tunnel back. Close the
   Windows App window; after ~30 s the status line says `Armed, connects on demand`.
8. Quit WGBar from the menu: `ls /var/run/wireguard` shows no `ileasing.name`. Relaunch from
   `~/Applications`: icon is `shield` again with no password prompt.
9. Right-click → untick **Connect on Demand**: interface goes down, classic left-click works.

- [ ] **Step 4: Final verification and wrap-up**

Run: `./test.sh && ./build.sh && git status --short`
Expected: all tests pass, build succeeds, working tree clean. Then follow the `superpowers:finishing-a-development-branch` skill (work is on `main` in this repo; there is no PR flow, so this means confirming the commits are in place and nothing is left uncommitted).
