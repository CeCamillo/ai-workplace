use std::collections::HashMap;

use crate::event::{Effect, Event, WorktreeSpawn};
use crate::ids::AuthoringSessionId;
use crate::ports::{AgentPort, GitPort, RenderModel, UiPort};
use crate::worktree::AgentWorktree;

/// The event-in/effect-out core of the Agent Review Loop.
pub struct Engine<A: AgentPort, G: GitPort, U: UiPort> {
    // Unused until later loop stages (Hunk Conversations, batched Reviews)
    // land behind this seam.
    #[allow(dead_code)]
    agent: A,
    git: G,
    ui: U,
    model: RenderModel,
    /// Each session's Agent Worktree, exact by construction (ADR-0004).
    worktrees: HashMap<AuthoringSessionId, AgentWorktree>,
}

impl<A: AgentPort, G: GitPort, U: UiPort> Engine<A, G, U> {
    pub fn new(agent: A, git: G, ui: U) -> Self {
        Self {
            agent,
            git,
            ui,
            model: RenderModel::default(),
            worktrees: HashMap::new(),
        }
    }

    pub fn git(&self) -> &G {
        &self.git
    }

    pub fn ui(&self) -> &U {
        &self.ui
    }

    /// Advance the loop: apply one event, re-render, and return the effects
    /// the host must carry out.
    pub fn handle_event(&mut self, event: Event) -> Vec<Effect> {
        let effects = match event {
            Event::AgentPaneSpawned { session, worktree } => {
                self.bind_worktree(session, worktree)
            }
            Event::AgentFinished { session } => {
                self.model.ready_for_review.push(session.clone());
                vec![Effect::ReadyForReview { session }]
            }
            Event::ChangesetDiscarded { session } => self.discard_changeset(session),
        };
        self.ui.render(&self.model);
        effects
    }

    fn bind_worktree(
        &mut self,
        session: AuthoringSessionId,
        spawn: WorktreeSpawn,
    ) -> Vec<Effect> {
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
                vec![Effect::WorktreeDiscarded {
                    session,
                    worktree: worktree.id,
                }]
            }
            Err(message) => vec![Effect::WorktreeOperationFailed { session, message }],
        }
    }
}
