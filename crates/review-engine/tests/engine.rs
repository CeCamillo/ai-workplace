//! End-to-end engine test at the one sanctioned seam: the engine's public
//! API. Events go in, effects come out; all three ports are fakes. No PTY,
//! no TUI, no real git.

use review_engine::fakes::{FakeAgentPort, FakeGitPort, FakeUiPort};
use review_engine::{AgentPort, AuthoringSessionId, Effect, Engine, Event, GitPort};

#[test]
fn agent_finished_event_produces_ready_for_review_effect() {
    let mut engine = Engine::new(FakeAgentPort::new(), FakeGitPort::new(), FakeUiPort::new());
    let session = AuthoringSessionId::from("session-1");

    let effects = engine.handle_event(Event::AgentFinished {
        session: session.clone(),
    });

    assert_eq!(effects, vec![Effect::ReadyForReview { session }]);
}

// Proves the acceptance criterion that all three fakes are usable from
// tests, ahead of the loop stages that will drive them through the engine.
#[test]
fn fakes_are_scriptable_from_tests() {
    let session = AuthoringSessionId::from("session-1");

    let mut agent = FakeAgentPort::new();
    assert!(!agent.session_is_live(&session));
    agent.set_live(session.clone());
    assert!(agent.session_is_live(&session));
    agent.deliver_instructions(&session, "fix the null check");
    assert_eq!(
        agent.delivered(),
        [(session.clone(), "fix the null check".to_string())]
    );

    let mut git = FakeGitPort::new();
    let worktree = git.create_worktree(&session);
    assert_eq!(git.created(), [worktree]);
}
