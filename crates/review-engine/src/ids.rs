/// Identifies the Authoring Session that produced a changeset. The session
/// may still be live in a pane or already gone; the engine never assumes
/// liveness — it asks [`crate::AgentPort`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AuthoringSessionId(pub String);

impl From<&str> for AuthoringSessionId {
    fn from(id: &str) -> Self {
        Self(id.to_string())
    }
}

/// Identifies the dedicated git worktree + branch bound to one agent pane.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AgentWorktreeId(pub String);

impl From<&str> for AgentWorktreeId {
    fn from(id: &str) -> Self {
        Self(id.to_string())
    }
}
