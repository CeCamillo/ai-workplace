use crate::event::{Effect, Event};
use crate::ports::{AgentPort, GitPort, RenderModel, UiPort};

/// The event-in/effect-out core of the Agent Review Loop.
pub struct Engine<A: AgentPort, G: GitPort, U: UiPort> {
    // Unused until later loop stages (Hunk Conversations, worktree
    // lifecycle) land behind this seam.
    #[allow(dead_code)]
    agent: A,
    #[allow(dead_code)]
    git: G,
    ui: U,
    model: RenderModel,
}

impl<A: AgentPort, G: GitPort, U: UiPort> Engine<A, G, U> {
    pub fn new(agent: A, git: G, ui: U) -> Self {
        Self {
            agent,
            git,
            ui,
            model: RenderModel::default(),
        }
    }

    /// Advance the loop: apply one event, re-render, and return the effects
    /// the host must carry out.
    pub fn handle_event(&mut self, event: Event) -> Vec<Effect> {
        let effects = match event {
            Event::AgentFinished { session } => {
                self.model.ready_for_review.push(session.clone());
                vec![Effect::ReadyForReview { session }]
            }
        };
        self.ui.render(&self.model);
        effects
    }
}
