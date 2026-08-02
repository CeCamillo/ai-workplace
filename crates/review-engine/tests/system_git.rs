//! Thin integration test: the real [`GitPort`] against a temporary repo.
//! Only the port's contract is exercised here — all lifecycle behavior is
//! covered by engine tests through fakes.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use review_engine::git::SystemGitPort;
use review_engine::{AuthoringSessionId, DiffLineKind, GitPort};

static NEXT_TEMP_ID: AtomicU32 = AtomicU32::new(0);

fn unique_temp_dir(name: &str) -> PathBuf {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("review-engine-{name}-{}-{id}", std::process::id()))
}

fn run_git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .expect("failed to run git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_stdout(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .expect("failed to run git");
    assert!(output.status.success(), "git {args:?} failed");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// A committed repo plus a port managing worktrees next to it.
fn repo_and_port(name: &str) -> (PathBuf, SystemGitPort) {
    let repo = unique_temp_dir(name).join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    run_git(&repo, &["init", "--initial-branch=main"]);
    run_git(&repo, &["config", "user.email", "test@example.com"]);
    run_git(&repo, &["config", "user.name", "Test"]);
    std::fs::write(repo.join("README.md"), "hello\n").unwrap();
    run_git(&repo, &["add", "."]);
    run_git(&repo, &["commit", "-m", "initial"]);
    let worktrees_root = repo.parent().unwrap().join("worktrees");
    let port = SystemGitPort::new(repo.clone(), worktrees_root);
    (repo, port)
}

#[test]
fn creates_a_registered_worktree_on_a_dedicated_branch() {
    let (repo, mut port) = repo_and_port("create");
    let session = AuthoringSessionId::from("pane 1");

    let worktree = port.create_worktree(&session).unwrap();

    assert_eq!(worktree.branch, "agent/pane-1");
    assert!(worktree.path.join("README.md").exists());
    let listing = git_stdout(&repo, &["worktree", "list", "--porcelain"]);
    assert!(listing.contains("branch refs/heads/agent/pane-1"));

    std::fs::remove_dir_all(repo.parent().unwrap().parent().unwrap()).ok();
}

#[test]
fn adopts_an_existing_branch_and_an_already_registered_worktree() {
    let (repo, mut port) = repo_and_port("adopt");
    run_git(&repo, &["branch", "feature/resume-me"]);

    // Branch with no worktree yet: adoption checks one out.
    let adopted = port.adopt_worktree("feature/resume-me").unwrap();
    assert_eq!(adopted.branch, "feature/resume-me");
    assert!(adopted.path.join("README.md").exists());

    // Already-registered worktree: adoption rebinds, no second checkout.
    let readopted = port.adopt_worktree("feature/resume-me").unwrap();
    assert_eq!(readopted.path, adopted.path);

    assert!(port.adopt_worktree("no-such-branch").is_err());

    // The primary checkout is not an Agent Worktree: adopting the branch
    // checked out there must fail, or a later discard would hard-reset the
    // user's real working tree.
    assert!(port.adopt_worktree("main").is_err());

    std::fs::remove_dir_all(repo.parent().unwrap().parent().unwrap()).ok();
}

#[test]
fn changeset_diff_covers_modified_and_untracked_files_without_staging_them() {
    let (repo, mut port) = repo_and_port("diff");
    let session = AuthoringSessionId::from("pane-1");
    let worktree = port.create_worktree(&session).unwrap();

    std::fs::write(worktree.path.join("README.md"), "changed\n").unwrap();
    std::fs::write(worktree.path.join("new.rs"), "pub fn b() {}\n").unwrap();

    let changeset = port.changeset_diff(&worktree).unwrap();

    let paths: Vec<&str> = changeset.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, ["README.md", "new.rs"]);
    let readme = &changeset.files[0];
    assert!(readme.hunks[0]
        .lines
        .iter()
        .any(|l| l.kind == DiffLineKind::Added && l.content == "changed"));
    let new = &changeset.files[1];
    assert_eq!(
        new.hunks.len(),
        1,
        "untracked files appear in the changeset"
    );
    // Intent-to-add must not stage content: a later discard still wipes it.
    port.discard_worktree(&worktree).unwrap();
    assert!(!worktree.path.join("new.rs").exists());

    std::fs::remove_dir_all(repo.parent().unwrap().parent().unwrap()).ok();
}

#[test]
fn discard_resets_the_worktree_and_leaves_the_repo_clean() {
    let (repo, mut port) = repo_and_port("discard");
    let session = AuthoringSessionId::from("pane-1");
    let worktree = port.create_worktree(&session).unwrap();

    std::fs::write(worktree.path.join("README.md"), "mangled\n").unwrap();
    std::fs::write(worktree.path.join("junk.txt"), "junk\n").unwrap();

    port.discard_worktree(&worktree).unwrap();

    let status = git_stdout(&worktree.path, &["status", "--porcelain"]);
    assert_eq!(status, "", "worktree should be clean after discard");
    assert!(worktree.path.join("README.md").exists());

    std::fs::remove_dir_all(repo.parent().unwrap().parent().unwrap()).ok();
}
