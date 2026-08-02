//! Thin integration tests for the Claude Code Harness Adapter: the
//! `herdr annotate` write side against a real repo and state store, and an
//! opt-in round-trip through a real Claude Code session. Engine behavior
//! (ingest, keying, rendering) is covered in review-engine's own tests.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Command;

fn herdr_bin() -> &'static str {
    env!("CARGO_BIN_EXE_herdr")
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
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// A committed repo with one Agent Worktree, plus an isolated state dir.
struct Fixture {
    base: PathBuf,
    worktree: PathBuf,
    state_dir: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let base = std::env::temp_dir().join(format!(
            "herdr-annotate-adapter-{name}-{}",
            std::process::id()
        ));
        std::fs::remove_dir_all(&base).ok();
        let repo = base.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        run_git(&repo, &["init", "--initial-branch=main"]);
        run_git(&repo, &["config", "user.email", "test@example.com"]);
        run_git(&repo, &["config", "user.name", "Test"]);
        std::fs::write(repo.join("README.md"), "hello\nworld\n").unwrap();
        run_git(&repo, &["add", "."]);
        run_git(&repo, &["commit", "-m", "initial"]);
        let worktree = repo.join(".herdr-agent-worktrees").join("agent-pane-1");
        run_git(
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                "agent/pane-1",
                worktree.to_str().unwrap(),
            ],
        );
        Self {
            base: base.clone(),
            worktree,
            state_dir: base.join("state"),
        }
    }

    fn annotate_command(&self) -> Command {
        let mut command = Command::new(herdr_bin());
        command
            .current_dir(&self.worktree)
            .env("XDG_STATE_HOME", &self.state_dir);
        command
    }

    /// The single store file the adapter wrote, parsed.
    fn store_content(&self) -> serde_json::Value {
        let dir = self.state_dir.join("herdr-dev").join("annotations");
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
            .expect("annotation store directory should exist")
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(files.len(), 1, "expected exactly one store file");
        let content = std::fs::read_to_string(files.remove(0)).unwrap();
        serde_json::from_str(&content).unwrap()
    }
}

#[test]
fn annotate_cli_persists_into_the_product_state_store_keyed_by_worktree_and_range() {
    let fixture = Fixture::new("cli");
    std::fs::write(fixture.worktree.join("README.md"), "hello\nchanged\n").unwrap();

    let output = fixture
        .annotate_command()
        .args([
            "annotate",
            "--file",
            "README.md",
            "--line",
            "2",
            "--what",
            "reword the greeting",
            "--why",
            "the old text shadowed the tagline",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "annotate failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let base = git_stdout(&fixture.worktree, &["rev-parse", "HEAD"]);
    let store = fixture.store_content();
    assert_eq!(store["worktree"], "agent/pane-1");
    let annotations = store["ranges"][base.as_str()].as_array().unwrap();
    assert_eq!(annotations.len(), 1);
    assert_eq!(annotations[0]["file"], "README.md");
    assert_eq!(annotations[0]["line"], 2);
    assert_eq!(annotations[0]["what"], "reword the greeting");
    assert_eq!(annotations[0]["why"], "the old text shadowed the tagline");

    std::fs::remove_dir_all(&fixture.base).ok();
}

#[test]
fn annotate_skill_flag_prints_the_bundled_protocol() {
    let output = Command::new(herdr_bin())
        .args(["annotate", "--skill"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let skill = String::from_utf8_lossy(&output.stdout);
    assert!(skill.contains("name: herdr-annotate"));
    assert!(skill.contains("herdr annotate --file"));
}

/// The full adapter loop against a live agent: the shipped SessionStart
/// hook script produces the protocol context (exactly as Claude Code
/// would receive it), a real Claude Code session gets it, edits a file,
/// and emits an Annotation via `herdr annotate`; the store must hold it.
/// Opt in with `--ignored` — it needs the `claude` CLI and API access.
#[test]
#[ignore = "round-trips a real Claude Code session; needs `claude` on PATH and API access"]
fn a_real_claude_code_session_round_trips_an_annotation() {
    use std::io::Write as _;

    if Command::new("claude").arg("--version").output().is_err() {
        eprintln!("skipping: claude CLI not found on PATH");
        return;
    }
    let fixture = Fixture::new("live");
    let bin_dir = Path::new(herdr_bin()).parent().unwrap();
    let path_env = format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    // Run the real hook script the adapter installs, with the SessionStart
    // input Claude Code would send from this worktree.
    let hook = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/integration/assets/claude/herdr-annotate.sh");
    let mut hook_process = Command::new("sh")
        .arg(&hook)
        .arg("session")
        .env("HERDR_ENV", "1")
        .env("PATH", &path_env)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    hook_process
        .stdin
        .take()
        .unwrap()
        .write_all(
            format!(
                r#"{{"hook_event_name":"SessionStart","cwd":"{}"}}"#,
                fixture.worktree.display()
            )
            .as_bytes(),
        )
        .unwrap();
    let hook_output = hook_process.wait_with_output().unwrap();
    assert!(hook_output.status.success());
    let hook_json: serde_json::Value = serde_json::from_slice(&hook_output.stdout)
        .expect("the hook should emit valid JSON inside an Agent Worktree");
    let context = hook_json["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();

    let prompt = format!(
        "{context}\n\nTask: change the second line of README.md from `world` to \
         `agents`, then follow the protocol above for that hunk."
    );

    let output = Command::new("claude")
        .current_dir(&fixture.worktree)
        .env("XDG_STATE_HOME", &fixture.state_dir)
        .env("PATH", &path_env)
        .args(["-p", &prompt, "--dangerously-skip-permissions"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "claude session failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let base = git_stdout(&fixture.worktree, &["rev-parse", "HEAD"]);
    let store = fixture.store_content();
    let annotations = store["ranges"][base.as_str()].as_array().unwrap();
    assert!(
        !annotations.is_empty(),
        "the session should have emitted at least one Annotation"
    );
    assert_eq!(annotations[0]["file"], "README.md");

    std::fs::remove_dir_all(&fixture.base).ok();
}
