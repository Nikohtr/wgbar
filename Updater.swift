// Updater — "Check for Updates…" for an app installed from a git clone (see install.sh).
// The clone's path is recorded by install.sh in defaults (`repoDir`). Checking fetches
// origin and compares; applying pulls and re-runs install.sh, which kills and relaunches WGBar.

import Foundation

enum UpdateStatus: Equatable {
    case upToDate(revision: String)
    case available(commits: [String])
    case failed(String)
}

struct Updater {
    /// The git clone WGBar was installed from ("" when unknown).
    let repoDir: String
    /// Runs an executable with arguments (injected so tests can fake git).
    let run: (String, [String]) -> CmdResult

    static let noRepoMessage = "WGBar does not know where its source checkout is. Run ./install.sh from the wgbar folder once."

    private func git(_ args: [String]) -> CmdResult { run("/usr/bin/git", ["-C", repoDir] + args) }

    /// Fetch origin and report whether the upstream branch has commits we lack.
    func check() -> UpdateStatus {
        guard !repoDir.isEmpty else { return .failed(Updater.noRepoMessage) }
        let fetch = git(["fetch", "--quiet", "origin"])
        guard fetch.status == 0 else { return .failed(fetch.output) }
        let count = git(["rev-list", "--count", "HEAD..@{u}"])
        guard count.status == 0 else { return .failed(count.output) }
        if Int(count.output) ?? 0 == 0 {
            return .upToDate(revision: git(["rev-parse", "--short", "HEAD"]).output)
        }
        let log = git(["log", "--oneline", "HEAD..@{u}"]).output
        return .available(commits: log.split(separator: "\n").map(String.init))
    }

    /// Fast-forward the clone. Returns an error message, or nil on success.
    func pull() -> String? {
        let r = git(["pull", "--ff-only", "--quiet"])
        return r.status == 0 ? nil : r.output
    }
}
