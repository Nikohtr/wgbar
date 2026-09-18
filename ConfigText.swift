// ConfigText — reading a tunnel config that this process may not be allowed to open.
//
// The configs hold private keys, so they are root-owned (0600) and WGBar cannot read them; the
// root helper prints them without their key material (`wgbar-helper print-public`), which is all
// WGBar needs: DNS, AllowedIPs, Address, the text behind the menu. Opening the file is still
// tried first, for folders the user does own.
//
// Reads happen from the menu, from the DNS check and from the on-demand queue, so the answer is
// kept until the file's modification date changes — otherwise this would be a sudo per second —
// and the cache is behind a lock.

import Foundation

final class ConfigReader {
    private let read: (String) -> String?
    private let viaHelper: (String) -> String?
    private let modified: (String) -> Date?
    private var cache: [String: (stamp: Date, text: String)] = [:]
    private let lock = NSLock()

    init(read: @escaping (String) -> String?,
         viaHelper: @escaping (String) -> String?,
         modified: @escaping (String) -> Date?) {
        self.read = read; self.viaHelper = viaHelper; self.modified = modified
    }

    /// The config text for `tunnel`, or nil when neither the file nor the helper can produce it.
    func text(_ tunnel: String) -> String? {
        let stamp = modified(tunnel)
        if let stamp {
            lock.lock(); let hit = cache[tunnel]; lock.unlock()
            if let hit, hit.stamp == stamp { return hit.text }
        }
        guard let text = read(tunnel) ?? viaHelper(tunnel) else { return nil }
        if let stamp {
            lock.lock(); cache[tunnel] = (stamp, text); lock.unlock()
        }
        return text
    }
}
