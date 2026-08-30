// WGBar — a tiny macOS menu bar toggle for a wg-quick (Homebrew wireguard-tools) tunnel.
// Left-click the icon: toggle the tunnel. Right-click: menu.

import Cocoa
import ServiceManagement

// Homebrew lives in /opt/homebrew on Apple silicon and /usr/local on Intel.
let brewPrefix = ["/opt/homebrew", "/usr/local"]
    .first { FileManager.default.fileExists(atPath: "\($0)/bin/wg-quick") } ?? "/opt/homebrew"
let brewBin = "\(brewPrefix)/bin"
let wgQuick = "\(brewBin)/wg-quick"
let confDir = "\(brewPrefix)/etc/wireguard"

func confPath(_ tunnel: String) -> String { "\(confDir)/\(tunnel).conf" }
func runFile(_ tunnel: String)  -> String { "/var/run/wireguard/\(tunnel).name" }

/// Tunnel names, taken from the *.conf files wg-quick knows about.
func availableTunnels() -> [String] {
    let names = (try? FileManager.default.contentsOfDirectory(atPath: confDir)) ?? []
    return names.filter { $0.hasSuffix(".conf") }.map { String($0.dropLast(5)) }.sorted()
}

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
    let r = run("/usr/bin/sudo", ["-n", wgQuick, action, tunnel])
    if r.status == 0 { return nil }
    if !r.output.contains("password") { return r.output }   // a real wg-quick failure

    // 2. Otherwise the standard macOS administrator dialog (supports Touch ID).
    let shell = "PATH=\(brewBin):$PATH \(wgQuick) \(action) \(tunnel)"
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
    private var tunnel = ""
    private var isUp = false
    private var busy = false

    /// The tunnel to control: the remembered choice if its config still exists, else the first one found.
    private func resolveTunnel() {
        let all = availableTunnels()
        let saved = UserDefaults.standard.string(forKey: "tunnel") ?? ""
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
        menu.addItem(statusLine)
        menu.addItem(toggleItem)
        menu.addItem(.separator())
        menu.addItem(tunnelItem)
        menu.addItem(loginItem)
        menu.addItem(.separator())
        menu.addItem(NSMenuItem(title: "Quit WGBar", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q"))

        // Register as a login item on first launch; the menu checkbox controls it afterwards.
        let defaults = UserDefaults.standard
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
        UserDefaults.standard.set(tunnel, forKey: "tunnel")
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
