//! OnDemand — keeps a split-tunnel WireGuard interface "armed" (up, routes installed, peer without
//! endpoint) and connects it when TCP traffic to its AllowedIPs appears, disconnecting when the
//! traffic stops. Design: docs/superpowers/specs/2026-09-02-on-demand-design.md

use crate::cmd::Runner;
use crate::dns_guard::{config_values, split_max1, trim_ws};
use std::ffi::CString;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

// MARK: Sockets

/// One TCP socket as reported by lsof: the connection state and the remote end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TcpSocket {
    pub state: String,
    pub remote_ip: String,
    pub remote_port: u16,
}

/// The command whose output [`parse_lsof`] reads. lsof is used instead of `netstat -p tcp` because
/// on macOS 27 the kernel hands netstat an empty socket list when any ancestor process is an
/// ad-hoc signed binary (as WGBar.app is), while lsof's per-process lookup still works.
///
/// Deliberately no `-sTCP:SYN_SENT,ESTABLISHED`: lsof treats each listed state as a search item
/// and exits 1 ("TCP state not located: SYN_SENT") whenever one has no socket. SYN_SENT exists
/// only for the instant a connection opens, so with the filter every quiet tick looked like a
/// failed read and a connected tunnel never idled out. [`vpn_bound`] filters the states instead.
pub const LSOF_ARGS: [&str; 4] = ["-nP", "-iTCP", "-F", "nT"];

/// Parses `lsof -F nT` output: an `n<local>-><remote>` line followed by `TST=<state>` per socket.
/// Records without a remote end (LISTEN, `*:port`) or without a state line are skipped.
pub fn parse_lsof(output: &str) -> Vec<TcpSocket> {
    let mut result = Vec::new();
    let mut pending: Option<(String, u16)> = None;
    for line in output.split('\n').filter(|l| !l.is_empty()) {
        if line.starts_with('n') {
            pending = None;
            let Some(arrow) = line.find("->") else {
                continue;
            };
            let remote = &line[arrow + 2..];
            let Some(colon) = remote.rfind(':') else {
                continue;
            };
            let Some(port) = remote[colon + 1..]
                .parse::<i64>()
                .ok()
                .and_then(|p| u16::try_from(p).ok())
            else {
                continue;
            };
            let mut ip = &remote[..colon];
            if ip.starts_with('[') && ip.ends_with(']') && ip.len() >= 2 {
                ip = &ip[1..ip.len() - 1]; // [fe80::1%lo0]
            }
            if let Some(pct) = ip.find('%') {
                ip = &ip[..pct];
            }
            pending = Some((ip.to_string(), port));
        } else if let (Some(state), Some(_)) = (line.strip_prefix("TST="), &pending) {
            let (ip, port) = pending.take().unwrap();
            result.push(TcpSocket {
                state: state.to_string(),
                remote_ip: ip,
                remote_port: port,
            });
        } else if line.starts_with('p') || line.starts_with('f') {
            pending = None;
        }
    }
    result
}

// MARK: Addresses

extern "C" {
    // POSIX; the libc crate does not expose it.
    fn inet_pton(af: libc::c_int, src: *const libc::c_char, dst: *mut libc::c_void) -> libc::c_int;
}

/// Raw bytes of an IPv4 (4) or IPv6 (16) address; `None` for anything else (hostnames included).
pub fn ip_bytes(s: &str) -> Option<Vec<u8>> {
    let c = CString::new(s).ok()?;
    let mut buf = [0u8; 16];
    // SAFETY: `c` is NUL-terminated and `buf` holds 16 bytes, enough for either address family.
    unsafe {
        if inet_pton(libc::AF_INET, c.as_ptr(), buf.as_mut_ptr().cast()) == 1 {
            return Some(buf[..4].to_vec());
        }
        if inet_pton(libc::AF_INET6, c.as_ptr(), buf.as_mut_ptr().cast()) == 1 {
            return Some(buf.to_vec());
        }
    }
    None
}

fn masked(bytes: &[u8], prefix: u32) -> Vec<u8> {
    bytes
        .iter()
        .enumerate()
        .map(|(i, &byte)| {
            let bits = prefix.saturating_sub(i as u32 * 8).min(8);
            if bits == 0 {
                0
            } else {
                byte & (0xFFu8 << (8 - bits))
            }
        })
        .collect()
}

/// A network in CIDR notation (`10.0.0.0/24`, `fd00::/8`); a bare address is a single host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cidr {
    pub network: Vec<u8>,
    pub prefix: u32,
}

impl Cidr {
    pub fn parse(text: &str) -> Option<Cidr> {
        let parts = split_max1(text, '/');
        let addr = ip_bytes(parts.first()?)?;
        let max_bits = addr.len() as i64 * 8;
        let prefix = if parts.len() == 2 {
            parts[1].parse::<i64>().ok()?
        } else {
            max_bits
        };
        if !(0..=max_bits).contains(&prefix) {
            return None;
        }
        let prefix = prefix as u32;
        Some(Cidr {
            network: masked(&addr, prefix),
            prefix,
        })
    }

    pub fn contains(&self, ip: &str) -> bool {
        match ip_bytes(ip) {
            Some(bytes) if bytes.len() == self.network.len() => {
                masked(&bytes, self.prefix) == self.network
            }
            _ => false,
        }
    }
}

/// Every network listed on `AllowedIPs =` lines of a wg-quick config.
pub fn parse_allowed_ips(config_text: &str) -> Vec<Cidr> {
    config_values(config_text, "allowedips")
        .flat_map(|v| v.split(','))
        .filter_map(|entry| Cidr::parse(trim_ws(entry)))
        .collect()
}

/// A config that routes (nearly) everything: arming it would black-hole all traffic.
pub fn is_full_tunnel(cidrs: &[Cidr]) -> bool {
    cidrs.iter().any(|c| c.prefix <= 1)
}

/// Connections that are being opened or are open to an address inside the tunnel.
pub fn vpn_bound(sockets: &[TcpSocket], allowed: &[Cidr]) -> Vec<TcpSocket> {
    sockets
        .iter()
        .filter(|s| {
            (s.state == "SYN_SENT" || s.state == "ESTABLISHED")
                && allowed.iter().any(|c| c.contains(&s.remote_ip))
        })
        .cloned()
        .collect()
}

// MARK: State machine

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnDemandState {
    Off,
    Armed,
    Connected,
    Paused,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnDemandAction {
    Connect,
    Disconnect,
    ResumeArmed,
}

/// What to do on a poll tick. `last_seen` is when VPN-bound sockets were last observed (or the last
/// connect/disconnect, whichever is later); `idle` is how long to wait without traffic.
/// Times are wall-clock `SystemTime` (Swift `Date`): a sleep must count as idle time. The quiet
/// interval is signed because the wall clock can step backwards.
pub fn decide(
    state: OnDemandState,
    sticky: bool,
    has_sockets: bool,
    last_seen: SystemTime,
    now: SystemTime,
    idle: Duration,
) -> Option<OnDemandAction> {
    let quiet_for = match now.duration_since(last_seen) {
        Ok(d) => d.as_secs_f64(),
        Err(e) => -e.duration().as_secs_f64(),
    };
    let idled = quiet_for >= idle.as_secs_f64();
    match state {
        OnDemandState::Armed => has_sockets.then_some(OnDemandAction::Connect),
        OnDemandState::Connected => {
            (!sticky && !has_sockets && idled).then_some(OnDemandAction::Disconnect)
        }
        OnDemandState::Paused => (!has_sockets && idled).then_some(OnDemandAction::ResumeArmed),
        OnDemandState::Off => None,
    }
}

// MARK: Controller

/// Wall clock source (injected so tests can move time).
pub type Clock = Arc<dyn Fn() -> SystemTime + Send + Sync>;

/// Drives one tunnel through armed ⇄ connected by polling `lsof` and calling the root helper.
/// Synchronous; call every method from a single serial queue. Callbacks fire on that queue.
pub struct OnDemandController {
    tunnel: String,
    allowed: Vec<Cidr>,
    idle: Duration,
    /// Pinged right after connect so the handshake starts now instead of at the next TCP retransmit.
    kick: Option<String>,
    run: Runner,
    now: Clock,

    state: OnDemandState,
    sticky: bool,
    last_seen: SystemTime,

    on_connected: Box<dyn FnMut() + Send>,
    on_disconnected: Box<dyn FnMut() + Send>,
    on_error: Box<dyn FnMut(String) + Send>,
    on_change: Box<dyn FnMut(OnDemandState) + Send>,
}

impl OnDemandController {
    pub const HELPER_PATH: &'static str = "/usr/local/libexec/wgbar-helper";
    pub const NEEDS_HELPER_MESSAGE: &'static str =
        "On-Demand needs the WGBar helper.\nRun ./sudoers.sh in the WGBar folder and try again.";

    pub fn helper_installed() -> bool {
        crate::tunnels::is_executable_file(Self::HELPER_PATH)
    }

    pub fn new(
        tunnel: String,
        allowed: Vec<Cidr>,
        idle: Duration,
        kick: Option<String>,
        run: Runner,
        now: Clock,
    ) -> Self {
        let last_seen = now();
        Self {
            tunnel,
            allowed,
            idle,
            kick,
            run,
            now,
            state: OnDemandState::Off,
            sticky: false,
            last_seen,
            on_connected: Box::new(|| {}),
            on_disconnected: Box::new(|| {}),
            on_error: Box::new(|_| {}),
            on_change: Box::new(|_| {}),
        }
    }

    pub fn state(&self) -> OnDemandState {
        self.state
    }

    pub fn sticky(&self) -> bool {
        self.sticky
    }

    pub fn tunnel(&self) -> &str {
        &self.tunnel
    }

    pub fn set_on_connected(&mut self, f: impl FnMut() + Send + 'static) {
        self.on_connected = Box::new(f);
    }

    pub fn set_on_disconnected(&mut self, f: impl FnMut() + Send + 'static) {
        self.on_disconnected = Box::new(f);
    }

    pub fn set_on_error(&mut self, f: impl FnMut(String) + Send + 'static) {
        self.on_error = Box::new(f);
    }

    pub fn set_on_change(&mut self, f: impl FnMut(OnDemandState) + Send + 'static) {
        self.on_change = Box::new(f);
    }

    fn set(&mut self, s: OnDemandState) {
        if s == self.state {
            return;
        }
        self.state = s;
        (self.on_change)(s);
    }

    /// `sudo -n wgbar-helper <verb> <tunnel>`. A password prompt means the sudoers rule is missing.
    fn helper(&self, verb: &str) -> (bool, String) {
        let args: Vec<String> = ["-n", Self::HELPER_PATH, verb, &self.tunnel]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let r = (self.run)("/usr/bin/sudo", &args);
        if r.status == 0 {
            return (true, r.output);
        }
        if r.output.contains("password") {
            return (false, Self::NEEDS_HELPER_MESSAGE.to_string());
        }
        if r.output.is_empty() {
            return (false, format!("wgbar-helper {verb} failed"));
        }
        (false, r.output)
    }

    /// `None` when `lsof` itself failed — a failed read must not be mistaken for "no traffic".
    /// lsof also exits 1 when simply nothing matched `-iTCP`; then it prints nothing (errors always
    /// come with a message on stderr, which `run` merges into `output`), and that is "no traffic".
    fn sockets(&self) -> Option<Vec<TcpSocket>> {
        let args: Vec<String> = LSOF_ARGS.iter().map(|s| s.to_string()).collect();
        let r = (self.run)("/usr/sbin/lsof", &args);
        if !(r.status == 0 || (r.status == 1 && r.output.is_empty())) {
            return None;
        }
        Some(vpn_bound(&parse_lsof(&r.output), &self.allowed))
    }

    /// Bring the interface up without an endpoint. False (and `on_error`) if the helper failed.
    pub fn arm(&mut self) -> bool {
        let (ok, output) = self.helper("arm");
        if ok {
            self.set(OnDemandState::Armed);
        } else {
            self.set(OnDemandState::Off);
            (self.on_error)(output);
        }
        ok
    }

    /// Adopt whatever state the interface is actually in (launch, wake).
    pub fn reconcile(&mut self) {
        let (ok, output) = self.helper("status");
        if !ok {
            self.set(OnDemandState::Off);
            (self.on_error)(output);
            return;
        }
        match output.split('\n').rfind(|l| !l.is_empty()).unwrap_or("") {
            // Only a genuine adoption clears stickiness; re-confirming a manual connection keeps it.
            "connected" => {
                if self.state != OnDemandState::Connected {
                    self.sticky = false;
                }
                self.last_seen = (self.now)();
                self.set(OnDemandState::Connected);
            }
            "armed" => self.set(OnDemandState::Armed),
            _ => self.set(OnDemandState::Off),
        }
    }

    /// One poll: look at the sockets, act on [`decide`]. Skipped (state unchanged) if `lsof` fails.
    pub fn tick(&mut self) {
        if self.state == OnDemandState::Off {
            return;
        }
        let Some(socks) = self.sockets() else {
            return;
        };
        let has = !socks.is_empty();
        let t = (self.now)();
        if has {
            self.last_seen = t;
        }
        match decide(self.state, self.sticky, has, self.last_seen, t, self.idle) {
            Some(OnDemandAction::Connect) => self.connect(false),
            Some(OnDemandAction::Disconnect) => self.disconnect(),
            Some(OnDemandAction::ResumeArmed) => self.set(OnDemandState::Armed),
            None => {}
        }
    }

    /// Left-click: connect (sticky) from armed/paused, disconnect from connected, re-arm from off.
    pub fn manual_toggle(&mut self) {
        match self.state {
            OnDemandState::Armed | OnDemandState::Paused => self.connect(true),
            OnDemandState::Connected => self.disconnect(),
            OnDemandState::Off => {
                self.arm();
            }
        }
    }

    /// Restore DNS if needed and take the interface down (quit, disable, tunnel change).
    /// `down` is issued unconditionally — a failed connect or disconnect leaves the state `Off`
    /// while the OS interface is still up, and the helper's `down` exits 0 when there is nothing
    /// to take down, so this is both safe and the only way to guarantee nothing is orphaned.
    pub fn shutdown(&mut self) {
        if self.state == OnDemandState::Connected {
            (self.on_disconnected)();
        }
        let (ok, output) = self.helper("down");
        if !ok {
            (self.on_error)(output);
        }
        self.set(OnDemandState::Off);
    }

    fn connect(&mut self, sticky: bool) {
        let (ok, output) = self.helper("connect");
        if !ok {
            // Off: no retry storm.
            self.set(OnDemandState::Off);
            (self.on_error)(output);
            return;
        }
        self.sticky = sticky;
        self.last_seen = (self.now)();
        if let Some(kick) = &self.kick {
            let args: Vec<String> = ["-c", "1", "-W", "1000", kick]
                .iter()
                .map(|s| s.to_string())
                .collect();
            let _ = (self.run)("/sbin/ping", &args);
        }
        self.set(OnDemandState::Connected);
        (self.on_connected)();
    }

    fn disconnect(&mut self) {
        (self.on_disconnected)(); // put DNS back before the tunnel stops answering
        let (ok, output) = self.helper("disconnect");
        self.sticky = false;
        self.last_seen = (self.now)();
        if !ok {
            self.set(OnDemandState::Off);
            (self.on_error)(output);
            return;
        }
        let quiet = self.sockets().unwrap_or_default().is_empty();
        self.set(if quiet {
            OnDemandState::Armed
        } else {
            OnDemandState::Paused
        });
    }
}
