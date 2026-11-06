//! Launch-latency probe for bench/measure.sh (macOS only).
//!
//! Usage: launch_probe <path to app executable>
//! Starts the executable, polls the window server every 2 ms until the process owns a window on
//! the status-bar layer (its menu bar icon), then prints `<pid> <milliseconds>` and exits, leaving
//! the app running so the caller can keep sampling it. Prints `<pid> timeout` after 15 s.

#[cfg(target_os = "macos")]
fn main() {
    use objc2::runtime::AnyObject;
    use objc2_core_graphics::{kCGNullWindowID, CGWindowListCopyWindowInfo, CGWindowListOption};
    use objc2_foundation::{ns_string, NSArray, NSDictionary, NSNumber, NSString};
    use std::time::{Duration, Instant};

    /// kCGStatusWindowLevel: the layer NSStatusItem windows live on.
    const STATUS_LAYER: i64 = 25;

    let exe = std::env::args()
        .nth(1)
        .expect("usage: launch_probe <executable>");
    let start = Instant::now();
    // Deliberately not waited on: the app must outlive the probe (measure.sh samples it next).
    #[allow(clippy::zombie_processes)]
    let child = std::process::Command::new(&exe).spawn().expect("spawn app");
    let pid = i64::from(child.id());

    let has_status_window = || -> bool {
        let Some(list) = CGWindowListCopyWindowInfo(CGWindowListOption::OptionAll, kCGNullWindowID)
        else {
            return false;
        };
        // SAFETY: CFArray of CFDictionary is toll-free bridged to NSArray of NSDictionary, and
        // `list` stays alive for the whole borrow.
        let windows: &NSArray<NSDictionary<NSString, AnyObject>> =
            unsafe { &*(&*list as *const objc2_core_foundation::CFArray).cast() };
        windows.iter().any(|w| {
            let num = |key: &NSString| {
                w.objectForKey(key)
                    .and_then(|v| v.downcast::<NSNumber>().ok())
                    .map(|n| n.as_i64())
            };
            num(ns_string!("kCGWindowOwnerPID")) == Some(pid)
                && num(ns_string!("kCGWindowLayer")) == Some(STATUS_LAYER)
        })
    };

    while start.elapsed() < Duration::from_secs(15) {
        if has_status_window() {
            println!("{pid} {:.1}", start.elapsed().as_secs_f64() * 1000.0);
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    println!("{pid} timeout");
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("launch_probe runs on macOS only");
    std::process::exit(1);
}
