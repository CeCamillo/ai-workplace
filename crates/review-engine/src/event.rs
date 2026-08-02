use crate::ids::{AgentWorktreeId, AuthoringSessionId};
use crate::worktree::AgentWorktree;

/// How a spawned agent pane gets its Agent Worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorktreeSpawn {
    /// Create a fresh worktree + branch dedicated to this session.
    Create,
    /// Adopt an existing worktree/branch instead of creating one.
    Adopt { branch: String },
}

/// Something that happened in the world, fed into the engine by the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The host spawned an agent pane; the engine must bind it to an Agent
    /// Worktree before the pane's agent starts working.
    AgentPaneSpawned {
        session: AuthoringSessionId,
        worktree: WorktreeSpawn,
    },
    /// The agent in a pane finished working.
    AgentFinished { session: AuthoringSessionId },
    /// The human discarded the session's changeset.
    ChangesetDiscarded { session: AuthoringSessionId },
}

/// Something the engine wants the host to do in response to an [`Event`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// The session's Agent Worktree exists; run the pane inside
    /// `worktree.path`.
    PaneWorktreeBound {
        session: AuthoringSessionId,
        worktree: AgentWorktree,
    },
    /// Surface the pane as Ready-for-Review in the host's status model
    /// (review is never auto-opened).
    ReadyForReview { session: AuthoringSessionId },
    /// The worktree was reset clean; the changeset is gone but the worktree
    /// and branch remain bound to the pane.
    WorktreeDiscarded {
        session: AuthoringSessionId,
        worktree: AgentWorktreeId,
    },
    /// A worktree operation failed; surface the message to the human.
    WorktreeOperationFailed {
        session: AuthoringSessionId,
        message: String,
    },
}
