use std::path::PathBuf;

use crate::ids::AgentWorktreeId;

/// The dedicated git worktree + branch bound to one agent pane (ADR-0004).
/// The pane runs inside `path`; Approve→commit lands on `branch`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentWorktree {
    pub id: AgentWorktreeId,
    pub branch: String,
    pub path: PathBuf,
}
