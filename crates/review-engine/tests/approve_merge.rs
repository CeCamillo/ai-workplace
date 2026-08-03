//! Approve — the exit of the Agent Review Loop — at the engine seam: the
//! human approves, the engine instructs the Agent to commit on its Agent
//! Worktree branch (the agent authors the message), then offers a
//! one-keypress merge to the default branch. Conflicts go back to the
//! agent; declining leaves the branch; the worktree is cleaned up after a
//! completed merge. All ports are faked.

use review_engine::fakes::{FakeAgentPort, FakeGitPort, FakeUiPort};
use review_engine::{
    parse_unified_diff, Annotation, AuthoringSessionId, Effect, Engine, Event, WorktreeSpawn,
};

fn dirty_changeset() -> review_engine::Changeset {
    parse_unified_diff(
        "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,1 +1,2 @@\n context\n+added\n",
    )
}

/// An engine with one spawned pane whose agent has finished: the state
/// Approve starts from.
fn engine_ready_for_review() -> (
    Engine<FakeAgentPort, FakeGitPort, FakeUiPort>,
    AuthoringSessionId,
) {
    let mut git = FakeGitPort::new();
    git.set_changeset(dirty_changeset());
    git.set_changeset_base("base-1");
    let mut engine = Engine::new(FakeAgentPort::new(), git, FakeUiPort::new());
    let session = AuthoringSessionId::from("session-1");
    engine.handle_event(Event::AgentPaneSpawned {
        session: session.clone(),
        worktree: WorktreeSpawn::Create,
    });
    engine.handle_event(Event::AgentFinished {
        session: session.clone(),
    });
    (engine, session)
}

/// Drive `engine_ready_for_review` through approve and the agent's commit,
/// up to the open merge offer.
fn engine_with_merge_offered() -> (
    Engine<FakeAgentPort, FakeGitPort, FakeUiPort>,
    AuthoringSessionId,
) {
    let (mut engine, session) = engine_ready_for_review();
    engine.handle_event(Event::ChangesetApproved {
        session: session.clone(),
    });
    // The agent committed everything: the changeset is empty and the
    // branch head moved.
    engine
        .git_mut()
        .set_changeset(review_engine::Changeset::default());
    engine.git_mut().set_changeset_base("base-2");
    engine.handle_event(Event::AgentFinished {
        session: session.clone(),
    });
    (engine, session)
}

#[test]
fn approving_delivers_a_commit_instruction_and_requests_the_commit() {
    let (mut engine, session) = engine_ready_for_review();

    let effects = engine.handle_event(Event::ChangesetApproved {
        session: session.clone(),
    });

    assert_eq!(
        effects,
        vec![Effect::CommitRequested {
            session: session.clone(),
        }]
    );
    let [(target, instructions)] = engine.agent().delivered() else {
        panic!("expected exactly one delivered instruction set");
    };
    assert_eq!(*target, session);
    assert!(
        instructions.contains("commit") && instructions.contains("agent/session-1"),
        "the instruction must ask for a commit on the worktree branch, got: {instructions}"
    );
    assert!(
        instructions.contains("message"),
        "the agent authors the commit message, got: {instructions}"
    );
    // The agent is working again: the pane leaves Ready-for-Review.
    let last_render = engine.ui().rendered().last().unwrap();
    assert!(!last_render.ready_for_review.contains(&session));
}

#[test]
fn approving_a_session_with_no_worktree_fails_visibly() {
    let mut engine = Engine::new(FakeAgentPort::new(), FakeGitPort::new(), FakeUiPort::new());
    let session = AuthoringSessionId::from("never-spawned");

    let effects = engine.handle_event(Event::ChangesetApproved {
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
fn the_agent_finishing_with_a_clean_changeset_offers_the_merge() {
    let (mut engine, session) = engine_ready_for_review();
    engine.handle_event(Event::ChangesetApproved {
        session: session.clone(),
    });

    engine
        .git_mut()
        .set_changeset(review_engine::Changeset::default());
    engine.git_mut().set_changeset_base("base-2");
    let effects = engine.handle_event(Event::AgentFinished {
        session: session.clone(),
    });

    assert_eq!(
        effects,
        vec![Effect::MergeOffered {
            session: session.clone(),
            branch: "agent/session-1".to_string(),
        }]
    );
    let last_render = engine.ui().rendered().last().unwrap();
    assert!(last_render.merge_offers.contains(&session));
    // The commit ended the review: not Ready-for-Review again.
    assert!(!last_render.ready_for_review.contains(&session));
}

#[test]
fn a_clean_tree_without_a_new_commit_does_not_offer_the_merge() {
    let (mut engine, session) = engine_ready_for_review();
    engine.handle_event(Event::ChangesetApproved {
        session: session.clone(),
    });

    // The tree is clean but the head never moved: the agent wiped its
    // work instead of committing it.
    engine
        .git_mut()
        .set_changeset(review_engine::Changeset::default());
    let effects = engine.handle_event(Event::AgentFinished {
        session: session.clone(),
    });

    assert!(
        effects.iter().any(
            |effect| matches!(effect, Effect::CommitFailed { session: s, .. } if *s == session)
        ),
        "expected CommitFailed, got {effects:?}"
    );
    let last_render = engine.ui().rendered().last().unwrap();
    assert!(!last_render.merge_offers.contains(&session));
}

#[test]
fn the_agent_finishing_with_a_still_dirty_changeset_fails_the_commit_visibly() {
    let (mut engine, session) = engine_ready_for_review();
    engine.handle_event(Event::ChangesetApproved {
        session: session.clone(),
    });

    // The changeset is still dirty: the agent did not commit.
    let effects = engine.handle_event(Event::AgentFinished {
        session: session.clone(),
    });

    assert!(
        effects.iter().any(
            |effect| matches!(effect, Effect::CommitFailed { session: s, .. } if *s == session)
        ),
        "expected CommitFailed, got {effects:?}"
    );
    // The uncommitted changeset is back in the human's court.
    assert!(effects
        .iter()
        .any(|effect| matches!(effect, Effect::ReadyForReview { session: s } if *s == session)));
    let last_render = engine.ui().rendered().last().unwrap();
    assert!(!last_render.merge_offers.contains(&session));
    // The approval is void: accepting a merge now is a no-op.
    let effects = engine.handle_event(Event::MergeAccepted {
        session: session.clone(),
    });
    assert_eq!(effects, vec![]);
    assert_eq!(engine.git().merged(), []);
}

#[test]
fn accepting_the_merge_merges_and_cleans_up_the_worktree() {
    let (mut engine, session) = engine_with_merge_offered();
    let worktree = engine.git().created()[0].clone();

    let effects = engine.handle_event(Event::MergeAccepted {
        session: session.clone(),
    });

    assert_eq!(
        effects,
        vec![Effect::MergeCompleted {
            session: session.clone(),
            worktree: worktree.id.clone(),
        }]
    );
    assert_eq!(engine.git().merged(), std::slice::from_ref(&worktree));
    assert_eq!(engine.git().removed(), std::slice::from_ref(&worktree));
    let last_render = engine.ui().rendered().last().unwrap();
    assert!(!last_render.merge_offers.contains(&session));
    assert!(last_render.reviews.is_empty());
    // The worktree is gone; nothing keyed by it survives.
    assert!(!engine.reviewed_marks_snapshot().contains_key(&worktree.id));
    assert!(!engine.annotations_snapshot().contains_key(&worktree.id));
}

#[test]
fn declining_the_merge_leaves_the_branch_as_is() {
    let (mut engine, session) = engine_with_merge_offered();

    let effects = engine.handle_event(Event::MergeDeclined {
        session: session.clone(),
    });

    assert_eq!(effects, vec![]);
    assert_eq!(engine.git().merged(), []);
    assert_eq!(engine.git().removed(), []);
    let last_render = engine.ui().rendered().last().unwrap();
    assert!(!last_render.merge_offers.contains(&session));
    // The offer is closed: a later accept without a new approval is a no-op.
    let effects = engine.handle_event(Event::MergeAccepted {
        session: session.clone(),
    });
    assert_eq!(effects, vec![]);
    assert_eq!(engine.git().merged(), []);
}

#[test]
fn merge_conflicts_are_handed_to_the_agent_and_resolution_completes_the_merge() {
    let (mut engine, session) = engine_with_merge_offered();
    engine
        .git_mut()
        .script_merge_conflicts(vec!["src/lib.rs".to_string(), "src/main.rs".to_string()]);

    let effects = engine.handle_event(Event::MergeAccepted {
        session: session.clone(),
    });

    assert_eq!(
        effects,
        vec![Effect::MergeConflictsHandedToAgent {
            session: session.clone(),
            files: vec!["src/lib.rs".to_string(), "src/main.rs".to_string()],
        }]
    );
    let (target, instructions) = engine.agent().delivered().last().unwrap().clone();
    assert_eq!(target, session);
    assert!(
        instructions.contains("conflict")
            && instructions.contains("src/lib.rs")
            && instructions.contains("src/main.rs"),
        "the conflict hand-off must name the conflicted files, got: {instructions}"
    );
    assert_eq!(engine.git().merged(), []);

    // The agent resolved the conflicts and finished: the merge completes.
    let worktree = engine.git().created()[0].clone();
    let effects = engine.handle_event(Event::AgentFinished {
        session: session.clone(),
    });
    assert_eq!(
        effects,
        vec![Effect::MergeCompleted {
            session: session.clone(),
            worktree: worktree.id.clone(),
        }]
    );
    assert_eq!(engine.git().merged(), std::slice::from_ref(&worktree));
    assert_eq!(engine.git().removed(), std::slice::from_ref(&worktree));
}

#[test]
fn a_failing_merge_surfaces_and_keeps_the_offer_open() {
    let (mut engine, session) = engine_with_merge_offered();
    engine.git_mut().fail_next("merge exploded".to_string());

    let effects = engine.handle_event(Event::MergeAccepted {
        session: session.clone(),
    });

    assert_eq!(
        effects,
        vec![Effect::MergeFailed {
            session: session.clone(),
            message: "merge exploded".to_string(),
        }]
    );
    let last_render = engine.ui().rendered().last().unwrap();
    assert!(
        last_render.merge_offers.contains(&session),
        "a failed merge keeps the offer open for a retry"
    );

    // The retry succeeds.
    let effects = engine.handle_event(Event::MergeAccepted {
        session: session.clone(),
    });
    assert!(effects
        .iter()
        .any(|effect| matches!(effect, Effect::MergeCompleted { .. })));
}

/// The acceptance path of the whole Agent Review Loop as one pure engine
/// test: spawn → annotate → review → fix-it → submit → approve → commit →
/// merge, with no PTY, TUI, or real git anywhere.
#[test]
fn the_full_agent_review_loop_runs_as_a_pure_engine_test() {
    let mut git = FakeGitPort::new();
    git.set_changeset(dirty_changeset());
    git.set_changeset_base("base-1");
    let mut engine = Engine::new(FakeAgentPort::new(), git, FakeUiPort::new());
    let session = AuthoringSessionId::from("session-1");

    // Spawn: the pane gets its Agent Worktree.
    let effects = engine.handle_event(Event::AgentPaneSpawned {
        session: session.clone(),
        worktree: WorktreeSpawn::Create,
    });
    assert!(matches!(effects[0], Effect::PaneWorktreeBound { .. }));
    let worktree = engine.git().created()[0].clone();

    // The agent works, annotating as it goes, then finishes.
    engine.agent_mut().push_annotation(
        &session,
        Annotation {
            file: "src/lib.rs".to_string(),
            line: 2,
            what: "add a line".to_string(),
            why: "the reviewer asked".to_string(),
        },
    );
    let effects = engine.handle_event(Event::AgentFinished {
        session: session.clone(),
    });
    assert!(matches!(effects[0], Effect::ReadyForReview { .. }));

    // The human reviews and sends one Fix-it Comment as a Review.
    engine.handle_event(Event::ReviewRequested {
        session: session.clone(),
    });
    engine.handle_event(Event::ReviewViewportResized {
        session: session.clone(),
        rows: 20,
    });
    engine.handle_event(Event::ReviewNavigated {
        session: session.clone(),
        motion: review_engine::ReviewMotion::NextHunk,
    });
    engine.handle_event(Event::CommentDrafted {
        session: session.clone(),
        text: "rename the new symbol".to_string(),
    });
    let effects = engine.handle_event(Event::ReviewSubmitted {
        session: session.clone(),
    });
    assert_eq!(
        effects,
        vec![Effect::ReviewDelivered {
            session: session.clone(),
        }]
    );

    // The agent applies the Review and finishes; the pane refreshes.
    let effects = engine.handle_event(Event::AgentFinished {
        session: session.clone(),
    });
    assert!(matches!(effects[0], Effect::ReadyForReview { .. }));

    // Approve: the agent is told to commit and does.
    let effects = engine.handle_event(Event::ChangesetApproved {
        session: session.clone(),
    });
    assert_eq!(
        effects,
        vec![Effect::CommitRequested {
            session: session.clone(),
        }]
    );
    engine
        .git_mut()
        .set_changeset(review_engine::Changeset::default());
    engine.git_mut().set_changeset_base("base-2");
    let effects = engine.handle_event(Event::AgentFinished {
        session: session.clone(),
    });
    assert_eq!(
        effects,
        vec![Effect::MergeOffered {
            session: session.clone(),
            branch: worktree.branch.clone(),
        }]
    );

    // One keypress: the merge lands and the worktree is cleaned up.
    let effects = engine.handle_event(Event::MergeAccepted {
        session: session.clone(),
    });
    assert_eq!(
        effects,
        vec![Effect::MergeCompleted {
            session: session.clone(),
            worktree: worktree.id.clone(),
        }]
    );
    assert_eq!(engine.git().merged(), std::slice::from_ref(&worktree));
    assert_eq!(engine.git().removed(), std::slice::from_ref(&worktree));

    // The loop is closed: every delivery in order — the Review, then the
    // commit instruction, and nothing after the merge.
    let delivered = engine.agent().delivered();
    assert_eq!(delivered.len(), 2);
    assert!(delivered[0].1.contains("rename the new symbol"));
    assert!(delivered[1].1.contains("commit"));
    let last_render = engine.ui().rendered().last().unwrap();
    assert!(last_render.reviews.is_empty());
    assert!(last_render.merge_offers.is_empty());
    assert!(last_render.ready_for_review.is_empty());
}
