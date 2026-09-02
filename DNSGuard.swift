// DNSGuard — detects and repairs DNS servers that wg-quick left behind on macOS.
//
// wg-quick applies a config's `DNS =` servers to every network service with `networksetup`,
// and the only thing that restores the originals is a background monitor shell it leaves
// running. If that shell dies without its exit trap (shutdown, crash, sleep races) the VPN
// DNS stays in the system preferences — across reboots — with no tunnel to reach it.
//
// Pure functions here are covered by tests/DNSGuardTests.swift (./test.sh).

import Foundation

/// The DNS server addresses declared by a wg-quick config (`DNS =` lines, comma-separated;
/// entries that are not IP addresses are search domains and are skipped, as wg-quick does).
func configDNSServers(_ text: String) -> [String] {
    var servers: [String] = []
    for line in text.split(separator: "\n") {
        let parts = line.split(separator: "=", maxSplits: 1).map { $0.trimmingCharacters(in: .whitespaces) }
        guard parts.count == 2, parts[0].lowercased() == "dns" else { continue }
        for entry in parts[1].split(separator: ",").map({ $0.trimmingCharacters(in: .whitespaces) }) {
            if isIPAddress(entry) { servers.append(entry) }
        }
    }
    return servers
}

/// IPv4 (digits and dots) or IPv6 (hex digits, colons, optional embedded IPv4) — nothing else.
func isIPAddress(_ s: String) -> Bool {
    guard !s.isEmpty else { return false }
    if s.contains(":") { return s.allSatisfy { $0.isHexDigit || $0 == ":" || $0 == "." } }
    return s.allSatisfy { $0.isNumber || $0 == "." }
}

/// Service names from `networksetup -listallnetworkservices` (header dropped, disabled marker stripped).
func parseServiceList(_ output: String) -> [String] {
    output.split(separator: "\n").dropFirst().map { line in
        let s = line.trimmingCharacters(in: .whitespaces)
        return s.hasPrefix("*") ? String(s.dropFirst()) : s
    }.filter { !$0.isEmpty }
}

/// Server list from `networksetup -getdnsservers <service>`: one address per line, or a sentence
/// ("There aren't any DNS Servers set…" / "** Error…") meaning none.
func parseDNSServers(_ output: String) -> [String] {
    let lines = output.split(separator: "\n").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
    return lines.allSatisfy(isIPAddress) ? lines : []
}

/// Services whose current DNS servers include any VPN address while the tunnel is down.
/// A service running the user's own DNS (e.g. 1.1.1.1) is never reported.
func staleServices(_ current: [String: [String]], vpnDNS: Set<String>) -> [String] {
    current.filter { !vpnDNS.isDisjoint(with: $0.value) }.keys.sorted()
}

struct CmdResult { let status: Int32; let output: String }

/// Per-service DNS settings as taken before connecting: `["Wi-Fi": ["servers": [...], "search": [...]]]`.
/// A plain plist-compatible shape so it can be stored in UserDefaults as is.
typealias DNSSnapshot = [String: [String: [String]]]

struct DNSGuard {
    /// Every DNS server address declared by the tunnel configs.
    let vpnDNS: Set<String>
    /// Runs an executable with arguments (injected so tests can fake `networksetup`).
    let run: (String, [String]) -> CmdResult

    private let networksetup = "/usr/sbin/networksetup"

    private func services() -> [String] {
        parseServiceList(run(networksetup, ["-listallnetworkservices"]).output)
    }

    /// Current DNS servers of every network service.
    func currentDNS() -> [String: [String]] {
        var result: [String: [String]] = [:]
        for s in services() { result[s] = parseDNSServers(run(networksetup, ["-getdnsservers", s]).output) }
        return result
    }

    /// Services still pointing at a VPN DNS server.
    func stale() -> [String] { staleServices(currentDNS(), vpnDNS: vpnDNS) }

    /// Record what to restore after the tunnel comes down. A service already holding a VPN
    /// address (left behind earlier) is recorded as empty so the repair never re-applies it.
    func snapshot() -> DNSSnapshot {
        var snap: DNSSnapshot = [:]
        for s in services() {
            let servers = parseDNSServers(run(networksetup, ["-getdnsservers", s]).output)
            let search = parseSearchDomains(run(networksetup, ["-getsearchdomains", s]).output)
            let isStale = !vpnDNS.isDisjoint(with: servers)
            snap[s] = ["servers": isStale ? [] : servers, "search": isStale ? [] : search]
        }
        return snap
    }

    /// Put every stale service back to its snapshot (or clear it, which means DHCP-provided DNS).
    func repair(snapshot: DNSSnapshot) -> (fixed: [String], errors: [String]) {
        var fixed: [String] = [], errors: [String] = []
        for s in stale() {
            let servers = snapshot[s]?["servers"] ?? []
            let search = snapshot[s]?["search"] ?? []
            let r1 = run(networksetup, ["-setdnsservers", s] + (servers.isEmpty ? ["Empty"] : servers))
            let r2 = run(networksetup, ["-setsearchdomains", s] + (search.isEmpty ? ["Empty"] : search))
            let failed = [r1, r2].filter { $0.status != 0 || $0.output.contains("Error") }
            if failed.isEmpty { fixed.append(s) }
            else { errors.append("\(s): " + failed.map(\.output).joined(separator: "; ")) }
        }
        return (fixed, errors)
    }
}

/// `networksetup -getsearchdomains` output: one domain per line, or a sentence meaning none.
func parseSearchDomains(_ output: String) -> [String] {
    let lines = output.split(separator: "\n").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
    return lines.allSatisfy { !$0.contains(" ") } ? lines : []
}
