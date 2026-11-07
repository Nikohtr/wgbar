//! wgbar-helper: the one privileged command WGBar's "Connect on Demand" mode needs.
//! Installed to /usr/local/libexec/wgbar-helper by sudoers.sh, which bakes the three paths below in
//! at build time and allows the current user to run this file (and nothing else) via sudo.
//! Design: docs/superpowers/specs/2026-09-02-on-demand-design.md
//!
//! Usage: wgbar-helper arm|connect|disconnect|down|status|print-armed <tunnel>
//!        wgbar-helper source-hash
//!   arm          bring the interface up from the "armed" config (no DNS, no Endpoint)
//!   connect      give the peer its Endpoint (and PersistentKeepalive) -> traffic flows
//!   disconnect   take the Endpoint away again; routes stay, packets are held
//!   down         wg-quick down (never touches DNS, since the armed config has none)
//!   status       print off | armed | connected
//!   print-armed  print the derived armed config (works without root; for tests)
//!   source-hash  print a hash of this file's source (install.sh uses it to detect a stale helper)
//!
//! Every argument is hostile: sudoers lets the user pass any arguments.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

const DEFAULT_CONF_DIR: &str = env!("WGBAR_BUILD_CONF_DIR");
const WG_QUICK: &str = env!("WGBAR_BUILD_WG_QUICK");
const WG: &str = env!("WGBAR_BUILD_WG");

const SOURCE: &str = include_str!("wgbar-helper.rs");

const fn fnv1a64(data: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut i = 0;
    while i < data.len() {
        hash ^= data[i] as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        i += 1;
    }
    hash
}

const SOURCE_HASH: u64 = fnv1a64(SOURCE.as_bytes());

const USAGE: &str = "usage: wgbar-helper arm|connect|disconnect|down|status|print-armed <tunnel>";

/// A failed run: exit code plus an optional message for stderr.
struct Fail(i32, Option<String>);

fn usage() -> Fail {
    Fail(2, Some(USAGE.to_string()))
}

fn die(msg: impl Into<String>) -> Fail {
    Fail(1, Some(msg.into()))
}

/// Removes the root-only temp dir holding the armed config when dropped.
struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Like mktemp -d /tmp/wgbar.XXXXXX, then chmod 700 (mkdtemp already creates it 0700).
fn make_temp_dir() -> Result<TempDir, Fail> {
    let mut template = b"/tmp/wgbar.XXXXXX".to_vec();
    template.push(0);
    // SAFETY: `template` is a writable, NUL-terminated buffer that outlives the call;
    // mkdtemp only rewrites the six X characters in place.
    let ptr = unsafe { libc::mkdtemp(template.as_mut_ptr().cast()) };
    if ptr.is_null() {
        return Err(die("mktemp: cannot create temp dir"));
    }
    template.pop();
    let dir = TempDir(PathBuf::from(OsString::from_vec(template)));
    fs::set_permissions(&dir.0, fs::Permissions::from_mode(0o700))
        .map_err(|e| die(format!("chmod: {e}")))?;
    Ok(dir)
}

fn is_root() -> bool {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() == 0 }
}

fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

fn trim(mut s: &[u8]) -> &[u8] {
    while let [first, rest @ ..] = s {
        if !is_space(*first) {
            break;
        }
        s = rest;
    }
    while let [rest @ .., last] = s {
        if !is_space(*last) {
            break;
        }
        s = rest;
    }
    s
}

/// Lines of a file; a missing final newline still yields the last line.
fn lines(data: &[u8]) -> impl Iterator<Item = &[u8]> {
    let mut parts: Vec<&[u8]> = data.split(|&b| b == b'\n').collect();
    if parts.last().is_some_and(|l| l.is_empty()) {
        parts.pop();
    }
    parts.into_iter()
}

fn key_of(line: &[u8]) -> Option<(Vec<u8>, &[u8])> {
    let eq = line.iter().position(|&b| b == b'=')?;
    Some((trim(&line[..eq]).to_ascii_lowercase(), &line[eq + 1..]))
}

/// The armed variant of the real config: every line except DNS, Endpoint and PersistentKeepalive.
fn armed_config(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for line in lines(data) {
        if let Some((key, _)) = key_of(line) {
            if matches!(
                key.as_slice(),
                b"dns" | b"endpoint" | b"persistentkeepalive"
            ) {
                continue;
            }
        }
        out.extend_from_slice(line);
        out.push(b'\n');
    }
    out
}

/// First value of key `want` (lowercase) in the real config; comments stripped.
fn field(data: &[u8], want: &str) -> Option<Vec<u8>> {
    for line in lines(data) {
        let line = line.split(|&b| b == b'#').next().unwrap_or(&[]);
        let Some((key, value)) = key_of(line) else {
            continue;
        };
        if key == want.as_bytes() {
            return Some(trim(value).to_vec());
        }
    }
    None
}

/// Exit code for a finished child, like a shell would report it.
fn code_of(status: ExitStatus) -> i32 {
    status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(0))
}

/// Child tools get a fixed PATH. Homebrew's dir must come FIRST: wg-quick's shebang is
/// `#!/usr/bin/env bash` and it aborts with "Version mismatch: bash 3 detected, when bash 4+
/// required" if env picks macOS's /bin/bash 3.2. This does not shadow the system tools wg-quick
/// uses: it re-exports a system-first PATH itself.
fn child_path() -> String {
    let dir = WG_QUICK.rfind('/').map_or(WG_QUICK, |i| &WG_QUICK[..i]);
    format!("{dir}:/usr/bin:/bin:/usr/sbin:/sbin")
}

fn command(program: &str) -> Command {
    let mut cmd = Command::new(program);
    cmd.env("PATH", child_path());
    cmd
}

/// Runs a tool with inherited stdio; a non-zero status becomes our exit status.
fn run_tool(mut cmd: Command) -> Result<(), Fail> {
    match cmd.status() {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(Fail(code_of(s), None)),
        Err(e) => Err(Fail(
            127,
            Some(format!("{}: {e}", cmd.get_program().to_string_lossy())),
        )),
    }
}

struct Ctx {
    tunnel: String,
    verb: String,
    conf: PathBuf,
    namefile: PathBuf,
}

impl Ctx {
    fn need_root(&self) -> Result<(), Fail> {
        if is_root() {
            Ok(())
        } else {
            Err(die(format!(
                "wgbar-helper {} must run as root (via sudo)",
                self.verb
            )))
        }
    }

    fn read_conf(&self) -> Result<Vec<u8>, Fail> {
        let mut data =
            fs::read(&self.conf).map_err(|e| die(format!("{}: {e}", self.conf.display())))?;
        data.retain(|&b| b != 0); // bash `read` silently drops NUL bytes; so must we
        Ok(data)
    }

    fn iface(&self) -> Result<String, Fail> {
        if !self.namefile.is_file() {
            return Err(die(format!("{} is not up", self.tunnel)));
        }
        read_iface(&self.namefile)
    }

    /// Writes the armed config (0600, in a root-only temp dir) and returns the guard and its path.
    fn make_armed(&self) -> Result<(TempDir, PathBuf), Fail> {
        let dir = make_temp_dir()?;
        let path = dir.0.join(format!("{}.conf", self.tunnel));
        let armed = armed_config(&self.read_conf()?);
        let mut f = open_private(&path)?;
        f.write_all(&armed)
            .map_err(|e| die(format!("{}: {e}", path.display())))?;
        Ok((dir, path))
    }
}

fn open_private(path: &Path) -> Result<File, Fail> {
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| die(format!("{}: {e}", path.display())))
}

/// Contents of the name file like `$(cat file)`: trailing newlines removed.
fn read_iface(namefile: &Path) -> Result<String, Fail> {
    let mut data = Vec::new();
    File::open(namefile)
        .and_then(|mut f| f.read_to_end(&mut data))
        .map_err(|e| die(format!("{}: {e}", namefile.display())))?;
    data.retain(|&b| b != 0); // bash `$(cat ...)` drops NUL bytes
    while data.last() == Some(&b'\n') {
        data.pop();
    }
    Ok(String::from_utf8_lossy(&data).into_owned())
}

fn valid_tunnel(t: &str) -> bool {
    // Same character set wg-quick accepts for interface names; in particular no "/" or "..".
    (1..=15).contains(&t.len())
        && t.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'=' | b'+' | b'.' | b'-'))
        && !t.contains("..")
}

fn run(args: Vec<OsString>) -> Result<(), Fail> {
    let verb = args
        .get(1)
        .map(|a| a.as_bytes().to_vec())
        .unwrap_or_default();
    let verb = String::from_utf8(verb).unwrap_or_default();
    if verb == "source-hash" {
        println!("{SOURCE_HASH:016x}");
        return Ok(());
    }
    if !matches!(
        verb.as_str(),
        "arm" | "connect" | "disconnect" | "down" | "status" | "print-armed"
    ) {
        return Err(usage());
    }
    let tunnel = args
        .get(2)
        .and_then(|a| a.to_str())
        .filter(|t| valid_tunnel(t))
        .ok_or_else(usage)?
        .to_string();

    // Root ignores every environment override.
    let conf_dir = match std::env::var_os("WGBAR_CONF_DIR") {
        Some(v) if !is_root() && !v.is_empty() => PathBuf::from(v),
        _ => PathBuf::from(DEFAULT_CONF_DIR),
    };
    let mut conf = conf_dir.into_os_string();
    conf.push(format!("/{tunnel}.conf"));
    let conf = PathBuf::from(conf);
    if !conf.is_file() {
        return Err(die(format!("no such config: {}", conf.display())));
    }
    let ctx = Ctx {
        namefile: PathBuf::from(format!("/var/run/wireguard/{tunnel}.name")),
        tunnel,
        verb,
        conf,
    };

    match ctx.verb.as_str() {
        "print-armed" => {
            let out = armed_config(&ctx.read_conf()?);
            std::io::stdout()
                .write_all(&out)
                .and_then(|_| std::io::stdout().flush())
                .map_err(|e| die(format!("write error: {e}")))
        }
        "status" => {
            if !ctx.namefile.is_file() {
                println!("off");
                return Ok(());
            }
            ctx.need_root()?;
            let wg_fail = || die(format!("wg show failed for {}", ctx.tunnel));
            let iface = read_iface(&ctx.namefile)?;
            let output = command(WG)
                .args(["show", &iface, "endpoints"])
                .stdin(Stdio::inherit())
                .stderr(Stdio::inherit())
                .output()
                .map_err(|_| wg_fail())?;
            if !output.status.success() {
                return Err(wg_fail());
            }
            // Connected if any peer's endpoint column is not "(none)".
            let connected = lines(&output.stdout).any(|l| {
                l.split(|&b| matches!(b, b' ' | b'\t' | b'\n'))
                    .filter(|f| !f.is_empty())
                    .nth(1)
                    .is_some_and(|f| f != b"(none)")
            });
            println!("{}", if connected { "connected" } else { "armed" });
            Ok(())
        }
        "arm" => {
            ctx.need_root()?;
            if ctx.namefile.is_file() {
                return Ok(());
            }
            let (_dir, armed) = ctx.make_armed()?;
            let mut cmd = command(WG_QUICK);
            cmd.arg("up").arg(&armed);
            run_tool(cmd)
        }
        "connect" => {
            ctx.need_root()?;
            let iface = ctx.iface()?;
            let data = ctx.read_conf()?;
            let missing = |what: &str| die(format!("no {what} in {}", ctx.conf.display()));
            let pk = field(&data, "publickey").ok_or_else(|| missing("PublicKey"))?;
            let ep = field(&data, "endpoint").ok_or_else(|| missing("Endpoint"))?;
            let ka = field(&data, "persistentkeepalive").unwrap_or_default();
            let mut cmd = command(WG);
            cmd.arg("set")
                .arg(&iface)
                .arg("peer")
                .arg(OsString::from_vec(pk))
                .arg("endpoint")
                .arg(OsString::from_vec(ep));
            if !ka.is_empty() {
                cmd.arg("persistent-keepalive").arg(OsString::from_vec(ka));
            }
            run_tool(cmd)
        }
        "disconnect" => {
            ctx.need_root()?;
            let iface = ctx.iface()?;
            let data = ctx.read_conf()?;
            let pk = field(&data, "publickey")
                .ok_or_else(|| die(format!("no PublicKey in {}", ctx.conf.display())))?;
            let (dir, armed) = ctx.make_armed()?;
            let stripped = dir.0.join("stripped.conf");
            let mut strip = command(WG_QUICK);
            strip
                .arg("strip")
                .arg(&armed)
                .stdout(Stdio::from(open_private(&stripped)?));
            run_tool(strip)?;
            let mut remove = command(WG);
            remove
                .arg("set")
                .arg(&iface)
                .arg("peer")
                .arg(OsString::from_vec(pk))
                .arg("remove");
            run_tool(remove)?;
            let mut addconf = command(WG);
            addconf.arg("addconf").arg(&iface).arg(&stripped);
            run_tool(addconf)
        }
        "down" => {
            ctx.need_root()?;
            if !ctx.namefile.is_file() {
                return Ok(());
            }
            let (_dir, armed) = ctx.make_armed()?;
            let mut cmd = command(WG_QUICK);
            cmd.arg("down").arg(&armed);
            run_tool(cmd)
        }
        _ => Err(usage()),
    }
}

fn main() -> std::process::ExitCode {
    // `run` owns the temp-dir guard, so cleanup happens before we return here
    // (std::process::exit would skip Drop).
    match run(std::env::args_os().collect()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(Fail(code, msg)) => {
            if let Some(m) = msg {
                eprintln!("{m}");
            }
            std::process::ExitCode::from(code.clamp(1, 255) as u8)
        }
    }
}
