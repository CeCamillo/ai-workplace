//! Fix-it Comment drafts: human-authored instructions attached to hunks,
//! local to the review until the whole batch submits as one Review.

/// One drafted Fix-it Comment, anchored to a hunk of the review's
/// changeset. Drafts never leave the engine until [`crate::Event::ReviewSubmitted`]
/// batches them into a single instruction set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixItComment {
    /// Post-change path of the anchor file, repo-relative.
    pub file: String,
    /// Hunk index inside the anchor file.
    pub hunk: usize,
    /// The instruction for the agent.
    pub text: String,
}
