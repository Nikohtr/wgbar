//! Resource probe for bench/measure.sh (macOS only).
//!
//! Usage: cpu_probe <pid>
//! Prints `cpu_ns <n>`, `wakeups <n>` and `footprint_kb <n>` for the process, from
//! `proc_pid_rusage(RUSAGE_INFO_V4)`: user+system CPU in nanoseconds (`ps` only resolves 10 ms),
//! idle + interrupt wakeups (what Activity Monitor's energy impact weighs most for an idle app),
//! and the physical footprint (Activity Monitor's "Memory" column; RSS also counts shared pages).
//! Two samples a known time apart give rates.

#[cfg(target_os = "macos")]
// libc points at the mach2 crate for mach_timebase_info; one call is not worth a dependency.
#[allow(deprecated)]
fn main() {
    let pid: libc::c_int = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .expect("usage: cpu_probe <pid>");
    // SAFETY: an all-zero rusage_info_v4 is a valid value (plain integers).
    let mut info: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
    // SAFETY: `info` is a valid, writable rusage_info_v4, the flavor that matches it.
    let rc = unsafe {
        libc::proc_pid_rusage(
            pid,
            libc::RUSAGE_INFO_V4,
            (&mut info as *mut libc::rusage_info_v4).cast(),
        )
    };
    if rc != 0 {
        eprintln!(
            "proc_pid_rusage({pid}): {}",
            std::io::Error::last_os_error()
        );
        std::process::exit(1);
    }
    // Mach absolute time units: 1 ns on Apple silicon, not on Intel; convert to be safe.
    let mut tb = libc::mach_timebase_info { numer: 0, denom: 0 };
    // SAFETY: `tb` is a valid out-parameter.
    unsafe { libc::mach_timebase_info(&mut tb) };
    let ns = |t: u64| t as u128 * tb.numer as u128 / tb.denom.max(1) as u128;
    println!("cpu_ns {}", ns(info.ri_user_time) + ns(info.ri_system_time));
    println!(
        "wakeups {}",
        info.ri_pkg_idle_wkups + info.ri_interrupt_wkups
    );
    println!("footprint_kb {}", info.ri_phys_footprint / 1024);
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("cpu_probe runs on macOS only");
    std::process::exit(1);
}
