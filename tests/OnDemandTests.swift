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
}
