//! DNSGuard — detects and repairs DNS servers that wg-quick left behind on macOS.
//!
//! wg-quick applies a config's `DNS =` servers to every network service with `networksetup`,
//! and the only thing that restores the originals is a background monitor shell it leaves
//! running. If that shell dies without its exit trap (shutdown, crash, sleep races) the VPN
//! DNS stays in the system preferences — across reboots — with no tunnel to reach it.
//! On-demand mode (`on_demand.rs`) applies and restores DNS through this module instead of wg-quick.

use crate::cmd::{CmdResult, Runner};
use std::collections::{BTreeMap, BTreeSet};

/// Swift `trimmingCharacters(in: .whitespaces)`: spaces and tabs (not newlines).
pub(crate) fn trim_ws(s: &str) -> &str {
    s.trim_matches(|c: char| {
        c.is_whitespace()
            && !matches!(
                c,
                '\n' | '\r' | '\x0b' | '\x0c' | '\u{85}' | '\u{2028}' | '\u{2029}'
            )
    })
}

/// Swift `split(separator:, maxSplits: 1)` with empty pieces omitted: at most two pieces,
/// split at the first separator that follows a non-empty piece.
pub(crate) fn split_max1(s: &str, sep: char) -> Vec<&str> {
    let s = s.trim_start_matches(sep);
    if s.is_empty() {
        return vec![];
    }
    match s.find(sep) {
        None => vec![s],
        Some(i) => {
            let rest = &s[i + sep.len_utf8()..];
            if rest.is_empty() {
                vec![&s[..i]]
            } else {
                vec![&s[..i], rest]
            }
        }
    }
}

/// `key = value` lines of a config whose key equals `key` (case-insensitive); values trimmed.
pub(crate) fn config_values<'a>(text: &'a str, key: &'a str) -> impl Iterator<Item = &'a str> {
    text.split('\n').filter_map(move |line| {
        let parts = split_max1(line, '=');
        (parts.len() == 2 && trim_ws(parts[0]).to_lowercase() == key).then(|| trim_ws(parts[1]))
    })
}

fn dns_entries(text: &str) -> impl Iterator<Item = &str> {
    config_values(text, "dns")
        .flat_map(|v| v.split(','))
        .map(trim_ws)
}

/// The DNS server addresses declared by a wg-quick config (`DNS =` lines, comma-separated;
/// entries that are not IP addresses are search domains and are skipped, as wg-quick does).
pub fn config_dns_servers(text: &str) -> Vec<String> {
    dns_entries(text)
        .filter(|e| is_ip_address(e))
        .map(String::from)
        .collect()
}

/// The search domains declared by a wg-quick config: `DNS =` entries that are not IP addresses.
pub fn config_dns_search(text: &str) -> Vec<String> {
    dns_entries(text)
        .filter(|e| !e.is_empty() && !is_ip_address(e))
        .map(String::from)
        .collect()
}

/// IPv4 (digits and dots) or IPv6 (hex digits, colons, optional embedded IPv4) — nothing else.
pub fn is_ip_address(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    if s.contains(':') {
        return s
            .chars()
            .all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.');
    }
    s.chars().all(|c| c.is_ascii_digit() || c == '.')
}

/// Non-empty lines of `output`, trimmed (Swift `split("\n").map(trim).filter(!isEmpty)`).
fn trimmed_lines(output: &str) -> Vec<String> {
    output
        .split('\n')
        .map(trim_ws)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect()
}

/// Service names from `networksetup -listallnetworkservices` (header dropped, disabled marker stripped).
pub fn parse_service_list(output: &str) -> Vec<String> {
    output
        .split('\n')
        .filter(|l| !l.is_empty())
        .skip(1)
        .map(|line| {
            let s = trim_ws(line);
            s.strip_prefix('*').unwrap_or(s).to_string()
        })
        .filter(|s| !s.is_empty())
        .collect()
}

/// Server list from `networksetup -getdnsservers <service>`: one address per line, or a sentence
/// ("There aren't any DNS Servers set…" / "** Error…") meaning none.
pub fn parse_dns_servers(output: &str) -> Vec<String> {
    let lines = trimmed_lines(output);
    if lines.iter().all(|l| is_ip_address(l)) {
        lines
    } else {
        vec![]
    }
}

/// `networksetup -getsearchdomains` output: one domain per line, or a sentence meaning none.
pub fn parse_search_domains(output: &str) -> Vec<String> {
    let lines = trimmed_lines(output);
    if lines.iter().all(|l| !l.contains(' ')) {
        lines
    } else {
        vec![]
    }
}

/// Services whose current DNS servers include any VPN address while the tunnel is down.
/// A service running the user's own DNS (e.g. 1.1.1.1) is never reported.
pub fn stale_services(
    current: &BTreeMap<String, Vec<String>>,
    vpn_dns: &BTreeSet<String>,
) -> Vec<String> {
    current
        .iter()
        .filter(|(_, servers)| servers.iter().any(|s| vpn_dns.contains(s)))
        .map(|(k, _)| k.clone())
        .collect()
}

/// Per-service DNS settings as taken before connecting: `{"Wi-Fi": {"servers": [...], "search": [...]}}`.
/// A plain plist-compatible shape so it can be stored in UserDefaults as is.
pub type DnsSnapshot = BTreeMap<String, BTreeMap<String, Vec<String>>>;

/// Outcome of [`DnsGuard::repair`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RepairResult {
    pub fixed: Vec<String>,
    pub errors: Vec<String>,
}

const NETWORKSETUP: &str = "/usr/sbin/networksetup";

pub struct DnsGuard {
    /// Every DNS server address declared by the tunnel configs.
    pub vpn_dns: BTreeSet<String>,
    /// Runs an executable with arguments (injected so tests can fake `networksetup`).
    pub run: Runner,
}

impl DnsGuard {
    pub fn new(vpn_dns: BTreeSet<String>, run: Runner) -> Self {
        Self { vpn_dns, run }
    }

    fn ns(&self, args: &[&str]) -> CmdResult {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        (self.run)(NETWORKSETUP, &args)
    }

    fn services(&self) -> Vec<String> {
        parse_service_list(&self.ns(&["-listallnetworkservices"]).output)
    }

    fn holds_vpn(&self, servers: &[String]) -> bool {
        servers.iter().any(|s| self.vpn_dns.contains(s))
    }

    /// Current DNS servers of every network service.
    pub fn current_dns(&self) -> BTreeMap<String, Vec<String>> {
        self.services()
            .into_iter()
            .map(|s| {
                let servers = parse_dns_servers(&self.ns(&["-getdnsservers", &s]).output);
                (s, servers)
            })
            .collect()
    }

    /// Services still pointing at a VPN DNS server.
    pub fn stale(&self) -> Vec<String> {
        stale_services(&self.current_dns(), &self.vpn_dns)
    }

    /// Record what to restore after the tunnel comes down. A service already holding a VPN
    /// address (left behind earlier) is recorded as empty so the repair never re-applies it.
    pub fn snapshot(&self) -> DnsSnapshot {
        let mut snap = DnsSnapshot::new();
        for s in self.services() {
            let servers = parse_dns_servers(&self.ns(&["-getdnsservers", &s]).output);
            let search = parse_search_domains(&self.ns(&["-getsearchdomains", &s]).output);
            let is_stale = self.holds_vpn(&servers);
            let entry = BTreeMap::from([
                (
                    "servers".to_string(),
                    if is_stale { vec![] } else { servers },
                ),
                ("search".to_string(), if is_stale { vec![] } else { search }),
            ]);
            snap.insert(s, entry);
        }
        snap
    }

    /// Put every stale service back to its snapshot (or clear it, which means DHCP-provided DNS).
    pub fn repair(&self, snapshot: &DnsSnapshot) -> RepairResult {
        let mut result = RepairResult::default();
        for s in self.stale() {
            let get = |key: &str| {
                snapshot
                    .get(&s)
                    .and_then(|m| m.get(key))
                    .cloned()
                    .unwrap_or_default()
            };
            match self.set_dns(&s, &get("servers"), &get("search")) {
                Some(error) => result.errors.push(error),
                None => result.fixed.push(s),
            }
        }
        result
    }

    /// Point every network service at the VPN DNS servers, exactly as wg-quick does on `up`.
    /// Used by on-demand connect, whose armed config carries no DNS line. Returns per-service errors.
    pub fn apply(&self, servers: &[String], search: &[String]) -> Vec<String> {
        self.services()
            .iter()
            .filter_map(|s| self.set_dns(s, servers, search))
            .collect()
    }

    /// Sets one service's DNS servers and search domains; returns an error string or `None`.
    fn set_dns(&self, service: &str, servers: &[String], search: &[String]) -> Option<String> {
        let with_list = |verb: &str, list: &[String]| {
            let mut args = vec![verb.to_string(), service.to_string()];
            if list.is_empty() {
                args.push("Empty".into());
            } else {
                args.extend(list.iter().cloned());
            }
            (self.run)(NETWORKSETUP, &args)
        };
        let r1 = with_list("-setdnsservers", servers);
        let r2 = with_list("-setsearchdomains", search);
        let failed: Vec<String> = [r1, r2]
            .into_iter()
            .filter(|r| r.status != 0 || r.output.contains("Error"))
            .map(|r| r.output)
            .collect();
        (!failed.is_empty()).then(|| format!("{service}: {}", failed.join("; ")))
    }
}
