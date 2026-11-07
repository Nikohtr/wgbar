//! The libproc socket scan against real sockets of this test process (macOS only).
#![cfg(target_os = "macos")]

use std::collections::BTreeSet;
use std::net::{TcpListener, TcpStream};
use wgbar::cmd::run_with_path;
use wgbar::on_demand::{parse_lsof, vpn_bound, Cidr, LSOF_ARGS};
use wgbar::proc_sockets::Scanner;

fn cidrs(list: &[&str]) -> Vec<Cidr> {
    list.iter().map(|c| Cidr::parse(c).unwrap()).collect()
}

/// A connected pair on `addr`; returns the listener's port (the client's remote port).
fn connected(addr: &str) -> (TcpListener, TcpStream, TcpStream, u16) {
    let listener = TcpListener::bind(addr).unwrap();
    let port = listener.local_addr().unwrap().port();
    let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (server, _) = listener.accept().unwrap();
    (listener, client, server, port)
}

#[test]
fn finds_own_ipv4_and_ipv6_connections() {
    let (_l4, _c4, _s4, port4) = connected("127.0.0.1:0");
    let (_l6, _c6, _s6, port6) = connected("[::1]:0");
    let found = Scanner::new()
        .vpn_bound(&cidrs(&["127.0.0.0/8", "::1/128"]))
        .expect("libproc lists processes");
    let has = |ip: &str, port: u16| {
        found
            .iter()
            .any(|s| s.remote_ip == ip && s.remote_port == port && s.state == "ESTABLISHED")
    };
    assert!(
        has("127.0.0.1", port4),
        "client side of the IPv4 pair: {found:?}"
    );
    assert!(has("::1", port6), "client side of the IPv6 pair: {found:?}");
}

#[test]
fn skips_addresses_outside_allowed_and_listeners() {
    let (_l, _c, _s, port) = connected("127.0.0.1:0");
    let found = Scanner::new()
        .vpn_bound(&cidrs(&["10.200.0.0/16"]))
        .unwrap();
    assert!(
        !found.iter().any(|s| s.remote_port == port),
        "loopback is outside 10.200/16: {found:?}"
    );
    let found = Scanner::new().vpn_bound(&cidrs(&["127.0.0.1/32"])).unwrap();
    assert!(found
        .iter()
        .all(|s| s.state == "ESTABLISHED" || s.state == "SYN_SENT"));
}

/// Same answer as the lsof path for this process's own connections (other processes' sockets
/// may come and go between the two reads, so only ours are compared).
#[test]
fn agrees_with_lsof() {
    let (_l, _c, _s, port) = connected("127.0.0.1:0");
    let allowed = cidrs(&["127.0.0.0/8"]);
    let key = |v: Vec<wgbar::on_demand::TcpSocket>| {
        v.into_iter()
            .filter(|s| s.remote_port == port)
            .map(|s| (s.state, s.remote_ip, s.remote_port))
            .collect::<BTreeSet<_>>()
    };
    let native = key(Scanner::new().vpn_bound(&allowed).unwrap());
    let args: Vec<String> = LSOF_ARGS.iter().map(|s| s.to_string()).collect();
    let r = run_with_path("/usr/bin", "/usr/sbin/lsof", &args);
    let lsof = key(vpn_bound(&parse_lsof(&r.output), &allowed));
    assert!(!native.is_empty());
    assert_eq!(native, lsof);
}

#[test]
fn reuses_buffers_across_scans() {
    let mut scanner = Scanner::new();
    let allowed = cidrs(&["127.0.0.0/8"]);
    for _ in 0..3 {
        assert!(scanner.vpn_bound(&allowed).is_some());
    }
}
