//! Fix-it Comments and the batched Review at the engine seam: the reviewer
//! drafts comments on hunks (add, edit, delete — all local until submit),
//! one explicit submit delivers the whole Review to the Agent as a single
//! structured, file/line-anchored instruction set, and when the agent
//! finishes acting the open Review Pane refreshes its diff with
//! reviewed-marks preserved wherever content is unchanged.

use review_engine::fakes::{FakeAgentPort, FakeGitPort, FakeUiPort};
use review_engine::{
    AuthoringSessionId, Changeset, DiffLine, DiffLineKind, Effect, Engine, Event, FileDiff, Hunk,
    MarkTarget, ReviewMotion, ReviewView, StreamRow, WorktreeSpawn,
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

/// Two files, three hunks: enough to draft on distinct anchors and to
/// exercise mark preservation per hunk and per file.
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

/// Spawn a pane on a fresh worktree with the given scripted changeset and
/// open its Review Pane.
fn reviewing_engine(
    session: &AuthoringSessionId,
    changeset: Changeset,
) -> Engine<FakeAgentPort, FakeGitPort, FakeUiPort> {
    let mut git = FakeGitPort::new();
    git.set_changeset(changeset);
    let mut engine = Engine::new(FakeAgentPort::new(), git, FakeUiPort::new());
    engine.handle_event(Event::AgentPaneSpawned {
        session: session.clone(),
        worktree: WorktreeSpawn::Create,
    });
    engine.handle_event(Event::AgentFinished {
        session: session.clone(),
    });
    engine.handle_event(Event::ReviewRequested {
        session: session.clone(),
    });
    engine
}

fn current_review(engine: &Engine<FakeAgentPort, FakeGitPort, FakeUiPort>) -> ReviewView {
    let model = engine.ui().rendered().last().expect("nothing rendered");
    model
        .reviews
        .first()
        .cloned()
        .expect("no review in render model")
}

fn navigate(
    engine: &mut Engine<FakeAgentPort, FakeGitPort, FakeUiPort>,
    session: &AuthoringSessionId,
    motion: ReviewMotion,
) {
    engine.handle_event(Event::ReviewNavigated {
        session: session.clone(),
        motion,
    });
}

/// The `(comment index, text)` of every drafted Fix-it Comment row in the
/// stream, in stream order.
fn comment_rows(view: &ReviewView) -> Vec<(usize, String)> {
    view.stream
        .iter()
        .filter_map(|row| match row {
            StreamRow::Comment { comment, text, .. } => Some((*comment, text.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn comments_draft_edit_and_delete_locally_until_submit() {
    let session = AuthoringSessionId::from("session-1");
    let mut engine = reviewing_engine(&session, sample_changeset());

    // Draft on the first hunk.
    navigate(&mut engine, &session, ReviewMotion::NextHunk);
    engine.handle_event(Event::CommentDrafted {
        session: session.clone(),
        text: "rename b".to_string(),
    });
    assert_eq!(
        comment_rows(&current_review(&engine)),
        vec![(0, "rename b".to_string())]
    );

    // Edit it.
    engine.handle_event(Event::CommentEdited {
        session: session.clone(),
        comment: 0,
        text: "rename module b to parser".to_string(),
    });
    assert_eq!(
        comment_rows(&current_review(&engine)),
        vec![(0, "rename module b to parser".to_string())]
    );

    // Draft a second one on the next hunk, then delete the first.
    navigate(&mut engine, &session, ReviewMotion::NextHunk);
    engine.handle_event(Event::CommentDrafted {
        session: session.clone(),
        text: "handle the error case".to_string(),
    });
    assert_eq!(comment_rows(&current_review(&engine)).len(), 2);
    engine.handle_event(Event::CommentDeleted {
        session: session.clone(),
        comment: 0,
    });
    assert_eq!(
        comment_rows(&current_review(&engine)),
        vec![(0, "handle the error case".to_string())]
    );

    // Drafts are local: nothing has reached the agent.
    assert!(engine.agent().delivered().is_empty());
}

#[test]
fn a_comment_only_drafts_on_a_hunk() {
    let session = AuthoringSessionId::from("session-1");
    let mut engine = reviewing_engine(&session, sample_changeset());

    // Cursor is on the first file header row: nothing to anchor to.
    engine.handle_event(Event::CommentDrafted {
        session: session.clone(),
        text: "orphan".to_string(),
    });
    assert!(comment_rows(&current_review(&engine)).is_empty());
}

#[test]
fn comment_rows_anchor_to_the_hunk_they_were_drafted_on() {
    let session = AuthoringSessionId::from("session-1");
    let mut engine = reviewing_engine(&session, sample_changeset());

    // Draft on the third hunk (src/new.rs's only hunk).
    navigate(&mut engine, &session, ReviewMotion::NextFile);
    navigate(&mut engine, &session, ReviewMotion::NextHunk);
    engine.handle_event(Event::CommentDrafted {
        session: session.clone(),
        text: "document b".to_string(),
    });

    let view = current_review(&engine);
    let comment = view
        .stream
        .iter()
        .find_map(|row| match row {
            StreamRow::Comment { file, hunk, .. } => Some((*file, *hunk)),
            _ => None,
        })
        .expect("no comment row in stream");
    assert_eq!(
        comment,
        (1, 0),
        "the comment must anchor to src/new.rs hunk 0"
    );
}

#[test]
fn submit_delivers_one_structured_file_line_anchored_instruction_set() {
    let session = AuthoringSessionId::from("session-1");
    let mut engine = reviewing_engine(&session, sample_changeset());

    navigate(&mut engine, &session, ReviewMotion::NextHunk);
    engine.handle_event(Event::CommentDrafted {
        session: session.clone(),
        text: "rename module b to parser".to_string(),
    });
    navigate(&mut engine, &session, ReviewMotion::NextHunk);
    engine.handle_event(Event::CommentDrafted {
        session: session.clone(),
        text: "handle the error case".to_string(),
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
    // One delivery, into the Authoring Session.
    let delivered = engine.agent().delivered();
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].0, session);
    let instructions = &delivered[0].1;
    // Every comment appears, anchored by file and post-image line.
    assert!(
        instructions.contains("src/lib.rs:1"),
        "first comment must be anchored to src/lib.rs line 1, got: {instructions}"
    );
    assert!(
        instructions.contains("src/lib.rs:11"),
        "second comment must be anchored to src/lib.rs line 11, got: {instructions}"
    );
    assert!(instructions.contains("@@ -1,3 +1,4 @@"));
    assert!(instructions.contains("@@ -10,2 +11,2 @@"));
    assert!(instructions.contains("rename module b to parser"));
    assert!(instructions.contains("handle the error case"));

    // Submitted drafts leave the pane, and the session is no longer
    // Ready-for-Review: the agent is working again.
    let model = engine.ui().rendered().last().unwrap();
    assert!(comment_rows(model.reviews.first().unwrap()).is_empty());
    assert!(!model.ready_for_review.contains(&session));
}

#[test]
fn submitting_with_no_drafts_is_a_no_op() {
    let session = AuthoringSessionId::from("session-1");
    let mut engine = reviewing_engine(&session, sample_changeset());

    let effects = engine.handle_event(Event::ReviewSubmitted {
        session: session.clone(),
    });

    assert!(effects.is_empty());
    assert!(engine.agent().delivered().is_empty());
    // Nothing was submitted, so the pane stays Ready-for-Review.
    let model = engine.ui().rendered().last().unwrap();
    assert!(model.ready_for_review.contains(&session));
}

#[test]
fn the_full_cycle_refreshes_the_diff_and_preserves_marks_on_unchanged_content() {
    let session = AuthoringSessionId::from("session-1");
    let mut engine = reviewing_engine(&session, sample_changeset());

    // Mark the first hunk of src/lib.rs and the whole of src/new.rs
    // reviewed, then draft a fix on the second hunk of src/lib.rs.
    navigate(&mut engine, &session, ReviewMotion::NextHunk);
    engine.handle_event(Event::ReviewMarkToggled {
        session: session.clone(),
        target: MarkTarget::Hunk,
    });
    navigate(&mut engine, &session, ReviewMotion::NextHunk);
    engine.handle_event(Event::CommentDrafted {
        session: session.clone(),
        text: "handle the error case".to_string(),
    });
    navigate(&mut engine, &session, ReviewMotion::NextFile);
    engine.handle_event(Event::ReviewMarkToggled {
        session: session.clone(),
        target: MarkTarget::File,
    });
    engine.handle_event(Event::ReviewSubmitted {
        session: session.clone(),
    });

    // The agent applies the Review: the commented hunk changed, the other
    // two are untouched.
    let mut applied = sample_changeset();
    applied.files[0].hunks[1] = Hunk {
        header: "@@ -10,2 +11,3 @@".to_string(),
        lines: vec![added("new()?"), added("handle_error()")],
    };
    engine.git_mut().set_changeset(applied);
    let effects = engine.handle_event(Event::AgentFinished {
        session: session.clone(),
    });
    assert!(effects.contains(&Effect::ReadyForReview {
        session: session.clone()
    }));

    let view = current_review(&engine);
    // The stream shows the refreshed diff.
    assert!(
        view.stream.iter().any(|row| matches!(
            row,
            StreamRow::Line { content, .. } if content == "handle_error()"
        )),
        "the refreshed stream must show the applied fix"
    );
    // Marks survived exactly where content is unchanged.
    let lib = &view.files[0];
    assert_eq!(lib.hunks_reviewed, 1, "the untouched hunk keeps its mark");
    assert!(
        !lib.reviewed,
        "the reworked hunk costs the file its progress"
    );
    assert!(view.files[1].reviewed, "the untouched file stays reviewed");
    let reworked_hunk_reviewed = view.stream.iter().any(
        |row| matches!(row, StreamRow::HunkHeader { file: 0, hunk: 1, reviewed, .. } if *reviewed),
    );
    assert!(
        !reworked_hunk_reviewed,
        "the hunk the agent reworked must need re-review"
    );
}

#[test]
fn reviewed_marks_follow_unchanged_hunks_across_index_shifts() {
    let session = AuthoringSessionId::from("session-1");
    let mut engine = reviewing_engine(&session, sample_changeset());

    // Mark the second hunk of src/lib.rs (index 1).
    navigate(&mut engine, &session, ReviewMotion::NextHunk);
    navigate(&mut engine, &session, ReviewMotion::NextHunk);
    engine.handle_event(Event::ReviewMarkToggled {
        session: session.clone(),
        target: MarkTarget::Hunk,
    });

    // The agent's next pass dissolves the first hunk; the marked hunk
    // slides up to index 0 with identical content.
    let mut shifted = sample_changeset();
    shifted.files[0].hunks.remove(0);
    engine.git_mut().set_changeset(shifted);
    engine.handle_event(Event::AgentFinished {
        session: session.clone(),
    });

    let view = current_review(&engine);
    let first_hunk_reviewed = view.stream.iter().any(
        |row| matches!(row, StreamRow::HunkHeader { file: 0, hunk: 0, reviewed, .. } if *reviewed),
    );
    assert!(
        first_hunk_reviewed,
        "the mark must follow the unchanged hunk to its new index"
    );
    assert_eq!(view.files[0].hunks_reviewed, 1);
}

#[test]
fn a_failed_refresh_surfaces_and_keeps_the_stale_review() {
    let session = AuthoringSessionId::from("session-1");
    let mut engine = reviewing_engine(&session, sample_changeset());
    let before = current_review(&engine);

    engine.git_mut().fail_next("index is locked".to_string());
    let effects = engine.handle_event(Event::AgentFinished {
        session: session.clone(),
    });

    assert!(effects.contains(&Effect::ReviewRefreshFailed {
        session: session.clone(),
        message: "index is locked".to_string(),
    }));
    // The pane keeps drawing the last good changeset.
    assert_eq!(current_review(&engine).stream, before.stream);
}
