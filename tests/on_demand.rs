//! Port of tests/OnDemandTests.swift.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use wgbar::cmd::{CmdResult, Runner};
use wgbar::on_demand::*;

fn cidr(s: &str) -> Cidr {
    Cidr::parse(s).unwrap()
}

fn sock(state: &str, ip: &str, port: u16) -> TcpSocket {
    TcpSocket {
        state: state.into(),
        remote_ip: ip.into(),
        remote_port: port,
    }
}

// --- lsof ------------------------------------------------------------------

const LSOF: &str = "p1657
f63
n10.42.66.9:60655->10.42.1.9:3389
TST=ESTABLISHED
TQR=0
TQS=0
f64
n10.5.102.162:60773->3.233.158.111:443
TST=ESTABLISHED
p1732
f24
n10.42.66.9:60700->10.42.1.63:3389
TST=SYN_SENT
TQR=0
TQS=0
f25
n*:5000
TST=LISTEN
f26
n[fe80::1%lo0]:1024->[fe80::1%lo0]:1025
TST=ESTABLISHED
f27
n10.5.102.162:60760->160.79.104.10:443";

#[test]
fn lsof_records() {
    let sockets = parse_lsof(LSOF);
    assert_eq!(
        sockets.len(),
        4,
        "LISTEN (no ->) and records without a state are skipped"
    );
    assert_eq!(
        sockets[0],
        sock("ESTABLISHED", "10.42.1.9", 3389),
        "remote ip and port split at the last colon"
    );
    assert_eq!(
        sockets[2],
        sock("SYN_SENT", "10.42.1.63", 3389),
        "SYN_SENT parsed"
    );
    assert_eq!(
        sockets[3],
        sock("ESTABLISHED", "fe80::1", 1025),
        "IPv6 brackets and scope id stripped"
    );
    assert!(parse_lsof("").is_empty(), "empty output");
}

// --- CIDR ------------------------------------------------------------------

#[test]
fn ip_bytes_cases() {
    assert_eq!(ip_bytes("10.0.0.22"), Some(vec![10, 0, 0, 22]), "v4 bytes");
    assert_eq!(ip_bytes("::1").map(|b| b.len()), Some(16), "v6 is 16 bytes");
    assert_eq!(
        ip_bytes("office.example"),
        None,
        "hostname is not an address"
    );
    assert_eq!(ip_bytes("1.2.3"), None);
    assert_eq!(ip_bytes(""), None);
}

#[test]
fn cidr_cases() {
    let lan = cidr("10.0.0.0/24");
    let wide = cidr("10.42.0.0/16");
    assert!(lan.contains("10.0.0.1"), "first host in /24");
    assert!(lan.contains("10.0.0.255"), "last address in /24");
    assert!(!lan.contains("10.0.1.0"), "next network is outside /24");
    assert!(wide.contains("10.42.1.9"), "/16 match");
    assert!(!wide.contains("10.43.0.1"), "/16 miss");
    assert!(
        cidr("10.42.1.9").contains("10.42.1.9"),
        "bare address is a /32"
    );
    assert!(
        !cidr("10.42.1.9").contains("10.42.1.8"),
        "/32 excludes neighbours"
    );
    assert!(cidr("fd00::/8").contains("fd00:1::5"), "v6 match");
    assert!(
        !cidr("fd00::/8").contains("10.0.0.1"),
        "v4 never matches a v6 network"
    );
    assert_eq!(
        Cidr::parse("10.0.0.0/33"),
        None,
        "prefix too long is rejected"
    );
    assert_eq!(Cidr::parse("nonsense/8"), None, "bad address is rejected");
    assert_eq!(Cidr::parse(""), None);
    assert_eq!(Cidr::parse("10.0.0.0/-1"), None);
    // host bits are masked off: 10.0.0.77/24 == 10.0.0.0/24
    assert_eq!(cidr("10.0.0.77/24"), lan);
}

// --- AllowedIPs ------------------------------------------------------------

#[test]
fn allowed_ips() {
    let conf = "[Interface]\nAddress = 10.42.66.9/24\nDNS = 10.0.0.22\n\n[Peer]\nAllowedIPs = 10.0.0.0/24,10.42.0.0/16\nallowedips = fd00::/8\n";
    assert_eq!(
        parse_allowed_ips(conf),
        vec![cidr("10.0.0.0/24"), cidr("10.42.0.0/16"), cidr("fd00::/8")],
        "comma list and repeated key, case-insensitive"
    );
    assert!(
        parse_allowed_ips("[Interface]\nAddress = 10.0.0.2/32\n").is_empty(),
        "none"
    );
    assert_eq!(
        parse_allowed_ips("AllowedIPs = 10.0.0.0/24 , bogus, 10.1.0.0/16").len(),
        2
    );
}

#[test]
fn full_tunnel() {
    assert!(
        !is_full_tunnel(&[cidr("10.0.0.0/24")]),
        "split config is not full"
    );
    assert!(is_full_tunnel(&[cidr("0.0.0.0/0")]), "default route");
    assert!(is_full_tunnel(&[cidr("::/0")]), "v6 default route");
    assert!(
        is_full_tunnel(&[cidr("0.0.0.0/1"), cidr("128.0.0.0/1")]),
        "two halves count as full"
    );
}

// --- filter ----------------------------------------------------------------

#[test]
fn vpn_bound_filter() {
    let sockets = parse_lsof(LSOF);
    let bound = vpn_bound(&sockets, &[cidr("10.0.0.0/24"), cidr("10.42.0.0/16")]);
    let ips: Vec<&str> = bound.iter().map(|s| s.remote_ip.as_str()).collect();
    assert_eq!(
        ips,
        ["10.42.1.9", "10.42.1.63"],
        "only SYN_SENT/ESTABLISHED inside AllowedIPs"
    );
    assert!(
        vpn_bound(&sockets, &[cidr("192.168.0.0/16")]).is_empty(),
        "nothing inside"
    );
}

// --- decide ----------------------------------------------------------------

fn at(s: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_000_000 + s)
}

fn d(state: OnDemandState, sticky: bool, has: bool, last: u64, now: u64) -> Option<OnDemandAction> {
    decide(
        state,
        sticky,
        has,
        at(last),
        at(now),
        Duration::from_secs(30),
    )
}

#[test]
fn decide_table() {
    use OnDemandAction::*;
    use OnDemandState::*;
    assert_eq!(
        d(Armed, false, true, 0, 0),
        Some(Connect),
        "armed + traffic -> connect"
    );
    assert_eq!(
        d(Armed, false, false, 0, 100),
        None,
        "armed + quiet -> nothing"
    );
    assert_eq!(
        d(Connected, false, true, 50, 50),
        None,
        "connected + traffic -> stay"
    );
    assert_eq!(
        d(Connected, false, false, 50, 70),
        None,
        "connected + quiet but not idle yet"
    );
    assert_eq!(
        d(Connected, false, false, 50, 80),
        Some(Disconnect),
        "connected + idle reached"
    );
    assert_eq!(
        d(Connected, true, false, 50, 5000),
        None,
        "sticky never idles out"
    );
    assert_eq!(
        d(Paused, false, true, 0, 0),
        None,
        "paused + traffic -> stay paused"
    );
    assert_eq!(
        d(Paused, false, false, 0, 10),
        None,
        "paused + quiet but not idle yet"
    );
    assert_eq!(
        d(Paused, false, false, 0, 30),
        Some(ResumeArmed),
        "paused + idle reached -> armed"
    );
    assert_eq!(d(Off, false, true, 0, 0), None, "off never acts");
}

#[test]
fn decide_clock_stepped_backwards_is_not_idle() {
    // lastSeen in the future: quiet interval is negative, never >= idle.
    assert_eq!(d(OnDemandState::Connected, false, false, 100, 50), None);
}

// --- controller (fake sudo/lsof/ping) --------------------------------------

#[derive(Default)]
struct Events {
    connected: u32,
    disconnected: u32,
    errors: Vec<String>,
    states: Vec<OnDemandState>,
    log: Vec<String>,
}

struct Sys {
    lsof: String,
    lsof_fails: bool,
    helper_fails: BTreeSet<String>,
    sudo_needs_password: bool,
    status_output: String,
    verbs: Vec<String>,
    pings: Vec<String>,
    clock: SystemTime,
}

struct Fixture {
    sys: Arc<Mutex<Sys>>,
    ev: Arc<Mutex<Events>>,
}

impl Fixture {
    fn new() -> Self {
        Fixture {
            sys: Arc::new(Mutex::new(Sys {
                lsof: String::new(),
                lsof_fails: false,
                helper_fails: BTreeSet::new(),
                sudo_needs_password: false,
                status_output: "armed".into(),
                verbs: vec![],
                pings: vec![],
                clock: UNIX_EPOCH + Duration::from_secs(1_000_000),
            })),
            ev: Arc::new(Mutex::new(Events::default())),
        }
    }

    fn with(f: impl FnOnce(&mut Sys)) -> Self {
        let fx = Self::new();
        f(&mut fx.sys.lock().unwrap());
        fx
    }

    fn set_lsof(&self, s: &str) {
        self.sys.lock().unwrap().lsof = s.into();
    }

    fn advance(&self, s: u64) {
        let mut g = self.sys.lock().unwrap();
        g.clock += Duration::from_secs(s);
    }

    fn verbs(&self) -> Vec<String> {
        self.sys.lock().unwrap().verbs.clone()
    }

    fn log(&self) -> Vec<String> {
        self.ev.lock().unwrap().log.clone()
    }

    fn runner(&self) -> Runner {
        let sys = self.sys.clone();
        let ev = self.ev.clone();
        Arc::new(move |exe, args| {
            let mut s = sys.lock().unwrap();
            match exe {
                "/usr/sbin/lsof" => {
                    // No -s state filter: lsof exits 1 ("TCP state not located") whenever a listed
                    // state has no socket. States are filtered in vpn_bound.
                    assert_eq!(
                        args,
                        ["-nP", "-iTCP", "-F", "nT"],
                        "lsof asked for all TCP sockets in field format, no -s"
                    );
                    if s.lsof_fails {
                        return CmdResult::new(1, "lsof: can't open /dev/mem");
                    }
                    // lsof exits 1 with nothing printed when no file matched -iTCP at all.
                    CmdResult::new(if s.lsof.is_empty() { 1 } else { 0 }, s.lsof.clone())
                }
                "/sbin/ping" => {
                    s.pings.push(args.last().cloned().unwrap_or_default());
                    CmdResult::new(0, "")
                }
                "/usr/bin/sudo" => {
                    assert_eq!(
                        args[..2],
                        ["-n", OnDemandController::HELPER_PATH],
                        "sudo -n helper"
                    );
                    assert_eq!(
                        args.last().map(String::as_str),
                        Some("ileasing"),
                        "tunnel name passed"
                    );
                    if s.sudo_needs_password {
                        return CmdResult::new(1, "sudo: a password is required");
                    }
                    let verb = args[2].clone();
                    s.verbs.push(verb.clone());
                    ev.lock().unwrap().log.push(verb.clone());
                    if s.helper_fails.contains(&verb) {
                        return CmdResult::new(1, "wg-quick: boom");
                    }
                    CmdResult::new(
                        0,
                        if verb == "status" {
                            s.status_output.clone()
                        } else {
                            String::new()
                        },
                    )
                }
                _ => CmdResult::new(1, format!("unexpected {exe}")),
            }
        })
    }

    fn controller_kick(&self, kick: Option<&str>) -> OnDemandController {
        let sys = self.sys.clone();
        let mut c = OnDemandController::new(
            "ileasing".into(),
            vec![cidr("10.0.0.0/24"), cidr("10.42.0.0/16")],
            Duration::from_secs(30),
            kick.map(String::from),
            self.runner(),
            Arc::new(move || sys.lock().unwrap().clock),
        );
        let ev = self.ev.clone();
        c.set_on_connected(move || {
            let mut e = ev.lock().unwrap();
            e.connected += 1;
            e.log.push("onConnected".into());
        });
        let ev = self.ev.clone();
        c.set_on_disconnected(move || {
            let mut e = ev.lock().unwrap();
            e.disconnected += 1;
            e.log.push("onDisconnected".into());
        });
        let ev = self.ev.clone();
        c.set_on_error(move |m| ev.lock().unwrap().errors.push(m));
        let ev = self.ev.clone();
        c.set_on_change(move |s| ev.lock().unwrap().states.push(s));
        c
    }

    fn controller(&self) -> OnDemandController {
        self.controller_kick(Some("10.0.0.22"))
    }
}

const RDP: &str = "p1\nf3\nn10.42.66.9:60655->10.42.1.9:3389\nTST=ESTABLISHED\n";
const SYN: &str = "p1\nf4\nn10.42.66.9:60700->10.42.1.63:3389\nTST=SYN_SENT\n";
const WEB: &str = "p2\nf5\nn10.5.102.162:60773->3.233.158.111:443\nTST=ESTABLISHED\n";

fn strs(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

#[test]
fn controller_is_send() {
    fn assert_send<T: Send>() {}
    assert_send::<OnDemandController>();
}

#[test]
fn arm() {
    let fx = Fixture::new();
    let mut c = fx.controller();
    assert!(c.arm(), "arm: succeeds");
    assert_eq!(fx.verbs(), ["arm"], "calls helper arm");
    assert_eq!(c.state(), OnDemandState::Armed);
    assert_eq!(
        fx.ev.lock().unwrap().states,
        [OnDemandState::Armed],
        "change reported"
    );
    assert_eq!(c.tunnel(), "ileasing");
}

#[test]
fn arm_failure() {
    let fx = Fixture::with(|s| {
        s.helper_fails.insert("arm".into());
    });
    let mut c = fx.controller();
    assert!(!c.arm(), "returns false");
    assert_eq!(c.state(), OnDemandState::Off);
    assert_eq!(
        fx.ev.lock().unwrap().errors,
        ["wg-quick: boom"],
        "helper output surfaced"
    );
}

#[test]
fn missing_sudoers_rule() {
    let fx = Fixture::with(|s| s.sudo_needs_password = true);
    let mut c = fx.controller();
    c.arm();
    assert_eq!(
        fx.ev.lock().unwrap().errors,
        [OnDemandController::NEEDS_HELPER_MESSAGE],
        "friendly message"
    );
}

#[test]
fn full_cycle() {
    let fx = Fixture::new();
    let mut c = fx.controller();
    c.arm();
    fx.set_lsof(WEB);
    c.tick();
    assert_eq!(
        c.state(),
        OnDemandState::Armed,
        "unrelated traffic does not connect"
    );
    fx.set_lsof(&format!("{WEB}{SYN}"));
    c.tick();
    assert_eq!(
        c.state(),
        OnDemandState::Connected,
        "SYN_SENT into the tunnel connects"
    );
    assert_eq!(fx.verbs(), ["arm", "connect"], "helper connect called once");
    assert_eq!(
        fx.sys.lock().unwrap().pings,
        ["10.0.0.22"],
        "handshake kicked with a ping"
    );
    assert_eq!(
        fx.ev.lock().unwrap().connected,
        1,
        "onConnected fired (DNS applied)"
    );
    assert_eq!(
        fx.log(),
        ["arm", "connect", "onConnected"],
        "onConnected fires after helper connect"
    );
    fx.set_lsof(RDP);
    fx.advance(600);
    c.tick();
    assert_eq!(
        c.state(),
        OnDemandState::Connected,
        "established session keeps it up"
    );
    fx.set_lsof(WEB);
    fx.advance(20);
    c.tick();
    assert_eq!(
        c.state(),
        OnDemandState::Connected,
        "20 s quiet is not idle yet"
    );
    fx.advance(10);
    c.tick();
    assert_eq!(
        c.state(),
        OnDemandState::Armed,
        "30 s quiet disconnects back to armed"
    );
    assert_eq!(
        fx.verbs(),
        ["arm", "connect", "disconnect"],
        "helper disconnect called"
    );
    assert_eq!(
        fx.ev.lock().unwrap().disconnected,
        1,
        "onDisconnected fired (DNS restored)"
    );
    assert_eq!(
        fx.ev.lock().unwrap().states,
        [
            OnDemandState::Armed,
            OnDemandState::Connected,
            OnDemandState::Armed
        ],
        "state changes reported"
    );
    assert_eq!(
        fx.log(),
        [
            "arm",
            "connect",
            "onConnected",
            "onDisconnected",
            "disconnect"
        ],
        "DNS restored before helper disconnect"
    );
}

#[test]
fn connect_failure_goes_off_without_retry_loop() {
    let fx = Fixture::with(|s| {
        s.helper_fails.insert("connect".into());
    });
    let mut c = fx.controller();
    c.arm();
    fx.set_lsof(SYN);
    c.tick();
    c.tick();
    assert_eq!(c.state(), OnDemandState::Off);
    assert_eq!(fx.verbs(), ["arm", "connect"], "not retried every tick");
    assert_eq!(fx.ev.lock().unwrap().errors.len(), 1, "one error");
    assert_eq!(fx.ev.lock().unwrap().connected, 0, "DNS untouched");
}

#[test]
fn manual_connect_is_sticky_manual_disconnect_with_live_sockets_pauses() {
    let fx = Fixture::new();
    let mut c = fx.controller_kick(None);
    c.arm();
    c.manual_toggle();
    assert_eq!(c.state(), OnDemandState::Connected, "click connects");
    assert!(c.sticky());
    assert!(
        fx.sys.lock().unwrap().pings.is_empty(),
        "no kick address -> no ping"
    );
    fx.set_lsof("");
    fx.advance(3600);
    c.tick();
    assert_eq!(c.state(), OnDemandState::Connected, "sticky ignores idle");
    fx.set_lsof(RDP);
    c.manual_toggle();
    assert_eq!(
        c.state(),
        OnDemandState::Paused,
        "disconnect while a session exists -> paused"
    );
    assert!(!c.sticky());
    assert_eq!(
        fx.ev.lock().unwrap().disconnected,
        1,
        "DNS restored on manual disconnect"
    );
    fx.set_lsof(SYN);
    c.tick();
    fx.advance(10);
    c.tick();
    assert_eq!(
        c.state(),
        OnDemandState::Paused,
        "reconnect attempts keep it paused"
    );
    assert_eq!(
        fx.verbs(),
        ["arm", "connect", "disconnect"],
        "paused does not reconnect"
    );
    fx.set_lsof("");
    c.tick();
    fx.advance(30);
    c.tick();
    assert_eq!(
        c.state(),
        OnDemandState::Armed,
        "quiet for idle -> armed again"
    );
    fx.set_lsof(SYN);
    c.tick();
    assert_eq!(
        c.state(),
        OnDemandState::Connected,
        "armed again reacts to new traffic"
    );
    assert_eq!(
        fx.verbs().last().map(String::as_str),
        Some("connect"),
        "auto connect after pause"
    );
}

#[test]
fn manual_disconnect_with_no_sockets_goes_straight_to_armed() {
    let fx = Fixture::new();
    let mut c = fx.controller();
    c.arm();
    c.manual_toggle();
    fx.set_lsof("");
    c.manual_toggle();
    assert_eq!(c.state(), OnDemandState::Armed);
}

#[test]
fn reconcile_at_launch_or_wake() {
    let fx = Fixture::new();
    let mut c = fx.controller();
    fx.sys.lock().unwrap().status_output = "off".into();
    c.reconcile();
    assert_eq!(c.state(), OnDemandState::Off);
    fx.sys.lock().unwrap().status_output = "armed".into();
    c.reconcile();
    assert_eq!(c.state(), OnDemandState::Armed);
    fx.sys.lock().unwrap().status_output = "Warning: something\nconnected".into();
    c.reconcile();
    assert_eq!(c.state(), OnDemandState::Connected, "last line wins");
    fx.set_lsof("");
    fx.advance(30);
    c.tick();
    assert_eq!(
        c.state(),
        OnDemandState::Armed,
        "adopted connection is not sticky and idles out"
    );
}

#[test]
fn reconcile_keeps_sticky_manual_connection() {
    let fx = Fixture::new();
    let mut c = fx.controller();
    c.arm();
    c.manual_toggle();
    assert_eq!(c.state(), OnDemandState::Connected, "manual connect first");
    fx.sys.lock().unwrap().status_output = "connected".into();
    c.reconcile();
    fx.set_lsof("");
    fx.advance(3600);
    c.tick();
    assert_eq!(
        c.state(),
        OnDemandState::Connected,
        "a re-confirmed manual connection stays sticky"
    );
    assert!(
        !fx.verbs().contains(&"disconnect".to_string()),
        "sticky connection is not idled out"
    );
}

#[test]
fn shutdown_cases() {
    let fx = Fixture::new();
    let mut c = fx.controller();
    c.arm();
    fx.set_lsof(SYN);
    c.tick();
    c.shutdown();
    assert_eq!(fx.verbs(), ["arm", "connect", "down"], "connected -> down");
    assert_eq!(fx.ev.lock().unwrap().disconnected, 1, "DNS restored first");
    assert_eq!(c.state(), OnDemandState::Off);
    assert_eq!(
        fx.log(),
        ["arm", "connect", "onConnected", "onDisconnected", "down"],
        "DNS restored before helper down"
    );

    let fx2 = Fixture::new();
    let mut c2 = fx2.controller();
    c2.shutdown();
    assert_eq!(
        fx2.verbs(),
        ["down"],
        "always issues down so a failed connect cannot orphan the interface"
    );
    assert_eq!(
        fx2.ev.lock().unwrap().disconnected,
        0,
        "no DNS restore when off"
    );
}

#[test]
fn manual_toggle_from_off_rearms() {
    let fx = Fixture::new();
    let mut c = fx.controller();
    c.manual_toggle();
    assert_eq!(fx.verbs(), ["arm"]);
    assert_eq!(c.state(), OnDemandState::Armed);
}

#[test]
fn failing_disconnect_still_restores_dns_and_goes_off() {
    let fx = Fixture::with(|s| {
        s.helper_fails.insert("disconnect".into());
    });
    let mut c = fx.controller();
    c.arm();
    fx.set_lsof(SYN);
    c.tick();
    assert_eq!(c.state(), OnDemandState::Connected, "connected first");
    fx.set_lsof("");
    fx.advance(30);
    c.tick();
    assert_eq!(c.state(), OnDemandState::Off);
    assert_eq!(fx.ev.lock().unwrap().disconnected, 1, "DNS restored anyway");
    assert_eq!(
        fx.ev.lock().unwrap().errors,
        ["wg-quick: boom"],
        "helper output surfaced"
    );
}

#[test]
fn failing_status_in_reconcile() {
    let fx = Fixture::with(|s| {
        s.helper_fails.insert("status".into());
    });
    let mut c = fx.controller();
    c.reconcile();
    assert_eq!(c.state(), OnDemandState::Off);
    assert_eq!(fx.ev.lock().unwrap().errors, ["wg-quick: boom"]);
}

#[test]
fn lsof_failure_is_unknown_not_quiet() {
    let fx = Fixture::new();
    let mut c = fx.controller();
    c.arm();
    fx.set_lsof(SYN);
    c.tick();
    assert_eq!(c.state(), OnDemandState::Connected, "connected first");
    fx.sys.lock().unwrap().lsof_fails = true;
    fx.advance(3600);
    c.tick();
    assert_eq!(
        c.state(),
        OnDemandState::Connected,
        "stays connected, does not idle out"
    );
    assert_eq!(fx.verbs(), ["arm", "connect"], "no disconnect verb called");
    assert_eq!(fx.ev.lock().unwrap().disconnected, 0, "DNS untouched");
}

#[test]
fn lsof_exit_1_without_output_is_no_traffic() {
    let fx = Fixture::new();
    let mut c = fx.controller();
    c.arm();
    fx.set_lsof(SYN);
    c.tick();
    assert_eq!(c.state(), OnDemandState::Connected, "connected first");
    fx.set_lsof(""); // fake returns status 1 for empty output
    fx.advance(30);
    c.tick();
    assert_eq!(
        c.state(),
        OnDemandState::Armed,
        "exit 1 without output is no traffic -> disconnects"
    );
    assert_eq!(
        fx.verbs(),
        ["arm", "connect", "disconnect"],
        "disconnect verb called"
    );
}

#[test]
fn lsof_listen_only_is_no_traffic() {
    let fx = Fixture::new();
    let mut c = fx.controller();
    c.arm();
    fx.set_lsof(SYN);
    c.tick();
    fx.set_lsof("p9\nf3\nn*:5000\nTST=LISTEN\n");
    fx.advance(30);
    c.tick();
    assert_eq!(
        c.state(),
        OnDemandState::Armed,
        "LISTEN records are not VPN traffic -> disconnects"
    );
}

#[test]
fn helper_failure_without_output_has_default_message() {
    let fx = Fixture::new();
    let run: Runner = Arc::new(|_, _| CmdResult::new(1, ""));
    let errors = Arc::new(Mutex::new(vec![]));
    let mut c = OnDemandController::new(
        "t".into(),
        vec![],
        Duration::from_secs(30),
        None,
        run,
        Arc::new(move || fx.sys.lock().unwrap().clock),
    );
    let e = errors.clone();
    c.set_on_error(move |m| e.lock().unwrap().push(m));
    c.arm();
    assert_eq!(*errors.lock().unwrap(), strs(&["wgbar-helper arm failed"]));
}
