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

    // --- controller (fake sudo/netstat/ping) ----------------------------------------
    /// Counts the controller's callbacks; `log` records callback/helper-verb order.
    final class Events { var connected = 0, disconnected = 0; var errors: [String] = []; var states: [OnDemandState] = []; var log: [String] = [] }
    /// Fakes every command the controller runs and records the helper verbs it asked for.
    final class FakeSystem {
        var netstat = ""
        var netstatFails = false
        var helperFails: Set<String> = []          // verbs that fail
        var sudoNeedsPassword = false
        var statusOutput = "armed"
        var verbs: [String] = []
        var pings: [String] = []
        var events: Events?
        var clock = Date(timeIntervalSince1970: 1_000_000)
        func advance(_ s: TimeInterval) { clock = clock.addingTimeInterval(s) }
        func run(_ exe: String, _ args: [String]) -> CmdResult {
            switch exe {
            case "/usr/sbin/netstat": return netstatFails ? CmdResult(status: 1, output: "") : CmdResult(status: 0, output: netstat)
            case "/sbin/ping": pings.append(args.last ?? ""); return CmdResult(status: 0, output: "")
            case "/usr/bin/sudo":
                expect(Array(args.prefix(2)), ["-n", OnDemandController.helperPath], "controller: sudo -n helper")
                expect(args.last, "ileasing", "controller: tunnel name passed")
                if sudoNeedsPassword { return CmdResult(status: 1, output: "sudo: a password is required") }
                let verb = args[2]; verbs.append(verb); events?.log.append(verb)
                if helperFails.contains(verb) { return CmdResult(status: 1, output: "wg-quick: boom") }
                return CmdResult(status: 0, output: verb == "status" ? statusOutput : "")
            default: return CmdResult(status: 1, output: "unexpected \(exe)")
            }
        }
        func controller(kick: String? = "10.0.0.22") -> (OnDemandController, Events) {
            let ev = Events()
            events = ev
            let c = OnDemandController(tunnel: "ileasing", allowed: [CIDR("10.0.0.0/24")!, CIDR("10.42.0.0/16")!],
                                       idle: 30, kick: kick, run: run, now: { self.clock })
            c.onConnected = { ev.connected += 1; ev.log.append("onConnected") }
            c.onDisconnected = { ev.disconnected += 1; ev.log.append("onDisconnected") }
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
        expect(ev.log, ["arm", "connect", "onConnected"], "cycle: onConnected fires after helper connect")
        sys.netstat = rdp; sys.advance(600); c.tick()
        expect(c.state, .connected, "cycle: established session keeps it up")
        sys.netstat = web; sys.advance(20); c.tick()
        expect(c.state, .connected, "cycle: 20 s quiet is not idle yet")
        sys.advance(10); c.tick()
        expect(c.state, .armed, "cycle: 30 s quiet disconnects back to armed")
        expect(sys.verbs, ["arm", "connect", "disconnect"], "cycle: helper disconnect called")
        expect(ev.disconnected, 1, "cycle: onDisconnected fired (DNS restored)")
        expect(ev.states, [.armed, .connected, .armed], "cycle: state changes reported")
        expect(ev.log, ["arm", "connect", "onConnected", "onDisconnected", "disconnect"], "cycle: DNS restored before helper disconnect")
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
    do {   // reconcile over an already-connected sticky controller keeps it sticky
        let sys = FakeSystem(); let (c, _) = sys.controller()
        c.arm(); c.manualToggle()
        expect(c.state, .connected, "reconcile sticky: manual connect first")
        sys.statusOutput = "connected"; c.reconcile()
        sys.netstat = ""; sys.advance(3600); c.tick()
        expect(c.state, .connected, "reconcile: a re-confirmed manual connection stays sticky")
        expect(sys.verbs.contains("disconnect"), false, "reconcile: sticky connection is not idled out")
    }
    do {   // shutdown
        let sys = FakeSystem(); let (c, ev) = sys.controller()
        c.arm(); sys.netstat = syn; c.tick()
        c.shutdown()
        expect(sys.verbs, ["arm", "connect", "down"], "shutdown: connected → down (helper's down handles the peer)")
        expect(ev.disconnected, 1, "shutdown: DNS restored first")
        expect(c.state, .off, "shutdown: off")
        expect(ev.log, ["arm", "connect", "onConnected", "onDisconnected", "down"], "shutdown: DNS restored before helper down")
        let sys2 = FakeSystem(); let (c2, ev2) = sys2.controller()
        c2.shutdown()
        expect(sys2.verbs, ["down"], "shutdown: always issues down so a failed connect cannot orphan the interface")
        expect(ev2.disconnected, 0, "shutdown: no DNS restore when off")
    }
    do {   // manualToggle from .off re-arms
        let sys = FakeSystem(); let (c, _) = sys.controller()
        c.manualToggle()
        expect(sys.verbs, ["arm"], "manual from off: calls helper arm")
        expect(c.state, .armed, "manual from off: state armed")
    }
    do {   // failing disconnect still restores DNS and goes off
        let sys = FakeSystem(); sys.helperFails = ["disconnect"]; let (c, ev) = sys.controller()
        c.arm(); sys.netstat = syn; c.tick()
        expect(c.state, .connected, "failing disconnect: connected first")
        sys.netstat = ""; sys.advance(30); c.tick()
        expect(c.state, .off, "failing disconnect: state off")
        expect(ev.disconnected, 1, "failing disconnect: DNS restored anyway")
        expect(ev.errors, ["wg-quick: boom"], "failing disconnect: helper output surfaced")
    }
    do {   // failing status in reconcile
        let sys = FakeSystem(); sys.helperFails = ["status"]; let (c, ev) = sys.controller()
        c.reconcile()
        expect(c.state, .off, "failing reconcile: state off")
        expect(ev.errors, ["wg-quick: boom"], "failing reconcile: helper output surfaced")
    }
    do {   // netstat failure during a tick is treated as unknown, not quiet — no idle disconnect
        let sys = FakeSystem(); let (c, ev) = sys.controller()
        c.arm(); sys.netstat = syn; c.tick()
        expect(c.state, .connected, "netstat failure: connected first")
        sys.netstatFails = true; sys.advance(3600); c.tick()
        expect(c.state, .connected, "netstat failure: stays connected, does not idle out")
        expect(sys.verbs, ["arm", "connect"], "netstat failure: no disconnect verb called")
        expect(ev.disconnected, 0, "netstat failure: DNS untouched")
    }
}
