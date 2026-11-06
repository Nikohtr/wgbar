//! Port of the updater section of tests/DNSGuardTests.swift (fake git).

use std::sync::Arc;
use wgbar::cmd::{CmdResult, Runner};
use wgbar::updater::*;

fn fake_git(fetch_status: i32, fetch_out: &str, behind: &str, log: &str, head: &str) -> Runner {
    let (fetch_out, behind, log, head) = (
        fetch_out.to_string(),
        behind.to_string(),
        log.to_string(),
        head.to_string(),
    );
    Arc::new(move |exe, args| {
        assert_eq!(exe, "/usr/bin/git", "fake git: only git is invoked");
        assert_eq!(&args[..2], ["-C", "/repo"], "fake git: runs in the repo");
        match args.get(2).map(String::as_str) {
            Some("fetch") => CmdResult::new(fetch_status, fetch_out.clone()),
            Some("rev-list") => CmdResult::new(0, behind.clone()),
            Some("log") => CmdResult::new(0, log.clone()),
            Some("rev-parse") => CmdResult::new(0, head.clone()),
            _ => CmdResult::new(1, format!("** unexpected {args:?}")),
        }
    })
}

fn default_git() -> Runner {
    fake_git(0, "", "0", "", "7ecd33d")
}

#[test]
fn update_nothing_new() {
    assert_eq!(
        Updater::new("/repo".into(), default_git()).check(),
        UpdateStatus::UpToDate {
            revision: "7ecd33d".into()
        }
    );
}

#[test]
fn update_lists_new_commits() {
    let run = fake_git(
        0,
        "",
        "2",
        "abc1234 Fix thing\ndef5678 Add other",
        "7ecd33d",
    );
    assert_eq!(
        Updater::new("/repo".into(), run).check(),
        UpdateStatus::Available {
            commits: vec!["abc1234 Fix thing".into(), "def5678 Add other".into()]
        }
    );
}

#[test]
fn update_fetch_failure_surfaced() {
    let run = fake_git(128, "fatal: unable to access", "0", "", "7ecd33d");
    assert_eq!(
        Updater::new("/repo".into(), run).check(),
        UpdateStatus::Failed("fatal: unable to access".into())
    );
}

#[test]
fn update_no_repo_recorded() {
    assert_eq!(
        Updater::new(String::new(), default_git()).check(),
        UpdateStatus::Failed(
            "WGBar does not know where its source checkout is. Run ./install.sh from the wgbar folder once.".into()
        )
    );
}

#[test]
fn pull_returns_error_text_or_none() {
    let ok: Runner = Arc::new(|_, args| {
        assert_eq!(&args[2..], ["pull", "--ff-only", "--quiet"]);
        CmdResult::new(0, "")
    });
    assert_eq!(Updater::new("/repo".into(), ok).pull(), None);
    let bad: Runner = Arc::new(|_, _| CmdResult::new(1, "fatal: not ff"));
    assert_eq!(
        Updater::new("/repo".into(), bad).pull(),
        Some("fatal: not ff".into())
    );
}
