//! Interface packet counters (macOS only).
#![cfg(target_os = "macos")]

use std::io::Write;
use std::net::{TcpListener, TcpStream};
use wgbar::iface::{out_packets, tunnel_addresses};

#[test]
fn parses_address_values() {
    assert_eq!(
        tunnel_addresses("10.43.66.2/32, fd00::2/128"),
        vec![vec![10, 43, 66, 2], {
            let mut v = vec![0xfd, 0];
            v.resize(15, 0);
            v.push(2);
            v
        }]
    );
    assert!(tunnel_addresses("").is_empty());
    assert_eq!(tunnel_addresses("10.0.0.2").len(), 1, "no prefix is fine");
}

#[test]
fn counts_packets_out_of_the_interface_holding_an_address() {
    let lo = tunnel_addresses("127.0.0.1/8");
    let before = out_packets(&lo).expect("lo0 has 127.0.0.1");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    client.write_all(b"ping").unwrap();
    let after = out_packets(&lo).unwrap();
    assert_ne!(
        before, after,
        "a loopback connection sends packets out of lo0"
    );
}

#[test]
fn unknown_address_is_none() {
    assert_eq!(out_packets(&tunnel_addresses("192.0.2.254")), None);
    assert_eq!(out_packets(&[]), None);
}
