use crate::ids::{AgentWorktreeId, AuthoringSessionId};

/// The Harness Adapter seam: how the engine talks to an agent product
/// (v1: Claude Code).
pub trait AgentPort {
    /// Whether the Authoring Session is still alive to converse with.
    fn session_is_live(&self, session: &AuthoringSessionId) -> bool;

    /// Deliver a structured instruction set (e.g. a submitted Review) to the
    /// session.
    fn deliver_instructions(&mut self, session: &AuthoringSessionId, instructions: &str);
}

/// Git operations on Agent Worktrees.
pub trait GitPort {
    /// Create the dedicated worktree + branch for an agent pane.
    fn create_worktree(&mut self, session: &AuthoringSessionId) -> AgentWorktreeId;
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
