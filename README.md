# WGBar

A tiny macOS menu bar toggle for WireGuard tunnels managed by `wg-quick`
(the Homebrew `wireguard-tools` package).

- **Left-click** the shield icon to connect / disconnect.
- **Right-click** for a menu: status and tunnel address, connect/disconnect,
  pick a tunnel (when you have more than one), launch at login, check for updates, quit.
- Icon: `shield.fill` = connected, `shield.slash` = disconnected.

It is a few hundred lines of Swift with no dependencies beyond Cocoa (`./test.sh` runs the unit tests). It exists because the
App Store WireGuard app uses its own tunnel stack (not your `wg-quick` configs), and
the other menu bar options are unmaintained.

## Requirements

- macOS 13 Ventura or newer (Apple silicon or Intel).
- Xcode Command Line Tools: `xcode-select --install`
- WireGuard tools from Homebrew: `brew install wireguard-tools`
- At least one tunnel config (`<name>.conf`) in one of the folders WGBar searches:
  `/opt/homebrew/etc/wireguard`, `/usr/local/etc/wireguard`, `/opt/local/etc/wireguard`
  (MacPorts) or `/etc/wireguard` — or anywhere else, chosen via **Config Folder…**
  (see below). If `wg-quick up <name>` works in Terminal, WGBar will work.

## Install

```sh
git clone https://github.com/Nikohtr/wgbar.git wgbar
cd wgbar
./install.sh
```

`install.sh` compiles the app, copies it to `~/Applications/WGBar.app`, and launches it.
On first launch it registers itself as a login item; untick **Launch at Login** in the
menu if you don't want that.

The app is built on your machine and ad-hoc signed, so there is no Gatekeeper prompt.
(There is no notarized download because that requires a paid Apple Developer ID.)

### Optional: toggle without a password prompt

`wg-quick` needs root. By default WGBar shows the standard macOS administrator dialog
(Touch ID works there). To skip the prompt entirely:

```sh
./sudoers.sh
```

This writes `/etc/sudoers.d/wgbar` allowing **only your user** to run
`wg-quick up/down <path-to-config>` for the tunnel configs that exist right now, after
validating it with `visudo -c`. Re-run it after adding a config or changing the config
folder (`./sudoers.sh /some/other/folder` to point it elsewhere). Remove it with
`sudo rm /etc/sudoers.d/wgbar`.

## Choosing a tunnel

WGBar picks the first config alphabetically. With more than one config a **Tunnel ▸**
submenu appears in the right-click menu; the choice is remembered in
`defaults read org.wgbar.WGBar tunnel`.

## Config folder and unusual setups

WGBar uses the first of `/opt/homebrew/etc/wireguard`, `/usr/local/etc/wireguard`,
`/opt/local/etc/wireguard`, `/etc/wireguard` that contains `.conf` files. If yours live
somewhere else, right-click → **Config Folder…** and pick it (hover the item to see the
folder currently in use). The folder must be listable by your user; WGBar hands
`wg-quick` the full path to the config, so it works even outside `wg-quick`'s own search
paths.

Settings can also be set from Terminal (quit and relaunch WGBar afterwards):

```sh
defaults write org.wgbar.WGBar confDir /path/to/wireguard   # config folder
defaults write org.wgbar.WGBar wgQuick /path/to/wg-quick    # non-standard wg-quick
defaults delete org.wgbar.WGBar confDir                     # back to auto-detect
```

## DNS left behind (and the fix)

`wg-quick` on macOS applies your config's `DNS =` servers to **every** network service, and
the only thing that puts the old DNS back is a background monitor process it leaves running.
If that process dies without cleaning up (shutdown or logout with the tunnel up, a crash,
sleep/wake races), the VPN DNS stays set in System Settings — across reboots — and with no
tunnel to reach it, nothing resolves. The usual symptom: "no internet" until you clear the DNS
servers by hand in Wi-Fi ▸ Details ▸ DNS.

WGBar repairs this automatically. When no tunnel is up but a network service still lists a
DNS address from one of your configs, WGBar puts that service back to what it had before the
last connect (or clears it, which means DHCP-provided DNS) and posts a notification. It checks
at launch, after waking from sleep, when a tunnel goes down, and whenever the system network
preferences change. It touches nothing that is not a VPN address from your configs, so DNS
servers you set yourself are left alone. No password is needed: macOS lets admin users change
DNS settings directly.

If a repair fails, the icon becomes `exclamationmark.shield` and the right-click menu shows
which service is affected; **Repair DNS** in that menu retries and reports the error.

## Update

Right-click → **Check for Updates…**. WGBar fetches the git clone it was installed from,
shows the new commits, and on **Update** pulls them, rebuilds, and relaunches itself (the
build output goes to `~/Library/Logs/WGBar-update.log`). This needs the clone to still be
where you ran `install.sh`; if you moved or deleted it, run `./install.sh` from the new
location once so WGBar learns the path.

The same thing by hand:

```sh
cd wgbar && git pull && ./install.sh
```

## Uninstall

```sh
./uninstall.sh
```

Removes the app, its settings, and the sudoers rule (if installed).

## How it works

- Tunnel state is read from `/var/run/wireguard/<name>.name`, which `wg-quick` creates
  on `up` and removes on `down`; it is polled every 2 s, so changes made from Terminal
  show up too.
- Toggling runs `sudo -n wg-quick up|down <folder>/<name>.conf`; if that fails for lack of
  a sudoers rule it falls back to `osascript ... with administrator privileges`.
- Login item uses `SMAppService` (hence macOS 13+).

## Hacking

Everything is in `main.swift`. `./build.sh` builds `build/WGBar.app` without
installing; `./install.sh` builds, installs and relaunches.
