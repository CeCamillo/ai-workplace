use crate::conversation::ConversationRouting;
use crate::ids::{AgentWorktreeId, AuthoringSessionId};
use crate::review::{MarkTarget, ReviewMotion};
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
    /// The Harness Adapter captured new Annotations from the session; the
    /// engine must drain and persist them.
    AnnotationsEmitted { session: AuthoringSessionId },
    /// The human discarded the session's changeset.
    ChangesetDiscarded { session: AuthoringSessionId },
    /// The human pressed the review keybind on a pane; open the Review Pane
    /// on that pane's Agent Worktree.
    ReviewRequested { session: AuthoringSessionId },
    /// The human closed the session's Review Pane.
    ReviewClosed { session: AuthoringSessionId },
    /// A vim motion inside the session's Review Pane.
    ReviewNavigated {
        session: AuthoringSessionId,
        motion: ReviewMotion,
    },
    /// The Review Pane's stream viewport is now `rows` tall (initial layout
    /// or a resize); half-page motions and scroll-follow depend on it.
    ReviewViewportResized {
        session: AuthoringSessionId,
        rows: usize,
    },
    /// The human toggled a reviewed-mark at the cursor.
    ReviewMarkToggled {
        session: AuthoringSessionId,
        target: MarkTarget,
    },
    /// The human opened a Hunk Conversation on the hunk under the cursor.
    /// No-op when the cursor is not on a hunk.
    ConversationOpened { session: AuthoringSessionId },
    /// The human closed the review's open Hunk Conversation.
    ConversationClosed { session: AuthoringSessionId },
    /// The human asked a question in the open Hunk Conversation. The first
    /// question resolves the routing (ADR-0003): the live Authoring
    /// Session when it exists, else a fresh seeded session.
    ConversationAsked {
        session: AuthoringSessionId,
        question: String,
    },
    /// A session answered a conversation question. `target` is the session
    /// the answer came from ([`Effect::ConversationRouted`]'s target), so
    /// an answer outliving its conversation cannot attach to another hunk.
    ConversationAnswerReceived {
        session: AuthoringSessionId,
        target: AuthoringSessionId,
        answer: String,
    },
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
    /// The review is open: mount a Review Pane for `session` and draw its
    /// [`crate::ReviewView`] from the render model.
    ReviewPaneOpened {
        session: AuthoringSessionId,
        worktree: AgentWorktreeId,
    },
    /// The review could not open; surface the message to the human.
    ReviewOpenFailed {
        session: AuthoringSessionId,
        message: String,
    },
    /// A conversation question was delivered to `target` — the live
    /// Authoring Session, or the seeded stand-in the engine had the
    /// AgentPort spawn. The answer must come back as
    /// [`Event::ConversationAnswerReceived`].
    ConversationRouted {
        session: AuthoringSessionId,
        target: AuthoringSessionId,
        routing: ConversationRouting,
    },
    /// The conversation question could not be delivered (e.g. the seeded
    /// session failed to spawn); surface the message to the human.
    ConversationFailed {
        session: AuthoringSessionId,
        message: String,
    },
}
