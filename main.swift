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
    env["GIT_TERMINAL_PROMPT"] = "0"   // never hang waiting for credentials
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

/// Like `anyTunnelUp`, but an on-demand interface that is merely armed (no endpoint, no VPN DNS)
/// does not count: its name file exists while DNS must still be the user's own.
func anyTunnelConnected(ignoring armedTunnel: String?) -> Bool {
    let names = (try? FileManager.default.contentsOfDirectory(atPath: "/var/run/wireguard")) ?? []
    return names.contains { $0.hasSuffix(".name") && $0 != armedTunnel.map { "\($0).name" } }
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
    private let updateItem = NSMenuItem(title: "Check for Updates…", action: #selector(checkForUpdates), keyEquivalent: "")
    private let onDemandItem = NSMenuItem(title: "Connect on Demand", action: #selector(toggleOnDemand), keyEquivalent: "")
    /// The controller as seen from odQueue: set right after a successful arm/reconcile, cleared
    /// after shutdown. Read and written only on `odQueue`, so a quit can always drain it.
    private var odOwned: OnDemandController?
    /// Mirror of the controller's state for the main thread (updated via onChange).
    private var odState: OnDemandState = .off
    private var odTunnel: String?
    private var odInFlight = false
    private let odQueue = DispatchQueue(label: "org.wgbar.ondemand")
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
        updateItem.target = self
        onDemandItem.target = self
        menu.addItem(statusLine)
        menu.addItem(dnsLine)
        menu.addItem(toggleItem)
        menu.addItem(repairItem)
        menu.addItem(.separator())
        menu.addItem(tunnelItem)
        menu.addItem(onDemandItem)
        menu.addItem(folderItem)
        menu.addItem(loginItem)
        menu.addItem(updateItem)
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
        Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in self?.onDemandTick() }
        if defaults.bool(forKey: "onDemand") { startOnDemand(atLaunch: true) }

        // DNS guard: a reboot or crash with the tunnel up leaves VPN DNS behind; so can waking.
        checkDNS()
        NSWorkspace.shared.notificationCenter.addObserver(forName: NSWorkspace.didWakeNotification, object: nil, queue: .main) { [weak self] _ in
            self?.odQueue.async { self?.odOwned?.reconcile() }
            DispatchQueue.main.asyncAfter(deadline: .now() + 3) { self?.checkDNS() }
        }
    }

    // MARK: State

    private func refresh() {
        if tunnel.isEmpty { resolveTunnel() }
        let wasUp = isUp
        isUp = odTunnel != nil ? odState == .connected
             : !tunnel.isEmpty && FileManager.default.fileExists(atPath: runFile(tunnel))
        updateIcon()
        // The tunnel went down outside WGBar, or the network preferences changed: re-check DNS.
        let prefsNow = networkPrefsModified()
        if (wasUp && !isUp) || prefsNow != prefsSeen {
            prefsSeen = prefsNow
            DispatchQueue.main.asyncAfter(deadline: .now() + 3) { [weak self] in self?.checkDNS() }
        }
    }

    private func updateIcon() {
        let armed = odTunnel != nil && (odState == .armed || odState == .paused)
        let name = busy ? "shield.lefthalf.filled"
            : isUp ? "shield.fill"
            : armed ? "shield"
            : dnsIssue.isEmpty ? "shield.slash" : "exclamationmark.shield"
        let desc = busy ? "WireGuard switching" : isUp ? "WireGuard on" : armed ? "WireGuard armed" : "WireGuard off"
        let image = NSImage(systemSymbolName: name, accessibilityDescription: desc)
        image?.isTemplate = true
        statusItem.button?.image = image
        statusItem.button?.toolTip = tunnel.isEmpty
            ? "No WireGuard configs found in \(confDir)"
            : !dnsIssue.isEmpty && !isUp
            ? "VPN DNS left behind on \(dnsIssue.joined(separator: ", ")) — right-click → Repair DNS"
            : statusText() + " — click to toggle"
    }

    /// "ileasing: Connected" etc., shared by the menu status line and the icon tooltip.
    private func statusText() -> String {
        let addr = tunnelAddress(tunnel).map { " — \($0)" } ?? ""
        guard odTunnel != nil else { return "\(tunnel): " + (isUp ? "Connected\(addr)" : "Disconnected") }
        switch odState {
        case .connected: return "\(tunnel): Connected (on demand)\(addr)"
        case .armed:     return "\(tunnel): Armed, connects on demand"
        case .paused:    return "\(tunnel): Paused"
        case .off:       return "\(tunnel): Off (on demand not armed)"
        }
    }

    // MARK: DNS guard

    /// If no tunnel is up but some network service still points at a VPN DNS server, restore
    /// that service's pre-connect DNS (or clear it, i.e. back to DHCP) and say so.
    private func checkDNS(interactive: Bool = false) {
        let armed = odTunnel != nil && odState != .connected ? odTunnel : nil
        guard !busy, !dnsChecking, !anyTunnelConnected(ignoring: armed) else { return }
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
            statusLine.title = statusText()
            toggleItem.title = isUp ? "Disconnect" : "Connect"
            toggleItem.isEnabled = !busy
        }
        dnsLine.isHidden = isUp || dnsIssue.isEmpty
        dnsLine.title = "VPN DNS left behind on \(dnsIssue.joined(separator: ", "))"
        repairItem.isHidden = isUp
        repairItem.isEnabled = !busy && !dnsChecking
        updateItem.isEnabled = !busy
        rebuildTunnelSubmenu()
        let confText = tunnel.isEmpty ? "" : (try? String(contentsOfFile: confPath(tunnel), encoding: .utf8)) ?? ""
        let full = isFullTunnel(parseAllowedIPs(confText))
        onDemandItem.state = odTunnel != nil ? .on : .off
        onDemandItem.isEnabled = !busy && !tunnel.isEmpty && (odTunnel != nil || !full)
        onDemandItem.toolTip = full ? "Not available for full-tunnel configs (AllowedIPs includes 0.0.0.0/0)"
                                    : "Keep the tunnel armed and connect it when traffic to its AllowedIPs appears"
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
        if odTunnel != nil { restartOnDemand() } else { refresh() }
    }

    func menuDidClose(_ menu: NSMenu) {
        statusItem.menu = nil   // so the next left-click toggles instead of opening the menu
    }

    @objc private func toggle() {
        guard !busy else { return }
        if odTunnel != nil {
            busy = true; updateIcon()
            odQueue.async {
                self.odOwned?.manualToggle()
                DispatchQueue.main.async { self.busy = false; self.refresh() }
            }
            return
        }
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

    // MARK: On demand

    private func onDemandTick() {
        guard odTunnel != nil, !odInFlight, !busy else { return }
        odInFlight = true
        odQueue.async {
            self.odOwned?.tick()
            DispatchQueue.main.async { self.odInFlight = false }
        }
    }

    private func makeController(_ tunnel: String, _ cidrs: [CIDR]) -> OnDemandController {
        let idle = (defaults.object(forKey: "onDemandIdle") as? NSNumber)?.doubleValue ?? 30
        let text = (try? String(contentsOfFile: confPath(tunnel), encoding: .utf8)) ?? ""
        let od = OnDemandController(tunnel: tunnel, allowed: cidrs, idle: idle,
                                    kick: configDNSServers(text).first, run: run, now: Date.init)
        od.onConnected = { [weak self] in self?.applyVPNDNS(for: tunnel) }
        od.onDisconnected = { [weak self] in self?.restoreDNS() }
        od.onError = { [weak self] msg in DispatchQueue.main.async { self?.showError("", msg) } }
        od.onChange = { [weak self] s in DispatchQueue.main.async { self?.odState = s; self?.refresh() } }
        return od
    }

    /// Runs on odQueue. Remember the current DNS, then set the config's servers everywhere (as wg-quick would).
    private func applyVPNDNS(for tunnel: String) {
        guard let text = try? String(contentsOfFile: confPath(tunnel), encoding: .utf8) else { return }
        let servers = configDNSServers(text)
        guard !servers.isEmpty else { return }
        let guardian = DNSGuard(vpnDNS: allConfigDNS(), run: run)
        defaults.set(guardian.snapshot(), forKey: "dnsSnapshot")
        let errors = guardian.apply(servers: servers, search: configDNSSearch(text))
        DispatchQueue.main.async {
            self.prefsSeen = networkPrefsModified()
            if !errors.isEmpty { self.showError("", "Could not set VPN DNS:\n" + errors.joined(separator: "\n")) }
        }
    }

    /// Runs on odQueue. Put back whatever DNS the services had before connect.
    private func restoreDNS() {
        let snapshot = defaults.dictionary(forKey: "dnsSnapshot") as? DNSSnapshot ?? [:]
        let result = DNSGuard(vpnDNS: allConfigDNS(), run: run).repair(snapshot: snapshot)
        DispatchQueue.main.async {
            self.prefsSeen = networkPrefsModified()
            if !result.errors.isEmpty { self.showError("", "Could not restore DNS:\n" + result.errors.joined(separator: "\n")) }
        }
    }

    /// Menu checkbox: enable or disable Connect on Demand for the selected tunnel.
    @objc private func toggleOnDemand() {
        guard !busy else { return }
        if odTunnel != nil { stopOnDemand(); return }
        startOnDemand(atLaunch: false)
    }

    /// Arm the selected tunnel. At launch, adopt the interface's real state first (it may still be up).
    private func startOnDemand(atLaunch: Bool) {
        guard !tunnel.isEmpty else {
            defaults.set(false, forKey: "onDemand")
            showError("", "No WireGuard configs found in \(confDir).")
            return
        }
        guard OnDemandController.helperInstalled() else {
            defaults.set(false, forKey: "onDemand")
            showError("", OnDemandController.needsHelperMessage)
            return
        }
        guard let text = try? String(contentsOfFile: confPath(tunnel), encoding: .utf8) else {
            defaults.set(false, forKey: "onDemand")
            showError("", "Could not read \(confPath(tunnel)).")
            return
        }
        let cidrs = parseAllowedIPs(text)
        guard !cidrs.isEmpty else {
            defaults.set(false, forKey: "onDemand")
            showError("", "The config has no AllowedIPs line.")
            return
        }
        guard !isFullTunnel(cidrs) else {
            defaults.set(false, forKey: "onDemand")
            showError("", "Connect on Demand needs a split-tunnel config (specific AllowedIPs, not 0.0.0.0/0).")
            return
        }
        busy = true; updateIcon()
        let tunnel = self.tunnel
        let od = makeController(tunnel, cidrs)
        odQueue.async {
            // A classic `wg-quick up` interface carries VPN DNS and a monitor that re-applies it: replace it.
            if !atLaunch, FileManager.default.fileExists(atPath: runFile(tunnel)) {
                if let error = wgQuick("down", tunnel) {
                    DispatchQueue.main.async { self.busy = false; self.refresh(); self.showError("down", error) }
                    return
                }
            }
            if atLaunch { od.reconcile() }
            let ok = od.state != .off || od.arm()
            // Take ownership here, on odQueue: a quit that lands before the main-thread mirrors
            // are set must still find the live controller and shut it down.
            if ok { self.odOwned = od }
            let s = od.state
            DispatchQueue.main.async {
                self.busy = false
                if ok {
                    self.odTunnel = tunnel; self.odState = s
                    defaults.set(true, forKey: "onDemand")
                } else {
                    defaults.set(false, forKey: "onDemand")
                }
                self.refresh()
            }
        }
    }

    /// Take the on-demand interface down and return to classic mode.
    private func stopOnDemand(then completion: (() -> Void)? = nil) {
        guard odTunnel != nil else { completion?(); return }
        busy = true; updateIcon()
        odTunnel = nil; odState = .off
        defaults.set(false, forKey: "onDemand")
        odQueue.async {
            self.odOwned?.shutdown()
            self.odOwned = nil
            DispatchQueue.main.async { self.busy = false; self.refresh(); completion?() }
        }
    }

    /// The tunnel or config folder changed while On-Demand is on: re-arm for the new selection.
    private func restartOnDemand() {
        guard odTunnel != nil else { return }
        stopOnDemand { [weak self] in
            self?.resolveTunnel()
            self?.startOnDemand(atLaunch: false)
        }
    }

    /// Quit: an armed interface with nobody watching would black-hole the VPN subnets. Take it down.
    /// Unconditional, so the queue drains first: an arm still running finishes and is then shut down,
    /// and a shutdown already queued by `stopOnDemand` completes before the process goes away.
    func applicationWillTerminate(_ notification: Notification) {
        odQueue.sync {
            self.odOwned?.shutdown()
            self.odOwned = nil
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
        resolveTunnel()
        if odTunnel != nil { restartOnDemand() } else { refresh() }
    }

    // MARK: Updates

    /// Fetch the clone this app was installed from and offer to pull + reinstall.
    @objc private func checkForUpdates() {
        guard !busy else { return }
        let updater = Updater(repoDir: defaults.string(forKey: "repoDir") ?? "", run: run)
        DispatchQueue.global(qos: .userInitiated).async {
            let status = updater.check()
            DispatchQueue.main.async { self.present(status, updater) }
        }
    }

    private func present(_ status: UpdateStatus, _ updater: Updater) {
        switch status {
        case .failed(let message):
            showError("", "Could not check for updates.\n\(message)")
        case .upToDate(let revision):
            showError("", "WGBar is up to date (\(revision)).", style: .informational)
        case .available(let commits):
            let alert = NSAlert()
            alert.messageText = "WGBar update available"
            alert.informativeText = "\(commits.count) new commit\(commits.count == 1 ? "" : "s"):\n\n"
                + commits.joined(separator: "\n")
                + "\n\nUpdate pulls the latest source, rebuilds, and relaunches WGBar."
            alert.addButton(withTitle: "Update")
            alert.addButton(withTitle: "Later")
            NSApp.activate(ignoringOtherApps: true)
            guard alert.runModal() == .alertFirstButtonReturn else { return }
            applyUpdate(updater)
        }
    }

    private func applyUpdate(_ updater: Updater) {
        busy = true
        updateIcon()
        DispatchQueue.global(qos: .userInitiated).async {
            if let error = updater.pull() {
                DispatchQueue.main.async {
                    self.busy = false
                    self.updateIcon()
                    self.showError("", "git pull failed.\n\(error)")
                }
                return
            }
            // install.sh kills and relaunches WGBar, so it must outlive this process and must not
            // write to a pipe we hold. Detach it with its output in a log file.
            let log = NSHomeDirectory() + "/Library/Logs/WGBar-update.log"
            FileManager.default.createFile(atPath: log, contents: nil)
            let p = Process()
            p.executableURL = URL(fileURLWithPath: "/bin/bash")
            p.arguments = ["./install.sh"]
            p.currentDirectoryURL = URL(fileURLWithPath: updater.repoDir)
            var env = ProcessInfo.processInfo.environment
            env["PATH"] = "\(brewBin):" + (env["PATH"] ?? "/usr/bin:/bin:/usr/sbin:/sbin")
            p.environment = env
            let out = FileHandle(forWritingAtPath: log)
            p.standardOutput = out
            p.standardError = out
            p.standardInput = FileHandle.nullDevice
            do { try p.run() } catch {
                DispatchQueue.main.async {
                    self.busy = false
                    self.updateIcon()
                    self.showError("", "Could not start install.sh.\n\(error)")
                }
            }
        }
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
