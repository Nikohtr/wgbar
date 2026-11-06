// WGBar — a tiny macOS menu bar toggle for a wg-quick (Homebrew wireguard-tools) tunnel.
// Left-click the icon: toggle the tunnel. Right-click: menu.

#[cfg(target_os = "macos")]
mod app;

#[cfg(target_os = "macos")]
fn main() {
    app::run();
}

/// The core library builds and tests on any OS, but the app itself is AppKit only.
#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("WGBar runs on macOS only");
    std::process::exit(1);
}
