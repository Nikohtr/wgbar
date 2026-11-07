//! WGBar core: everything that does not touch AppKit. Platform-independent and unit-tested
//! on any OS (`cargo test`). The macOS menu bar app lives in `src/app` (see `src/main.rs`).

pub mod cmd;
pub mod dns_guard;
pub mod on_demand;
#[cfg(target_os = "macos")]
pub mod proc_sockets;
pub mod tunnels;
pub mod updater;
