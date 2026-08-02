//! `herdr review-pane`: the Review Pane, hosted as a pane. Runs as its own
//! process inside a normal herdr pane (spawned by the `open_review`
//! keybind), so it survives detach/reattach with every other PTY and herdr's
//! core stays untouched. All review state lives in the Review Loop Engine;
//! this binary reads keys, feeds engine events, and draws the render model.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crossterm::event::{Event as TermEvent, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use review_engine::{
    AgentPort, AuthoringSessionId, DiffLineKind, Effect, Engine, Event, MarkTarget, RenderModel,
    ReviewMotion, ReviewView, StreamRow, UiPort,
};

const USAGE: &str = "usage: herdr review-pane --worktree <path>";

/// The read side of the Claude Code Harness Adapter: Annotations come from
/// the product state store that `herdr annotate` (the write side, run by
/// the agent at authoring time) filled — so they survive detach/reattach
/// and session death. Conversations are a later loop stage; every
/// Authoring Session still looks gone from here.
struct StoreAgentPort {
    store: crate::annotation_store::AnnotationStore,
    repo_root: PathBuf,
    branch: String,
    /// The worktree path, so the commit-range key is resolved at drain
    /// time — the same moment the engine resolves its own — instead of a
    /// stale snapshot from pane launch.
    worktree: PathBuf,
    drained: bool,
}

impl AgentPort for StoreAgentPort {
    fn session_is_live(&self, _session: &AuthoringSessionId) -> bool {
        false
    }

    fn deliver_instructions(&mut self, _session: &AuthoringSessionId, _instructions: &str) {}

    fn take_annotations(
        &mut self,
        _session: &AuthoringSessionId,
    ) -> Vec<review_engine::Annotation> {
        // A drain, like the trait demands: the engine now owns them, and a
        // reopen in this process must not ingest duplicates.
        if self.drained {
            return Vec::new();
        }
        self.drained = true;
        let Ok(base) = super::worktree_identity::head_commit(&self.worktree) else {
            return Vec::new();
        };
        self.store
            .annotations_for(&self.repo_root, &self.branch, &base)
    }
}

/// Keeps the engine's latest render model for the draw loop.
#[derive(Default)]
struct LatestRender {
    model: RenderModel,
}

impl UiPort for LatestRender {
    fn render(&mut self, model: &RenderModel) {
        self.model = model.clone();
    }
}

pub(super) fn run_review_pane_command(args: &[String]) -> std::io::Result<i32> {
    let mut worktree: Option<PathBuf> = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--worktree" => match iter.next() {
                Some(path) => worktree = Some(PathBuf::from(path)),
                None => {
                    eprintln!("{USAGE}");
                    return Ok(2);
                }
            },
            _ => {
                eprintln!("{USAGE}");
                return Ok(2);
            }
        }
    }
    let Some(worktree) = worktree else {
        eprintln!("{USAGE}");
        return Ok(2);
    };

    match open_review(&worktree) {
        Ok(engine) => run_tui(engine),
        Err(message) => {
            eprintln!("cannot open review: {message}");
            // Keep the failure on screen: this process is a pane the user
            // just opened, and exiting instantly would flash and close it.
            eprint!("press enter to close");
            std::io::stderr().flush().ok();
            let mut line = String::new();
            std::io::stdin().read_line(&mut line).ok();
            Ok(1)
        }
    }
}

type ReviewEngine = Engine<StoreAgentPort, review_engine::git::SystemGitPort, LatestRender>;

/// Adopt the worktree through the engine and open the review on it. The
/// engine's own guards apply: the primary checkout is not an Agent Worktree.
fn open_review(worktree: &Path) -> Result<ReviewEngine, String> {
    // The same resolution `herdr annotate` keys its writes with, so the
    // two adapter halves can never disagree on the store file.
    let identity = super::worktree_identity::resolve_worktree_identity(worktree)?;
    let branch = identity.branch;
    let worktrees_root = identity.repo_root.join(".herdr-agent-worktrees");
    let agent = StoreAgentPort {
        store: crate::annotation_store::AnnotationStore::product(),
        repo_root: identity.repo_root.clone(),
        branch: branch.clone(),
        worktree: worktree.to_path_buf(),
        drained: false,
    };
    let git = review_engine::git::SystemGitPort::new(identity.repo_root, worktrees_root);
    let mut engine = Engine::new(agent, git, LatestRender::default());

    let session = AuthoringSessionId::from(branch.as_str());
    let effects = engine.handle_event(Event::AgentPaneSpawned {
        session: session.clone(),
        worktree: review_engine::WorktreeSpawn::Adopt { branch },
    });
    if let [Effect::WorktreeOperationFailed { message, .. }] = effects.as_slice() {
        return Err(message.clone());
    }
    let effects = engine.handle_event(Event::ReviewRequested { session });
    if let [Effect::ReviewOpenFailed { message, .. }] = effects.as_slice() {
        return Err(message.clone());
    }
    Ok(engine)
}

fn session_of(engine: &ReviewEngine) -> Option<AuthoringSessionId> {
    engine.ui().model.reviews.first().map(|r| r.session.clone())
}

fn run_tui(mut engine: ReviewEngine) -> std::io::Result<i32> {
    let Some(session) = session_of(&engine) else {
        eprintln!("cannot open review: the engine reported no open review");
        return Ok(1);
    };
    let mut terminal = ratatui::init();
    let mut viewport_rows = 0usize;
    let result = loop {
        let Some(view) = engine.ui().model.reviews.first().cloned() else {
            break Ok(0);
        };
        let mut stream_rows = 0usize;
        if let Err(err) = terminal.draw(|frame| {
            stream_rows = draw(frame, &view);
        }) {
            break Err(err);
        }
        if stream_rows != viewport_rows {
            viewport_rows = stream_rows;
            engine.handle_event(Event::ReviewViewportResized {
                session: session.clone(),
                rows: viewport_rows,
            });
            continue;
        }

        if !crossterm::event::poll(Duration::from_millis(250))? {
            continue;
        }
        let TermEvent::Key(key) = crossterm::event::read()? else {
            continue;
        };
        if key.kind == KeyEventKind::Release {
            continue;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let motion = match key.code {
            KeyCode::Char('j') | KeyCode::Down => Some(ReviewMotion::LineDown),
            KeyCode::Char('k') | KeyCode::Up => Some(ReviewMotion::LineUp),
            KeyCode::Char('d') if ctrl => Some(ReviewMotion::HalfPageDown),
            KeyCode::Char('u') if ctrl => Some(ReviewMotion::HalfPageUp),
            KeyCode::Char(']') => Some(ReviewMotion::NextHunk),
            KeyCode::Char('[') => Some(ReviewMotion::PrevHunk),
            KeyCode::Char('}') => Some(ReviewMotion::NextFile),
            KeyCode::Char('{') => Some(ReviewMotion::PrevFile),
            _ => None,
        };
        if let Some(motion) = motion {
            engine.handle_event(Event::ReviewNavigated {
                session: session.clone(),
                motion,
            });
            continue;
        }
        match key.code {
            KeyCode::Char('m') | KeyCode::Char(' ') => {
                engine.handle_event(Event::ReviewMarkToggled {
                    session: session.clone(),
                    target: MarkTarget::Hunk,
                });
            }
            KeyCode::Char('M') => {
                engine.handle_event(Event::ReviewMarkToggled {
                    session: session.clone(),
                    target: MarkTarget::File,
                });
            }
            KeyCode::Char('q') | KeyCode::Esc => {
                engine.handle_event(Event::ReviewClosed { session });
                break Ok(0);
            }
            _ => {}
        }
    };
    ratatui::restore();
    result
}

/// Draw the Review Pane; returns the stream viewport height so the engine
/// can be told about resizes.
fn draw(frame: &mut Frame, view: &ReviewView) -> usize {
    let [header_area, body_area, footer_area] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .areas(frame.area());
    let sidebar_width = (body_area.width / 3).clamp(20, 40).min(body_area.width);
    let [sidebar_area, stream_area] = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(sidebar_width), Constraint::Min(1)])
        .areas(body_area);

    let reviewed_files = view.files.iter().filter(|f| f.reviewed).count();
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(
                " Review: {} — {}/{} files reviewed",
                view.worktree.0,
                reviewed_files,
                view.files.len()
            ),
            Style::default().add_modifier(Modifier::BOLD),
        ))),
        header_area,
    );

    draw_file_sidebar(frame, sidebar_area, view);
    let stream_rows = draw_stream(frame, stream_area, view);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            " j/k move · C-d/C-u half page · [ ] hunk · { } file · m mark hunk · M mark file · q quit",
            Style::default().fg(Color::DarkGray),
        ))),
        footer_area,
    );
    stream_rows
}

fn draw_file_sidebar(frame: &mut Frame, area: Rect, view: &ReviewView) {
    let cursor_file = view.cursor_file;
    let lines: Vec<Line> = view
        .files
        .iter()
        .enumerate()
        .map(|(idx, file)| {
            let mark = if file.reviewed { "✓" } else { " " };
            let mut style = if file.reviewed {
                Style::default().fg(Color::Green)
            } else {
                Style::default()
            };
            if Some(idx) == cursor_file {
                style = style.add_modifier(Modifier::BOLD).bg(Color::DarkGray);
            }
            Line::from(Span::styled(
                format!(
                    "{mark} {} ({}/{})",
                    file.path, file.hunks_reviewed, file.hunks_total
                ),
                style,
            ))
        })
        .collect();
    frame.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::RIGHT).title("files")),
        area,
    );
}

/// Render the visible slice of the continuous multi-file stream; returns
/// the number of stream rows that fit.
fn draw_stream(frame: &mut Frame, area: Rect, view: &ReviewView) -> usize {
    let rows = area.height as usize;
    let lines: Vec<Line> = view
        .stream
        .iter()
        .enumerate()
        .skip(view.scroll_top)
        .take(rows)
        .map(|(idx, row)| {
            let (text, mut style) = match row {
                StreamRow::FileHeader { path, reviewed, .. } => (
                    format!("── {path} {}", if *reviewed { "✓" } else { "" }),
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                ),
                StreamRow::Annotation { what, why, .. } => (
                    format!("● {what} — {why}"),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::ITALIC),
                ),
                StreamRow::HunkHeader {
                    header, reviewed, ..
                } => (
                    format!("{header} {}", if *reviewed { "✓ reviewed" } else { "" }),
                    Style::default().fg(Color::Cyan),
                ),
                StreamRow::Line { kind, content } => match kind {
                    DiffLineKind::Added => {
                        (format!("+{content}"), Style::default().fg(Color::Green))
                    }
                    DiffLineKind::Removed => {
                        (format!("-{content}"), Style::default().fg(Color::Red))
                    }
                    DiffLineKind::Context => (format!(" {content}"), Style::default()),
                },
            };
            if idx == view.cursor {
                style = style.add_modifier(Modifier::REVERSED);
            }
            Line::from(Span::styled(text, style))
        })
        .collect();
    if view.stream.is_empty() {
        frame.render_widget(
            Paragraph::new("changeset is empty — nothing to review"),
            area,
        );
    } else {
        frame.render_widget(Paragraph::new(lines), area);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotation_store::{AnnotationStore, StoredAnnotation};
    use std::process::Command;

    fn run_git(repo: &Path, args: &[&str]) {
        let output = Command::new("git")
            .current_dir(repo)
            .args(args)
            .output()
            .expect("failed to run git");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn store_agent_port_drains_the_worktrees_current_range_exactly_once() {
        let base =
            std::env::temp_dir().join(format!("herdr-store-agent-port-{}", std::process::id()));
        std::fs::remove_dir_all(&base).ok();
        let repo = base.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        run_git(&repo, &["init", "--initial-branch=agent/pane-1"]);
        run_git(&repo, &["config", "user.email", "test@example.com"]);
        run_git(&repo, &["config", "user.name", "Test"]);
        std::fs::write(repo.join("README.md"), "hello\n").unwrap();
        run_git(&repo, &["add", "."]);
        run_git(&repo, &["commit", "-m", "initial"]);
        let head = super::super::worktree_identity::head_commit(&repo).unwrap();
        let repo = repo.canonicalize().unwrap();

        let store = AnnotationStore::at(base.join("store"));
        store
            .append(
                &repo,
                "agent/pane-1",
                &head,
                StoredAnnotation {
                    file: "README.md".to_string(),
                    line: 1,
                    what: "greet".to_string(),
                    why: "politeness".to_string(),
                },
            )
            .unwrap();

        let mut port = StoreAgentPort {
            store,
            repo_root: repo.clone(),
            branch: "agent/pane-1".to_string(),
            worktree: repo,
            drained: false,
        };
        let session = AuthoringSessionId::from("agent/pane-1");
        let drained = port.take_annotations(&session);
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].what, "greet");
        assert!(
            port.take_annotations(&session).is_empty(),
            "a second drain must not duplicate annotations into the engine"
        );

        std::fs::remove_dir_all(&base).ok();
    }
}
