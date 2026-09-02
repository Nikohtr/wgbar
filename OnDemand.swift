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

    /// nil when `netstat` itself failed — a failed read must not be mistaken for "no traffic".
    private func sockets() -> [TCPSocket]? {
        let r = run("/usr/sbin/netstat", ["-n", "-p", "tcp"])
        guard r.status == 0 else { return nil }
        return vpnBound(parseNetstat(r.output), allowed)
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
        // Only a genuine adoption clears stickiness; re-confirming a manual connection keeps it.
        case "connected": if state != .connected { sticky = false }; lastSeen = now(); set(.connected)
        case "armed":     set(.armed)
        default:          set(.off)
        }
    }

    /// One poll: look at the sockets, act on `decide`. Skipped (state unchanged) if `netstat` fails.
    func tick() {
        guard state != .off else { return }
        guard let socks = sockets() else { return }
        let has = !socks.isEmpty
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
    /// `down` is issued unconditionally — a failed connect or disconnect leaves the state `.off`
    /// while the OS interface is still up, and the helper's `down` exits 0 when there is nothing
    /// to take down, so this is both safe and the only way to guarantee nothing is orphaned.
    func shutdown() {
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
        set((sockets() ?? []).isEmpty ? .armed : .paused)
    }
}
