//! The macOS menu bar app: a port of `AppDelegate` and the top-level globals of main.swift.
//!
//! Threading model (mirrors the Swift code):
//! - The delegate and every AppKit object live on the main thread. Its mutable state is in
//!   `Cell`/`RefCell` ivars, so it is only reachable with a [`MainThreadMarker`].
//! - Background work runs on global dispatch queues or on the serial On-Demand queue and hops
//!   back with [`on_main`] / [`on_main_after`] (Swift `DispatchQueue.main.async[After]`).
//! - The delegate is installed once in [`DELEGATE`] and never dropped (Swift: the top-level
//!   `let delegate`), so those hops can reach it from `Send` closures without capturing it.

mod defaults;
mod delegate;
mod ui;

use std::collections::BTreeSet;
use std::sync::{Arc, LazyLock, OnceLock};

use dispatch2::{DispatchQueue, DispatchTime, MainThreadBound};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
use wgbar::cmd::{run_with_path, Runner};
use wgbar::tunnels;

use delegate::AppDelegate;

/// wg-quick and the command runner, resolved once at launch (Swift globals `wgQuick`, `brewBin`
/// and `run`).
struct Tools {
    wg_quick: String,
    brew_bin: String,
    run: Runner,
}

/// Where wg-quick may live: Homebrew (Apple silicon / Intel) or MacPorts.
/// Override with:  defaults write org.wgbar.WGBar wgQuick /path/to/wg-quick
static TOOLS: LazyLock<Tools> = LazyLock::new(|| {
    let wg_quick = tunnels::resolve_wg_quick(defaults::string(defaults::WG_QUICK).as_deref());
    let brew_bin = tunnels::brew_bin(&wg_quick);
    let path = brew_bin.clone();
    // Run a command synchronously with Homebrew on PATH, capturing stdout+stderr.
    let run: Runner = Arc::new(move |exe: &str, args: &[String]| run_with_path(&path, exe, args));
    Tools {
        wg_quick,
        brew_bin,
        run,
    }
});

fn runner() -> Runner {
    Arc::clone(&TOOLS.run)
}

fn brew_bin() -> &'static str {
    &TOOLS.brew_bin
}

/// The config folder in use (re-read every time: the user may change it from the menu).
fn conf_dir() -> String {
    tunnels::resolve_conf_dir(defaults::string(defaults::CONF_DIR).as_deref())
}

fn conf_path(tunnel: &str) -> String {
    tunnels::conf_path(&conf_dir(), tunnel)
}

fn available_tunnels() -> Vec<String> {
    tunnels::conf_names(&conf_dir())
}

/// The text of a tunnel's config, or `None` when it is missing or unreadable.
fn read_conf(tunnel: &str) -> Option<String> {
    std::fs::read_to_string(conf_path(tunnel)).ok()
}

/// Every DNS server declared by any config in the folder (a stale entry may come from a
/// tunnel other than the selected one).
fn all_config_dns() -> BTreeSet<String> {
    tunnels::all_config_dns(&conf_dir())
}

/// Bring the tunnel up or down. Returns an error message, or `None` on success/cancel.
/// Blocks (it may show the administrator dialog), so never call it on the main thread.
fn wg_quick(action: &str, tunnel: &str) -> Option<String> {
    let tools = &*TOOLS;
    tunnels::wg_quick(
        &tools.run,
        &tools.wg_quick,
        &tools.brew_bin,
        action,
        &conf_path(tunnel),
    )
}

/// The one app delegate. Set before the run loop starts and never dropped.
static DELEGATE: OnceLock<MainThreadBound<Retained<AppDelegate>>> = OnceLock::new();

/// Run `f` with the delegate. Only for code already on the main thread (main-queue blocks,
/// timers and observers on the main run loop). Off the main thread it does nothing: it must
/// never block on the main thread, which may itself be waiting on the On-Demand queue
/// (`applicationWillTerminate`).
fn with_delegate(f: impl FnOnce(&AppDelegate)) {
    let Some(mtm) = MainThreadMarker::new() else {
        debug_assert!(false, "with_delegate called off the main thread");
        return;
    };
    if let Some(delegate) = DELEGATE.get() {
        f(delegate.get(mtm));
    }
}

/// Swift `DispatchQueue.main.async { self.… }`.
fn on_main(f: impl FnOnce(&AppDelegate) + Send + 'static) {
    DispatchQueue::main().exec_async(move || with_delegate(f));
}

/// Swift `DispatchQueue.main.asyncAfter(deadline: .now() + seconds) { self.… }`.
fn on_main_after(seconds: i64, f: impl FnOnce(&AppDelegate) + Send + 'static) {
    const NSEC_PER_SEC: i64 = 1_000_000_000;
    let when = DispatchTime::NOW.time(seconds * NSEC_PER_SEC);
    // `after` never fails; the `Result` only exists for API symmetry.
    let _ = DispatchQueue::main().after(when, move || with_delegate(f));
}

/// The app entry point (Swift top-level code): an accessory app (no Dock icon) whose only UI
/// is the status item.
pub fn run() {
    let Some(mtm) = MainThreadMarker::new() else {
        eprintln!("WGBar must be started on the main thread");
        std::process::exit(1);
    };
    // Resolve wg-quick at launch, as the Swift globals were.
    LazyLock::force(&TOOLS);

    let app = NSApplication::sharedApplication(mtm);
    let delegate = AppDelegate::new(mtm);
    // `NSApplication.delegate` is weak; `DELEGATE` keeps the delegate alive for the process.
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    if DELEGATE.set(MainThreadBound::new(delegate, mtm)).is_err() {
        unreachable!("run() is called once");
    }
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    app.run();
}
