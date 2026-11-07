//! Running external commands (Swift `run` and `CmdResult`).

use std::io::Read;
use std::os::unix::process::ExitStatusExt;
use std::process::Command;
use std::sync::Arc;

/// Exit status and merged, trimmed output of a finished command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CmdResult {
    pub status: i32,
    pub output: String,
}

impl CmdResult {
    pub fn new(status: i32, output: impl Into<String>) -> Self {
        Self {
            status,
            output: output.into(),
        }
    }
}

/// Runs an executable with args, stdout+stderr merged, output trimmed of whitespace/newlines.
pub type Runner = Arc<dyn Fn(&str, &[String]) -> CmdResult + Send + Sync>;

/// Run a command synchronously with Homebrew on PATH, capturing stdout+stderr.
///
/// `PATH` is `"<brew_bin>:"` plus the inherited `PATH` (default `/usr/bin:/bin:/usr/sbin:/sbin`)
/// and `GIT_TERMINAL_PROMPT=0` so git never hangs waiting for credentials. A spawn failure gives
/// status -1 and the error text as output.
pub fn run_with_path(brew_bin: &str, exe: &str, args: &[String]) -> CmdResult {
    let path = std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin:/usr/sbin:/sbin".into());
    let fail = |e: std::io::Error| CmdResult::new(-1, e.to_string());

    // One pipe for both streams, read to EOF before waiting, so large output cannot deadlock.
    let (mut reader, writer) = match std::io::pipe() {
        Ok(p) => p,
        Err(e) => return fail(e),
    };
    let writer_err = match writer.try_clone() {
        Ok(w) => w,
        Err(e) => return fail(e),
    };
    let mut cmd = Command::new(exe);
    cmd.args(args)
        .env("PATH", format!("{brew_bin}:{path}"))
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdout(writer)
        .stderr(writer_err);
    let spawned = cmd.spawn();
    // Drop the Command so the parent's copies of the write end close; otherwise EOF never comes.
    drop(cmd);
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => return fail(e),
    };
    let mut data = Vec::new();
    let _ = reader.read_to_end(&mut data);
    let status = match child.wait() {
        // Like Foundation's terminationStatus: the signal number when killed by a signal.
        Ok(s) => s.code().or_else(|| s.signal()).unwrap_or(-1),
        Err(e) => return fail(e),
    };
    CmdResult::new(status, String::from_utf8_lossy(&data).trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn merges_streams_and_trims() {
        let r = run_with_path(
            "/x",
            "/bin/sh",
            &args(&["-c", "echo out; echo err >&2; exit 3"]),
        );
        assert_eq!(r.status, 3);
        assert_eq!(r.output, "out\nerr");
    }

    #[test]
    fn path_and_git_prompt_env() {
        let r = run_with_path(
            "/brew/bin",
            "/bin/sh",
            &args(&["-c", "echo $PATH; echo $GIT_TERMINAL_PROMPT"]),
        );
        let mut lines = r.output.lines();
        assert!(lines.next().unwrap().starts_with("/brew/bin:"));
        assert_eq!(lines.next(), Some("0"));
    }

    #[test]
    fn large_output_does_not_deadlock() {
        let r = run_with_path(
            "/x",
            "/bin/sh",
            &args(&["-c", "head -c 1000000 /dev/zero | tr '\\0' a"]),
        );
        assert_eq!(r.status, 0);
        assert_eq!(r.output.len(), 1_000_000);
    }

    #[test]
    fn spawn_failure_is_minus_one() {
        let r = run_with_path("/x", "/nonexistent/exe", &[]);
        assert_eq!(r.status, -1);
        assert!(!r.output.is_empty());
    }

    #[test]
    fn invalid_utf8_is_lossy() {
        let r = run_with_path("/x", "/bin/sh", &args(&["-c", "printf 'a\\377b'"]));
        assert_eq!(r.output, "a\u{FFFD}b");
    }
}
