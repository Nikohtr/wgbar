//! Tests for src/tunnels.rs (no Swift twin): wg-quick fallback logic, config discovery.

use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use wgbar::cmd::{CmdResult, Runner};
use wgbar::tunnels::*;

/// A fresh empty temp dir, removed on drop.
struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("wgbar-test-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        Tmp(p)
    }
    fn s(&self) -> &str {
        self.0.to_str().unwrap()
    }
    fn touch(&self, name: &str, text: &str) {
        fs::write(self.0.join(name), text).unwrap();
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

type Calls = Arc<Mutex<Vec<(String, Vec<String>)>>>;

/// Scripted runner: answers per executable from `replies`, records calls.
fn scripted(replies: Vec<(&'static str, CmdResult)>) -> (Runner, Calls) {
    let calls: Calls = Arc::new(Mutex::new(vec![]));
    let c = calls.clone();
    let run: Runner = Arc::new(move |exe, args| {
        c.lock().unwrap().push((exe.to_string(), args.to_vec()));
        replies
            .iter()
            .find(|(e, _)| *e == exe)
            .map(|(_, r)| r.clone())
            .unwrap_or_else(|| CmdResult::new(1, "unexpected"))
    });
    (run, calls)
}

#[test]
fn wg_quick_sudo_success() {
    let (run, calls) = scripted(vec![("/usr/bin/sudo", CmdResult::new(0, ""))]);
    assert_eq!(
        wg_quick(&run, "/o/bin/wg-quick", "/o/bin", "up", "/c/x.conf"),
        None
    );
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].1, ["-n", "/o/bin/wg-quick", "up", "/c/x.conf"]);
}

#[test]
fn wg_quick_real_failure_is_returned_without_dialog() {
    let (run, calls) = scripted(vec![(
        "/usr/bin/sudo",
        CmdResult::new(1, "wg-quick: bad config"),
    )]);
    assert_eq!(
        wg_quick(&run, "/o/bin/wg-quick", "/o/bin", "up", "/c/x.conf"),
        Some("wg-quick: bad config".into())
    );
    assert_eq!(calls.lock().unwrap().len(), 1, "osascript not tried");
}

#[test]
fn wg_quick_password_falls_back_to_osascript() {
    let (run, calls) = scripted(vec![
        (
            "/usr/bin/sudo",
            CmdResult::new(1, "sudo: a password is required"),
        ),
        ("/usr/bin/osascript", CmdResult::new(0, "")),
    ]);
    assert_eq!(
        wg_quick(&run, "/o/bin/wg-quick", "/o/bin", "down", "/c/x.conf"),
        None
    );
    let calls = calls.lock().unwrap();
    assert_eq!(calls[1].0, "/usr/bin/osascript");
    assert_eq!(
        calls[1].1,
        [
            "-e",
            "do shell script \"PATH=/o/bin:$PATH '/o/bin/wg-quick' down '/c/x.conf'\" with administrator privileges"
        ]
    );
}

#[test]
fn wg_quick_osascript_cancel_is_none() {
    let (run, _) = scripted(vec![
        ("/usr/bin/sudo", CmdResult::new(1, "password")),
        (
            "/usr/bin/osascript",
            CmdResult::new(1, "execution error: User canceled. (-128)"),
        ),
    ]);
    assert_eq!(wg_quick(&run, "w", "b", "up", "c"), None);
}

#[test]
fn wg_quick_osascript_failure_is_returned() {
    let (run, _) = scripted(vec![
        ("/usr/bin/sudo", CmdResult::new(1, "password")),
        (
            "/usr/bin/osascript",
            CmdResult::new(1, "execution error: nope (1)"),
        ),
    ]);
    assert_eq!(
        wg_quick(&run, "w", "b", "up", "c"),
        Some("execution error: nope (1)".into())
    );
}

#[test]
fn conf_names_sorted_and_filtered() {
    let t = Tmp::new("names");
    t.touch("b.conf", "");
    t.touch("a.conf", "");
    t.touch("notes.txt", "");
    t.touch("c.conf.bak", "");
    assert_eq!(conf_names(t.s()), ["a", "b"]);
    assert!(conf_names("/nonexistent/dir").is_empty());
}

#[test]
fn resolve_conf_dir_order() {
    let empty = Tmp::new("empty");
    let full = Tmp::new("full");
    full.touch("x.conf", "");
    let missing = "/nonexistent/wgbar";
    // custom wins; empty custom is ignored
    assert_eq!(resolve_conf_dir_in(Some("/my/dir"), &[full.s()]), "/my/dir");
    assert_eq!(resolve_conf_dir_in(Some(""), &[full.s()]), full.s());
    // first dir with configs
    assert_eq!(
        resolve_conf_dir_in(None, &[missing, empty.s(), full.s()]),
        full.s()
    );
    // else first existing
    assert_eq!(resolve_conf_dir_in(None, &[missing, empty.s()]), empty.s());
    // else first
    assert_eq!(
        resolve_conf_dir_in(None, &[missing, "/nonexistent/b"]),
        missing
    );
    assert_eq!(resolve_conf_dir(Some("/z")), "/z");
}

#[test]
fn any_tunnel_up_and_connected() {
    let t = Tmp::new("run");
    assert!(!any_tunnel_up_in(t.s()));
    assert!(!any_tunnel_connected_in(t.s(), None));
    assert!(!any_tunnel_up_in("/nonexistent/run"));
    t.touch("other.txt", "");
    assert!(!any_tunnel_up_in(t.s()), "only .name files count");
    t.touch("armed.name", "wg0");
    assert!(any_tunnel_up_in(t.s()));
    assert!(any_tunnel_connected_in(t.s(), None));
    assert!(
        !any_tunnel_connected_in(t.s(), Some("armed")),
        "armed tunnel ignored"
    );
    assert!(any_tunnel_connected_in(t.s(), Some("other")));
    t.touch("second.name", "wg1");
    assert!(any_tunnel_connected_in(t.s(), Some("armed")));
}

#[test]
fn all_config_dns_unions_every_config() {
    let t = Tmp::new("dns");
    t.touch("a.conf", "[Interface]\nDNS = 10.0.0.22, corp.example\n");
    t.touch("b.conf", "DNS = 10.0.0.4,10.0.0.22\n");
    t.touch("c.conf", "[Interface]\n");
    let got: Vec<String> = all_config_dns(t.s()).into_iter().collect();
    assert_eq!(got, ["10.0.0.22", "10.0.0.4"]);
}

#[test]
fn paths_and_names() {
    assert_eq!(
        conf_path("/etc/wireguard", "wg0"),
        "/etc/wireguard/wg0.conf"
    );
    assert_eq!(run_file("wg0"), "/var/run/wireguard/wg0.name");
    assert_eq!(brew_bin("/opt/homebrew/bin/wg-quick"), "/opt/homebrew/bin");
    assert_eq!(brew_bin("/wg-quick"), "/");
    assert_eq!(brew_bin("wg-quick"), "");
}

#[test]
fn tunnel_address_parsing() {
    assert_eq!(
        tunnel_address("[Interface]\nPrivateKey = x=\nAddress = 10.0.0.2/32\n"),
        Some("10.0.0.2/32".into())
    );
    assert_eq!(
        tunnel_address("ADDRESS=fd00::2/128, 10.0.0.2/32"),
        Some("fd00::2/128, 10.0.0.2/32".into())
    );
    assert_eq!(tunnel_address("[Interface]\n"), None);
}

#[test]
fn resolve_wg_quick_custom_and_fallback() {
    use std::os::unix::fs::PermissionsExt;
    let t = Tmp::new("wgq");
    t.touch("wg-quick", "#!/bin/sh\n");
    let p = t.0.join("wg-quick");
    assert!(
        resolve_wg_quick(Some(p.to_str().unwrap())) != p.to_str().unwrap(),
        "not executable yet"
    );
    fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        resolve_wg_quick(Some(p.to_str().unwrap())),
        p.to_str().unwrap()
    );
    assert!(resolve_wg_quick(None).ends_with("/wg-quick"));
}
