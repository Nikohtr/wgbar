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
