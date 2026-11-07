//! Non-UI free functions of main.swift: where wg-quick and the configs live, tunnel state files,
//! and the privileged `wg-quick` invocation. Pure where possible so they are testable.

use crate::cmd::Runner;
use crate::dns_guard::{config_dns_servers, config_values};
use std::collections::BTreeSet;
use std::ffi::CString;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Folders where tunnel configs are looked for, in order.
pub const DEFAULT_CONF_DIRS: [&str; 4] = [
    "/opt/homebrew/etc/wireguard",
    "/usr/local/etc/wireguard",
    "/opt/local/etc/wireguard",
    "/etc/wireguard",
];
/// Where wg-quick may live: Homebrew (Apple silicon / Intel) or MacPorts.
pub const WG_QUICK_DIRS: [&str; 3] = ["/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin"];
pub const RUN_DIR: &str = "/var/run/wireguard";
pub const NETWORK_PREFS: &str = "/Library/Preferences/SystemConfiguration/preferences.plist";

/// Swift `FileManager.isExecutableFile`: a non-directory the process may execute.
pub(crate) fn is_executable_file(path: &str) -> bool {
    let Ok(c) = CString::new(path) else {
        return false;
    };
    // SAFETY: `c` is a valid NUL-terminated path.
    Path::new(path).is_file() && unsafe { libc::access(c.as_ptr(), libc::X_OK) } == 0
}

/// The wg-quick to use: the user's `wgQuick` default if executable
/// (`defaults write org.wgbar.WGBar wgQuick /path/to/wg-quick`), else the first executable in
/// [`WG_QUICK_DIRS`], else the Homebrew path.
pub fn resolve_wg_quick(custom: Option<&str>) -> String {
    if let Some(c) = custom.filter(|c| is_executable_file(c)) {
        return c.to_string();
    }
    WG_QUICK_DIRS
        .iter()
        .map(|d| format!("{d}/wg-quick"))
        .find(|p| is_executable_file(p))
        .unwrap_or_else(|| "/opt/homebrew/bin/wg-quick".to_string())
}

/// Parent directory of `wg_quick` (Foundation `deletingLastPathComponent`).
pub fn brew_bin(wg_quick: &str) -> String {
    match wg_quick.trim_end_matches('/').rfind('/') {
        None => String::new(),
        Some(0) => "/".to_string(),
        Some(i) => wg_quick[..i].to_string(),
    }
}

/// Names (without `.conf`) of the configs in `dir`, sorted.
pub fn conf_names(dir: &str) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter_map(|n| n.strip_suffix(".conf").map(String::from))
        .collect();
    names.sort();
    names
}

/// The config folder in use: the user's choice (menu "Config Folder…" or
/// `defaults write org.wgbar.WGBar confDir /path`), else the first default folder
/// that has configs, else the first that exists.
pub fn resolve_conf_dir(custom: Option<&str>) -> String {
    resolve_conf_dir_in(custom, &DEFAULT_CONF_DIRS)
}

/// [`resolve_conf_dir`] over an explicit candidate list (the first entry is the last resort).
pub fn resolve_conf_dir_in(custom: Option<&str>, dirs: &[&str]) -> String {
    if let Some(c) = custom.filter(|c| !c.is_empty()) {
        return c.to_string();
    }
    dirs.iter()
        .find(|d| !conf_names(d).is_empty())
        .or_else(|| dirs.iter().find(|d| Path::new(d).exists()))
        .or_else(|| dirs.first())
        .map(|d| d.to_string())
        .unwrap_or_default()
}

pub fn conf_path(conf_dir: &str, tunnel: &str) -> String {
    format!("{conf_dir}/{tunnel}.conf")
}

pub fn run_file(tunnel: &str) -> String {
    format!("{RUN_DIR}/{tunnel}.name")
}

/// The `Address =` value of a config, if any.
pub fn tunnel_address(config_text: &str) -> Option<String> {
    config_values(config_text, "address")
        .next()
        .map(String::from)
}

/// Every DNS server declared by any config in the folder (a stale entry may come from a
/// tunnel other than the selected one).
pub fn all_config_dns(conf_dir: &str) -> BTreeSet<String> {
    let mut all = BTreeSet::new();
    for name in conf_names(conf_dir) {
        if let Ok(text) = fs::read_to_string(conf_path(conf_dir, &name)) {
            all.extend(config_dns_servers(&text));
        }
    }
    all
}

fn name_files(dir: &str) -> Vec<String> {
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.ends_with(".name"))
        .collect()
}

/// True while any wg-quick tunnel is up (not only the selected one), when VPN DNS is legitimate.
pub fn any_tunnel_up() -> bool {
    any_tunnel_up_in(RUN_DIR)
}

pub fn any_tunnel_up_in(run_dir: &str) -> bool {
    !name_files(run_dir).is_empty()
}

/// Like [`any_tunnel_up`], but an on-demand interface that is merely armed (no endpoint, no VPN DNS)
/// does not count: its name file exists while DNS must still be the user's own.
pub fn any_tunnel_connected(ignoring_armed: Option<&str>) -> bool {
    any_tunnel_connected_in(RUN_DIR, ignoring_armed)
}

pub fn any_tunnel_connected_in(run_dir: &str, ignoring_armed: Option<&str>) -> bool {
    let armed = ignoring_armed.map(|t| format!("{t}.name"));
    name_files(run_dir)
        .iter()
        .any(|n| armed.as_deref() != Some(n.as_str()))
}

/// Modification time of the system network preferences; the Unix epoch when unreadable.
pub fn network_prefs_modified() -> SystemTime {
    fs::metadata(NETWORK_PREFS)
        .and_then(|m| m.modified())
        .unwrap_or(UNIX_EPOCH)
}

/// Bring the tunnel up or down (Swift `wgQuick(action, tunnel)`). Returns an error message,
/// or `None` on success/cancel.
pub fn wg_quick(
    run: &Runner,
    wg_quick: &str,
    brew_bin: &str,
    action: &str,
    conf_path: &str,
) -> Option<String> {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<String>>();
    // 1. Passwordless sudo, if a sudoers rule allows it (see sudoers.sh).
    let r = run("/usr/bin/sudo", &s(&["-n", wg_quick, action, conf_path]));
    if r.status == 0 {
        return None;
    }
    if !r.output.contains("password") {
        return Some(r.output); // a real wg-quick failure
    }

    // 2. Otherwise the standard macOS administrator dialog (supports Touch ID).
    let shell = format!("PATH={brew_bin}:$PATH '{wg_quick}' {action} '{conf_path}'");
    let script = format!("do shell script \"{shell}\" with administrator privileges");
    let r2 = run("/usr/bin/osascript", &s(&["-e", &script]));
    if r2.status == 0 {
        return None;
    }
    if r2.output.contains("-128") {
        return None; // user pressed Cancel
    }
    Some(r2.output)
}
