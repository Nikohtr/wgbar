# WGBar Swift → Rust port

Branch: `rust-port`. Goal: replace every Swift source and the bash helper with Rust, keep
behaviour, user defaults (`org.wgbar.WGBar`) and on-disk formats identical, and do not regress
speed or memory against the Swift baseline (see `bench/`).

## Phases

1. **Linux (done in the cloud session)** — core library + ported unit tests (`cargo test`),
   macOS app type-checked with `cargo check --target aarch64-apple-darwin`, Rust helper +
   `tests/helper_test.sh`, build/install scripts, bench scripts.
2. **Mac** — `bench/baseline.sh` on `main` (Swift), then on `rust-port` (Rust); fix runtime
   problems found by running the app; compare; delete Swift sources.

## Crate layout

```
Cargo.toml            lib `wgbar`, bins `WGBar` (src/main.rs) and `wgbar-helper` (src/bin/wgbar-helper.rs)
src/lib.rs            pub mod cmd, dns_guard, on_demand, tunnels, updater        (owner: core)
src/cmd.rs            CmdResult, Runner, run_with_path                              (core)
src/dns_guard.rs      port of DNSGuard.swift                                       (core)
src/on_demand.rs      port of OnDemand.swift                                       (core)
src/updater.rs        port of Updater.swift                                        (core)
src/tunnels.rs        non-UI free functions of main.swift                          (core)
tests/*.rs            port of tests/DNSGuardTests.swift + tests/OnDemandTests.swift (core)
src/main.rs           cfg(macos) -> app::run(); else prints "macOS only", exit 1    (app)
src/app/*.rs          cfg(macos) AppDelegate port with objc2                       (app)
src/bin/wgbar-helper.rs + build.rs   port of helper/wgbar-helper                   (helper)
tests/helper_test.sh, sudoers.sh     updated for the Rust helper                   (helper)
build.sh test.sh install.sh uninstall.sh bench/ README.md                          (orchestrator)
```

## Core API contract (src/ — what the app layer codes against)

```rust
// cmd.rs
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CmdResult { pub status: i32, pub output: String }
/// Runs an executable with args, stdout+stderr merged, output trimmed of whitespace/newlines.
pub type Runner = std::sync::Arc<dyn Fn(&str, &[String]) -> CmdResult + Send + Sync>;
/// Swift `run`: PATH = "<brew_bin>:" + inherited PATH (default "/usr/bin:/bin:/usr/sbin:/sbin"),
/// GIT_TERMINAL_PROMPT=0. Spawn failure -> status -1, output = error text.
pub fn run_with_path(brew_bin: &str, exe: &str, args: &[String]) -> CmdResult;

// dns_guard.rs
pub fn config_dns_servers(text: &str) -> Vec<String>;
pub fn config_dns_search(text: &str) -> Vec<String>;
pub fn is_ip_address(s: &str) -> bool;
pub fn parse_service_list(output: &str) -> Vec<String>;
pub fn parse_dns_servers(output: &str) -> Vec<String>;
pub fn parse_search_domains(output: &str) -> Vec<String>;
pub fn stale_services(current: &BTreeMap<String, Vec<String>>, vpn_dns: &BTreeSet<String>) -> Vec<String>;
/// {"Wi-Fi": {"servers": [...], "search": [...]}} — same shape as the NSUserDefaults plist.
pub type DnsSnapshot = BTreeMap<String, BTreeMap<String, Vec<String>>>;
pub struct RepairResult { pub fixed: Vec<String>, pub errors: Vec<String> }
pub struct DnsGuard { pub vpn_dns: BTreeSet<String>, pub run: Runner }
impl DnsGuard {
    pub fn new(vpn_dns: BTreeSet<String>, run: Runner) -> Self;
    pub fn current_dns(&self) -> BTreeMap<String, Vec<String>>;
    pub fn stale(&self) -> Vec<String>;
    pub fn snapshot(&self) -> DnsSnapshot;
    pub fn repair(&self, snapshot: &DnsSnapshot) -> RepairResult;
    pub fn apply(&self, servers: &[String], search: &[String]) -> Vec<String>;
}

// on_demand.rs
pub struct TcpSocket { pub state: String, pub remote_ip: String, pub remote_port: u16 }
pub const LSOF_ARGS: [&str; 4] = ["-nP", "-iTCP", "-F", "nT"];
pub fn parse_lsof(output: &str) -> Vec<TcpSocket>;
pub fn ip_bytes(s: &str) -> Option<Vec<u8>>;
pub struct Cidr { pub network: Vec<u8>, pub prefix: u32 }
impl Cidr { pub fn parse(text: &str) -> Option<Cidr>; pub fn contains(&self, ip: &str) -> bool; }
pub fn parse_allowed_ips(config_text: &str) -> Vec<Cidr>;
pub fn is_full_tunnel(cidrs: &[Cidr]) -> bool;
pub fn vpn_bound(sockets: &[TcpSocket], allowed: &[Cidr]) -> Vec<TcpSocket>;
pub enum OnDemandState { Off, Armed, Connected, Paused }   // Copy, Eq, Debug
pub enum OnDemandAction { Connect, Disconnect, ResumeArmed }
/// Times are wall-clock SystemTime (Swift Date): a sleep must count as idle time.
pub fn decide(state, sticky: bool, has_sockets: bool, last_seen: SystemTime, now: SystemTime, idle: Duration) -> Option<OnDemandAction>;
pub type Clock = Arc<dyn Fn() -> SystemTime + Send + Sync>;
pub struct OnDemandController { /* Send; driven from one serial queue */ }
impl OnDemandController {
    pub const HELPER_PATH: &'static str = "/usr/local/libexec/wgbar-helper";
    pub const NEEDS_HELPER_MESSAGE: &'static str;
    pub fn helper_installed() -> bool;
    pub fn new(tunnel: String, allowed: Vec<Cidr>, idle: Duration, kick: Option<String>, run: Runner, now: Clock) -> Self;
    pub fn state(&self) -> OnDemandState;  pub fn sticky(&self) -> bool;  pub fn tunnel(&self) -> &str;
    // callbacks: Box<dyn FnMut(..) + Send>, default no-ops
    pub fn set_on_connected(&mut self, f: impl FnMut() + Send + 'static);
    pub fn set_on_disconnected(&mut self, f: impl FnMut() + Send + 'static);
    pub fn set_on_error(&mut self, f: impl FnMut(String) + Send + 'static);
    pub fn set_on_change(&mut self, f: impl FnMut(OnDemandState) + Send + 'static);
    pub fn arm(&mut self) -> bool;  pub fn reconcile(&mut self);  pub fn tick(&mut self);
    pub fn manual_toggle(&mut self);  pub fn shutdown(&mut self);
}

// updater.rs
pub enum UpdateStatus { UpToDate { revision: String }, Available { commits: Vec<String> }, Failed(String) }
pub struct Updater { pub repo_dir: String, pub run: Runner }
impl Updater { pub const NO_REPO_MESSAGE: &'static str; pub fn new(repo_dir: String, run: Runner) -> Self;
               pub fn check(&self) -> UpdateStatus; pub fn pull(&self) -> Option<String>; }

// tunnels.rs  (free functions from main.swift; pure where possible so they are testable)
pub const DEFAULT_CONF_DIRS: [&str; 4] = ["/opt/homebrew/etc/wireguard", "/usr/local/etc/wireguard",
                                          "/opt/local/etc/wireguard", "/etc/wireguard"];
pub const WG_QUICK_DIRS: [&str; 3] = ["/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin"];
pub const RUN_DIR: &str = "/var/run/wireguard";
pub const NETWORK_PREFS: &str = "/Library/Preferences/SystemConfiguration/preferences.plist";
pub fn resolve_wg_quick(custom: Option<&str>) -> String;       // defaults "wgQuick" override, else first executable, else /opt/homebrew/bin/wg-quick
pub fn brew_bin(wg_quick: &str) -> String;                     // parent dir
pub fn conf_names(dir: &str) -> Vec<String>;                   // *.conf stems, sorted
pub fn resolve_conf_dir(custom: Option<&str>) -> String;       // defaults "confDir" (non-empty) else first dir with configs, else first existing, else DEFAULT_CONF_DIRS[0]
pub fn conf_path(conf_dir: &str, tunnel: &str) -> String;
pub fn run_file(tunnel: &str) -> String;                       // /var/run/wireguard/<t>.name
pub fn tunnel_address(config_text: &str) -> Option<String>;
pub fn all_config_dns(conf_dir: &str) -> BTreeSet<String>;
pub fn any_tunnel_up() -> bool;                                // also *_in(dir) variants for tests
pub fn any_tunnel_connected(ignoring_armed: Option<&str>) -> bool;
pub fn network_prefs_modified() -> SystemTime;                 // UNIX_EPOCH when missing
/// Swift `wgQuick(action, tunnel)`: sudo -n first, then osascript admin dialog; None on success/cancel.
pub fn wg_quick(run: &Runner, wg_quick: &str, brew_bin: &str, action: &str, conf_path: &str) -> Option<String>;
```

## Rules

- Behaviour parity with the Swift code is the spec. Every Swift unit test has a Rust twin.
- No new runtime dependencies beyond Cargo.toml without a reason in the commit message.
- `cargo test`, `cargo clippy --all-targets -- -D warnings` (host) and
  `cargo clippy --target aarch64-apple-darwin --bins -- -D warnings` must pass.

## Status

- **Phase 1 — done (2026-11-06, Linux).** Core + 70 tests, helper + 13 shell checks, AppKit layer
  type-checked for aarch64/x86_64 macOS, clippy clean on both, Swift/bash sources removed,
  scripts and README updated. Two independent reviews: no crash, deadlock or security regression
  found; their small fixes are in. Linux numbers (not comparable to a Mac, for reference only):
  `logic_bench` parse_lsof_400 ≈ 24 µs, vpn_bound_400 ≈ 27 µs, poll_tick_400 ≈ 51 µs,
  parse_config ≈ 0.8 µs.
- **Phase 2 — in progress on the Mac (started 2026-11-07).**

## Phase 2 loop (run on the Mac: `/loop` with "continue the WGBar Rust port, phase 2 of docs/rust-port/plan.md")

Each iteration: pick the first unchecked item, do it, tick it here, commit. Stop when all are ticked.

- [x] `./test.sh` passes on macOS (core tests use real `inet_pton`, temp dirs).
      2026-11-07, macOS 27 arm64, rustc 1.99.1: 70 Rust tests + 12 helper checks pass;
      `cargo clippy --all-targets -- -D warnings` clean on the Mac host.
- [x] Baseline: Connect on Demand off, then `bench/compare.sh` → `bench/results/*.md` committed.
      2026-11-07 (`bench/results/2026-11-07-Nikos-MacBook-Pro.md`): launch median equal (61.4 ms),
      RSS 50.9 vs 54.1 MB, idle CPU 140 vs 170 ms/min, logic 2–23× faster; binary 503 vs 273 KB.
      `launch_probe` rewritten: the status-window check cannot work on macOS 27 (MenuBarAgent).
- [x] `cargo build` (debug) app runs from `target/debug/WGBar` without objc2 encoding panics.
      2026-11-07: launch, 2 s refresh/1 s tick timers, DNS check, status menu and Tunnel
      submenu opened by hand — no panic, no stderr, no crash report. Note: unbundled, it uses
      the `WGBar` defaults domain, not `org.wgbar.WGBar`; pre-seed `didRegisterLoginItem` there
      (`defaults write WGBar didRegisterLoginItem -bool true`) or it registers a login item.
- [ ] Left-click toggles; right-click opens the menu; next left-click toggles again.
- [ ] Busy states grey out items now (autoenabling off) — check nothing stays wrongly disabled.
- [ ] Tunnel submenu (≥2 configs), Config Folder… panel comes to front, choice persists.
- [ ] Launch at Login: first-launch registration, checkbox state, toggle; error text readable.
- [ ] DNS guard: leave VPN DNS behind (`networksetup -setdnsservers Wi-Fi <vpn dns>`), relaunch →
      repaired + notification; Repair DNS alert both ways; wake re-check after 3 s.
- [ ] `dnsSnapshot` written by the Swift app is read back and restored correctly.
- [x] `./sudoers.sh` builds + installs the Rust helper; `install.sh` staleness note goes away.
      2026-11-07: Rust app installed over the Swift one with `install.sh` (quit the Swift app
      first: `pkill` would leave an armed interface); it re-armed via the old bash helper (same
      verbs). `sudoers.sh` then installed the arm64 helper; installed and built `source-hash`
      match, `sudo -n wgbar-helper status` → armed.
- [ ] Connect on Demand: arm, connect on RDP/TCP traffic, idle-out, sticky manual connect,
      tunnel switch, quit while armed/connected (interface down, DNS restored).
      2026-11-07 (perf pass build): re-arm at launch, connect 0.3–1.2 s after a SYN to
      10.42.1.1:3389, idle-out 30 s after the last socket, quit while connected → `off` — OK.
      Still to check: sticky manual connect, tunnel switch, DNS restore on quit.
- [ ] Check for Updates: up to date / available / install.sh relaunch survives `pkill -x WGBar`.
- [ ] Child-process fd hygiene: no stray pipe fds inherited by osascript (review finding,
      unverified) — `lsof -p <osascript pid>` during an admin prompt.
- [x] Bench Rust ≤ Swift on launch, RSS and idle CPU; if not, profile (Instruments) and fix.
      2026-11-07, see "Performance pass" below and `bench/results/*-optimized.md`.

## Performance pass (2026-11-07, `/loop` "more efficient than the Swift code")

| | Swift (`main`) | Rust before | Rust after |
|---|---:|---:|---:|
| idle CPU, On-Demand off (side by side) | 150–290 ms/min | 147 ms/min | ~2–3 ms/min |
| idle wakeups, On-Demand off | 137–200 /min | 134 /min | 2–4 /min |
| On-Demand armed, app CPU | (+ ~2 s/min in lsof children) | 258 ms/min (+ ~1.5 s/min lsof) | 48 ms/min, no children |
| one socket poll (wall / CPU) | 100 ms / 34 ms | 26 ms / 25 ms | 0.6 ms / 0.6 ms, mostly skipped while armed |
| clean build / test.sh | 3.6 s / 1.9 s | 18.5 s / 31 s | 7 s / 7 s |
| launch, footprint | equal within noise | | equal / slightly lower |

What changed: libproc socket scan instead of spawning lsof (`src/proc_sockets.rs`); while
armed, scan only when the tunnel's utun output counter moved (`src/iface.rs`); On-Demand tick
is a dispatch timer on `od_queue` and exists only while On-Demand is on; refresh is driven by
kqueue watches on the run folder, network prefs and config folder (`src/watch.rs`) with a
30 s safety timer; icon/tooltip only re-set on change; objc2 features trimmed.

Notes for whoever measures next:
- Use `bench/idle.sh` (both apps at once); sequential idle numbers swing 2× with system load.
- The On-Demand poll runs on efficiency cores (App Nap / QoS), where a scan takes ~3–5 ms of
  CPU time instead of 0.6 ms; compare `instructions` (cpu_probe), not only CPU ms.
- `wgbar-helper status` says `connected` whenever the peer has an endpoint, and WireGuard
  learns one by roaming when the server sends a handshake; an armed tunnel can thus show
  `connected` without WGBar connecting it (seen with wg-monexo-niko). Not a regression.

## Follow-ups (not parity; decide later)

- Helper: open the config with O_NOFOLLOW + fstat and cap the read (pre-existing symlink read).
- Helper: remove the temp dir on SIGTERM/SIGHUP/SIGINT.
- Configs with CRLF line endings: trim `\r` (Swift and Rust both mis-parse them, differently).
- ~~`vpn_bound` re-parses every remote IP for every CIDR~~ — moot: the app now polls with libproc (`src/proc_sockets.rs`), matching raw address bytes.
