use std::collections::HashMap;

use crate::annotation::AnnotationsSnapshot;
use crate::event::{Effect, Event, WorktreeSpawn};
use crate::ids::AuthoringSessionId;
use crate::ports::{AgentPort, GitPort, RenderModel, UiPort};
use crate::review::{MarkTarget, ReviewState, ReviewedMarksSnapshot};
use crate::worktree::AgentWorktree;

/// The event-in/effect-out core of the Agent Review Loop.
pub struct Engine<A: AgentPort, G: GitPort, U: UiPort> {
    agent: A,
    git: G,
    ui: U,
    model: RenderModel,
    /// Each session's Agent Worktree, exact by construction (ADR-0004).
    worktrees: HashMap<AuthoringSessionId, AgentWorktree>,
    /// The open Review Panes, in the order they were opened.
    reviews: Vec<(AuthoringSessionId, ReviewState)>,
    /// Reviewed-marks, keyed by Agent Worktree so they outlive any one
    /// Review Pane (close/reopen, detach/reattach).
    marks: ReviewedMarksSnapshot,
    /// Annotations, keyed by Agent Worktree and commit range so they outlive
    /// the Authoring Session that emitted them (ADR-0003).
    annotations: AnnotationsSnapshot,
}

impl<A: AgentPort, G: GitPort, U: UiPort> Engine<A, G, U> {
    pub fn new(agent: A, git: G, ui: U) -> Self {
        Self {
            agent,
            git,
            ui,
            model: RenderModel::default(),
            worktrees: HashMap::new(),
            reviews: Vec::new(),
            marks: ReviewedMarksSnapshot::new(),
            annotations: AnnotationsSnapshot::new(),
        }
    }

    pub fn agent(&self) -> &A {
        &self.agent
    }

    pub fn agent_mut(&mut self) -> &mut A {
        &mut self.agent
    }

    pub fn git(&self) -> &G {
        &self.git
    }

    pub fn git_mut(&mut self) -> &mut G {
        &mut self.git
    }

    pub fn ui(&self) -> &U {
        &self.ui
    }

    /// All reviewed-marks, for the host to persist across server restarts.
    pub fn reviewed_marks_snapshot(&self) -> ReviewedMarksSnapshot {
        self.marks.clone()
    }

    /// Restore host-persisted reviewed-marks (e.g. on server start).
    pub fn restore_reviewed_marks(&mut self, snapshot: ReviewedMarksSnapshot) {
        self.marks = snapshot;
    }

    /// All Annotations, for the host to persist in the product state store.
    pub fn annotations_snapshot(&self) -> AnnotationsSnapshot {
        self.annotations.clone()
    }

    /// Restore host-persisted Annotations (e.g. on server start).
    pub fn restore_annotations(&mut self, snapshot: AnnotationsSnapshot) {
        self.annotations = snapshot;
    }

    /// Advance the loop: apply one event, re-render, and return the effects
    /// the host must carry out.
    pub fn handle_event(&mut self, event: Event) -> Vec<Effect> {
        let effects = match event {
            Event::AgentPaneSpawned { session, worktree } => self.bind_worktree(session, worktree),
            Event::AgentFinished { session } => {
                self.model.ready_for_review.push(session.clone());
                vec![Effect::ReadyForReview { session }]
            }
            Event::AnnotationsEmitted { session } => self.ingest_annotations(session),
            Event::ChangesetDiscarded { session } => self.discard_changeset(session),
            Event::ReviewRequested { session } => self.open_review(session),
            Event::ReviewClosed { session } => {
                self.reviews.retain(|(s, _)| *s != session);
                vec![]
            }
            Event::ReviewNavigated { session, motion } => {
                self.with_review(&session, |review| review.apply_motion(motion));
                vec![]
            }
            Event::ReviewViewportResized { session, rows } => {
                self.with_review(&session, |review| review.resize_viewport(rows));
                vec![]
            }
            Event::ReviewMarkToggled { session, target } => {
                self.toggle_mark(&session, target);
                vec![]
            }
        };
        self.render();
        effects
    }

    /// Rebuild the declarative render model from engine state and push it
    /// through the UiPort. Render must not mutate state, so a worktree
    /// with no marks yet reads from a shared empty default.
    fn render(&mut self) {
        let no_marks = crate::review::ReviewedMarks::default();
        let mut views = Vec::with_capacity(self.reviews.len());
        for (session, review) in &self.reviews {
            let marks = self.marks.get(&review.worktree.id).unwrap_or(&no_marks);
            views.push(review.view(session, marks));
        }
        self.model.reviews = views;
        self.ui.render(&self.model);
    }

    fn open_review(&mut self, session: AuthoringSessionId) -> Vec<Effect> {
        let Some(worktree) = self.worktrees.get(&session).cloned() else {
            return vec![Effect::ReviewOpenFailed {
                session,
                message: "no Agent Worktree is bound to this session".to_string(),
            }];
        };
        let changeset = match self.git.changeset_diff(&worktree) {
            Ok(changeset) => changeset,
            Err(message) => return vec![Effect::ReviewOpenFailed { session, message }],
        };
        let base = match self.git.changeset_base(&worktree) {
            Ok(base) => base,
            Err(message) => return vec![Effect::ReviewOpenFailed { session, message }],
        };
        // Annotations still sitting in the adapter belong to this review.
        self.drain_annotations(&session, &worktree, &base);
        let annotations = self
            .annotations
            .get(&worktree.id)
            .and_then(|ranges| ranges.get(&base))
            .cloned()
            .unwrap_or_default();
        let worktree_id = worktree.id.clone();
        self.reviews.retain(|(s, _)| *s != session);
        self.reviews.push((
            session.clone(),
            ReviewState::new(worktree, changeset, base, annotations),
        ));
        vec![Effect::ReviewPaneOpened {
            session,
            worktree: worktree_id,
        }]
    }

    fn ingest_annotations(&mut self, session: AuthoringSessionId) -> Vec<Effect> {
        let Some(worktree) = self.worktrees.get(&session).cloned() else {
            return vec![Effect::WorktreeOperationFailed {
                session,
                message: "no Agent Worktree is bound to this session".to_string(),
            }];
        };
        // Resolve the commit-range key before draining: a failure here must
        // leave the annotations in the adapter, not lose them.
        let base = match self.git.changeset_base(&worktree) {
            Ok(base) => base,
            Err(message) => return vec![Effect::WorktreeOperationFailed { session, message }],
        };
        self.drain_annotations(&session, &worktree, &base);
        vec![]
    }

    /// Move the adapter's captured Annotations into the persistent store
    /// under `(worktree, base)`, refreshing an open review on that range.
    fn drain_annotations(
        &mut self,
        session: &AuthoringSessionId,
        worktree: &AgentWorktree,
        base: &str,
    ) {
        let drained = self.agent.take_annotations(session);
        if drained.is_empty() {
            return;
        }
        let stored = self
            .annotations
            .entry(worktree.id.clone())
            .or_default()
            .entry(base.to_string())
            .or_default();
        stored.extend(drained);
        let stored = stored.clone();
        if let Some((_, review)) = self.reviews.iter_mut().find(|(s, _)| s == session) {
            if review.base == base {
                review.annotations = stored;
            }
        }
    }

    fn with_review(&mut self, session: &AuthoringSessionId, apply: impl FnOnce(&mut ReviewState)) {
        if let Some((_, review)) = self.reviews.iter_mut().find(|(s, _)| s == session) {
            apply(review);
        }
    }

    fn toggle_mark(&mut self, session: &AuthoringSessionId, target: MarkTarget) {
        if let Some((_, review)) = self.reviews.iter().find(|(s, _)| s == session) {
            let marks = self.marks.entry(review.worktree.id.clone()).or_default();
            review.toggle_mark(marks, target);
        }
    }

    fn bind_worktree(&mut self, session: AuthoringSessionId, spawn: WorktreeSpawn) -> Vec<Effect> {
        let result = match spawn {
            WorktreeSpawn::Create => self.git.create_worktree(&session),
            WorktreeSpawn::Adopt { branch } => self.git.adopt_worktree(&branch),
        };
        match result {
            Ok(worktree) => {
                self.worktrees.insert(session.clone(), worktree.clone());
                vec![Effect::PaneWorktreeBound { session, worktree }]
            }
            Err(message) => vec![Effect::WorktreeOperationFailed { session, message }],
        }
    }

    fn discard_changeset(&mut self, session: AuthoringSessionId) -> Vec<Effect> {
        let Some(worktree) = self.worktrees.get(&session).cloned() else {
            return vec![Effect::WorktreeOperationFailed {
                session,
                message: "no Agent Worktree is bound to this session".to_string(),
            }];
        };
        match self.git.discard_worktree(&worktree) {
            Ok(()) => {
                self.model.ready_for_review.retain(|s| *s != session);
                // The changeset the marks and review referred to is gone.
                self.reviews.retain(|(s, _)| *s != session);
                self.marks.remove(&worktree.id);
                self.annotations.remove(&worktree.id);
                vec![Effect::WorktreeDiscarded {
                    session,
                    worktree: worktree.id,
                }]
            }
            Err(message) => vec![Effect::WorktreeOperationFailed { session, message }],
        }
    }
}
