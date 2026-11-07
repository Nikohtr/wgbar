# WGBar benchmark — 2026-11-07 14:43

Mac: Mac15,6, Apple M3 Pro, macOS 27.0.1.
Swift: `main` (c2634d2), swift-driver version: 1.168.6 Apple Swift version 6.4.2 (swiftlang-6.4.2.1.9 clang-2100.4.1.1).
Rust: `rust-port` (af08855), rustc 1.99.1 (58cf8f9c3 2026-10-13) (Homebrew).
Idle window: 60s. Logic timings are ns per call (median of 7).
sbs_* rows: both apps idle at the same time (bench/idle.sh), median of 3 windows.

| metric | Swift | Rust |
|---|---:|---:|
| launch_ms_median | 64.3 | 63.5 |
| launch_ms_min | 61.1 | 56.4 |
| rss_kb_5s | 54160 | 54256 |
| rss_kb_60s | 54480 | 54128 |
| footprint_kb_5s | 13184 | 15664 |
| footprint_kb_60s | 13472 | 15536 |
| idle_cpu_ms_per_min | 304.1 | 3.5 |
| wakeups_per_min | 182 | 6 |
| sbs_idle_cpu_ms_per_min | 291.8 | 2.2 |
| sbs_wakeups_per_min | 200 | 2 |
| sbs_footprint_kb | 16256 | 15584 |
| threads | 5 | 3 |
| open_fds | 30 | 32 |
| binary_bytes | 273216 | 503840 |
| bundle_kb | 276 | 504 |
| build_s | 3.6 | 7.0 |
| test_s | 1.9 | 7.1 |
| parse_lsof_400 | 850076 | 35609 |
| vpn_bound_400 | 126868 | 63783 |
| poll_tick_400 | 976805 | 100994 |
| parse_config | 33314 | 1898 |
| poll_live_ns | 100001151 | 640208 |
| poll_live_cpu_ns | 33777304 | 635464 |
| poll_lsof_ns | — | 22345479 |
| poll_lsof_cpu_ns | — | 21776714 |

Notes (after the optimization pass, commits 273b835..af08855):

- Idle: the Rust app now wakes only for file-system events and a 30 s safety timer (it used to
  run a 2 s refresh and a 1 s On-Demand tick). The `sbs_*` rows (both apps idle at the same
  time) are the fair comparison: ~2 ms/min and 2 wakeups/min against Swift's ~290 ms/min and
  ~200/min. Swift's own idle cost varies with what else the Mac does (150–300 ms/min today).
- On-Demand armed (not run here: arming would fight the installed app over the tunnel): one
  poll costs Swift ~100 ms wall / ~34 ms CPU (Process + lsof + parse), Rust ~0.6 ms of both
  (`poll_live_*`, in-process libproc). At one poll a second that is ~2 s CPU/min against ~40 ms.
  `poll_lsof_*` is Rust's fallback path (lsof via `run_with_path`).
- Footprint moves by ±3 MB with system state between runs (both apps alike); in the same window
  Rust stays below Swift (`sbs_footprint_kb`). Sequential `footprint_kb_*` rows are less reliable.
- Launch: equal within noise. ~60 of ~65 ms is AppKit (`sharedApplication` ~34 ms, status item
  ~14 ms, run loop start ~9 ms); WGBar's own launch work is ~8 ms in either language.
- Binary: +230 KB is Rust's statically linked std, mostly the panic backtrace printer (gimli,
  addr2line); removing it needs nightly `-Zbuild-std`. Swift's binary leans on the OS's Swift
  runtime instead. Those pages are never touched at run time (footprint is not higher).
- Build/test from clean: 7 s each (from 18.5 s / 31 s) after trimming objc2 features; Swift's
  single swiftc call is still faster to build (3.6 s).
