//! The read-only Review Pane at the engine seam: a keybind opens the pane
//! scoped to the session's Agent Worktree, the changeset renders as one
//! continuous multi-file stream with a file sidebar, vim motions move the
//! cursor, and reviewed-marks on files/hunks survive close/reopen.

use review_engine::fakes::{FakeAgentPort, FakeGitPort, FakeUiPort};
use review_engine::{
    AuthoringSessionId, Changeset, DiffLine, DiffLineKind, Effect, Engine, Event, FileDiff, Hunk,
    MarkTarget, ReviewMotion, ReviewView, StreamRow, WorktreeSpawn,
};

fn engine_with_git(git: FakeGitPort) -> Engine<FakeAgentPort, FakeGitPort, FakeUiPort> {
    Engine::new(FakeAgentPort::new(), git, FakeUiPort::new())
}

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

fn removed(content: &str) -> DiffLine {
    DiffLine {
        kind: DiffLineKind::Removed,
        content: content.to_string(),
    }
}

/// Two files, three hunks: the smallest changeset that exercises both the
/// multi-file stream and per-file hunk grouping.
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
                        lines: vec![removed("old()"), added("new()")],
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

/// Spawn a pane, finish the agent, script the diff: the state from which a
/// review can be requested.
fn ready_engine(
    session: &AuthoringSessionId,
    changeset: Changeset,
) -> Engine<FakeAgentPort, FakeGitPort, FakeUiPort> {
    let mut engine = engine_with_git(FakeGitPort::new());
    engine.handle_event(Event::AgentPaneSpawned {
        session: session.clone(),
        worktree: WorktreeSpawn::Create,
    });
    engine.git_mut().set_changeset(changeset);
    engine.handle_event(Event::AgentFinished {
        session: session.clone(),
    });
    engine
}

fn open_review(
    engine: &mut Engine<FakeAgentPort, FakeGitPort, FakeUiPort>,
    session: &AuthoringSessionId,
) {
    let effects = engine.handle_event(Event::ReviewRequested {
        session: session.clone(),
    });
    assert!(
        matches!(effects.as_slice(), [Effect::ReviewPaneOpened { .. }]),
        "expected ReviewPaneOpened, got {effects:?}"
    );
}

fn current_review(engine: &Engine<FakeAgentPort, FakeGitPort, FakeUiPort>) -> ReviewView {
    let model = engine.ui().rendered().last().expect("nothing rendered");
    model
        .reviews
        .first()
        .cloned()
        .expect("no review in render model")
}

#[test]
fn requesting_review_opens_the_pane_scoped_to_the_sessions_worktree() {
    let session = AuthoringSessionId::from("session-1");
    let mut engine = ready_engine(&session, sample_changeset());

    let effects = engine.handle_event(Event::ReviewRequested {
        session: session.clone(),
    });

    let [Effect::ReviewPaneOpened {
        session: opened,
        worktree,
    }] = effects.as_slice()
    else {
        panic!("expected exactly ReviewPaneOpened, got {effects:?}");
    };
    assert_eq!(*opened, session);
    assert_eq!(worktree.0, "agent/session-1");
    let review = current_review(&engine);
    assert_eq!(review.session, session);
    assert_eq!(review.worktree.0, "agent/session-1");
}

#[test]
fn requesting_review_without_a_bound_worktree_fails_visibly() {
    let mut engine = engine_with_git(FakeGitPort::new());
    let session = AuthoringSessionId::from("never-spawned");

    let effects = engine.handle_event(Event::ReviewRequested {
        session: session.clone(),
    });

    let [Effect::ReviewOpenFailed {
        session: failed, ..
    }] = effects.as_slice()
    else {
        panic!("expected exactly ReviewOpenFailed, got {effects:?}");
    };
    assert_eq!(*failed, session);
}

#[test]
fn a_failing_diff_surfaces_as_review_open_failed() {
    let session = AuthoringSessionId::from("session-1");
    let mut engine = ready_engine(&session, sample_changeset());
    engine.git_mut().fail_next("diff exploded".to_string());

    let effects = engine.handle_event(Event::ReviewRequested {
        session: session.clone(),
    });

    assert_eq!(
        effects,
        vec![Effect::ReviewOpenFailed {
            session,
            message: "diff exploded".to_string(),
        }]
    );
}

#[test]
fn the_changeset_renders_as_one_continuous_multi_file_stream() {
    let session = AuthoringSessionId::from("session-1");
    let mut engine = ready_engine(&session, sample_changeset());

    open_review(&mut engine, &session);

    let review = current_review(&engine);
    let kinds: Vec<&StreamRow> = review.stream.iter().collect();
    // File header, its hunks and lines, then the next file — one stream.
    assert!(
        matches!(
            kinds.first(),
            Some(StreamRow::FileHeader { path, .. }) if path == "src/lib.rs"
        ),
        "stream should start with the first file header, got {:?}",
        kinds.first()
    );
    let file_headers: Vec<&StreamRow> = review
        .stream
        .iter()
        .filter(|row| matches!(row, StreamRow::FileHeader { .. }))
        .collect();
    assert_eq!(file_headers.len(), 2, "both files appear in one stream");
    let hunk_headers = review
        .stream
        .iter()
        .filter(|row| matches!(row, StreamRow::HunkHeader { .. }))
        .count();
    assert_eq!(hunk_headers, 3);
    let line_rows = review
        .stream
        .iter()
        .filter(|row| matches!(row, StreamRow::Line { .. }))
        .count();
    assert_eq!(line_rows, 6);
}

#[test]
fn the_file_sidebar_lists_every_file_with_hunk_progress() {
    let session = AuthoringSessionId::from("session-1");
    let mut engine = ready_engine(&session, sample_changeset());

    open_review(&mut engine, &session);

    let review = current_review(&engine);
    assert_eq!(review.files.len(), 2);
    assert_eq!(review.files[0].path, "src/lib.rs");
    assert_eq!(review.files[0].hunks_total, 2);
    assert_eq!(review.files[0].hunks_reviewed, 0);
    assert!(!review.files[0].reviewed);
    assert_eq!(review.files[1].path, "src/new.rs");
    assert_eq!(review.files[1].hunks_total, 1);
}

#[test]
fn line_motions_move_the_cursor_and_clamp_at_the_stream_edges() {
    let session = AuthoringSessionId::from("session-1");
    let mut engine = ready_engine(&session, sample_changeset());
    open_review(&mut engine, &session);

    engine.handle_event(Event::ReviewNavigated {
        session: session.clone(),
        motion: ReviewMotion::LineUp,
    });
    assert_eq!(current_review(&engine).cursor, 0, "clamps at the top");

    engine.handle_event(Event::ReviewNavigated {
        session: session.clone(),
        motion: ReviewMotion::LineDown,
    });
    assert_eq!(current_review(&engine).cursor, 1);

    let stream_len = current_review(&engine).stream.len();
    for _ in 0..stream_len * 2 {
        engine.handle_event(Event::ReviewNavigated {
            session: session.clone(),
            motion: ReviewMotion::LineDown,
        });
    }
    assert_eq!(
        current_review(&engine).cursor,
        stream_len - 1,
        "clamps at the bottom"
    );
}

#[test]
fn half_page_motions_move_by_half_the_viewport_and_scroll_follows() {
    let session = AuthoringSessionId::from("session-1");
    let mut engine = ready_engine(&session, sample_changeset());
    open_review(&mut engine, &session);
    engine.handle_event(Event::ReviewViewportResized {
        session: session.clone(),
        rows: 4,
    });

    engine.handle_event(Event::ReviewNavigated {
        session: session.clone(),
        motion: ReviewMotion::HalfPageDown,
    });
    let review = current_review(&engine);
    assert_eq!(review.cursor, 2, "half of a 4-row viewport");
    engine.handle_event(Event::ReviewNavigated {
        session: session.clone(),
        motion: ReviewMotion::HalfPageDown,
    });
    let review = current_review(&engine);
    assert_eq!(review.cursor, 4);
    assert!(
        (review.scroll_top..review.scroll_top + 4).contains(&review.cursor),
        "scroll keeps the cursor inside the viewport (scroll_top {}, cursor {})",
        review.scroll_top,
        review.cursor
    );

    engine.handle_event(Event::ReviewNavigated {
        session: session.clone(),
        motion: ReviewMotion::HalfPageUp,
    });
    let review = current_review(&engine);
    assert_eq!(review.cursor, 2);
}

#[test]
fn hunk_and_file_jumps_land_on_the_next_header_rows() {
    let session = AuthoringSessionId::from("session-1");
    let mut engine = ready_engine(&session, sample_changeset());
    open_review(&mut engine, &session);

    engine.handle_event(Event::ReviewNavigated {
        session: session.clone(),
        motion: ReviewMotion::NextHunk,
    });
    let review = current_review(&engine);
    assert!(
        matches!(review.stream[review.cursor], StreamRow::HunkHeader { .. }),
        "NextHunk lands on a hunk header, got {:?}",
        review.stream[review.cursor]
    );
    let first_hunk = review.cursor;

    engine.handle_event(Event::ReviewNavigated {
        session: session.clone(),
        motion: ReviewMotion::NextHunk,
    });
    let second_hunk = current_review(&engine).cursor;
    assert!(second_hunk > first_hunk, "advances to the following hunk");

    engine.handle_event(Event::ReviewNavigated {
        session: session.clone(),
        motion: ReviewMotion::PrevHunk,
    });
    assert_eq!(current_review(&engine).cursor, first_hunk);

    engine.handle_event(Event::ReviewNavigated {
        session: session.clone(),
        motion: ReviewMotion::NextFile,
    });
    let review = current_review(&engine);
    assert!(
        matches!(
            &review.stream[review.cursor],
            StreamRow::FileHeader { path, .. } if path == "src/new.rs"
        ),
        "NextFile lands on the second file header, got {:?}",
        review.stream[review.cursor]
    );

    engine.handle_event(Event::ReviewNavigated {
        session: session.clone(),
        motion: ReviewMotion::PrevFile,
    });
    let review = current_review(&engine);
    assert!(
        matches!(
            &review.stream[review.cursor],
            StreamRow::FileHeader { path, .. } if path == "src/lib.rs"
        ),
        "PrevFile lands back on the first file header"
    );
}

#[test]
fn marking_the_hunk_under_the_cursor_flags_it_and_updates_file_progress() {
    let session = AuthoringSessionId::from("session-1");
    let mut engine = ready_engine(&session, sample_changeset());
    open_review(&mut engine, &session);

    // Land inside the first hunk (header row) and mark it.
    engine.handle_event(Event::ReviewNavigated {
        session: session.clone(),
        motion: ReviewMotion::NextHunk,
    });
    engine.handle_event(Event::ReviewMarkToggled {
        session: session.clone(),
        target: MarkTarget::Hunk,
    });

    let review = current_review(&engine);
    assert!(
        matches!(
            review.stream[review.cursor],
            StreamRow::HunkHeader { reviewed: true, .. }
        ),
        "the hunk under the cursor is marked reviewed"
    );
    assert_eq!(review.files[0].hunks_reviewed, 1);
    assert!(
        !review.files[0].reviewed,
        "one of two hunks is not the file"
    );

    // Toggling again clears it.
    engine.handle_event(Event::ReviewMarkToggled {
        session: session.clone(),
        target: MarkTarget::Hunk,
    });
    let review = current_review(&engine);
    assert_eq!(review.files[0].hunks_reviewed, 0);
}

#[test]
fn marking_a_file_marks_all_its_hunks_and_all_hunks_marked_means_file_reviewed() {
    let session = AuthoringSessionId::from("session-1");
    let mut engine = ready_engine(&session, sample_changeset());
    open_review(&mut engine, &session);

    engine.handle_event(Event::ReviewMarkToggled {
        session: session.clone(),
        target: MarkTarget::File,
    });

    let review = current_review(&engine);
    assert!(review.files[0].reviewed);
    assert_eq!(review.files[0].hunks_reviewed, 2);
    assert!(
        !review.files[1].reviewed,
        "only the cursor's file is marked"
    );

    // Unmarking the file clears its hunks too.
    engine.handle_event(Event::ReviewMarkToggled {
        session: session.clone(),
        target: MarkTarget::File,
    });
    let review = current_review(&engine);
    assert!(!review.files[0].reviewed);
    assert_eq!(review.files[0].hunks_reviewed, 0);
}

#[test]
fn reviewed_marks_survive_closing_and_reopening_the_review() {
    let session = AuthoringSessionId::from("session-1");
    let mut engine = ready_engine(&session, sample_changeset());
    open_review(&mut engine, &session);

    engine.handle_event(Event::ReviewNavigated {
        session: session.clone(),
        motion: ReviewMotion::NextHunk,
    });
    engine.handle_event(Event::ReviewMarkToggled {
        session: session.clone(),
        target: MarkTarget::Hunk,
    });
    engine.handle_event(Event::ReviewClosed {
        session: session.clone(),
    });
    let model = engine.ui().rendered().last().unwrap();
    assert!(model.reviews.is_empty(), "closing removes the review view");

    open_review(&mut engine, &session);

    let review = current_review(&engine);
    assert_eq!(
        review.files[0].hunks_reviewed, 1,
        "marks persist across close/reopen (detach/reattach)"
    );
}

#[test]
fn reviewed_marks_can_be_exported_and_restored_for_host_persistence() {
    let session = AuthoringSessionId::from("session-1");
    let mut engine = ready_engine(&session, sample_changeset());
    open_review(&mut engine, &session);
    engine.handle_event(Event::ReviewNavigated {
        session: session.clone(),
        motion: ReviewMotion::NextHunk,
    });
    engine.handle_event(Event::ReviewMarkToggled {
        session: session.clone(),
        target: MarkTarget::Hunk,
    });

    let snapshot = engine.reviewed_marks_snapshot();

    // A fresh engine (new server process) restores the snapshot and the
    // marks are back on reopen.
    let mut engine = ready_engine(&session, sample_changeset());
    engine.restore_reviewed_marks(snapshot);
    open_review(&mut engine, &session);
    assert_eq!(current_review(&engine).files[0].hunks_reviewed, 1);
}
