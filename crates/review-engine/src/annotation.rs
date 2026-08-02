use std::collections::HashMap;

use crate::ids::AgentWorktreeId;

/// A per-hunk explanation (what was done and why) emitted by the agent at
/// authoring time through the Harness Adapter (ADR-0003). Anchored by the
/// post-change file path and a line number inside the annotated hunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Annotation {
    /// Post-change path of the annotated file, repo-relative.
    pub file: String,
    /// A line number in the post-change file that the annotated hunk covers.
    pub line: usize,
    /// What the agent did in this hunk.
    pub what: String,
    /// Why the agent did it this way.
    pub why: String,
}

/// All Annotations, keyed by Agent Worktree and commit range (the base
/// commit the uncommitted changeset sits on) — the unit the host persists
/// in the product state store, never as sidecar files in the repo.
pub type AnnotationsSnapshot = HashMap<AgentWorktreeId, HashMap<String, Vec<Annotation>>>;
