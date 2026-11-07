//! Port of tests/DNSGuardTests.swift (parsing, detection, orchestration, apply).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use wgbar::cmd::{CmdResult, Runner};
use wgbar::dns_guard::*;

fn v(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

fn set(a: &[&str]) -> BTreeSet<String> {
    a.iter().map(|s| s.to_string()).collect()
}

fn map(a: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
    a.iter().map(|(k, x)| (k.to_string(), v(x))).collect()
}

// --- parsing ---------------------------------------------------------------

#[test]
fn config_comma_separated_dns_line() {
    assert_eq!(
        config_dns_servers("[Interface]\nAddress = 10.0.0.2/32\nDNS = 10.0.0.22, 10.0.0.4\n"),
        v(&["10.0.0.22", "10.0.0.4"])
    );
}

#[test]
fn config_multiple_dns_lines_case_insensitive_key_no_spaces() {
    assert_eq!(
        config_dns_servers("dns=10.0.0.22\nDNS = 10.0.0.4\n"),
        v(&["10.0.0.22", "10.0.0.4"])
    );
}

#[test]
fn config_search_domains_on_the_dns_line_are_not_servers() {
    assert_eq!(
        config_dns_servers("DNS = 10.0.0.22, corp.example\n"),
        v(&["10.0.0.22"])
    );
}

#[test]
fn config_no_dns_line() {
    assert!(config_dns_servers("[Interface]\nAddress = 10.0.0.2/32\n").is_empty());
}

#[test]
fn config_ipv6_server() {
    assert_eq!(
        config_dns_servers("DNS = fd00::1, 10.0.0.22\n"),
        v(&["fd00::1", "10.0.0.22"])
    );
}

#[test]
fn services_header_dropped_disabled_marker_stripped() {
    assert_eq!(
        parse_service_list(
            "An asterisk (*) denotes that a network service is disabled.\nWi-Fi\n*Thunderbolt Bridge\niPhone USB\n"
        ),
        v(&["Wi-Fi", "Thunderbolt Bridge", "iPhone USB"])
    );
}

#[test]
fn getdnsservers_none_set() {
    assert!(parse_dns_servers("There aren't any DNS Servers set on Wi-Fi.").is_empty());
}

#[test]
fn getdnsservers_one_per_line() {
    assert_eq!(
        parse_dns_servers("10.0.0.22\n10.0.0.4"),
        v(&["10.0.0.22", "10.0.0.4"])
    );
}

#[test]
fn getdnsservers_error_text_is_not_a_server() {
    assert!(parse_dns_servers("** Error: The parameters were not valid.").is_empty());
}

// --- detection -------------------------------------------------------------

#[test]
fn stale_services_holding_any_vpn_address_sorted() {
    let vpn = set(&["10.0.0.22", "10.0.0.4"]);
    let cur = map(&[
        ("Wi-Fi", &[]),
        ("iPhone USB", &["10.0.0.22", "10.0.0.4"]),
        ("AX88179A", &["10.0.0.4"]),
    ]);
    assert_eq!(stale_services(&cur, &vpn), v(&["AX88179A", "iPhone USB"]));
}

#[test]
fn stale_users_own_dns_is_never_stale() {
    let vpn = set(&["10.0.0.22", "10.0.0.4"]);
    let cur = map(&[("Wi-Fi", &["1.1.1.1"]), ("Bridge", &[])]);
    assert!(stale_services(&cur, &vpn).is_empty());
}

#[test]
fn stale_nothing_to_match_when_configs_have_no_dns() {
    let cur = map(&[("Wi-Fi", &["10.0.0.22"])]);
    assert!(stale_services(&cur, &set(&[])).is_empty());
}

// --- orchestration (fake networksetup) -------------------------------------

/// A fake `networksetup`: answers from its state, records every set call.
#[derive(Default)]
struct Fake {
    dns: BTreeMap<String, Vec<String>>,
    search: BTreeMap<String, Vec<String>>,
    calls: Mutex<Vec<Vec<String>>>,
    /// Make `-setdnsservers` fail with "** Error: nope".
    fail_set_dns: bool,
}

impl Fake {
    fn new(dns: &[(&str, &[&str])], search: &[(&str, &[&str])]) -> Arc<Self> {
        Arc::new(Fake {
            dns: map(dns),
            search: map(search),
            ..Default::default()
        })
    }

    fn failing(dns: &[(&str, &[&str])]) -> Arc<Self> {
        Arc::new(Fake {
            dns: map(dns),
            fail_set_dns: true,
            ..Default::default()
        })
    }

    fn runner(self: &Arc<Self>) -> Runner {
        let f = self.clone();
        Arc::new(move |exe, args| {
            assert_eq!(
                exe, "/usr/sbin/networksetup",
                "fake: only networksetup is invoked"
            );
            let list =
                |m: &BTreeMap<String, Vec<String>>| m.get(&args[1]).cloned().unwrap_or_default();
            match args[0].as_str() {
                "-listallnetworkservices" => {
                    let names: Vec<&str> = f.dns.keys().map(String::as_str).collect();
                    CmdResult::new(
                        0,
                        format!(
                            "An asterisk (*) denotes that a network service is disabled.\n{}",
                            names.join("\n")
                        ),
                    )
                }
                "-getdnsservers" => {
                    let l = list(&f.dns);
                    CmdResult::new(
                        0,
                        if l.is_empty() {
                            format!("There aren't any DNS Servers set on {}.", args[1])
                        } else {
                            l.join("\n")
                        },
                    )
                }
                "-getsearchdomains" => {
                    let l = list(&f.search);
                    CmdResult::new(
                        0,
                        if l.is_empty() {
                            format!("There aren't any Search Domains set on {}.", args[1])
                        } else {
                            l.join("\n")
                        },
                    )
                }
                "-setdnsservers" | "-setsearchdomains" => {
                    f.calls.lock().unwrap().push(args.to_vec());
                    if f.fail_set_dns && args[0] == "-setdnsservers" {
                        CmdResult::new(1, "** Error: nope")
                    } else {
                        CmdResult::new(0, "")
                    }
                }
                _ => CmdResult::new(1, format!("** Error: unexpected {args:?}")),
            }
        })
    }

    fn called(&self, args: &[&str]) -> bool {
        self.calls.lock().unwrap().contains(&v(args))
    }
}

const VPN2: &[&str] = &["10.0.0.22"];

#[test]
fn scan_reads_every_service() {
    let f = Fake::new(&[("Wi-Fi", &[]), ("iPhone USB", &["10.0.0.22"])], &[]);
    let g = DnsGuard::new(set(VPN2), f.runner());
    assert_eq!(
        g.current_dns(),
        map(&[("Wi-Fi", &[]), ("iPhone USB", &["10.0.0.22"])])
    );
    assert_eq!(g.stale(), v(&["iPhone USB"]));
}

#[test]
fn snapshot_records_users_settings_never_a_stale_vpn_entry() {
    let f = Fake::new(
        &[("Wi-Fi", &["1.1.1.1"]), ("iPhone USB", &["10.0.0.22"])],
        &[("Wi-Fi", &["home.lan"])],
    );
    let snap = DnsGuard::new(set(VPN2), f.runner()).snapshot();
    assert_eq!(
        snap["Wi-Fi"]["servers"],
        v(&["1.1.1.1"]),
        "snapshot: user DNS kept"
    );
    assert_eq!(
        snap["Wi-Fi"]["search"],
        v(&["home.lan"]),
        "snapshot: search domains kept"
    );
    assert_eq!(
        snap["iPhone USB"]["servers"],
        Vec::<String>::new(),
        "snapshot: already-stale service recorded as empty"
    );
}

#[test]
fn repair_restores_from_snapshot_or_clears_when_there_is_none() {
    let f = Fake::new(
        &[
            ("Wi-Fi", &["10.0.0.22"]),
            ("AX88179A", &["10.0.0.22"]),
            ("Bridge", &[]),
        ],
        &[],
    );
    let g = DnsGuard::new(set(VPN2), f.runner());
    let mut snap = DnsSnapshot::new();
    snap.insert(
        "Wi-Fi".into(),
        BTreeMap::from([
            ("servers".to_string(), v(&["1.1.1.1"])),
            ("search".to_string(), v(&["home.lan"])),
        ]),
    );
    let repaired = g.repair(&snap);
    assert_eq!(
        repaired.fixed,
        v(&["AX88179A", "Wi-Fi"]),
        "repair: returns the services it touched"
    );
    assert!(repaired.errors.is_empty(), "repair: no errors");
    assert!(
        f.called(&["-setdnsservers", "Wi-Fi", "1.1.1.1"]),
        "Wi-Fi restored from snapshot"
    );
    assert!(
        f.called(&["-setsearchdomains", "Wi-Fi", "home.lan"]),
        "Wi-Fi search restored"
    );
    assert!(
        f.called(&["-setdnsservers", "AX88179A", "Empty"]),
        "no snapshot -> Empty"
    );
    assert!(
        f.called(&["-setsearchdomains", "AX88179A", "Empty"]),
        "no snapshot -> search Empty"
    );
    assert!(
        !f.calls.lock().unwrap().iter().any(|c| c[1] == "Bridge"),
        "untouched services are left alone"
    );
}

#[test]
fn repair_reports_networksetup_errors_instead_of_hiding_them() {
    let f = Fake::failing(&[("Wi-Fi", &["10.0.0.22"])]);
    let r = DnsGuard::new(set(VPN2), f.runner()).repair(&DnsSnapshot::new());
    assert!(r.fixed.is_empty(), "nothing counted as fixed");
    assert_eq!(
        r.errors,
        v(&["Wi-Fi: ** Error: nope"]),
        "surfaced per service"
    );
}

// --- apply -----------------------------------------------------------------

#[test]
fn config_search_domains_are_the_non_ip_dns_entries() {
    assert_eq!(
        config_dns_search("DNS = 10.0.0.22, corp.example, 10.0.0.4\nDNS = office.lan\n"),
        v(&["corp.example", "office.lan"])
    );
}

#[test]
fn config_no_search_domains() {
    assert!(config_dns_search("DNS = 10.0.0.22\n").is_empty());
}

#[test]
fn apply_sets_servers_on_every_service() {
    let f = Fake::new(&[("Wi-Fi", &["1.1.1.1"]), ("Bridge", &[])], &[]);
    let g = DnsGuard::new(set(VPN2), f.runner());
    let errors = g.apply(&v(&["10.0.0.22", "10.0.0.4"]), &[]);
    assert!(errors.is_empty(), "apply: no errors");
    assert!(f.called(&["-setdnsservers", "Wi-Fi", "10.0.0.22", "10.0.0.4"]));
    assert!(f.called(&["-setdnsservers", "Bridge", "10.0.0.22", "10.0.0.4"]));
    assert!(
        f.called(&["-setsearchdomains", "Wi-Fi", "Empty"]),
        "no search domains -> Empty"
    );
}

#[test]
fn apply_search_domains_applied() {
    let f = Fake::new(&[("Wi-Fi", &[])], &[]);
    let g = DnsGuard::new(set(VPN2), f.runner());
    let _ = g.apply(&v(&["10.0.0.22"]), &v(&["corp.example"]));
    assert!(f.called(&["-setsearchdomains", "Wi-Fi", "corp.example"]));
}

#[test]
fn apply_error_surfaced_per_service() {
    let f = Fake::failing(&[("Wi-Fi", &[])]);
    let g = DnsGuard::new(set(VPN2), f.runner());
    assert_eq!(
        g.apply(&v(&["10.0.0.22"]), &[]),
        v(&["Wi-Fi: ** Error: nope"])
    );
}

// --- Swift-vs-Rust semantic traps (extra) ----------------------------------

#[test]
fn extra_parsing_edges() {
    assert!(
        is_ip_address("::1")
            && is_ip_address("10.0.0.1")
            && !is_ip_address("")
            && !is_ip_address("a.b")
    );
    // Swift split drops empty pieces: blank lines and leading '=' are harmless.
    assert_eq!(config_dns_servers("\n\nDNS=1.1.1.1\n\n"), v(&["1.1.1.1"]));
    assert_eq!(
        config_dns_servers("DNS = 1.1.1.1,,2.2.2.2"),
        v(&["1.1.1.1", "2.2.2.2"])
    );
    assert!(config_dns_servers("DNS =").is_empty());
    // spaces and tabs are trimmed.
    assert_eq!(config_dns_servers("DNS\t=\t1.1.1.1 \t"), v(&["1.1.1.1"]));
    assert_eq!(parse_search_domains("a.lan\nb.lan"), v(&["a.lan", "b.lan"]));
    assert!(parse_search_domains("There aren't any Search Domains set on Wi-Fi.").is_empty());
}
