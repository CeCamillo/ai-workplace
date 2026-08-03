//! Fake port implementations for engine tests: a scriptable agent, an
//! in-memory git, and a render-model capture.

use std::collections::{HashMap, HashSet};

use crate::annotation::Annotation;
use crate::conversation::SessionSeed;
use crate::diff::Changeset;
use crate::ids::{AgentWorktreeId, AuthoringSessionId};
use crate::ports::{AgentPort, GitPort, MergeOutcome, RenderModel, UiPort};
use crate::worktree::AgentWorktree;

/// Scriptable [`AgentPort`]: liveness is set by the test, delivered
/// instructions are recorded for assertion, and pushed Annotations wait to
/// be drained like a real adapter's capture buffer.
#[derive(Debug, Default)]
pub struct FakeAgentPort {
    live: HashSet<AuthoringSessionId>,
    delivered: Vec<(AuthoringSessionId, String)>,
    pending_annotations: HashMap<AuthoringSessionId, Vec<Annotation>>,
    spawned: Vec<SessionSeed>,
    fail_next_spawn: Option<String>,
}

impl FakeAgentPort {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_live(&mut self, session: AuthoringSessionId) {
        self.live.insert(session);
    }

    pub fn delivered(&self) -> &[(AuthoringSessionId, String)] {
        &self.delivered
    }

    /// Script an Annotation the adapter captured from this session.
    pub fn push_annotation(&mut self, session: &AuthoringSessionId, annotation: Annotation) {
        self.pending_annotations
            .entry(session.clone())
            .or_default()
            .push(annotation);
    }

    /// The seeds of every seeded session spawned, in spawn order. Spawn `n`
    /// got the id `seeded-<n>`, 1-based.
    pub fn spawned(&self) -> &[SessionSeed] {
        &self.spawned
    }

    /// Make the next seeded-session spawn fail with this message.
    pub fn fail_next_spawn(&mut self, message: String) {
        self.fail_next_spawn = Some(message);
    }
}

impl AgentPort for FakeAgentPort {
    fn session_is_live(&self, session: &AuthoringSessionId) -> bool {
        self.live.contains(session)
    }

    fn deliver_instructions(&mut self, session: &AuthoringSessionId, instructions: &str) {
        self.delivered
            .push((session.clone(), instructions.to_string()));
    }

    fn take_annotations(&mut self, session: &AuthoringSessionId) -> Vec<Annotation> {
        self.pending_annotations.remove(session).unwrap_or_default()
    }

    fn spawn_seeded_session(&mut self, seed: &SessionSeed) -> Result<AuthoringSessionId, String> {
        if let Some(message) = self.fail_next_spawn.take() {
            return Err(message);
        }
        self.spawned.push(seed.clone());
        Ok(AuthoringSessionId(format!("seeded-{}", self.spawned.len())))
    }
}

/// In-memory [`GitPort`]: worktrees are derived from the session id, no
/// filesystem or real git involved. Adoptable branches and the next failure
/// are scripted by the test.
#[derive(Debug, Default)]
pub struct FakeGitPort {
    existing: HashMap<String, AgentWorktree>,
    created: Vec<AgentWorktree>,
    discarded: Vec<AgentWorktree>,
    merged: Vec<AgentWorktree>,
    removed: Vec<AgentWorktree>,
    changeset: Changeset,
    changeset_base: String,
    /// Scripted outcomes for coming merges, oldest first; an unscripted
    /// merge succeeds cleanly.
    merge_outcomes: Vec<MergeOutcome>,
    fail_next: Option<String>,
}

impl FakeGitPort {
    pub fn new() -> Self {
        Self::default()
    }

    /// Script a worktree/branch that [`GitPort::adopt_worktree`] will find.
    pub fn add_existing(&mut self, worktree: AgentWorktree) {
        self.existing.insert(worktree.branch.clone(), worktree);
    }

    /// Make the next git operation fail with this message.
    pub fn fail_next(&mut self, message: String) {
        self.fail_next = Some(message);
    }

    /// Script the changeset every worktree's [`GitPort::changeset_diff`]
    /// returns.
    pub fn set_changeset(&mut self, changeset: Changeset) {
        self.changeset = changeset;
    }

    /// Script the commit every worktree's changeset sits on.
    pub fn set_changeset_base(&mut self, base: &str) {
        self.changeset_base = base.to_string();
    }

    pub fn created(&self) -> &[AgentWorktree] {
        &self.created
    }

    pub fn discarded(&self) -> &[AgentWorktree] {
        &self.discarded
    }

    /// Worktrees whose branches were merged into the default branch.
    pub fn merged(&self) -> &[AgentWorktree] {
        &self.merged
    }

    /// Worktrees removed (with their branches) after the loop closed.
    pub fn removed(&self) -> &[AgentWorktree] {
        &self.removed
    }

    /// Script the next merge to hit conflicts in these files.
    pub fn script_merge_conflicts(&mut self, files: Vec<String>) {
        self.merge_outcomes.push(MergeOutcome::Conflicts { files });
    }

    fn scripted_failure(&mut self) -> Option<String> {
        self.fail_next.take()
    }
}

impl GitPort for FakeGitPort {
    fn create_worktree(&mut self, session: &AuthoringSessionId) -> Result<AgentWorktree, String> {
        if let Some(message) = self.scripted_failure() {
            return Err(message);
        }
        let branch = format!("agent/{}", session.0);
        let worktree = AgentWorktree {
            id: AgentWorktreeId(branch.clone()),
            path: format!("/fake/worktrees/{}", session.0).into(),
            branch,
        };
        self.created.push(worktree.clone());
        Ok(worktree)
    }

    fn adopt_worktree(&mut self, branch: &str) -> Result<AgentWorktree, String> {
        if let Some(message) = self.scripted_failure() {
            return Err(message);
        }
        self.existing
            .get(branch)
            .cloned()
            .ok_or_else(|| format!("branch '{branch}' does not exist"))
    }

    fn discard_worktree(&mut self, worktree: &AgentWorktree) -> Result<(), String> {
        if let Some(message) = self.scripted_failure() {
            return Err(message);
        }
        self.discarded.push(worktree.clone());
        Ok(())
    }

    fn changeset_diff(&mut self, _worktree: &AgentWorktree) -> Result<Changeset, String> {
        if let Some(message) = self.scripted_failure() {
            return Err(message);
        }
        Ok(self.changeset.clone())
    }

    fn changeset_base(&mut self, _worktree: &AgentWorktree) -> Result<String, String> {
        if let Some(message) = self.scripted_failure() {
            return Err(message);
        }
        Ok(self.changeset_base.clone())
    }

    fn merge_into_default(&mut self, worktree: &AgentWorktree) -> Result<MergeOutcome, String> {
        if let Some(message) = self.scripted_failure() {
            return Err(message);
        }
        if self.merge_outcomes.is_empty() {
            self.merged.push(worktree.clone());
            return Ok(MergeOutcome::Merged);
        }
        let outcome = self.merge_outcomes.remove(0);
        if outcome == MergeOutcome::Merged {
            self.merged.push(worktree.clone());
        }
        Ok(outcome)
    }

    fn remove_worktree(&mut self, worktree: &AgentWorktree) -> Result<(), String> {
        if let Some(message) = self.scripted_failure() {
            return Err(message);
        }
        self.removed.push(worktree.clone());
        Ok(())
    }
}

/// Capturing [`UiPort`]: every render model the engine emits, in order.
#[derive(Debug, Default)]
pub struct FakeUiPort {
    rendered: Vec<RenderModel>,
}

impl FakeUiPort {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn rendered(&self) -> &[RenderModel] {
        &self.rendered
    }
}

impl UiPort for FakeUiPort {
    fn render(&mut self, model: &RenderModel) {
        self.rendered.push(model.clone());
    }
}
