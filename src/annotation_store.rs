//! The product state store for Annotations: one JSON file per Agent
//! Worktree under herdr's state directory, keyed inside by commit range
//! (ADR-0003). Annotations never live as sidecar files in the repo — that
//! would pollute worktrees and diffs.
//!
//! The Claude Code Harness Adapter writes here (`herdr annotate`, run by
//! the agent at authoring time) and the Review Pane reads back through its
//! AgentPort, so Annotations survive detach/reattach and session death.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// One persisted Annotation; the on-disk shape of
/// [`review_engine::Annotation`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StoredAnnotation {
    pub file: String,
    pub line: usize,
    pub what: String,
    pub why: String,
}

/// Everything stored for one Agent Worktree: annotations in emission
/// order, keyed by the commit the changeset sat on when they were emitted.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct WorktreeAnnotations {
    pub worktree: String,
    pub repo_root: String,
    pub ranges: HashMap<String, Vec<StoredAnnotation>>,
}

/// The store rooted at a directory of per-worktree JSON files.
#[derive(Debug, Clone)]
pub(crate) struct AnnotationStore {
    dir: PathBuf,
}

impl AnnotationStore {
    /// The product state store, alongside herdr's other session state.
    pub(crate) fn product() -> Self {
        Self {
            dir: crate::config::state_dir().join("annotations"),
        }
    }

    #[cfg(test)]
    pub(crate) fn at(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// One file per (repository, Agent Worktree branch): different repos
    /// may reuse branch names, so the repo root is hashed into the name.
    fn file_for(&self, repo_root: &Path, branch: &str) -> PathBuf {
        let repo_hash = Sha256::digest(repo_root.to_string_lossy().as_bytes());
        let repo_hash = repo_hash
            .iter()
            .take(6)
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let branch_slug: String = branch
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        self.dir.join(format!("{repo_hash}-{branch_slug}.json"))
    }

    fn load(&self, repo_root: &Path, branch: &str) -> WorktreeAnnotations {
        let file = self.file_for(repo_root, branch);
        let Ok(content) = std::fs::read_to_string(&file) else {
            return WorktreeAnnotations::default();
        };
        serde_json::from_str(&content).unwrap_or_else(|err| {
            tracing::warn!(
                file = %file.display(),
                %err,
                "annotation store file is corrupt; treating it as empty"
            );
            WorktreeAnnotations::default()
        })
    }

    /// Append one Annotation under `(repo, branch, base)`.
    pub(crate) fn append(
        &self,
        repo_root: &Path,
        branch: &str,
        base: &str,
        annotation: StoredAnnotation,
    ) -> io::Result<()> {
        let mut stored = self.load(repo_root, branch);
        stored.worktree = branch.to_string();
        stored.repo_root = repo_root.to_string_lossy().into_owned();
        stored
            .ranges
            .entry(base.to_string())
            .or_default()
            .push(annotation);
        std::fs::create_dir_all(&self.dir)?;
        let json = serde_json::to_string_pretty(&stored).map_err(io::Error::other)?;
        std::fs::write(self.file_for(repo_root, branch), json)
    }

    /// The Annotations authored on `(repo, branch, base)`, in emission
    /// order, as the engine consumes them.
    pub(crate) fn annotations_for(
        &self,
        repo_root: &Path,
        branch: &str,
        base: &str,
    ) -> Vec<review_engine::Annotation> {
        let mut stored = self.load(repo_root, branch);
        stored
            .ranges
            .remove(base)
            .unwrap_or_default()
            .into_iter()
            .map(|a| review_engine::Annotation {
                file: a.file,
                line: a.line,
                what: a.what,
                why: a.why,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn annotation(what: &str) -> StoredAnnotation {
        StoredAnnotation {
            file: "src/lib.rs".to_string(),
            line: 3,
            what: what.to_string(),
            why: "because".to_string(),
        }
    }

    fn temp_store(name: &str) -> AnnotationStore {
        let dir = std::env::temp_dir().join(format!(
            "herdr-annotation-store-{name}-{}",
            std::process::id()
        ));
        std::fs::remove_dir_all(&dir).ok();
        AnnotationStore::at(dir)
    }

    #[test]
    fn appends_and_reads_back_in_emission_order_scoped_by_commit_range() {
        let store = temp_store("roundtrip");
        let repo = Path::new("/some/repo");

        store
            .append(repo, "agent/pane-1", "base-1", annotation("first"))
            .unwrap();
        store
            .append(repo, "agent/pane-1", "base-1", annotation("second"))
            .unwrap();
        store
            .append(repo, "agent/pane-1", "base-2", annotation("other range"))
            .unwrap();

        let annotations = store.annotations_for(repo, "agent/pane-1", "base-1");
        let whats: Vec<&str> = annotations.iter().map(|a| a.what.as_str()).collect();
        assert_eq!(whats, ["first", "second"]);
        assert_eq!(annotations[0].file, "src/lib.rs");
        assert_eq!(annotations[0].line, 3);

        assert_eq!(
            store.annotations_for(repo, "agent/pane-1", "base-2").len(),
            1
        );
        assert!(store
            .annotations_for(repo, "agent/pane-1", "base-3")
            .is_empty());
        assert!(store
            .annotations_for(repo, "agent/other", "base-1")
            .is_empty());
    }

    #[test]
    fn repos_with_the_same_branch_name_do_not_collide() {
        let store = temp_store("collide");
        store
            .append(
                Path::new("/repo/a"),
                "agent/pane-1",
                "base",
                annotation("a"),
            )
            .unwrap();
        store
            .append(
                Path::new("/repo/b"),
                "agent/pane-1",
                "base",
                annotation("b"),
            )
            .unwrap();

        let a = store.annotations_for(Path::new("/repo/a"), "agent/pane-1", "base");
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].what, "a");
    }

    #[test]
    fn a_missing_or_corrupt_store_file_reads_as_empty() {
        let store = temp_store("corrupt");
        let repo = Path::new("/some/repo");
        assert!(store
            .annotations_for(repo, "agent/pane-1", "base")
            .is_empty());

        store
            .append(repo, "agent/pane-1", "base", annotation("kept"))
            .unwrap();
        let file = store.file_for(repo, "agent/pane-1");
        std::fs::write(&file, "not json").unwrap();
        assert!(store
            .annotations_for(repo, "agent/pane-1", "base")
            .is_empty());
    }
}
