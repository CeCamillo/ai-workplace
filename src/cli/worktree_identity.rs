//! Shared resolution of an Agent Worktree's identity — repository root,
//! branch, and base commit — from a path inside it. Both halves of the
//! Harness Adapter (`herdr annotate` writing, the Review Pane reading)
//! must derive these identically, or the annotation store keys drift
//! apart and Annotations silently disappear from review.

use std::path::{Path, PathBuf};
use std::process::Command;

pub(super) fn git_stdout(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .map_err(|err| format!("failed to run git: {err}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// The store-keying identity of one Agent Worktree.
pub(super) struct WorktreeIdentity {
    /// Canonicalized primary-repository root (macOS reports `/var`
    /// worktrees as `/private/var`; both sides must agree).
    pub repo_root: PathBuf,
    /// The Agent Worktree's branch.
    pub branch: String,
}

pub(super) fn resolve_worktree_identity(worktree: &Path) -> Result<WorktreeIdentity, String> {
    let branch = git_stdout(worktree, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    if branch == "HEAD" {
        return Err("the worktree is on a detached HEAD, not an Agent Worktree branch".to_string());
    }
    let git_common_dir = git_stdout(
        worktree,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let repo_root = Path::new(&git_common_dir)
        .parent()
        .ok_or_else(|| format!("cannot resolve repository root from {git_common_dir}"))?
        .to_path_buf();
    Ok(WorktreeIdentity {
        repo_root: repo_root.canonicalize().unwrap_or(repo_root),
        branch,
    })
}

/// The commit the worktree's uncommitted changeset sits on — the
/// commit-range key Annotations are stored under.
pub(super) fn head_commit(worktree: &Path) -> Result<String, String> {
    git_stdout(worktree, &["rev-parse", "HEAD"])
}
