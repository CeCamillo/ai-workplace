//! Agent Worktree lifecycle at the engine seam (ADR-0004): spawning an
//! agent pane creates or adopts a dedicated worktree + branch, discarding a
//! changeset resets it cleanly. All git happens through a faked [`GitPort`].

use review_engine::fakes::{FakeAgentPort, FakeGitPort, FakeUiPort};
use review_engine::{
    AgentWorktree, AgentWorktreeId, AuthoringSessionId, Effect, Engine, Event, WorktreeSpawn,
};

fn engine_with_git(
    git: FakeGitPort,
) -> Engine<FakeAgentPort, FakeGitPort, FakeUiPort> {
    Engine::new(FakeAgentPort::new(), git, FakeUiPort::new())
}

#[test]
fn spawning_a_pane_creates_a_worktree_and_binds_the_pane_to_it() {
    let mut engine = engine_with_git(FakeGitPort::new());
    let session = AuthoringSessionId::from("session-1");

    let effects = engine.handle_event(Event::AgentPaneSpawned {
        session: session.clone(),
        worktree: WorktreeSpawn::Create,
    });

    let [Effect::PaneWorktreeBound {
        session: bound_session,
        worktree,
    }] = effects.as_slice()
    else {
        panic!("expected exactly PaneWorktreeBound, got {effects:?}");
    };
    assert_eq!(*bound_session, session);
    assert_eq!(worktree.branch, "agent/session-1");
    assert_eq!(engine.git().created(), std::slice::from_ref(worktree));
}

#[test]
fn spawning_can_adopt_an_existing_worktree_instead_of_creating_one() {
    let mut git = FakeGitPort::new();
    let existing = AgentWorktree {
        id: AgentWorktreeId::from("feature/resume-me"),
        branch: "feature/resume-me".to_string(),
        path: "/fake/worktrees/resume-me".into(),
    };
    git.add_existing(existing.clone());
    let mut engine = engine_with_git(git);
    let session = AuthoringSessionId::from("session-1");

    let effects = engine.handle_event(Event::AgentPaneSpawned {
        session: session.clone(),
        worktree: WorktreeSpawn::Adopt {
            branch: "feature/resume-me".to_string(),
        },
    });

    assert_eq!(
        effects,
        vec![Effect::PaneWorktreeBound {
            session,
            worktree: existing,
        }]
    );
    assert_eq!(engine.git().created(), []);
}

#[test]
fn adopting_a_branch_the_git_port_does_not_know_fails_visibly() {
    let mut engine = engine_with_git(FakeGitPort::new());
    let session = AuthoringSessionId::from("session-1");

    let effects = engine.handle_event(Event::AgentPaneSpawned {
        session: session.clone(),
        worktree: WorktreeSpawn::Adopt {
            branch: "no-such-branch".to_string(),
        },
    });

    let [Effect::WorktreeOperationFailed {
        session: failed_session,
        ..
    }] = effects.as_slice()
    else {
        panic!("expected exactly WorktreeOperationFailed, got {effects:?}");
    };
    assert_eq!(*failed_session, session);
}

#[test]
fn discarding_a_changeset_resets_the_worktree_and_clears_ready_for_review() {
    let mut engine = engine_with_git(FakeGitPort::new());
    let session = AuthoringSessionId::from("session-1");

    let spawn_effects = engine.handle_event(Event::AgentPaneSpawned {
        session: session.clone(),
        worktree: WorktreeSpawn::Create,
    });
    let [Effect::PaneWorktreeBound { worktree, .. }] = spawn_effects.as_slice() else {
        panic!("expected PaneWorktreeBound, got {spawn_effects:?}");
    };
    let worktree = worktree.clone();
    engine.handle_event(Event::AgentFinished {
        session: session.clone(),
    });

    let effects = engine.handle_event(Event::ChangesetDiscarded {
        session: session.clone(),
    });

    assert_eq!(
        effects,
        vec![Effect::WorktreeDiscarded {
            session: session.clone(),
            worktree: worktree.id.clone(),
        }]
    );
    assert_eq!(engine.git().discarded(), [worktree]);
    let last_render = engine.ui().rendered().last().unwrap();
    assert!(!last_render.ready_for_review.contains(&session));
}

#[test]
fn discarding_a_session_with_no_worktree_fails_visibly() {
    let mut engine = engine_with_git(FakeGitPort::new());
    let session = AuthoringSessionId::from("never-spawned");

    let effects = engine.handle_event(Event::ChangesetDiscarded {
        session: session.clone(),
    });

    let [Effect::WorktreeOperationFailed {
        session: failed_session,
        ..
    }] = effects.as_slice()
    else {
        panic!("expected exactly WorktreeOperationFailed, got {effects:?}");
    };
    assert_eq!(*failed_session, session);
}

#[test]
fn a_failing_git_operation_surfaces_as_a_failed_effect_not_a_binding() {
    let mut git = FakeGitPort::new();
    git.fail_next("disk on fire".to_string());
    let mut engine = engine_with_git(git);
    let session = AuthoringSessionId::from("session-1");

    let effects = engine.handle_event(Event::AgentPaneSpawned {
        session: session.clone(),
        worktree: WorktreeSpawn::Create,
    });

    assert_eq!(
        effects,
        vec![Effect::WorktreeOperationFailed {
            session,
            message: "disk on fire".to_string(),
        }]
    );
}
