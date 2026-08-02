use crate::ids::AuthoringSessionId;
use crate::worktree::AgentWorktree;

/// The Harness Adapter seam: how the engine talks to an agent product
/// (v1: Claude Code).
pub trait AgentPort {
    /// Whether the Authoring Session is still alive to converse with.
    fn session_is_live(&self, session: &AuthoringSessionId) -> bool;

    /// Deliver a structured instruction set (e.g. a submitted Review) to the
    /// session.
    fn deliver_instructions(&mut self, session: &AuthoringSessionId, instructions: &str);
}

/// Git operations on Agent Worktrees. All fallible: real git can refuse any
/// of these, and the engine surfaces failures as effects.
pub trait GitPort {
    /// Create the dedicated worktree + branch for an agent pane.
    fn create_worktree(&mut self, session: &AuthoringSessionId) -> Result<AgentWorktree, String>;

    /// Adopt an existing worktree/branch instead of creating one. Fails if
    /// the branch does not exist.
    fn adopt_worktree(&mut self, branch: &str) -> Result<AgentWorktree, String>;

    /// Reset the worktree clean, destroying the uncommitted changeset. The
    /// worktree and branch survive.
    fn discard_worktree(&mut self, worktree: &AgentWorktree) -> Result<(), String>;
}

/// What the Review Pane should draw. The engine owns this state; the pane
/// only renders it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RenderModel {
    /// Sessions whose changesets are waiting for a human.
    pub ready_for_review: Vec<AuthoringSessionId>,
}

/// The declarative-render seam consumed by the Review Pane TUI.
pub trait UiPort {
    fn render(&mut self, model: &RenderModel);
}
