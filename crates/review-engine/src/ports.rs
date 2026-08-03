use crate::annotation::Annotation;
use crate::conversation::SessionSeed;
use crate::diff::Changeset;
use crate::ids::AuthoringSessionId;
use crate::review::ReviewView;
use crate::worktree::AgentWorktree;

/// The Harness Adapter seam: how the engine talks to an agent product
/// (v1: Claude Code).
pub trait AgentPort {
    /// Whether the Authoring Session is still alive to converse with.
    fn session_is_live(&self, session: &AuthoringSessionId) -> bool;

    /// Deliver a structured instruction set (e.g. a submitted Review) to the
    /// session.
    fn deliver_instructions(&mut self, session: &AuthoringSessionId, instructions: &str);

    /// Drain the Annotations the adapter captured from this session since
    /// the last drain. The engine persists them; the adapter must not.
    fn take_annotations(&mut self, session: &AuthoringSessionId) -> Vec<Annotation>;

    /// Spawn a fresh agent session seeded to stand in for a gone Authoring
    /// Session in a Hunk Conversation (ADR-0003 fallback). Returns the new
    /// session's id.
    fn spawn_seeded_session(&mut self, seed: &SessionSeed) -> Result<AuthoringSessionId, String>;
}

/// How an attempted merge to the default branch ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeOutcome {
    /// The worktree's branch is merged into the default branch.
    Merged,
    /// The merge hit conflicts in these files. The conflicted merge is left
    /// in progress in the Agent Worktree for the agent to resolve.
    Conflicts { files: Vec<String> },
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

    /// The worktree's current changeset (everything the agent changed and
    /// has not committed), as a parsed diff.
    fn changeset_diff(&mut self, worktree: &AgentWorktree) -> Result<Changeset, String>;

    /// The commit the uncommitted changeset sits on — the commit-range key
    /// Annotations are persisted under.
    fn changeset_base(&mut self, worktree: &AgentWorktree) -> Result<String, String>;

    /// Merge the worktree's branch into the default branch. Conflicts are
    /// not an `Err`: they come back as [`MergeOutcome::Conflicts`], left in
    /// progress in the worktree for the agent to resolve.
    fn merge_into_default(&mut self, worktree: &AgentWorktree) -> Result<MergeOutcome, String>;

    /// Remove the worktree and delete its branch — cleanup once the loop is
    /// over and the branch's work is merged (or explicitly let go).
    fn remove_worktree(&mut self, worktree: &AgentWorktree) -> Result<(), String>;
}

/// What the Review Pane should draw. The engine owns this state; the pane
/// only renders it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RenderModel {
    /// Sessions whose changesets are waiting for a human.
    pub ready_for_review: Vec<AuthoringSessionId>,
    /// Sessions whose approved changesets are committed, each waiting on
    /// the one-keypress merge offer.
    pub merge_offers: Vec<AuthoringSessionId>,
    /// The open Review Panes, one view per reviewing session.
    pub reviews: Vec<ReviewView>,
}

/// The declarative-render seam consumed by the Review Pane TUI.
pub trait UiPort {
    fn render(&mut self, model: &RenderModel);
}
