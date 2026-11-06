// Bakes the privileged helper's paths into the binary at compile time.
// sudoers.sh sets these variables; the defaults match a Homebrew install on Apple silicon.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    for (name, default) in [
        ("WGBAR_BUILD_CONF_DIR", "/opt/homebrew/etc/wireguard"),
        ("WGBAR_BUILD_WG_QUICK", "/opt/homebrew/bin/wg-quick"),
        ("WGBAR_BUILD_WG", "/opt/homebrew/bin/wg"),
    ] {
        println!("cargo:rerun-if-env-changed={name}");
        let value = std::env::var(name).unwrap_or_else(|_| default.to_string());
        println!("cargo:rustc-env={name}={value}");
    }
}
