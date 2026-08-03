use std::collections::HashMap;

use crate::annotation::{Annotation, AnnotationsSnapshot};
use crate::conversation::{ConversationEntry, ConversationRouting};
use crate::diff::Changeset;
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
                let mut effects = vec![Effect::ReadyForReview {
                    session: session.clone(),
                }];
                effects.extend(self.refresh_review(&session));
                effects
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
            Event::ConversationOpened { session } => {
                self.with_review(&session, |review| review.open_conversation());
                vec![]
            }
            Event::ConversationClosed { session } => {
                self.with_review(&session, |review| review.close_conversation());
                vec![]
            }
            Event::ConversationAsked { session, question } => {
                self.ask_conversation(session, question)
            }
            Event::ConversationAnswerReceived {
                session,
                target,
                answer,
            } => {
                self.with_review(&session, |review| review.receive_answer(&target, answer));
                vec![]
            }
            Event::CommentDrafted { session, text } => {
                self.with_review(&session, |review| review.draft_comment(text));
                vec![]
            }
            Event::CommentEdited {
                session,
                comment,
                text,
            } => {
                self.with_review(&session, |review| review.edit_comment(comment, text));
                vec![]
            }
            Event::CommentDeleted { session, comment } => {
                self.with_review(&session, |review| review.delete_comment(comment));
                vec![]
            }
            Event::ReviewSubmitted { session } => self.submit_review(session),
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

    /// The inputs a Review Pane draws from the ports: the worktree's
    /// current changeset, the commit-range key it sits on, and the stored
    /// Annotations for that range — with anything still sitting in the
    /// adapter drained in first.
    fn load_review_inputs(
        &mut self,
        session: &AuthoringSessionId,
        worktree: &AgentWorktree,
    ) -> Result<(Changeset, String, Vec<Annotation>), String> {
        let changeset = self.git.changeset_diff(worktree)?;
        let base = self.git.changeset_base(worktree)?;
        self.drain_annotations(session, worktree, &base);
        let annotations = self
            .annotations
            .get(&worktree.id)
            .and_then(|ranges| ranges.get(&base))
            .cloned()
            .unwrap_or_default();
        Ok((changeset, base, annotations))
    }

    fn open_review(&mut self, session: AuthoringSessionId) -> Vec<Effect> {
        let Some(worktree) = self.worktrees.get(&session).cloned() else {
            return vec![Effect::ReviewOpenFailed {
                session,
                message: "no Agent Worktree is bound to this session".to_string(),
            }];
        };
        let (changeset, base, annotations) = match self.load_review_inputs(&session, &worktree) {
            Ok(inputs) => inputs,
            Err(message) => return vec![Effect::ReviewOpenFailed { session, message }],
        };
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

    /// Deliver a conversation question, resolving the routing on the first
    /// ask (ADR-0003): the live Authoring Session when the AgentPort says
    /// it is alive, else a fresh session spawned seeded with the stored
    /// Annotation and hunk diff. Once routed, follow-ups stay on the same
    /// session.
    fn ask_conversation(&mut self, session: AuthoringSessionId, question: String) -> Vec<Effect> {
        // Split borrow: the review is mutated while the AgentPort routes.
        let Self { agent, reviews, .. } = self;
        let Some((_, review)) = reviews.iter_mut().find(|(s, _)| *s == session) else {
            return vec![];
        };
        let Some(seed) = review.conversation_seed() else {
            return vec![];
        };
        let Some(conversation) = review.conversation.as_mut() else {
            return vec![];
        };
        let resolved = match conversation.target.clone() {
            Some(target) => Ok(target),
            None if agent.session_is_live(&session) => {
                Ok((session.clone(), ConversationRouting::LiveAuthoringSession))
            }
            None => agent
                .spawn_seeded_session(&seed)
                .map(|id| (id, ConversationRouting::SeededSession)),
        };
        let (target, routing) = match resolved {
            Ok(target) => target,
            Err(message) => {
                // The failure joins the transcript so the pane shows what
                // was asked and why it went unanswered.
                conversation
                    .entries
                    .push(ConversationEntry::Question(question));
                conversation
                    .entries
                    .push(ConversationEntry::Answer(format!("(no answer: {message})")));
                return vec![Effect::ConversationFailed { session, message }];
            }
        };
        conversation.target = Some((target.clone(), routing));
        // Anchor with the full hunk diff: a live session may have compacted
        // the hunk out of its context, and the header alone won't place it.
        let prompt = format!(
            "The reviewer is asking about this hunk of {}:\n{}\n\n{question}",
            seed.file, seed.diff
        );
        agent.deliver_instructions(&target, &prompt);
        conversation
            .entries
            .push(ConversationEntry::Question(question));
        conversation.awaiting_answer = true;
        vec![Effect::ConversationRouted {
            session,
            target,
            routing,
        }]
    }

    /// Batch the review's drafted Fix-it Comments into one structured
    /// instruction set and deliver it to the Agent. The session drops out
    /// of Ready-for-Review: it is working again. No-op without an open
    /// review or without drafts.
    fn submit_review(&mut self, session: AuthoringSessionId) -> Vec<Effect> {
        let Some((_, review)) = self.reviews.iter_mut().find(|(s, _)| *s == session) else {
            return vec![];
        };
        let Some(instructions) = review.take_review_instructions() else {
            return vec![];
        };
        self.agent.deliver_instructions(&session, &instructions);
        self.model.ready_for_review.retain(|s| *s != session);
        vec![Effect::ReviewDelivered { session }]
    }

    /// Re-fetch the diff for the session's open Review Pane after the
    /// agent finished acting (e.g. on a submitted Review). Reviewed-marks
    /// and remaining drafts survive wherever hunk content is unchanged. A
    /// failure keeps the pane on the last good changeset.
    fn refresh_review(&mut self, session: &AuthoringSessionId) -> Vec<Effect> {
        let Some((_, review)) = self.reviews.iter().find(|(s, _)| s == session) else {
            return vec![];
        };
        let worktree = review.worktree.clone();
        let (changeset, base, annotations) = match self.load_review_inputs(session, &worktree) {
            Ok(inputs) => inputs,
            Err(message) => {
                return vec![Effect::ReviewRefreshFailed {
                    session: session.clone(),
                    message,
                }]
            }
        };
        // Split borrow: the review mutates while the marks map is entered.
        let Self { reviews, marks, .. } = self;
        if let Some((_, review)) = reviews.iter_mut().find(|(s, _)| s == session) {
            let marks = marks.entry(worktree.id.clone()).or_default();
            review.refresh(changeset, base, annotations, marks);
        }
        vec![]
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
