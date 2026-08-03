//! Hunk Conversation state: an interactive exchange between the human and
//! an agent about one specific hunk, routed live-first (ADR-0003).

use crate::annotation::Annotation;
use crate::ids::AuthoringSessionId;

/// Which path a Hunk Conversation was routed down (ADR-0003).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversationRouting {
    /// The Authoring Session was still alive; it answers from the memory
    /// that actually made the decision.
    LiveAuthoringSession,
    /// The Authoring Session was gone; a fresh session seeded with the
    /// stored Annotation and hunk diff answers instead.
    SeededSession,
}

/// One utterance in a Hunk Conversation, in exchange order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversationEntry {
    Question(String),
    Answer(String),
}

/// Everything a fresh session needs to stand in for a gone Authoring
/// Session: the hunk under discussion and the Annotations stored for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSeed {
    /// Post-change path of the file the hunk belongs to, repo-relative.
    pub file: String,
    pub hunk_header: String,
    /// The hunk in unified diff format, header included.
    pub diff: String,
    /// The stored Annotations placed on this hunk, in emission order.
    pub annotations: Vec<Annotation>,
}

/// One open Hunk Conversation, anchored to a hunk of its review. Routing
/// is resolved on the first question and then sticky: follow-ups keep
/// talking to whichever session answered first.
#[derive(Debug)]
pub(crate) struct Conversation {
    /// Post-change path of the anchor file.
    pub file: String,
    /// Hunk index inside the anchor file.
    pub hunk: usize,
    /// The session this conversation talks to, once the first question
    /// resolved the routing.
    pub target: Option<(AuthoringSessionId, ConversationRouting)>,
    pub entries: Vec<ConversationEntry>,
    pub awaiting_answer: bool,
}

/// What the Review Pane draws for the open Hunk Conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationView {
    pub file: String,
    pub hunk: usize,
    pub hunk_header: String,
    /// `None` until the first question resolves the routing.
    pub routing: Option<ConversationRouting>,
    pub entries: Vec<ConversationEntry>,
    pub awaiting_answer: bool,
}
