//! `herdr annotate`: the Claude Code Harness Adapter's CLI surface.
//!
//! The write side: the agent runs `herdr annotate --file … --line … --what
//! … --why …` from inside its Agent Worktree, per hunk, at authoring time;
//! the Annotation lands in the product state store keyed by worktree and
//! commit range, where the Review Pane reads it back.
//!
//! The adapter itself installs with `herdr annotate install-hook`: a
//! SessionStart hook that teaches Claude Code the protocol whenever a
//! session starts inside an Agent Worktree, plus the bundled skill.

use std::path::Path;

use super::worktree_identity::{head_commit, resolve_worktree_identity};
use crate::annotation_store::{AnnotationStore, StoredAnnotation};

const USAGE: &str = "usage: herdr annotate --file <repo-relative-path> --line <line> \
--what <text> --why <text>\n       herdr annotate --skill\n       \
herdr annotate install-hook | uninstall-hook";

// Bundled at build time so the printed protocol always matches this binary.
const ANNOTATE_SKILL: &str = include_str!("../../skills/annotate/SKILL.md");
const ANNOTATE_HOOK_ASSET: &str = include_str!("../integration/assets/claude/herdr-annotate.sh");
const ANNOTATE_HOOK_INSTALL_NAME: &str = "herdr-annotate.sh";
const ANNOTATE_SKILL_DIR: &str = "herdr-annotate";

pub(super) fn run_annotate_command(args: &[String]) -> std::io::Result<i32> {
    if args.iter().any(|arg| arg == "--skill") {
        print!("{ANNOTATE_SKILL}");
        return Ok(0);
    }
    match args.first().map(|arg| arg.as_str()) {
        Some("install-hook") => return install_hook(),
        Some("uninstall-hook") => return uninstall_hook(),
        _ => {}
    }

    let mut file: Option<String> = None;
    let mut line: Option<usize> = None;
    let mut what: Option<String> = None;
    let mut why: Option<String> = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let target = match arg.as_str() {
            "--file" => &mut file,
            "--what" => &mut what,
            "--why" => &mut why,
            "--line" => {
                match iter.next().and_then(|value| value.parse().ok()) {
                    Some(value) => line = Some(value),
                    None => {
                        eprintln!("--line expects a positive number\n{USAGE}");
                        return Ok(2);
                    }
                }
                continue;
            }
            "--help" | "-h" => {
                println!("{USAGE}");
                return Ok(0);
            }
            _ => {
                eprintln!("unknown argument '{arg}'\n{USAGE}");
                return Ok(2);
            }
        };
        match iter.next() {
            Some(value) if !value.is_empty() => *target = Some(value.clone()),
            _ => {
                eprintln!("{arg} expects a non-empty value\n{USAGE}");
                return Ok(2);
            }
        }
    }
    let (Some(file), Some(line), Some(what), Some(why)) = (file, line, what, why) else {
        eprintln!("{USAGE}");
        return Ok(2);
    };

    match annotate(&std::env::current_dir()?, &file, line, &what, &why) {
        Ok(branch) => {
            println!("annotated {file}:{line} on {branch}");
            Ok(0)
        }
        Err(message) => {
            eprintln!("cannot annotate: {message}");
            Ok(1)
        }
    }
}

/// Resolve the Agent Worktree identity from `cwd` and persist one
/// Annotation; returns the worktree's branch.
fn annotate(cwd: &Path, file: &str, line: usize, what: &str, why: &str) -> Result<String, String> {
    let identity = resolve_worktree_identity(cwd)?;
    let base = head_commit(cwd)?;

    AnnotationStore::product()
        .append(
            &identity.repo_root,
            &identity.branch,
            &base,
            StoredAnnotation {
                file: file.to_string(),
                line,
                what: what.to_string(),
                why: why.to_string(),
            },
        )
        .map_err(|err| format!("cannot write the annotation store: {err}"))?;
    Ok(identity.branch)
}

/// The annotate hook is a POSIX shell script; a Windows adapter needs its
/// own asset first.
#[cfg(windows)]
fn install_hook() -> std::io::Result<i32> {
    eprintln!("the annotate hook is not supported on Windows yet");
    Ok(1)
}

/// Install the Harness Adapter into Claude Code: the SessionStart hook
/// script, its settings registration (through the same careful settings
/// editor the claude integration uses), and the bundled skill.
#[cfg(not(windows))]
fn install_hook() -> std::io::Result<i32> {
    let dir = crate::integration::claude_dir()?;
    if !dir.is_dir() {
        eprintln!(
            "claude directory not found at {}. install claude code first",
            dir.display()
        );
        return Ok(1);
    }

    let hooks_dir = dir.join("hooks");
    std::fs::create_dir_all(&hooks_dir)?;
    let hook_path = hooks_dir.join(ANNOTATE_HOOK_INSTALL_NAME);
    std::fs::write(&hook_path, ANNOTATE_HOOK_ASSET)?;
    crate::integration::make_executable(&hook_path)?;

    let settings_path = dir.join("settings.json");
    let existing = if settings_path.is_file() {
        std::fs::read_to_string(&settings_path)?
    } else {
        "{}".to_string()
    };
    let updated =
        crate::integration::claude_settings::install(&existing, &settings_path, &hook_path)?;
    if updated != existing {
        std::fs::write(&settings_path, updated)?;
    }

    let skill_dir = dir.join("skills").join(ANNOTATE_SKILL_DIR);
    std::fs::create_dir_all(&skill_dir)?;
    std::fs::write(skill_dir.join("SKILL.md"), ANNOTATE_SKILL)?;

    println!("installed annotate hook to {}", hook_path.display());
    println!("ensured claude settings at {}", settings_path.display());
    println!(
        "installed herdr-annotate skill to {}",
        skill_dir.join("SKILL.md").display()
    );
    Ok(0)
}

/// Remove everything `install_hook` put in place.
fn uninstall_hook() -> std::io::Result<i32> {
    let dir = crate::integration::claude_dir()?;
    let hook_path = dir.join("hooks").join(ANNOTATE_HOOK_INSTALL_NAME);
    let settings_path = dir.join("settings.json");

    if settings_path.is_file() {
        let existing = std::fs::read_to_string(&settings_path)?;
        let updated =
            crate::integration::claude_settings::uninstall(&existing, &settings_path, &hook_path)?;
        if updated != existing {
            std::fs::write(&settings_path, updated)?;
            println!("removed annotate hook from {}", settings_path.display());
        }
    }
    if hook_path.is_file() {
        std::fs::remove_file(&hook_path)?;
        println!("removed {}", hook_path.display());
    }
    let skill_dir = dir.join("skills").join(ANNOTATE_SKILL_DIR);
    if skill_dir.is_dir() {
        std::fs::remove_dir_all(&skill_dir)?;
        println!("removed {}", skill_dir.display());
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use serde_json::Value;

    fn temp_claude_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("herdr-annotate-hook-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[cfg(not(windows))]
    #[test]
    fn install_hook_registers_session_start_and_skill_and_is_idempotent() {
        let _lock = crate::integration::integration_env_lock();
        let dir = temp_claude_dir("install");
        std::env::set_var("CLAUDE_CONFIG_DIR", &dir);

        assert_eq!(install_hook().unwrap(), 0);
        assert_eq!(install_hook().unwrap(), 0);

        let hook_path = dir.join("hooks").join(ANNOTATE_HOOK_INSTALL_NAME);
        assert_eq!(
            std::fs::read_to_string(&hook_path).unwrap(),
            ANNOTATE_HOOK_ASSET
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("skills").join(ANNOTATE_SKILL_DIR).join("SKILL.md"))
                .unwrap(),
            ANNOTATE_SKILL
        );
        let settings: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("settings.json")).unwrap())
                .unwrap();
        let session_start = settings["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(session_start.len(), 1, "reinstall must not duplicate");
        let command = session_start[0]["hooks"][0]["command"].as_str().unwrap();
        assert!(command.contains(ANNOTATE_HOOK_INSTALL_NAME));

        assert_eq!(uninstall_hook().unwrap(), 0);
        assert!(!hook_path.exists());
        assert!(!dir.join("skills").join(ANNOTATE_SKILL_DIR).exists());
        let settings: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("settings.json")).unwrap())
                .unwrap();
        assert!(settings["hooks"].get("SessionStart").is_none());

        std::env::remove_var("CLAUDE_CONFIG_DIR");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(not(windows))]
    #[test]
    fn install_hook_leaves_the_agent_state_integration_entry_alone() {
        let _lock = crate::integration::integration_env_lock();
        let dir = temp_claude_dir("coexist");
        // The claude agent-state integration is already installed.
        let state_hook = dir.join("hooks").join("herdr-agent-state.sh");
        let settings = serde_json::json!({
            "hooks": {
                "SessionStart": [{
                    "matcher": "*",
                    "hooks": [{
                        "type": "command",
                        "command": format!("bash '{}' session", state_hook.display()),
                        "timeout": 10
                    }]
                }]
            }
        });
        std::fs::write(
            dir.join("settings.json"),
            serde_json::to_string(&settings).unwrap(),
        )
        .unwrap();
        std::env::set_var("CLAUDE_CONFIG_DIR", &dir);

        assert_eq!(install_hook().unwrap(), 0);

        let settings: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("settings.json")).unwrap())
                .unwrap();
        let session_start = settings["hooks"]["SessionStart"].as_array().unwrap();
        let commands: Vec<String> = session_start
            .iter()
            .flat_map(|entry| entry["hooks"].as_array().unwrap().iter())
            .map(|hook| hook["command"].as_str().unwrap().to_string())
            .collect();
        assert!(commands.iter().any(|c| c.contains("herdr-agent-state.sh")));
        assert!(commands
            .iter()
            .any(|c| c.contains(ANNOTATE_HOOK_INSTALL_NAME)));

        assert_eq!(uninstall_hook().unwrap(), 0);
        let settings: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("settings.json")).unwrap())
                .unwrap();
        let session_start = settings["hooks"]["SessionStart"].as_array().unwrap();
        let commands: Vec<String> = session_start
            .iter()
            .flat_map(|entry| entry["hooks"].as_array().unwrap().iter())
            .map(|hook| hook["command"].as_str().unwrap().to_string())
            .collect();
        assert!(commands.iter().any(|c| c.contains("herdr-agent-state.sh")));
        assert!(!commands
            .iter()
            .any(|c| c.contains(ANNOTATE_HOOK_INSTALL_NAME)));

        std::env::remove_var("CLAUDE_CONFIG_DIR");
        std::fs::remove_dir_all(&dir).ok();
    }
}
