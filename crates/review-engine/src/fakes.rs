//! Fake port implementations for engine tests: a scriptable agent, an
//! in-memory git, and a render-model capture.

use std::collections::HashSet;

use crate::ids::{AgentWorktreeId, AuthoringSessionId};
use crate::ports::{AgentPort, GitPort, RenderModel, UiPort};

/// Scriptable [`AgentPort`]: liveness is set by the test, delivered
/// instructions are recorded for assertion.
#[derive(Debug, Default)]
pub struct FakeAgentPort {
    live: HashSet<AuthoringSessionId>,
    delivered: Vec<(AuthoringSessionId, String)>,
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
}

impl AgentPort for FakeAgentPort {
    fn session_is_live(&self, session: &AuthoringSessionId) -> bool {
        self.live.contains(session)
    }

    fn deliver_instructions(&mut self, session: &AuthoringSessionId, instructions: &str) {
        self.delivered
            .push((session.clone(), instructions.to_string()));
    }
}

/// In-memory [`GitPort`]: worktree ids are derived from the session id, no
/// filesystem or real git involved.
#[derive(Debug, Default)]
pub struct FakeGitPort {
    created: Vec<AgentWorktreeId>,
}

impl FakeGitPort {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn created(&self) -> &[AgentWorktreeId] {
        &self.created
    }
}

impl GitPort for FakeGitPort {
    fn create_worktree(&mut self, session: &AuthoringSessionId) -> AgentWorktreeId {
        let worktree = AgentWorktreeId(format!("worktree-{}", session.0));
        self.created.push(worktree.clone());
        worktree
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
