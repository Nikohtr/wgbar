# WGBar benchmark — 2026-11-07 13:43

Mac: Mac15,6, Apple M3 Pro, macOS 27.0.1.
Swift: `main` (c2634d2), swift-driver version: 1.168.6 Apple Swift version 6.4.2 (swiftlang-6.4.2.1.9 clang-2100.4.1.1).
Rust: `rust-port` (0f0fd9b), rustc 1.99.1 (58cf8f9c3 2026-10-13) (Homebrew).
Idle window: 60s. Logic timings are ns per call (median of 7).

| metric | Swift | Rust |
|---|---:|---:|
| launch_ms_median | 61.4 | 61.4 |
| launch_ms_min | 41.6 | 61.1 |
| rss_kb_5s | 54016 | 50864 |
| rss_kb_60s | 54080 | 50928 |
| idle_cpu_ms_per_min | 170 | 140 |
| threads | 5 | 5 |
| open_fds | 30 | 29 |
| binary_bytes | 273216 | 502976 |
| bundle_kb | 276 | 500 |
| build_s | 7.1 | 18.5 |
| test_s | 1.8 | 31.2 |
| parse_lsof_400 | 830826 | 35764 |
| vpn_bound_400 | 124370 | 71422 |
| poll_tick_400 | 956668 | 107262 |
| parse_config | 32776 | 1905 |

Notes:

- Launch rows come from a second, launch-only run (same builds, 7 launches each after a warm-up)
  with the fixed `launch_probe`. In the full run the old probe timed out for both apps: since
  macOS 27, MenuBarAgent draws every app's status items, so no window is owned by the app. The
  probe now waits for `NSRunningApplication.isFinishedLaunching`, which both apps reach right
  after creating their NSStatusItem in `applicationDidFinishLaunching`.
- `build_s` and `test_s` for Rust start from `cargo clean` (full dependency build).
- Logic timings: Rust is 2–23× faster; RSS −3 MB, idle CPU −30 ms/min, binary +230 KB.
