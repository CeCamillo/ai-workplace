use crate::ids::AuthoringSessionId;

/// Something that happened in the world, fed into the engine by the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The agent in a pane finished working.
    AgentFinished { session: AuthoringSessionId },
}

/// Something the engine wants the host to do in response to an [`Event`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Surface the pane as Ready-for-Review in the host's status model
    /// (review is never auto-opened).
    ReadyForReview { session: AuthoringSessionId },
}
