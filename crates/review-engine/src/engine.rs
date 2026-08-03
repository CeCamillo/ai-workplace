use std::collections::HashMap;

use crate::annotation::{Annotation, AnnotationsSnapshot};
use crate::conversation::{ConversationEntry, ConversationRouting};
use crate::diff::Changeset;
use crate::event::{Effect, Event, WorktreeSpawn};
use crate::ids::AuthoringSessionId;
use crate::ports::{AgentPort, GitPort, MergeOutcome, RenderModel, UiPort};
use crate::review::{MarkTarget, ReviewState, ReviewedMarksSnapshot};
use crate::worktree::AgentWorktree;

/// Where a session's Approve stands. Approve is the loop's exit: commit
/// instruction out → commit lands → merge offer → merged (with conflict
/// hand-offs in between) or declined.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ApprovalPhase {
    /// The commit instruction is with the agent; the commit lands when the
    /// agent next finishes. `base` is the branch head at approval, so a
    /// landed commit is observable — not inferred from a clean tree alone.
    AwaitingCommit { base: String },
    /// The changeset is committed; the one-keypress merge offer is open.
    MergeOffered,
    /// The merge conflicted and the conflicts are with the agent; the merge
    /// retries when the agent next finishes.
    AwaitingConflictResolution,
}

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
    /// Each session's in-flight Approve, if any.
    approvals: HashMap<AuthoringSessionId, ApprovalPhase>,
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
            approvals: HashMap::new(),
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
            Event::AgentFinished { session } => self.agent_finished(session),
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
            Event::ChangesetApproved { session } => self.approve_changeset(session),
            Event::MergeAccepted { session } => {
                if self.approvals.get(&session) == Some(&ApprovalPhase::MergeOffered) {
                    self.attempt_merge(session)
                } else {
                    vec![]
                }
            }
            Event::MergeDeclined { session } => {
                if self.approvals.get(&session) == Some(&ApprovalPhase::MergeOffered) {
                    self.approvals.remove(&session);
                    self.model.merge_offers.retain(|s| *s != session);
                }
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
        let worktree = match self.bound_worktree(&session) {
            Ok(worktree) => worktree,
            Err(effect) => return vec![effect],
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

    /// The session's bound Agent Worktree, or the failure effect every
    /// worktree-scoped event surfaces without one.
    fn bound_worktree(&self, session: &AuthoringSessionId) -> Result<AgentWorktree, Effect> {
        self.worktrees
            .get(session)
            .cloned()
            .ok_or_else(|| Effect::WorktreeOperationFailed {
                session: session.clone(),
                message: "no Agent Worktree is bound to this session".to_string(),
            })
    }

    fn discard_changeset(&mut self, session: AuthoringSessionId) -> Vec<Effect> {
        let worktree = match self.bound_worktree(&session) {
            Ok(worktree) => worktree,
            Err(effect) => return vec![effect],
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

    /// The agent finished a turn. In an Approve, that turn is the loop
    /// advancing — the commit landing or conflicts resolved; otherwise it is
    /// work waiting for a human: Ready-for-Review, refreshing an open pane.
    fn agent_finished(&mut self, session: AuthoringSessionId) -> Vec<Effect> {
        match self.approvals.get(&session).cloned() {
            Some(ApprovalPhase::AwaitingCommit { base }) => {
                self.commit_turn_finished(session, base)
            }
            Some(ApprovalPhase::AwaitingConflictResolution) => self.attempt_merge(session),
            _ => {
                self.model.ready_for_review.push(session.clone());
                let mut effects = vec![Effect::ReadyForReview {
                    session: session.clone(),
                }];
                effects.extend(self.refresh_review(&session));
                effects
            }
        }
    }

    /// The human Approved: instruct the Agent to commit the changeset on
    /// its Agent Worktree branch, authoring the commit message itself. The
    /// session is working again until the commit lands.
    fn approve_changeset(&mut self, session: AuthoringSessionId) -> Vec<Effect> {
        let worktree = match self.bound_worktree(&session) {
            Ok(worktree) => worktree,
            Err(effect) => return vec![effect],
        };
        // The head the commit must move off; unresolvable means the
        // approval cannot be verified later, so it fails now.
        let base = match self.git.changeset_base(&worktree) {
            Ok(base) => base,
            Err(message) => return vec![Effect::WorktreeOperationFailed { session, message }],
        };
        let instructions = format!(
            "The reviewer approved your changeset. Commit everything in your \
             worktree on branch '{}' now, in one commit whose message you \
             author: summarize what the changeset does and why. Do not push.",
            worktree.branch
        );
        self.agent.deliver_instructions(&session, &instructions);
        self.approvals
            .insert(session.clone(), ApprovalPhase::AwaitingCommit { base });
        self.model.ready_for_review.retain(|s| *s != session);
        vec![Effect::CommitRequested { session }]
    }

    /// The agent finished after the commit instruction. The commit landed
    /// when the changeset is clean *and* the branch head moved off `base` —
    /// a clean tree alone could be the agent wiping its work. Then the
    /// merge offer opens; anything else voids the approval and puts the
    /// changeset back in the human's court.
    fn commit_turn_finished(&mut self, session: AuthoringSessionId, base: String) -> Vec<Effect> {
        let worktree = match self.bound_worktree(&session) {
            Ok(worktree) => worktree,
            Err(effect) => {
                self.approvals.remove(&session);
                return vec![effect];
            }
        };
        let committed = match (
            self.git.changeset_diff(&worktree),
            self.git.changeset_base(&worktree),
        ) {
            (Ok(changeset), Ok(head)) => {
                if !changeset.files.is_empty() {
                    Err("the agent finished but the changeset is not committed".to_string())
                } else if head == base {
                    Err("the agent finished but no commit landed on the branch".to_string())
                } else {
                    Ok(())
                }
            }
            (Err(message), _) | (_, Err(message)) => Err(message),
        };
        match committed {
            Ok(()) => {
                self.approvals
                    .insert(session.clone(), ApprovalPhase::MergeOffered);
                self.model.merge_offers.push(session.clone());
                vec![Effect::MergeOffered {
                    session,
                    branch: worktree.branch,
                }]
            }
            Err(message) => {
                self.approvals.remove(&session);
                let mut effects = vec![Effect::CommitFailed {
                    session: session.clone(),
                    message,
                }];
                effects.extend(self.agent_finished(session));
                effects
            }
        }
    }

    /// Run the merge to the default branch: complete the loop on a clean
    /// merge, hand conflicts to the agent, keep the offer open on a
    /// failure. Reached from the accepted offer and again after each
    /// conflict-resolution turn.
    fn attempt_merge(&mut self, session: AuthoringSessionId) -> Vec<Effect> {
        let worktree = match self.bound_worktree(&session) {
            Ok(worktree) => worktree,
            Err(effect) => {
                self.approvals.remove(&session);
                return vec![effect];
            }
        };
        match self.git.merge_into_default(&worktree) {
            Ok(MergeOutcome::Merged) => self.complete_merge(session, worktree),
            Ok(MergeOutcome::Conflicts { files }) => {
                let instructions = format!(
                    "Merging your branch '{}' into the default branch hit \
                     conflicts. A conflicted merge is in progress in your \
                     worktree, in these files:\n{}\nResolve every conflict, \
                     then conclude the merge by committing it. Do not push.",
                    worktree.branch,
                    files
                        .iter()
                        .map(|file| format!("- {file}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                );
                self.agent.deliver_instructions(&session, &instructions);
                self.approvals
                    .insert(session.clone(), ApprovalPhase::AwaitingConflictResolution);
                self.model.merge_offers.retain(|s| *s != session);
                vec![Effect::MergeConflictsHandedToAgent { session, files }]
            }
            Err(message) => vec![Effect::MergeFailed { session, message }],
        }
    }

    /// The branch is merged: clean up the Agent Worktree and everything
    /// keyed by it — the loop is over for this changeset.
    fn complete_merge(
        &mut self,
        session: AuthoringSessionId,
        worktree: AgentWorktree,
    ) -> Vec<Effect> {
        let mut effects = Vec::new();
        if let Err(message) = self.git.remove_worktree(&worktree) {
            effects.push(Effect::WorktreeOperationFailed {
                session: session.clone(),
                message,
            });
        }
        self.approvals.remove(&session);
        self.worktrees.remove(&session);
        self.reviews.retain(|(s, _)| *s != session);
        self.marks.remove(&worktree.id);
        self.annotations.remove(&worktree.id);
        self.model.ready_for_review.retain(|s| *s != session);
        self.model.merge_offers.retain(|s| *s != session);
        effects.push(Effect::MergeCompleted {
            session,
            worktree: worktree.id,
        });
        effects
    }
}
