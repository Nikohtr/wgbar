# WGBar

A tiny macOS menu bar toggle for WireGuard tunnels managed by `wg-quick`
(the Homebrew `wireguard-tools` package).

- **Left-click** the shield icon to connect / disconnect.
- **Right-click** for a menu: status and tunnel address, connect/disconnect,
  pick a tunnel (when you have more than one), launch at login, quit.
- Icon: `shield.fill` = connected, `shield.slash` = disconnected.

It is ~200 lines of Swift with no dependencies beyond Cocoa. It exists because the
App Store WireGuard app uses its own tunnel stack (not your `wg-quick` configs), and
the other menu bar options are unmaintained.

## Requirements

- macOS 13 Ventura or newer (Apple silicon or Intel).
- Xcode Command Line Tools: `xcode-select --install`
- WireGuard tools from Homebrew: `brew install wireguard-tools`
- At least one tunnel config in `$(brew --prefix)/etc/wireguard/<name>.conf`
  (`/opt/homebrew/etc/wireguard/` on Apple silicon, `/usr/local/etc/wireguard/` on Intel).
  If `wg-quick up <name>` works in Terminal, WGBar will work.

## Install

```sh
git clone <this repo> wgbar
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
`wg-quick up/down <name>` for the tunnel configs that exist right now, after validating
it with `visudo -c`. Re-run it after adding a new config. Remove it with
`sudo rm /etc/sudoers.d/wgbar`.

## Choosing a tunnel

WGBar picks the first config alphabetically. With more than one config a **Tunnel ▸**
submenu appears in the right-click menu; the choice is remembered in
`defaults read org.wgbar.WGBar tunnel`.

## Update

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
- Toggling runs `sudo -n wg-quick up|down <name>`; if that fails for lack of a sudoers
  rule it falls back to `osascript ... with administrator privileges`.
- Login item uses `SMAppService` (hence macOS 13+).

## Hacking

Everything is in `main.swift`. `./build.sh` builds `build/WGBar.app` without
installing; `./install.sh` builds, installs and relaunches.
