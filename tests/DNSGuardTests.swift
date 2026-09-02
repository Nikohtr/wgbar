// Tests for DNSGuard.swift. Run with ./test.sh (no XCTest: swiftc + a tiny assert harness).
import Foundation

var failures = 0
var passes = 0

func expect<T: Equatable>(_ actual: T, _ expected: T, _ name: String, file: String = #file, line: Int = #line) {
    if actual == expected { passes += 1 }
    else { failures += 1; print("FAIL \(name) (line \(line))\n  expected: \(expected)\n  actual:   \(actual)") }
}

@main struct Tests {
    static func main() {
        // --- parsing -----------------------------------------------------
        expect(configDNSServers("[Interface]\nAddress = 10.0.0.2/32\nDNS = 10.0.0.22, 10.0.0.4\n"),
               ["10.0.0.22", "10.0.0.4"], "config: comma-separated DNS line")
        expect(configDNSServers("dns=10.0.0.22\nDNS = 10.0.0.4\n"),
               ["10.0.0.22", "10.0.0.4"], "config: multiple DNS lines, case-insensitive key, no spaces")
        expect(configDNSServers("DNS = 10.0.0.22, corp.example\n"),
               ["10.0.0.22"], "config: search domains on the DNS line are not servers")
        expect(configDNSServers("[Interface]\nAddress = 10.0.0.2/32\n"),
               [], "config: no DNS line")
        expect(configDNSServers("DNS = fd00::1, 10.0.0.22\n"),
               ["fd00::1", "10.0.0.22"], "config: IPv6 server")

        expect(parseServiceList("An asterisk (*) denotes that a network service is disabled.\nWi-Fi\n*Thunderbolt Bridge\niPhone USB\n"),
               ["Wi-Fi", "Thunderbolt Bridge", "iPhone USB"], "services: header dropped, disabled marker stripped")

        expect(parseDNSServers("There aren't any DNS Servers set on Wi-Fi."),
               [], "getdnsservers: none set")
        expect(parseDNSServers("10.0.0.22\n10.0.0.4"),
               ["10.0.0.22", "10.0.0.4"], "getdnsservers: one per line")
        expect(parseDNSServers("** Error: The parameters were not valid."),
               [], "getdnsservers: error text is not a server")

        // --- detection ---------------------------------------------------
        let vpn: Set<String> = ["10.0.0.22", "10.0.0.4"]
        expect(staleServices(["Wi-Fi": [], "iPhone USB": ["10.0.0.22", "10.0.0.4"], "AX88179A": ["10.0.0.4"]], vpnDNS: vpn),
               ["AX88179A", "iPhone USB"], "stale: services holding any VPN address, sorted")
        expect(staleServices(["Wi-Fi": ["1.1.1.1"], "Bridge": []], vpnDNS: vpn),
               [], "stale: user's own DNS is never stale")
        expect(staleServices(["Wi-Fi": ["10.0.0.22"]], vpnDNS: []),
               [], "stale: nothing to match when configs have no DNS")

        // --- orchestration (fake networksetup) --------------------------------
        /// A fake `networksetup`: answers from `state`, records every set call.
        final class Fake {
            var dns: [String: [String]]
            var search: [String: [String]]
            var calls: [[String]] = []
            init(dns: [String: [String]], search: [String: [String]] = [:]) { self.dns = dns; self.search = search }
            func run(_ exe: String, _ args: [String]) -> CmdResult {
                expect(exe, "/usr/sbin/networksetup", "fake: only networksetup is invoked")
                switch args.first {
                case "-listallnetworkservices":
                    return CmdResult(status: 0, output: "An asterisk (*) denotes that a network service is disabled.\n" + dns.keys.sorted().joined(separator: "\n"))
                case "-getdnsservers":
                    let list = dns[args[1]] ?? []
                    return CmdResult(status: 0, output: list.isEmpty ? "There aren't any DNS Servers set on \(args[1])." : list.joined(separator: "\n"))
                case "-getsearchdomains":
                    let list = search[args[1]] ?? []
                    return CmdResult(status: 0, output: list.isEmpty ? "There aren't any Search Domains set on \(args[1])." : list.joined(separator: "\n"))
                case "-setdnsservers", "-setsearchdomains":
                    calls.append(args)
                    return CmdResult(status: 0, output: "")
                default:
                    return CmdResult(status: 1, output: "** Error: unexpected \(args)")
                }
            }
        }

        let vpn2: Set<String> = ["10.0.0.22"]
        do {   // scan reads every service
            let f = Fake(dns: ["Wi-Fi": [], "iPhone USB": ["10.0.0.22"]])
            let g = DNSGuard(vpnDNS: vpn2, run: f.run)
            expect(g.currentDNS(), ["Wi-Fi": [], "iPhone USB": ["10.0.0.22"]], "scan: one entry per service")
            expect(g.stale(), ["iPhone USB"], "scan: stale services")
        }
        do {   // snapshot records the user's own settings, never a stale VPN entry
            let f = Fake(dns: ["Wi-Fi": ["1.1.1.1"], "iPhone USB": ["10.0.0.22"]], search: ["Wi-Fi": ["home.lan"]])
            let g = DNSGuard(vpnDNS: vpn2, run: f.run)
            let snap = g.snapshot()
            expect(snap["Wi-Fi"]?["servers"] ?? [], ["1.1.1.1"], "snapshot: user DNS kept")
            expect(snap["Wi-Fi"]?["search"] ?? [], ["home.lan"], "snapshot: search domains kept")
            expect(snap["iPhone USB"]?["servers"] ?? ["unset"], [], "snapshot: already-stale service recorded as empty")
        }
        do {   // repair restores from the snapshot, or clears when there is none
            let f = Fake(dns: ["Wi-Fi": ["10.0.0.22"], "AX88179A": ["10.0.0.22"], "Bridge": []])
            let g = DNSGuard(vpnDNS: vpn2, run: f.run)
            let snap: DNSSnapshot = ["Wi-Fi": ["servers": ["1.1.1.1"], "search": ["home.lan"]]]
            let repaired = g.repair(snapshot: snap)
            expect(repaired.fixed, ["AX88179A", "Wi-Fi"], "repair: returns the services it touched")
            expect(repaired.errors, [], "repair: no errors")
            expect(f.calls.contains(["-setdnsservers", "Wi-Fi", "1.1.1.1"]), true, "repair: Wi-Fi restored from snapshot")
            expect(f.calls.contains(["-setsearchdomains", "Wi-Fi", "home.lan"]), true, "repair: Wi-Fi search domains restored")
            expect(f.calls.contains(["-setdnsservers", "AX88179A", "Empty"]), true, "repair: no snapshot → Empty")
            expect(f.calls.contains(["-setsearchdomains", "AX88179A", "Empty"]), true, "repair: no snapshot → search Empty")
            expect(f.calls.contains { $0[1] == "Bridge" }, false, "repair: untouched services are left alone")
        }
        do {   // repair reports networksetup errors instead of hiding them
            let f = Fake(dns: ["Wi-Fi": ["10.0.0.22"]])
            let failing: (String, [String]) -> CmdResult = { exe, args in
                args.first == "-setdnsservers" ? CmdResult(status: 1, output: "** Error: nope") : f.run(exe, args)
            }
            let g = DNSGuard(vpnDNS: vpn2, run: failing)
            let r = g.repair(snapshot: [:])
            expect(r.fixed, [], "repair error: nothing counted as fixed")
            expect(r.errors, ["Wi-Fi: ** Error: nope"], "repair error: surfaced per service")
        }

        print("\(passes) passed, \(failures) failed")
        exit(failures == 0 ? 0 : 1)
    }
}
