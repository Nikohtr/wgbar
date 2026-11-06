//! "Check for Updates…" for an app installed from a git clone (see install.sh).
//! The clone's path is recorded by install.sh in defaults (`repoDir`). Checking fetches
//! origin and compares; applying pulls and re-runs install.sh, which kills and relaunches WGBar.

use crate::cmd::{CmdResult, Runner};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateStatus {
    UpToDate { revision: String },
    Available { commits: Vec<String> },
    Failed(String),
}

pub struct Updater {
    /// The git clone WGBar was installed from (empty when unknown).
    pub repo_dir: String,
    /// Runs an executable with arguments (injected so tests can fake git).
    pub run: Runner,
}

impl Updater {
    pub const NO_REPO_MESSAGE: &'static str = "WGBar does not know where its source checkout is. Run ./install.sh from the wgbar folder once.";

    pub fn new(repo_dir: String, run: Runner) -> Self {
        Self { repo_dir, run }
    }

    fn git(&self, args: &[&str]) -> CmdResult {
        let mut full: Vec<String> = vec!["-C".into(), self.repo_dir.clone()];
        full.extend(args.iter().map(|s| s.to_string()));
        (self.run)("/usr/bin/git", &full)
    }

    /// Fetch origin and report whether the upstream branch has commits we lack.
    pub fn check(&self) -> UpdateStatus {
        if self.repo_dir.is_empty() {
            return UpdateStatus::Failed(Self::NO_REPO_MESSAGE.into());
        }
        let fetch = self.git(&["fetch", "--quiet", "origin"]);
        if fetch.status != 0 {
            return UpdateStatus::Failed(fetch.output);
        }
        let count = self.git(&["rev-list", "--count", "HEAD..@{u}"]);
        if count.status != 0 {
            return UpdateStatus::Failed(count.output);
        }
        if count.output.parse::<i64>().unwrap_or(0) == 0 {
            return UpdateStatus::UpToDate {
                revision: self.git(&["rev-parse", "--short", "HEAD"]).output,
            };
        }
        let log = self.git(&["log", "--oneline", "HEAD..@{u}"]).output;
        UpdateStatus::Available {
            commits: log
                .split('\n')
                .filter(|l| !l.is_empty())
                .map(String::from)
                .collect(),
        }
    }

    /// Fast-forward the clone. Returns an error message, or `None` on success.
    pub fn pull(&self) -> Option<String> {
        let r = self.git(&["pull", "--ff-only", "--quiet"]);
        (r.status != 0).then_some(r.output)
    }
}
