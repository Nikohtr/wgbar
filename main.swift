// WGBar — a tiny macOS menu bar toggle for a wg-quick (Homebrew wireguard-tools) tunnel.
// Left-click the icon: toggle the tunnel. Right-click: menu.

import Cocoa
import ServiceManagement
import UserNotifications

let defaults = UserDefaults.standard

/// Where wg-quick may live: Homebrew (Apple silicon / Intel) or MacPorts.
/// Override with:  defaults write org.wgbar.WGBar wgQuick /path/to/wg-quick
let wgQuick: String = {
    if let custom = defaults.string(forKey: "wgQuick"), FileManager.default.isExecutableFile(atPath: custom) { return custom }
    return ["/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin"].map { "\($0)/wg-quick" }
        .first { FileManager.default.isExecutableFile(atPath: $0) } ?? "/opt/homebrew/bin/wg-quick"
}()
let brewBin = (wgQuick as NSString).deletingLastPathComponent

/// Folders where tunnel configs are looked for, in order.
let defaultConfDirs = ["/opt/homebrew/etc/wireguard", "/usr/local/etc/wireguard",
                       "/opt/local/etc/wireguard", "/etc/wireguard"]

func confNames(in dir: String) -> [String] {
    let names = (try? FileManager.default.contentsOfDirectory(atPath: dir)) ?? []
    return names.filter { $0.hasSuffix(".conf") }.map { String($0.dropLast(5)) }.sorted()
}

/// The config folder in use: the user's choice (menu "Config Folder…" or
/// `defaults write org.wgbar.WGBar confDir /path`), else the first default folder
/// that has configs, else the first that exists.
var confDir: String {
    if let custom = defaults.string(forKey: "confDir"), !custom.isEmpty { return custom }
    return defaultConfDirs.first { !confNames(in: $0).isEmpty }
        ?? defaultConfDirs.first { FileManager.default.fileExists(atPath: $0) }
        ?? defaultConfDirs[0]
}

func confPath(_ tunnel: String) -> String { "\(confDir)/\(tunnel).conf" }
func runFile(_ tunnel: String)  -> String { "/var/run/wireguard/\(tunnel).name" }
func availableTunnels() -> [String] { confNames(in: confDir) }

/// Run a command synchronously with Homebrew on PATH, capturing stdout+stderr.
func run(_ exe: String, _ args: [String]) -> CmdResult {
    let p = Process()
    p.executableURL = URL(fileURLWithPath: exe)
    p.arguments = args
    var env = ProcessInfo.processInfo.environment
    env["PATH"] = "\(brewBin):" + (env["PATH"] ?? "/usr/bin:/bin:/usr/sbin:/sbin")
    p.environment = env
    let pipe = Pipe()
    p.standardOutput = pipe
    p.standardError = pipe
    do { try p.run() } catch { return CmdResult(status: -1, output: "\(error)") }
    let data = pipe.fileHandleForReading.readDataToEndOfFile()
    p.waitUntilExit()
    return CmdResult(status: p.terminationStatus,
                     output: String(decoding: data, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines))
}

/// Bring the tunnel up or down. Returns an error message, or nil on success/cancel.
func wgQuick(_ action: String, _ tunnel: String) -> String? {
    // 1. Passwordless sudo, if a sudoers rule allows it (see sudoers.sh).
    let conf = confPath(tunnel)
    let r = run("/usr/bin/sudo", ["-n", wgQuick, action, conf])
    if r.status == 0 { return nil }
    if !r.output.contains("password") { return r.output }   // a real wg-quick failure

    // 2. Otherwise the standard macOS administrator dialog (supports Touch ID).
    let shell = "PATH=\(brewBin):$PATH '\(wgQuick)' \(action) '\(conf)'"
    let script = "do shell script \"\(shell)\" with administrator privileges"
    let r2 = run("/usr/bin/osascript", ["-e", script])
    if r2.status == 0 { return nil }
    if r2.output.contains("-128") { return nil }             // user pressed Cancel
    return r2.output
}

/// Every DNS server declared by any config in the folder (a stale entry may come from a
/// tunnel other than the selected one).
func allConfigDNS() -> Set<String> {
    var all = Set<String>()
    for name in availableTunnels() {
        if let text = try? String(contentsOfFile: confPath(name), encoding: .utf8) { all.formUnion(configDNSServers(text)) }
    }
    return all
}

/// True while any wg-quick tunnel is up (not only the selected one), when VPN DNS is legitimate.
func anyTunnelUp() -> Bool {
    let names = (try? FileManager.default.contentsOfDirectory(atPath: "/var/run/wireguard")) ?? []
    return names.contains { $0.hasSuffix(".name") }
}

let networkPrefs = "/Library/Preferences/SystemConfiguration/preferences.plist"
func networkPrefsModified() -> Date {
    (try? FileManager.default.attributesOfItem(atPath: networkPrefs))?[.modificationDate] as? Date ?? .distantPast
}

func tunnelAddress(_ tunnel: String) -> String? {
    guard let text = try? String(contentsOfFile: confPath(tunnel), encoding: .utf8) else { return nil }
    for line in text.split(separator: "\n") {
        let parts = line.split(separator: "=", maxSplits: 1).map { $0.trimmingCharacters(in: .whitespaces) }
        if parts.count == 2, parts[0].lowercased() == "address" { return parts[1] }
    }
    return nil
}

final class AppDelegate: NSObject, NSApplicationDelegate, NSMenuDelegate {
    private var statusItem: NSStatusItem!
    private let menu = NSMenu()
    private let statusLine = NSMenuItem(title: "", action: nil, keyEquivalent: "")
    private let dnsLine    = NSMenuItem(title: "", action: nil, keyEquivalent: "")
    private let toggleItem = NSMenuItem(title: "", action: #selector(toggle), keyEquivalent: "")
    private let loginItem  = NSMenuItem(title: "Launch at Login", action: #selector(toggleLogin), keyEquivalent: "")
    private let tunnelItem = NSMenuItem(title: "Tunnel", action: nil, keyEquivalent: "")
    private let folderItem = NSMenuItem(title: "Config Folder…", action: #selector(chooseFolder), keyEquivalent: "")
    private let repairItem = NSMenuItem(title: "Repair DNS", action: #selector(repairDNS), keyEquivalent: "")
    private var tunnel = ""
    private var isUp = false
    private var busy = false
    /// Services still pointing at VPN DNS that could not be repaired automatically.
    private var dnsIssue: [String] = []
    private var dnsChecking = false
    private var prefsSeen = networkPrefsModified()

    /// The tunnel to control: the remembered choice if its config still exists, else the first one found.
    private func resolveTunnel() {
        let all = availableTunnels()
        let saved = defaults.string(forKey: "tunnel") ?? ""
        tunnel = all.contains(saved) ? saved : (all.first ?? "")
    }

    func applicationDidFinishLaunching(_ note: Notification) {
        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        if let button = statusItem.button {
            button.target = self
            button.action = #selector(clicked)
            button.sendAction(on: [.leftMouseUp, .rightMouseUp])
        }

        menu.delegate = self
        statusLine.isEnabled = false
        dnsLine.isEnabled = false
        toggleItem.target = self
        loginItem.target = self
        tunnelItem.submenu = NSMenu()
        folderItem.target = self
        repairItem.target = self
        menu.addItem(statusLine)
        menu.addItem(dnsLine)
        menu.addItem(toggleItem)
        menu.addItem(repairItem)
        menu.addItem(.separator())
        menu.addItem(tunnelItem)
        menu.addItem(folderItem)
        menu.addItem(loginItem)
        menu.addItem(.separator())
        menu.addItem(NSMenuItem(title: "Quit WGBar", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q"))

        // Register as a login item on first launch; the menu checkbox controls it afterwards.
        if !defaults.bool(forKey: "didRegisterLoginItem") {
            try? SMAppService.mainApp.register()
            defaults.set(true, forKey: "didRegisterLoginItem")
        }

        resolveTunnel()
        refresh()
        Timer.scheduledTimer(withTimeInterval: 2, repeats: true) { [weak self] _ in self?.refresh() }

        // DNS guard: a reboot or crash with the tunnel up leaves VPN DNS behind; so can waking.
        checkDNS()
        NSWorkspace.shared.notificationCenter.addObserver(forName: NSWorkspace.didWakeNotification, object: nil, queue: .main) { [weak self] _ in
            DispatchQueue.main.asyncAfter(deadline: .now() + 3) { self?.checkDNS() }
        }
    }

    // MARK: State

    private func refresh() {
        if tunnel.isEmpty { resolveTunnel() }
        let wasUp = isUp
        isUp = !tunnel.isEmpty && FileManager.default.fileExists(atPath: runFile(tunnel))
        updateIcon()
        // The tunnel went down outside WGBar, or the network preferences changed: re-check DNS.
        let prefsNow = networkPrefsModified()
        if (wasUp && !isUp) || prefsNow != prefsSeen {
            prefsSeen = prefsNow
            DispatchQueue.main.asyncAfter(deadline: .now() + 3) { [weak self] in self?.checkDNS() }
        }
    }

    private func updateIcon() {
        let name = busy ? "shield.lefthalf.filled"
            : isUp ? "shield.fill"
            : dnsIssue.isEmpty ? "shield.slash" : "exclamationmark.shield"
        let desc = busy ? "WireGuard switching" : (isUp ? "WireGuard on" : "WireGuard off")
        let image = NSImage(systemSymbolName: name, accessibilityDescription: desc)
        image?.isTemplate = true
        statusItem.button?.image = image
        statusItem.button?.toolTip = tunnel.isEmpty
            ? "No WireGuard configs found in \(confDir)"
            : !dnsIssue.isEmpty && !isUp
            ? "VPN DNS left behind on \(dnsIssue.joined(separator: ", ")) — right-click → Repair DNS"
            : "\(tunnel): \(isUp ? "connected" : "disconnected") — click to toggle"
    }

    // MARK: DNS guard

    /// If no tunnel is up but some network service still points at a VPN DNS server, restore
    /// that service's pre-connect DNS (or clear it, i.e. back to DHCP) and say so.
    private func checkDNS(interactive: Bool = false) {
        guard !busy, !dnsChecking, !anyTunnelUp() else { return }
        let vpnDNS = allConfigDNS()
        guard !vpnDNS.isEmpty else { return }
        dnsChecking = true
        let snapshot = defaults.dictionary(forKey: "dnsSnapshot") as? DNSSnapshot ?? [:]
        DispatchQueue.global(qos: .utility).async {
            let guardian = DNSGuard(vpnDNS: vpnDNS, run: run)
            let stale = guardian.stale()
            let result = stale.isEmpty ? (fixed: [], errors: []) : guardian.repair(snapshot: snapshot)
            let stillStale = stale.isEmpty ? [] : guardian.stale()
            DispatchQueue.main.async {
                self.dnsChecking = false
                self.dnsIssue = stillStale
                self.prefsSeen = networkPrefsModified()   // our own repair changed the file; don't loop on it
                self.updateIcon()
                if !result.fixed.isEmpty {
                    self.notify("Restored DNS on \(result.fixed.joined(separator: ", ")) that WireGuard left behind.")
                }
                if !result.errors.isEmpty, interactive {
                    self.showError("", "Could not repair DNS:\n" + result.errors.joined(separator: "\n"))
                } else if stale.isEmpty, interactive {
                    self.showError("", "DNS is clean — no VPN servers left behind.", style: .informational)
                }
            }
        }
    }

    @objc private func repairDNS() { checkDNS(interactive: true) }

    private func notify(_ body: String) {
        guard Bundle.main.bundleIdentifier != nil else { return }   // UNUserNotificationCenter needs a bundle
        let center = UNUserNotificationCenter.current()
        center.requestAuthorization(options: [.alert]) { granted, _ in
            guard granted else { return }
            let content = UNMutableNotificationContent()
            content.title = "WGBar"
            content.body = body
            center.add(UNNotificationRequest(identifier: UUID().uuidString, content: content, trigger: nil))
        }
    }

    // MARK: Actions

    @objc private func clicked() {
        if NSApp.currentEvent?.type == .rightMouseUp { showMenu() } else { toggle() }
    }

    private func showMenu() {
        resolveTunnel()
        refresh()
        if tunnel.isEmpty {
            statusLine.title = "No configs in \(confDir)"
            toggleItem.title = "Connect"
            toggleItem.isEnabled = false
        } else {
            statusLine.title = isUp
                ? "\(tunnel): Connected" + (tunnelAddress(tunnel).map { " — \($0)" } ?? "")
                : "\(tunnel): Disconnected"
            toggleItem.title = isUp ? "Disconnect" : "Connect"
            toggleItem.isEnabled = !busy
        }
        dnsLine.isHidden = isUp || dnsIssue.isEmpty
        dnsLine.title = "VPN DNS left behind on \(dnsIssue.joined(separator: ", "))"
        repairItem.isHidden = isUp
        repairItem.isEnabled = !busy && !dnsChecking
        rebuildTunnelSubmenu()
        folderItem.toolTip = confDir
        loginItem.state = SMAppService.mainApp.status == .enabled ? .on : .off
        statusItem.menu = menu
        statusItem.button?.performClick(nil)
    }

    /// "Tunnel ▸" submenu, only shown when there is something to choose between.
    private func rebuildTunnelSubmenu() {
        let all = availableTunnels()
        tunnelItem.isHidden = all.count < 2
        guard let sub = tunnelItem.submenu else { return }
        sub.removeAllItems()
        for name in all {
            let item = NSMenuItem(title: name, action: #selector(selectTunnel(_:)), keyEquivalent: "")
            item.target = self
            item.state = name == tunnel ? .on : .off
            item.isEnabled = !busy
            sub.addItem(item)
        }
    }

    @objc private func selectTunnel(_ sender: NSMenuItem) {
        guard !busy, sender.title != tunnel else { return }
        tunnel = sender.title
        defaults.set(tunnel, forKey: "tunnel")
        refresh()
    }

    func menuDidClose(_ menu: NSMenu) {
        statusItem.menu = nil   // so the next left-click toggles instead of opening the menu
    }

    @objc private func toggle() {
        guard !busy else { return }
        if tunnel.isEmpty {
            showError("", "No WireGuard configs found in \(confDir).\nPut a <name>.conf there (see README) and try again.")
            return
        }
        busy = true
        updateIcon()
        let action = isUp ? "down" : "up"
        let tunnel = self.tunnel
        let vpnDNS = allConfigDNS()
        DispatchQueue.global(qos: .userInitiated).async {
            // Remember the DNS settings to put back, before wg-quick overwrites them.
            if action == "up", !vpnDNS.isEmpty, !anyTunnelUp() {
                defaults.set(DNSGuard(vpnDNS: vpnDNS, run: run).snapshot(), forKey: "dnsSnapshot")
            }
            let error = wgQuick(action, tunnel)
            DispatchQueue.main.async {
                self.busy = false
                self.refresh()
                if let error { self.showError(action, error) }
                // Give wg-quick's own monitor a moment to restore DNS; fix whatever it did not.
                if action == "down" { DispatchQueue.main.asyncAfter(deadline: .now() + 3) { self.checkDNS() } }
            }
        }
    }

    /// Pick the folder holding <name>.conf files (for setups outside the default locations).
    @objc private func chooseFolder() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = false
        panel.directoryURL = URL(fileURLWithPath: confDir)
        panel.message = "Choose the folder containing your WireGuard .conf files"
        panel.prompt = "Use Folder"
        NSApp.activate(ignoringOtherApps: true)
        guard panel.runModal() == .OK, let url = panel.url else { return }
        let dir = url.path
        if confNames(in: dir).isEmpty {
            showError("", "No .conf files found in \(dir)")
            return
        }
        defaults.set(dir, forKey: "confDir")
        defaults.removeObject(forKey: "tunnel")
        tunnel = ""
        refresh()
    }

    @objc private func toggleLogin() {
        do {
            if SMAppService.mainApp.status == .enabled {
                try SMAppService.mainApp.unregister()
            } else {
                try SMAppService.mainApp.register()
            }
        } catch {
            showError("login item", "\(error)")
        }
    }

    private func showError(_ what: String, _ message: String, style: NSAlert.Style = .warning) {
        let alert = NSAlert()
        alert.alertStyle = style
        alert.messageText = what.isEmpty ? "WGBar" : "wg-quick \(what) \(tunnel) failed"
        alert.informativeText = message
        NSApp.activate(ignoringOtherApps: true)
        alert.runModal()
    }
}

let app = NSApplication.shared
let delegate = AppDelegate()
app.delegate = delegate
app.setActivationPolicy(.accessory)
app.run()
