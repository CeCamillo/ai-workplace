//! Annotation capture at the engine seam: the Harness Adapter (faked here
//! as the AgentPort) hands the engine per-hunk Annotations, the engine
//! persists them keyed by Agent Worktree and commit range, and the Review
//! Pane stream renders each one inline above its hunk.

use review_engine::fakes::{FakeAgentPort, FakeGitPort, FakeUiPort};
use review_engine::{
    Annotation, AuthoringSessionId, Changeset, DiffLine, DiffLineKind, Engine, Event, FileDiff,
    Hunk, ReviewView, StreamRow, WorktreeSpawn,
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

/// Two files, three hunks, with headers whose post-image ranges the engine
/// must parse to place Annotations: src/lib.rs covers lines 1-4 and 11-12,
/// src/new.rs covers lines 1-2.
fn sample_changeset() -> Changeset {
    Changeset {
        files: vec![
            FileDiff {
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
            },
            FileDiff {
                path: "src/new.rs".to_string(),
                hunks: vec![Hunk {
                    header: "@@ -0,0 +1,2 @@".to_string(),
                    lines: vec![added("pub fn b() {}"), added("// done")],
                }],
            },
        ],
    }
}

fn annotation(file: &str, line: usize, what: &str, why: &str) -> Annotation {
    Annotation {
        file: file.to_string(),
        line,
        what: what.to_string(),
        why: why.to_string(),
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

fn open_view(
    engine: &mut Engine<FakeAgentPort, FakeGitPort, FakeUiPort>,
    session: &AuthoringSessionId,
) -> ReviewView {
    engine.handle_event(Event::ReviewRequested {
        session: session.clone(),
    });
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

/// The `(what, why)` payloads of Annotation rows sitting directly above the
/// header of hunk `hunk` of file `file`, in stream order.
fn annotations_above_hunk(view: &ReviewView, file: usize, hunk: usize) -> Vec<(String, String)> {
    let header_at = view
        .stream
        .iter()
        .position(|row| {
            matches!(row, StreamRow::HunkHeader { file: f, hunk: h, .. } if *f == file && *h == hunk)
        })
        .expect("hunk header should be in the stream");
    view.stream[..header_at]
        .iter()
        .rev()
        .take_while(|row| matches!(row, StreamRow::Annotation { .. }))
        .filter_map(|row| match row {
            StreamRow::Annotation { what, why, .. } => Some((what.clone(), why.clone())),
            _ => None,
        })
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn total_annotation_rows(view: &ReviewView) -> usize {
    view.stream
        .iter()
        .filter(|row| matches!(row, StreamRow::Annotation { .. }))
        .count()
}

#[test]
fn ingested_annotations_render_inline_above_their_hunks() {
    let session = AuthoringSessionId::from("session-1");
    let mut engine = engine_for(&session);

    engine.agent_mut().push_annotation(
        &session,
        annotation(
            "src/lib.rs",
            11,
            "swap old() for new()",
            "old() is deprecated",
        ),
    );
    engine.agent_mut().push_annotation(
        &session,
        annotation("src/new.rs", 1, "add module b", "splits parsing out of lib"),
    );
    engine.handle_event(Event::AnnotationsEmitted {
        session: session.clone(),
    });

    let view = open_view(&mut engine, &session);

    assert_eq!(
        annotations_above_hunk(&view, 0, 1),
        [(
            "swap old() for new()".to_string(),
            "old() is deprecated".to_string()
        )]
    );
    assert_eq!(
        annotations_above_hunk(&view, 1, 0),
        [(
            "add module b".to_string(),
            "splits parsing out of lib".to_string()
        )]
    );
    assert!(annotations_above_hunk(&view, 0, 0).is_empty());
}

#[test]
fn annotations_pending_in_the_adapter_are_drained_when_the_review_opens() {
    let session = AuthoringSessionId::from("session-1");
    let mut engine = engine_for(&session);

    // No AnnotationsEmitted event: the adapter still holds them when the
    // human opens the review.
    engine.agent_mut().push_annotation(
        &session,
        annotation("src/lib.rs", 1, "declare module b", "new parser module"),
    );

    let view = open_view(&mut engine, &session);

    assert_eq!(
        annotations_above_hunk(&view, 0, 0),
        [(
            "declare module b".to_string(),
            "new parser module".to_string()
        )]
    );
}

#[test]
fn annotations_survive_reopen_engine_restart_and_session_death() {
    let session = AuthoringSessionId::from("session-1");
    let mut engine = engine_for(&session);
    engine.agent_mut().push_annotation(
        &session,
        annotation(
            "src/lib.rs",
            11,
            "swap old() for new()",
            "old() is deprecated",
        ),
    );
    engine.handle_event(Event::AnnotationsEmitted {
        session: session.clone(),
    });

    // Close and reopen: the annotation is engine state, not pane state.
    let _ = open_view(&mut engine, &session);
    engine.handle_event(Event::ReviewClosed {
        session: session.clone(),
    });
    let view = open_view(&mut engine, &session);
    assert_eq!(annotations_above_hunk(&view, 0, 1).len(), 1);

    // A fresh engine (host restart) with a dead Authoring Session and an
    // empty adapter still renders the annotation from the restored snapshot.
    let snapshot = engine.annotations_snapshot();
    let mut revived = engine_for(&session);
    revived.restore_annotations(snapshot);
    let view = open_view(&mut revived, &session);
    assert_eq!(
        annotations_above_hunk(&view, 0, 1),
        [(
            "swap old() for new()".to_string(),
            "old() is deprecated".to_string()
        )]
    );
}

#[test]
fn annotations_are_scoped_to_the_commit_range_they_were_authored_on() {
    let session = AuthoringSessionId::from("session-1");
    let mut engine = engine_for(&session);
    engine.git_mut().set_changeset_base("base-1");
    engine.agent_mut().push_annotation(
        &session,
        annotation(
            "src/lib.rs",
            11,
            "swap old() for new()",
            "old() is deprecated",
        ),
    );
    engine.handle_event(Event::AnnotationsEmitted {
        session: session.clone(),
    });

    // The agent committed; the changeset now sits on a new base. Old
    // annotations belong to the old range and must not bleed through.
    engine.git_mut().set_changeset_base("base-2");
    let view = open_view(&mut engine, &session);
    assert_eq!(total_annotation_rows(&view), 0);

    engine.git_mut().set_changeset_base("base-1");
    let view = open_view(&mut engine, &session);
    assert_eq!(total_annotation_rows(&view), 1);
}

#[test]
fn an_annotation_outside_every_hunk_attaches_to_the_nearest_hunk_in_its_file() {
    let session = AuthoringSessionId::from("session-1");
    let mut engine = engine_for(&session);

    // Line 999 is far past both hunks of src/lib.rs; the later hunk
    // (lines 11-12) is nearest. An annotation for a file outside the
    // changeset has no hunk to attach to and is not rendered.
    engine.agent_mut().push_annotation(
        &session,
        annotation("src/lib.rs", 999, "tail change", "keeps API table sorted"),
    );
    engine.agent_mut().push_annotation(
        &session,
        annotation("src/gone.rs", 1, "orphan", "file left the changeset"),
    );

    let view = open_view(&mut engine, &session);

    assert_eq!(
        annotations_above_hunk(&view, 0, 1),
        [(
            "tail change".to_string(),
            "keeps API table sorted".to_string()
        )]
    );
    assert_eq!(total_annotation_rows(&view), 1);
}
