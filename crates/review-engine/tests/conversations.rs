//! Hunk Conversations at the engine seam (ADR-0003): a question on a hunk
//! routes to the live Authoring Session when the AgentPort says it is
//! alive, and otherwise to a fresh session the engine asks the port to
//! spawn, seeded with the stored Annotation and the hunk's diff. Answers
//! come back as events and render in the conversation view, anchored to
//! the hunk.

use review_engine::fakes::{FakeAgentPort, FakeGitPort, FakeUiPort};
use review_engine::{
    Annotation, AuthoringSessionId, Changeset, ConversationEntry, ConversationRouting, DiffLine,
    DiffLineKind, Effect, Engine, Event, FileDiff, Hunk, ReviewMotion, ReviewView, WorktreeSpawn,
};

fn context(content: &str) -> DiffLine {
    DiffLine {
        kind: DiffLineKind::Context,
        content: content.to_string(),
    }
}

fn added(content: &str) -> DiffLine {
    DiffLine {
        kind: DiffLineKind::Added,
        content: content.to_string(),
    }
}

fn sample_changeset() -> Changeset {
    Changeset {
        files: vec![FileDiff {
            path: "src/lib.rs".to_string(),
            hunks: vec![
                Hunk {
                    header: "@@ -1,3 +1,4 @@".to_string(),
                    lines: vec![context("mod a;"), added("mod b;")],
                },
                Hunk {
                    header: "@@ -10,2 +11,2 @@".to_string(),
                    lines: vec![added("new()")],
                },
            ],
        }],
    }
}

fn engine_for(session: &AuthoringSessionId) -> Engine<FakeAgentPort, FakeGitPort, FakeUiPort> {
    let mut git = FakeGitPort::new();
    git.set_changeset(sample_changeset());
    let mut engine = Engine::new(FakeAgentPort::new(), git, FakeUiPort::new());
    engine.handle_event(Event::AgentPaneSpawned {
        session: session.clone(),
        worktree: WorktreeSpawn::Create,
    });
    engine
}

fn current_view(engine: &Engine<FakeAgentPort, FakeGitPort, FakeUiPort>) -> ReviewView {
    engine
        .ui()
        .rendered()
        .last()
        .unwrap()
        .reviews
        .last()
        .unwrap()
        .clone()
}

/// Open the review and put the cursor on the first hunk's header.
fn open_on_first_hunk(
    engine: &mut Engine<FakeAgentPort, FakeGitPort, FakeUiPort>,
    session: &AuthoringSessionId,
) {
    engine.handle_event(Event::ReviewRequested {
        session: session.clone(),
    });
    engine.handle_event(Event::ReviewNavigated {
        session: session.clone(),
        motion: ReviewMotion::NextHunk,
    });
}

#[test]
fn a_question_on_a_hunk_reaches_the_live_authoring_session() {
    let session = AuthoringSessionId::from("session-1");
    let mut engine = engine_for(&session);
    engine.agent_mut().set_live(session.clone());
    open_on_first_hunk(&mut engine, &session);

    engine.handle_event(Event::ConversationOpened {
        session: session.clone(),
    });
    let effects = engine.handle_event(Event::ConversationAsked {
        session: session.clone(),
        question: "why this way?".to_string(),
    });

    assert_eq!(
        effects,
        vec![Effect::ConversationRouted {
            session: session.clone(),
            target: session.clone(),
            routing: ConversationRouting::LiveAuthoringSession,
        }]
    );
    assert!(
        engine.agent().spawned().is_empty(),
        "a live Authoring Session must not be replaced by a seeded one"
    );
    let delivered = engine.agent().delivered();
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].0, session);
    let prompt = &delivered[0].1;
    assert!(prompt.contains("why this way?"));
    assert!(
        prompt.contains("src/lib.rs") && prompt.contains("@@ -1,3 +1,4 @@"),
        "the question must be anchored to the hunk, got: {prompt}"
    );

    let conversation = current_view(&engine).conversation.unwrap();
    assert_eq!(conversation.file, "src/lib.rs");
    assert_eq!(conversation.hunk, 0);
    assert_eq!(
        conversation.routing,
        Some(ConversationRouting::LiveAuthoringSession)
    );
    assert_eq!(
        conversation.entries,
        vec![ConversationEntry::Question("why this way?".to_string())]
    );
    assert!(conversation.awaiting_answer);
}

#[test]
fn with_the_authoring_session_gone_a_fresh_seeded_session_answers_instead() {
    let session = AuthoringSessionId::from("session-1");
    let mut engine = engine_for(&session);
    engine.agent_mut().push_annotation(
        &session,
        Annotation {
            file: "src/lib.rs".to_string(),
            line: 2,
            what: "declare module b".to_string(),
            why: "new parser module".to_string(),
        },
    );
    open_on_first_hunk(&mut engine, &session);

    engine.handle_event(Event::ConversationOpened {
        session: session.clone(),
    });
    let effects = engine.handle_event(Event::ConversationAsked {
        session: session.clone(),
        question: "why this way?".to_string(),
    });

    let seeded = AuthoringSessionId::from("seeded-1");
    assert_eq!(
        effects,
        vec![Effect::ConversationRouted {
            session: session.clone(),
            target: seeded.clone(),
            routing: ConversationRouting::SeededSession,
        }]
    );
    let seeds = engine.agent().spawned();
    assert_eq!(seeds.len(), 1);
    assert_eq!(seeds[0].file, "src/lib.rs");
    assert_eq!(seeds[0].hunk_header, "@@ -1,3 +1,4 @@");
    assert!(
        seeds[0].diff.contains("+mod b;"),
        "the seed must carry the hunk's diff, got: {}",
        seeds[0].diff
    );
    assert_eq!(
        seeds[0].annotations,
        vec![Annotation {
            file: "src/lib.rs".to_string(),
            line: 2,
            what: "declare module b".to_string(),
            why: "new parser module".to_string(),
        }],
        "the seed must carry the stored Annotation for the hunk"
    );
    let delivered = engine.agent().delivered();
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].0, seeded);
    assert!(delivered[0].1.contains("why this way?"));

    let conversation = current_view(&engine).conversation.unwrap();
    assert_eq!(
        conversation.routing,
        Some(ConversationRouting::SeededSession)
    );
}

#[test]
fn answers_render_in_pane_and_follow_ups_stay_on_the_routed_session() {
    let session = AuthoringSessionId::from("session-1");
    let mut engine = engine_for(&session);
    open_on_first_hunk(&mut engine, &session);
    engine.handle_event(Event::ConversationOpened {
        session: session.clone(),
    });
    engine.handle_event(Event::ConversationAsked {
        session: session.clone(),
        question: "why this way?".to_string(),
    });
    engine.handle_event(Event::ConversationAnswerReceived {
        session: session.clone(),
        target: AuthoringSessionId::from("seeded-1"),
        answer: "the old API is deprecated".to_string(),
    });

    let conversation = current_view(&engine).conversation.unwrap();
    assert_eq!(
        conversation.entries,
        vec![
            ConversationEntry::Question("why this way?".to_string()),
            ConversationEntry::Answer("the old API is deprecated".to_string()),
        ]
    );
    assert!(!conversation.awaiting_answer);

    // A follow-up keeps talking to the same seeded session instead of
    // spawning another.
    engine.handle_event(Event::ConversationAsked {
        session: session.clone(),
        question: "any trade-off?".to_string(),
    });
    assert_eq!(engine.agent().spawned().len(), 1);
    let delivered = engine.agent().delivered();
    assert_eq!(delivered.len(), 2);
    assert_eq!(delivered[1].0, AuthoringSessionId::from("seeded-1"));
}

#[test]
fn a_conversation_only_opens_on_a_hunk() {
    let session = AuthoringSessionId::from("session-1");
    let mut engine = engine_for(&session);
    // Cursor stays on the file header row: nothing to anchor to.
    engine.handle_event(Event::ReviewRequested {
        session: session.clone(),
    });

    engine.handle_event(Event::ConversationOpened {
        session: session.clone(),
    });
    assert!(current_view(&engine).conversation.is_none());

    // Asking without an open conversation goes nowhere.
    let effects = engine.handle_event(Event::ConversationAsked {
        session: session.clone(),
        question: "why?".to_string(),
    });
    assert!(effects.is_empty());
    assert!(engine.agent().delivered().is_empty());
}

#[test]
fn closing_a_conversation_removes_it_from_the_view() {
    let session = AuthoringSessionId::from("session-1");
    let mut engine = engine_for(&session);
    open_on_first_hunk(&mut engine, &session);
    engine.handle_event(Event::ConversationOpened {
        session: session.clone(),
    });
    assert!(current_view(&engine).conversation.is_some());

    engine.handle_event(Event::ConversationClosed {
        session: session.clone(),
    });
    assert!(current_view(&engine).conversation.is_none());
}

#[test]
fn a_failed_seeded_spawn_surfaces_as_an_effect() {
    let session = AuthoringSessionId::from("session-1");
    let mut engine = engine_for(&session);
    open_on_first_hunk(&mut engine, &session);
    engine.handle_event(Event::ConversationOpened {
        session: session.clone(),
    });
    engine
        .agent_mut()
        .fail_next_spawn("claude is not installed".to_string());

    let effects = engine.handle_event(Event::ConversationAsked {
        session: session.clone(),
        question: "why?".to_string(),
    });

    assert_eq!(
        effects,
        vec![Effect::ConversationFailed {
            session: session.clone(),
            message: "claude is not installed".to_string(),
        }]
    );
    assert!(engine.agent().delivered().is_empty());
    // The failure shows up in the transcript, next to what was asked.
    let conversation = current_view(&engine).conversation.unwrap();
    assert_eq!(
        conversation.entries,
        vec![
            ConversationEntry::Question("why?".to_string()),
            ConversationEntry::Answer("(no answer: claude is not installed)".to_string()),
        ]
    );
    assert!(!conversation.awaiting_answer);
}

#[test]
fn an_answer_owed_to_an_abandoned_conversation_never_reaches_another_hunk() {
    let session = AuthoringSessionId::from("session-1");
    let mut engine = engine_for(&session);
    // Both conversations route to the same live Authoring Session — the
    // hardest case to tell apart.
    engine.agent_mut().set_live(session.clone());
    open_on_first_hunk(&mut engine, &session);

    engine.handle_event(Event::ConversationOpened {
        session: session.clone(),
    });
    engine.handle_event(Event::ConversationAsked {
        session: session.clone(),
        question: "why hunk one?".to_string(),
    });
    // Abandon the conversation while its answer is still in flight, and
    // start over on the next hunk.
    engine.handle_event(Event::ConversationClosed {
        session: session.clone(),
    });
    engine.handle_event(Event::ReviewNavigated {
        session: session.clone(),
        motion: ReviewMotion::NextHunk,
    });
    engine.handle_event(Event::ConversationOpened {
        session: session.clone(),
    });
    engine.handle_event(Event::ConversationAsked {
        session: session.clone(),
        question: "why hunk two?".to_string(),
    });

    // The stale answer arrives first: it must be dropped, not attached.
    engine.handle_event(Event::ConversationAnswerReceived {
        session: session.clone(),
        target: session.clone(),
        answer: "about hunk one".to_string(),
    });
    let conversation = current_view(&engine).conversation.unwrap();
    assert_eq!(conversation.hunk, 1);
    assert_eq!(
        conversation.entries,
        vec![ConversationEntry::Question("why hunk two?".to_string())]
    );
    assert!(conversation.awaiting_answer);

    // The answer actually owed to this conversation still lands.
    engine.handle_event(Event::ConversationAnswerReceived {
        session: session.clone(),
        target: session.clone(),
        answer: "about hunk two".to_string(),
    });
    let conversation = current_view(&engine).conversation.unwrap();
    assert_eq!(
        conversation.entries,
        vec![
            ConversationEntry::Question("why hunk two?".to_string()),
            ConversationEntry::Answer("about hunk two".to_string()),
        ]
    );
    assert!(!conversation.awaiting_answer);
}
