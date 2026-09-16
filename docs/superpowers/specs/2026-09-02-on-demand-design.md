# On-Demand Connect — design

Date: 2026-09-02
Status: approved for implementation (phase 1)

## Problem

The user reaches office machines over RDP (Microsoft "Windows App", bundle id
`com.microsoft.rdc.macos`) through a split-tunnel WireGuard config. Today they must
click WGBar before opening a session and click again after closing it. They want the
tunnel to come up automatically when a session starts and go down when it ends, and
the first connection attempt must not fail.

### Why a plain "connect when traffic appears" fails

When a TCP socket calls `connect()` the kernel fixes its source address from the route
that exists at that moment. If the tunnel is down, that is the Wi-Fi address. Bringing
the tunnel up afterwards re-routes the retransmitted SYNs into `utun`, but the
WireGuard server drops them because the source address is not in the peer's
`AllowedIPs`. The attempt times out (macOS keeps retrying for 75 s).

### Why an "armed" interface avoids it

If the WireGuard interface, its address, and its routes are already present while
"disconnected", the socket binds the tunnel address at `connect()` time. wireguard-go
stages outbound packets for a peer that has no active handshake (queue of 128), and a
peer with no endpoint cannot handshake, so the SYN just waits. When WGBar sets the
endpoint and provokes a handshake, the staged SYN is delivered and the connection
completes. Nothing fails; the session opens 1 to 2 s later than usual.

## Facts about the target setup (2026-09-02)

- Configs in `/opt/homebrew/etc/wireguard`: `ileasing`, `ileasingiPad`, `ileasingJasmin`,
  `ileasingMobile`. All split tunnel: `AllowedIPs = 10.0.0.0/24,10.42.0.0/16`,
  `DNS = 10.0.0.22, 10.0.0.4`, `PersistentKeepalive = 25`, `Endpoint = <host>:42660`.
- An RDP session is a single TCP connection to port 3389 (no UDP observed).
- Saved PCs in the Windows App: `10.42.1.63` (IP) and `nws.office.ileasing.eu`
  (split-horizon name: public DNS gives a Cloudflare address, VPN DNS gives 10.42.1.9).
  The hostname case is phase 2 (see "Out of scope"); for phase 1 the user saves that PC by IP.
- No passwordless sudo installed yet. macOS 26.5, wireguard-tools 1.0.20260223,
  wireguard-go 0.0.20250522.
- `networksetup -setdnsservers` works for admin users without root (already relied on by
  the DNS guard).

## Scope

Phase 1: On-Demand mode for the selected tunnel, triggered by TCP traffic to the
tunnel's `AllowedIPs`, with a privileged helper and unit tests.

Out of scope (phase 2 and later):

- Local DNS forwarder so split-horizon hostnames trigger the connection
  (`/etc/resolver/<domain>` pointing at a WGBar listener on localhost).
- Full-tunnel configs (`AllowedIPs` containing `0.0.0.0/0` or `::/0`). An armed
  interface would black-hole all traffic. The menu item is disabled for such configs
  with a tooltip saying why.
- Scoped (per-domain) DNS while connected.

## 1. Modes and states

Two modes per WGBar instance, applying to the selected tunnel:

- **Classic** (today): `wg-quick up` / `wg-quick down` on click.
- **On-Demand**: enabled by the "Connect on Demand" checkbox in the right-click menu,
  stored as `defaults write org.wgbar.WGBar onDemand -bool YES`.

On-Demand states:

| State | Interface | Peer endpoint | System DNS | Meaning |
|---|---|---|---|---|
| `off` | absent | – | untouched | not armed (WGBar not running, arm failed, or busy transitioning) |
| `armed` | up, routes present | none | untouched | waiting for traffic |
| `connected` | up | set | VPN DNS applied | in use |
| `paused` | up | none | untouched | after a manual disconnect while VPN-bound sockets still exist |

Plus a `busy` flag while a helper call is in flight (reuses the existing `busy`).

Manual left-click in On-Demand mode:

- From `armed` or `paused`: connect and mark **sticky**. A sticky connection ignores the
  idle timer; only a manual click disconnects it.
- From `connected`: disconnect. If VPN-bound sockets still exist, enter `paused`;
  otherwise `armed`. `paused` becomes `armed` once no VPN-bound sockets have been seen
  for the idle period. This stops an RDP client's automatic reconnect from re-triggering
  the tunnel the user just closed.

## 2. Triggers

While in On-Demand mode WGBar polls `lsof -nP -iTCP` every 1 s
(no root required; `netstat` was used originally, but macOS 27 gives ad-hoc signed apps an empty list)
and keeps the sockets whose remote address is inside the tunnel's `AllowedIPs`.

- **Start**: state is `armed` and at least one such socket is in `SYN_SENT` or
  `ESTABLISHED`. Destination-based: any app reaching the VPN subnets triggers it, not
  only the Windows App. (Scoping to one process is a possible later option.)
- **End**: state is `connected`, not sticky, and no such socket has been seen for the
  idle period. Default 30 s; `defaults write org.wgbar.WGBar onDemandIdle 30` overrides.
  The RDP client holds one TCP connection for the whole session, so this means
  "30 s after the VM window closes".
- After `connect` returns, WGBar sends one `ping -c 1 -W 1000` to the config's first VPN
  DNS server, if it has one. Any packet toward the peer starts the handshake; the ping
  merely avoids waiting for the next TCP retransmit (1 s, then 3 s). Configs without a
  `DNS` line skip the ping and rely on the retransmit.

The decision logic is a pure function
`decide(state:, sticky:, sockets:, lastSeen:, now:, idle:) -> Action?` with actions
`connect`, `disconnect`, `resumeArmed`, or none, so it is unit-testable without a network.

## 3. Privileged helper

`helper/wgbar-helper` in the repo, a bash script installed to
`/usr/local/libexec/wgbar-helper` (root:wheel, mode 755) by `sudoers.sh`. The sudoers
rule allows the current user to run only this script with `NOPASSWD`. The config folder
is baked into the installed copy (`CONF_DIR=...` line rewritten by `sudoers.sh`), and the
helper refuses tunnel names that do not resolve to `<CONF_DIR>/<name>.conf` (no `/`, no
`..`). Trust level equals the existing rule: `wg-quick up` on a user-owned config already
runs its `PostUp` hooks as root.

Verbs (`wgbar-helper <verb> <tunnel>`):

- `arm`: if `/var/run/wireguard/<tunnel>.name` exists, exit 0. Else derive the armed
  config (below) into a `mktemp -d` directory (mode 700) as `<tunnel>.conf` (mode 600),
  run `wg-quick up <that path>`, remove the directory. The interface name file is the
  same as for a classic `up`, so WGBar's existing `isUp` check keeps working.
- `connect`: read `Endpoint`, `PublicKey`, `PersistentKeepalive` from the real config;
  `wg set <utun> peer <PublicKey> endpoint <Endpoint> [persistent-keepalive N]`.
- `disconnect`: `wg set <utun> peer <PublicKey> remove`, then
  `wg addconf <utun> <(wg-quick strip <armed conf>)` so the peer is back with its
  `AllowedIPs` (and `PresharedKey` if any) but no endpoint. Kernel routes were added by
  wg-quick and are unaffected by peer changes, so traffic keeps entering `utun`.
- `down`: `wg-quick down <regenerated armed conf>`. Using the armed config (no `DNS`)
  guarantees wg-quick does not touch system DNS on the way down.
- `status`: prints `off` (no name file), `connected` (`wg show <utun> endpoints` lists
  an endpoint for the peer), else `armed`.
- `print-armed`: prints the derived armed config to stdout. Runs as any user (reads the
  config the user can read anyway); used by tests.

Armed config derivation: copy the real config line by line, dropping lines whose key
(case-insensitive, trimmed) is `DNS`, `Endpoint`, or `PersistentKeepalive`. Everything
else (`PrivateKey`, `Address`, `MTU`, `Table`, hooks, `PublicKey`, `PresharedKey`,
`AllowedIPs`) is preserved. `PersistentKeepalive` is dropped because with no endpoint it
only produces "no known endpoint" log noise; `connect` re-applies it.

`<utun>` is read from `/var/run/wireguard/<tunnel>.name`. All verbs exit non-zero with a
one-line message on stderr on failure; WGBar shows that text in its error alert.

## 4. DNS

- Armed config has no `DNS` line, so wg-quick's route monitor never rewrites DNS.
- On `connect` success WGBar (not the helper): snapshot with the existing
  `DNSGuard.snapshot()` into `dnsSnapshot`, then apply the config's DNS servers (and
  non-IP `DNS` entries as search domains, as wg-quick does) to every network service
  via a new `DNSGuard.apply(servers:search:)`.
- On `disconnect` and `down`: `DNSGuard.repair(snapshot:)` restores the snapshot on
  every service still pointing at a VPN server.
- The DNS guard's gate `anyTunnelUp()` changes to `anyTunnelConnected()`: the On-Demand
  tunnel's name file does not count while its state is `armed`, `paused`, or `off`.
  Other tunnels' name files count as before. A crash while connected is therefore still
  repaired on the next launch (the interface may be up, but the state reconciles to
  `connected` and the normal disconnect path restores DNS).

## 5. Lifecycle

- **Enable** (menu): if the selected tunnel is up classically, `wg-quick down` it via the
  existing path first. Then `sudo -n wgbar-helper arm <tunnel>`. If sudo asks for a
  password or the helper is missing, show one alert ("On-Demand needs the WGBar helper.
  Run ./sudoers.sh in the WGBar folder and try again.") and leave the checkbox off.
  Never fall back to the osascript admin prompt for On-Demand: it would fire at every login.
- **Launch** with `onDemand` set: `status`, then `arm` if `off`; if `connected`, adopt
  that state (sticky = false) so the idle timer can bring it down. Start the 1 s poll.
- **Quit** (`applicationWillTerminate`): if `connected`, restore DNS; then `down`. An
  armed interface without a watcher would silently black-hole the VPN subnets.
- **Sleep**: nothing. **Wake**: `status`, reconcile, then the existing DNS check.
- **Disable** (menu): restore DNS if connected, `down`, stop the poll, back to Classic.
- **Tunnel change or Config Folder change** while enabled: `down` the current tunnel,
  then `arm` the new one (or turn On-Demand off if the new tunnel is full-tunnel).
- **Failures**: if `arm` fails (at enable or at launch), On-Demand is switched off: the
  checkbox reverts, the default is cleared, and one alert shows the helper's message; the
  user re-enables it after fixing the cause. If `connect` or `disconnect` fails, state
  becomes `off` and the icon shows `shield.slash` while the interface may still exist;
  disabling or quitting always issues `down` so nothing is orphaned.

## 6. Icon and menu

| Situation | Symbol |
|---|---|
| connected (either mode) | `shield.fill` |
| armed or paused | `shield` |
| off / disconnected | `shield.slash` |
| busy | `shield.lefthalf.filled` (unchanged) |
| DNS issue while not connected | `exclamationmark.shield` (unchanged) |

Menu additions and changes:

- New checkbox **Connect on Demand** below the Tunnel submenu. Disabled with tooltip
  "Not available for full-tunnel configs (AllowedIPs includes 0.0.0.0/0)" when applicable.
- Status line examples: `ileasing: Armed, connects on demand`,
  `ileasing: Connected (on demand) — 10.42.66.9/24`, `ileasing: Paused`.
- Toggle item: `Connect` when armed/paused, `Disconnect` when connected.
- Tooltip on the icon mirrors the status line.

## 7. Code layout

- `OnDemand.swift` (new)
  - `struct TCPSocket { state, remoteIP, remotePort }`
  - `func parseNetstat(_ output: String) -> [TCPSocket]`
  - `struct CIDR { init?(String); func contains(_ ip: String) -> Bool }` (IPv4 and IPv6)
  - `func parseAllowedIPs(_ configText: String) -> [CIDR]`, `func isFullTunnel(_ cidrs:) -> Bool`
  - `enum OnDemandState { off, armed, connected, paused }`, `enum OnDemandAction { connect, disconnect, resumeArmed }`
  - `func decide(...) -> OnDemandAction?` (pure)
  - `final class OnDemandController` holding state, `sticky`, `lastSeen`, with injected
    `run: (String, [String]) -> CmdResult`, `now: () -> Date`, `helper: String`. Methods:
    `enable()`, `disable()`, `tick()`, `manualToggle()`, `reconcile()`, `shutdown()`.
    Callbacks to `AppDelegate` for DNS apply/restore and UI refresh.
- `DNSGuard.swift`: add `apply(servers: [String], search: [String]) -> [String]` (errors).
- `main.swift`: menu item, state plumbing, icon mapping, `applicationWillTerminate`,
  `anyTunnelConnected()`.
- `helper/wgbar-helper` (new bash).
- `sudoers.sh`: also installs the helper with `CONF_DIR` baked in; rule gains the helper
  path. `uninstall.sh`: removes the helper. `install.sh`: if
  `/usr/local/libexec/wgbar-helper` exists and differs from `helper/wgbar-helper`
  (ignoring the `CONF_DIR=` line), print "helper is outdated, run ./sudoers.sh".
- `build.sh`, `test.sh`: include `OnDemand.swift`.
- README: new section "Connect on demand" (what it does, the armed-interface
  explanation in two sentences, requirements: split tunnel and `./sudoers.sh`, the
  `onDemandIdle` default, phase-2 note about hostnames).

## 8. Testing

Unit tests (`tests/OnDemandTests.swift`, run by `./test.sh`):

- `parseNetstat`: real macOS output samples with `ESTABLISHED`, `SYN_SENT`, `LISTEN`,
  IPv6 lines, and the header; ports split off correctly from `10.42.1.9.3389`.
- `CIDR.contains`: boundaries of `/24` and `/16`, non-matching addresses, IPv6 basic.
- `isFullTunnel`: `0.0.0.0/0`, `::/0`, `0.0.0.0/1`+`128.0.0.0/1` counted as full.
- `decide`: armed + SYN_SENT → connect; connected + sockets → none; connected + no
  sockets before idle → none; after idle → disconnect; sticky never disconnects;
  paused + no sockets past idle → resumeArmed; paused + sockets → none.
- `DNSGuard.apply`: issues one `-setdnsservers` and one `-setsearchdomains` per service
  with the fake runner; `Empty` when search is empty.

Helper tests (`tests/helper_test.sh`, run by `./test.sh`): `print-armed` on a fixture
config with mixed-case keys, comments, and two `DNS` lines yields the expected output;
a name with `/` or `..` is rejected.

Manual acceptance (documented in the plan):

1. `./sudoers.sh`, relaunch, enable Connect on Demand. `netstat -rn` shows the
   10.0.0.0/24 and 10.42.0.0/16 routes on a utun; `scutil --dns` shows home DNS.
2. Open the `10.42.1.63` PC in the Windows App. Session opens; note the delay.
   `scutil --dns` now shows 10.0.0.22 / 10.0.0.4.
3. Close the VM. Within ~30 s the icon returns to `shield`, DNS is back to DHCP.
4. Left-click to connect manually; wait > 30 s; still connected (sticky). Click again;
   armed.
5. Quit WGBar; `ls /var/run/wireguard` is empty; relaunch; armed again without a prompt.
