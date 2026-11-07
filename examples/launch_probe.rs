//! Launch-latency probe for bench/measure.sh (macOS only).
//!
//! Usage: launch_probe <path to app executable>
//! Starts the executable and polls every 2 ms until LaunchServices reports the process as finished
//! launching, then prints `<pid> <milliseconds>` and exits, leaving the app running so the caller
//! can keep sampling it. Prints `<pid> timeout` after 15 s.
//!
//! Why not "until its menu bar icon window exists": since macOS 27 the status items of all apps
//! are drawn by MenuBarAgent, so no window is owned by the app (the old probe timed out for both
//! Swift and Rust). Both apps create their NSStatusItem first thing in
//! applicationDidFinishLaunching, and `isFinishedLaunching` turns true once that returns, so the
//! two are measured at the same point. The Accessibility API would see the icon itself, but it
//! needs a permission grant for the terminal that runs the bench.

#[cfg(target_os = "macos")]
fn main() {
    use objc2_app_kit::NSRunningApplication;
    use std::time::{Duration, Instant};

    let exe = std::env::args()
        .nth(1)
        .expect("usage: launch_probe <executable>");
    let start = Instant::now();
    // Deliberately not waited on: the app must outlive the probe (measure.sh samples it next).
    #[allow(clippy::zombie_processes)]
    let child = std::process::Command::new(&exe).spawn().expect("spawn app");
    let pid = child.id() as libc::pid_t;

    let finished = || {
        NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
            .is_some_and(|app| app.isFinishedLaunching())
    };

    while start.elapsed() < Duration::from_secs(15) {
        if finished() {
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
