//! The real [`GitPort`]: shells out to `git`, the same strategy herdr's own
//! worktree handling uses. Covered by a thin integration test against a
//! temporary repo; all engine behavior is tested through [`crate::fakes`].

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::diff::{parse_unified_diff, Changeset};
use crate::ids::{AgentWorktreeId, AuthoringSessionId};
use crate::ports::GitPort;
use crate::worktree::AgentWorktree;

/// Product-managed Agent Worktrees on a real repository (ADR-0004).
/// Worktrees are created under `worktrees_root`, on branches named
/// `agent/<session>`.
#[derive(Debug)]
pub struct SystemGitPort {
    repo_root: PathBuf,
    worktrees_root: PathBuf,
}

impl SystemGitPort {
    pub fn new(repo_root: PathBuf, worktrees_root: PathBuf) -> Self {
        Self {
            repo_root,
            worktrees_root,
        }
    }

    fn git(&self, cwd: &Path, args: &[&str]) -> Result<String, String> {
        let output = Command::new("git")
            .current_dir(cwd)
            .args(args)
            .output()
            .map_err(|err| format!("failed to run git: {err}"))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            Err(format!(
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }

    /// The worktree already checked out on `branch`, if any.
    fn registered_worktree_for_branch(&self, branch: &str) -> Result<Option<PathBuf>, String> {
        let listing = self.git(&self.repo_root, &["worktree", "list", "--porcelain"])?;
        let mut current_path: Option<PathBuf> = None;
        for line in listing.lines() {
            if let Some(path) = line.strip_prefix("worktree ") {
                current_path = Some(PathBuf::from(path));
            } else if line.strip_prefix("branch refs/heads/") == Some(branch) {
                return Ok(current_path);
            }
        }
        Ok(None)
    }

    fn branch_exists(&self, branch: &str) -> bool {
        self.git(
            &self.repo_root,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ],
        )
        .is_ok()
    }
}

/// The canonical form git itself reports in `worktree list` (on macOS,
/// `/var/...` worktrees come back as `/private/var/...`), so a worktree's
/// bound path always matches what listing-based adoption would return.
fn canonical_or_original(path: PathBuf) -> PathBuf {
    path.canonicalize().unwrap_or(path)
}

fn path_to_str(path: &Path) -> Result<&str, String> {
    path.to_str()
        .ok_or_else(|| format!("worktree path is not valid UTF-8: {}", path.display()))
}

/// A branch/session name reduced to a safe directory name, mirroring
/// herdr's own branch-to-path slugging.
fn path_slug(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

impl GitPort for SystemGitPort {
    fn create_worktree(&mut self, session: &AuthoringSessionId) -> Result<AgentWorktree, String> {
        let branch = format!("agent/{}", path_slug(&session.0));
        let path = self.worktrees_root.join(path_slug(&branch));
        self.git(
            &self.repo_root,
            &["worktree", "add", "-b", &branch, path_to_str(&path)?],
        )?;
        Ok(AgentWorktree {
            id: AgentWorktreeId(branch.clone()),
            branch,
            path: canonical_or_original(path),
        })
    }

    fn adopt_worktree(&mut self, branch: &str) -> Result<AgentWorktree, String> {
        let path = match self.registered_worktree_for_branch(branch)? {
            Some(path) => {
                // The primary checkout also appears in `git worktree list`;
                // it is the user's working tree, never an Agent Worktree,
                // and adopting it would let discard hard-reset it.
                if canonical_or_original(path.clone())
                    == canonical_or_original(self.repo_root.clone())
                {
                    return Err(format!(
                        "branch '{branch}' is checked out in the primary working tree, \
                         which cannot be an Agent Worktree"
                    ));
                }
                path
            }
            None => {
                if !self.branch_exists(branch) {
                    return Err(format!("branch '{branch}' does not exist"));
                }
                let path = self.worktrees_root.join(path_slug(branch));
                self.git(
                    &self.repo_root,
                    &["worktree", "add", path_to_str(&path)?, branch],
                )?;
                path
            }
        };
        Ok(AgentWorktree {
            id: AgentWorktreeId(branch.to_string()),
            branch: branch.to_string(),
            path: canonical_or_original(path),
        })
    }

    fn discard_worktree(&mut self, worktree: &AgentWorktree) -> Result<(), String> {
        self.git(&worktree.path, &["reset", "--hard"])?;
        self.git(&worktree.path, &["clean", "-fd"])?;
        Ok(())
    }

    fn changeset_diff(&mut self, worktree: &AgentWorktree) -> Result<Changeset, String> {
        // Intent-to-add makes files the agent created visible to `diff HEAD`
        // without staging their content; discard's `reset --hard` undoes it.
        self.git(&worktree.path, &["add", "--intent-to-add", "--all"])?;
        let text = self.git(&worktree.path, &["diff", "HEAD"])?;
        Ok(parse_unified_diff(&text))
    }

    fn changeset_base(&mut self, worktree: &AgentWorktree) -> Result<String, String> {
        Ok(self
            .git(&worktree.path, &["rev-parse", "HEAD"])?
            .trim()
            .to_string())
    }
}
