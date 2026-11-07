// Swift twin of examples/logic_bench.rs. Built by bench/compare.sh against the Swift sources on
// `main` (OnDemand.swift, DNSGuard.swift) plus a copy of main.swift's tunnelAddress logic.
import Foundation

func tunnelAddressText(_ text: String) -> String? {
    for line in text.split(separator: "\n") {
        let parts = line.split(separator: "=", maxSplits: 1).map { $0.trimmingCharacters(in: .whitespaces) }
        if parts.count == 2, parts[0].lowercased() == "address" { return parts[1] }
    }
    return nil
}

/// main.swift's `run`, minus the PATH tweak: what the app pays to spawn lsof.
func spawn(_ exe: String, _ args: [String]) -> String {
    let p = Process()
    p.executableURL = URL(fileURLWithPath: exe)
    p.arguments = args
    let pipe = Pipe()
    p.standardOutput = pipe
    p.standardError = pipe
    try! p.run()
    let data = pipe.fileHandleForReading.readDataToEndOfFile()
    p.waitUntilExit()
    return String(decoding: data, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
}

/// CPU (user+sys) of this process and its waited-for children, in ns.
func cpuNs() -> Double {
    var a = rusage(), b = rusage()
    getrusage(RUSAGE_SELF, &a); getrusage(RUSAGE_CHILDREN, &b)
    func ns(_ t: timeval) -> Double { Double(t.tv_sec) * 1e9 + Double(t.tv_usec) * 1e3 }
    return ns(a.ru_utime) + ns(a.ru_stime) + ns(b.ru_utime) + ns(b.ru_stime)
}

@inline(never) func blackHole(_ x: Int) { if x == -1 { print(x) } }

func medianNs(_ iters: Int, _ f: () -> Int) -> Double {
    var runs: [Double] = []
    for _ in 0..<7 {
        let start = DispatchTime.now().uptimeNanoseconds
        var sink = 0
        for _ in 0..<iters { sink &+= f() }
        blackHole(sink)
        runs.append(Double(DispatchTime.now().uptimeNanoseconds - start) / Double(iters))
    }
    return runs.sorted()[3]
}

@main struct LogicBench {
    static func main() {
        let args = CommandLine.arguments
        let lsof = try! String(contentsOfFile: args[1], encoding: .utf8)
        let conf = try! String(contentsOfFile: args[2], encoding: .utf8)
        let allowed = parseAllowedIPs(conf)
        let sockets = parseLsof(lsof)

        let parse = medianNs(2_000) { parseLsof(lsof).count }
        let bound = medianNs(2_000) { vpnBound(sockets, allowed).count }
        let tick = medianNs(2_000) { vpnBound(parseLsof(lsof), allowed).count }
        let config = medianNs(200_000) {
            configDNSServers(conf).count + parseAllowedIPs(conf).count + (tunnelAddressText(conf)?.count ?? 0)
        }
        print(String(format: "parse_lsof_400 %.0f", parse))
        print(String(format: "vpn_bound_400 %.0f", bound))
        print(String(format: "poll_tick_400 %.0f", tick))
        print(String(format: "parse_config %.0f", config))

        // One real on-demand poll as the app does it: spawn lsof, parse, filter.
        let polls = 50
        let c0 = cpuNs()
        let live = medianNs(polls / 7 + 1) { vpnBound(parseLsof(spawn("/usr/sbin/lsof", lsofArgs)), allowed).count }
        let c1 = cpuNs()
        print(String(format: "poll_live_ns %.0f", live))
        print(String(format: "poll_live_cpu_ns %.0f", (c1 - c0) / Double(7 * (polls / 7 + 1))))
    }
}
