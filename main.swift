// WGBar — a tiny macOS menu bar toggle for a wg-quick (Homebrew wireguard-tools) tunnel.
// Left-click the icon: toggle the tunnel. Right-click: menu.

import Cocoa
import ServiceManagement

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

struct CmdResult { let status: Int32; let output: String }

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
    private let toggleItem = NSMenuItem(title: "", action: #selector(toggle), keyEquivalent: "")
    private let loginItem  = NSMenuItem(title: "Launch at Login", action: #selector(toggleLogin), keyEquivalent: "")
    private let tunnelItem = NSMenuItem(title: "Tunnel", action: nil, keyEquivalent: "")
    private let folderItem = NSMenuItem(title: "Config Folder…", action: #selector(chooseFolder), keyEquivalent: "")
    private var tunnel = ""
    private var isUp = false
    private var busy = false

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
        toggleItem.target = self
        loginItem.target = self
        tunnelItem.submenu = NSMenu()
        folderItem.target = self
        menu.addItem(statusLine)
        menu.addItem(toggleItem)
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
    }

    // MARK: State

    private func refresh() {
        if tunnel.isEmpty { resolveTunnel() }
        isUp = !tunnel.isEmpty && FileManager.default.fileExists(atPath: runFile(tunnel))
        updateIcon()
    }

    private func updateIcon() {
        let name = busy ? "shield.lefthalf.filled" : (isUp ? "shield.fill" : "shield.slash")
        let desc = busy ? "WireGuard switching" : (isUp ? "WireGuard on" : "WireGuard off")
        let image = NSImage(systemSymbolName: name, accessibilityDescription: desc)
        image?.isTemplate = true
        statusItem.button?.image = image
        statusItem.button?.toolTip = tunnel.isEmpty
            ? "No WireGuard configs found in \(confDir)"
            : "\(tunnel): \(isUp ? "connected" : "disconnected") — click to toggle"
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
        DispatchQueue.global(qos: .userInitiated).async {
            let error = wgQuick(action, tunnel)
            DispatchQueue.main.async {
                self.busy = false
                self.refresh()
                if let error { self.showError(action, error) }
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

    private func showError(_ what: String, _ message: String) {
        let alert = NSAlert()
        alert.alertStyle = .warning
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
