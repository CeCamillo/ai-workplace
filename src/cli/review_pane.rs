//! `herdr review-pane`: the Review Pane, hosted as a pane. Runs as its own
//! process inside a normal herdr pane (spawned by the `open_review`
//! keybind), so it survives detach/reattach with every other PTY and herdr's
//! core stays untouched. All review state lives in the Review Loop Engine;
//! this binary reads keys, feeds engine events, and draws the render model.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;

use crossterm::event::{Event as TermEvent, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;
use review_engine::{
    AgentPort, AuthoringSessionId, ConversationEntry, ConversationRouting, ConversationView,
    DiffLineKind, Effect, Engine, Event, MarkTarget, RenderModel, ReviewMotion, ReviewView,
    SessionSeed, StreamRow, UiPort,
};

use crate::api::client::ApiClient;
use crate::api::schema::{EmptyParams, Method, Request};

const USAGE: &str = "usage: herdr review-pane --worktree <path>";

/// A Hunk Conversation answer produced by a background `claude -p` run,
/// handed back to the TUI loop over a channel.
struct ConversationAnswer {
    /// The engine-side session the question was delivered to.
    target: AuthoringSessionId,
    /// The claude session that answered, so follow-ups can `--resume` it
    /// and keep the conversation's own history.
    claude_session: Option<String>,
    text: String,
}

/// The Claude Code Harness Adapter as seen from the Review Pane process.
/// Annotations come from the product state store that `herdr annotate`
/// (the write side, run by the agent at authoring time) filled — so they
/// survive detach/reattach and session death. Conversations route per
/// ADR-0003: the live Authoring Session is found through the herdr API
/// (the claude pane working in this worktree) and reached with
/// `claude -p --resume`; when it is gone, a fresh `claude -p` session is
/// spawned seeded with the stored Annotation and hunk diff.
struct StoreAgentPort {
    store: crate::annotation_store::AnnotationStore,
    repo_root: PathBuf,
    branch: String,
    /// The worktree path, so the commit-range key is resolved at drain
    /// time — the same moment the engine resolves its own — instead of a
    /// stale snapshot from pane launch.
    worktree: PathBuf,
    drained: bool,
    router: ConversationRouter,
}

/// The adapter's conversation routing state: where answers go, the seed
/// prompts of spawned sessions not yet primed by a first question, and
/// which claude session continues each engine-side one.
struct ConversationRouter {
    answers: Sender<ConversationAnswer>,
    seeds: HashMap<AuthoringSessionId, String>,
    continuations: HashMap<AuthoringSessionId, String>,
    seeded_count: usize,
}

impl ConversationRouter {
    fn new(answers: Sender<ConversationAnswer>) -> Self {
        Self {
            answers,
            seeds: HashMap::new(),
            continuations: HashMap::new(),
            seeded_count: 0,
        }
    }

    /// Record which claude session answered for `target`; follow-up
    /// questions `--resume` it.
    fn record_claude_session(&mut self, target: &AuthoringSessionId, claude_session: String) {
        self.continuations.insert(target.clone(), claude_session);
    }

    fn fail(&self, target: &AuthoringSessionId, message: String) {
        tracing::warn!(target = %target.0, %message, "hunk conversation went unanswered");
        self.answers
            .send(ConversationAnswer {
                target: target.clone(),
                claude_session: None,
                text: no_answer(&message),
            })
            .ok();
    }
}

/// The transcript entry a delivery failure turns into, in place of the
/// answer.
fn no_answer(message: &str) -> String {
    format!("(no answer: {message})")
}

impl AgentPort for StoreAgentPort {
    fn session_is_live(&self, _session: &AuthoringSessionId) -> bool {
        live_authoring_claude_session(&self.worktree).is_some()
    }

    fn deliver_instructions(&mut self, session: &AuthoringSessionId, instructions: &str) {
        let resolved = if let Some(claude_session) = self.router.continuations.get(session) {
            Ok((Some(claude_session.clone()), instructions.to_string()))
        } else if let Some(seed) = self.router.seeds.remove(session) {
            Ok((None, format!("{seed}\n\n{instructions}")))
        } else {
            match live_authoring_claude_session(&self.worktree) {
                Some(claude_session) => Ok((Some(claude_session), instructions.to_string())),
                None => Err(
                    "the Authoring Session disappeared before the question was delivered"
                        .to_string(),
                ),
            }
        };
        let (resume, prompt) = match resolved {
            Ok(resolved) => resolved,
            Err(message) => return self.router.fail(session, message),
        };
        let target = session.clone();
        let worktree = self.worktree.clone();
        let answers = self.router.answers.clone();
        std::thread::spawn(move || {
            let answer = match run_claude_print(&worktree, resume.as_deref(), &prompt) {
                Ok((text, claude_session)) => ConversationAnswer {
                    target,
                    claude_session,
                    text,
                },
                Err(message) => {
                    tracing::warn!(
                        target = %target.0,
                        %message,
                        "hunk conversation went unanswered"
                    );
                    ConversationAnswer {
                        target,
                        claude_session: None,
                        text: no_answer(&message),
                    }
                }
            };
            answers.send(answer).ok();
        });
    }

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

    fn spawn_seeded_session(&mut self, seed: &SessionSeed) -> Result<AuthoringSessionId, String> {
        self.router.seeded_count += 1;
        let session = AuthoringSessionId(format!("review-seeded-{}", self.router.seeded_count));
        self.router.seeds.insert(session.clone(), seed_prompt(seed));
        Ok(session)
    }
}

/// The context a fresh session gets in place of the gone Authoring
/// Session's memory: the hunk diff and the stored Annotations (ADR-0003).
fn seed_prompt(seed: &SessionSeed) -> String {
    let mut prompt = format!(
        "You are answering a code reviewer's questions about a change whose \
         authoring agent session is gone. The hunk under discussion, from \
         {}:\n\n{}\n",
        seed.file, seed.diff
    );
    for annotation in &seed.annotations {
        prompt.push_str(&format!(
            "\nThe authoring agent annotated this hunk — what: {} — why: {}\n",
            annotation.what, annotation.why
        ));
    }
    prompt.push_str(
        "\nAnswer the reviewer concisely in plain text, using the repository \
         for extra context where needed.",
    );
    prompt
}

/// The claude session of the live Authoring Session working in `worktree`,
/// found through the herdr API: a claude agent pane whose working
/// directory is the worktree and which is interactively ready.
fn live_authoring_claude_session(worktree: &Path) -> Option<String> {
    let worktree = worktree
        .canonicalize()
        .unwrap_or_else(|_| worktree.to_path_buf());
    let response = ApiClient::local()
        .request_value_with_timeout(
            &Request {
                id: "review-pane:agent-list".into(),
                method: Method::AgentList(EmptyParams::default()),
            },
            Duration::from_secs(2),
        )
        .ok()?;
    let agents = response["result"]["agents"].as_array()?;
    agents.iter().find_map(|agent| {
        if agent["agent"].as_str() != Some("claude")
            || !agent["interactive_ready"].as_bool().unwrap_or(false)
        {
            return None;
        }
        let in_worktree = [&agent["cwd"], &agent["foreground_cwd"]]
            .iter()
            .filter_map(|cwd| cwd.as_str())
            .any(|cwd| {
                let cwd = Path::new(cwd);
                cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf()) == worktree
            });
        if !in_worktree {
            return None;
        }
        let session = &agent["agent_session"];
        (session["agent"].as_str() == Some("claude") && session["kind"].as_str() == Some("id"))
            .then(|| session["value"].as_str().map(str::to_string))
            .flatten()
    })
}

/// Run one non-interactive claude turn in the worktree and return
/// `(answer, claude session id)`. `resume` continues an existing session —
/// the live Authoring Session's memory, or an earlier conversation turn.
fn run_claude_print(
    worktree: &Path,
    resume: Option<&str>,
    prompt: &str,
) -> Result<(String, Option<String>), String> {
    let mut command = std::process::Command::new("claude");
    command
        .current_dir(worktree)
        .args(["-p", "--output-format", "json"]);
    if let Some(session) = resume {
        command.args(["--resume", session]);
    }
    let output = command
        .arg(prompt)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|err| format!("could not run claude: {err}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("claude failed: {}", stderr.trim()));
    }
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|err| format!("claude output was not json: {err}"))?;
    let text = parsed["result"]
        .as_str()
        .ok_or_else(|| "claude output had no result".to_string())?
        .trim()
        .to_string();
    let claude_session = parsed["session_id"].as_str().map(str::to_string);
    Ok((text, claude_session))
}

/// Which Fix-it Comment the input box is writing: a fresh draft on the
/// hunk under the cursor, or an edit of an existing draft. TUI
/// presentation state, like the input buffer itself.
enum CommentInput {
    New,
    Edit(usize),
}

/// Which agent turn the pane is waiting on. While one is out, the answers
/// channel's completed `claude -p` turn means "the agent finished" and
/// advances the engine.
enum AwaitingTurn {
    /// The submitted Review is being applied.
    Review,
    /// The Approve commit instruction is being carried out.
    Commit,
    /// Handed-off merge conflicts are being resolved.
    ConflictResolution,
}

impl AwaitingTurn {
    fn label(&self) -> &'static str {
        match self {
            AwaitingTurn::Review => " — Review submitted, agent working…",
            AwaitingTurn::Commit => " — Approved, agent committing…",
            AwaitingTurn::ConflictResolution => " — merge conflicts with the agent…",
        }
    }
}

/// Track the approve/commit/merge flow across an event's effects: which
/// agent turn is now pending, and any failure to surface in the header.
fn apply_flow_effects(
    effects: &[Effect],
    awaiting: &mut Option<AwaitingTurn>,
    notice: &mut Option<String>,
) {
    for effect in effects {
        match effect {
            Effect::ReviewDelivered { .. } => *awaiting = Some(AwaitingTurn::Review),
            Effect::CommitRequested { .. } => *awaiting = Some(AwaitingTurn::Commit),
            Effect::MergeConflictsHandedToAgent { .. } => {
                *awaiting = Some(AwaitingTurn::ConflictResolution)
            }
            Effect::CommitFailed { message, .. }
            | Effect::MergeFailed { message, .. }
            | Effect::WorktreeOperationFailed { message, .. } => *notice = Some(message.clone()),
            _ => {}
        }
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
        Ok((engine, answers)) => run_tui(engine, answers),
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
fn open_review(worktree: &Path) -> Result<(ReviewEngine, Receiver<ConversationAnswer>), String> {
    // The same resolution `herdr annotate` keys its writes with, so the
    // two adapter halves can never disagree on the store file.
    let identity = super::worktree_identity::resolve_worktree_identity(worktree)?;
    let branch = identity.branch;
    let worktrees_root = identity.repo_root.join(".herdr-agent-worktrees");
    let (answers_tx, answers_rx) = channel();
    let agent = StoreAgentPort {
        store: crate::annotation_store::AnnotationStore::product(),
        repo_root: identity.repo_root.clone(),
        branch: branch.clone(),
        worktree: worktree.to_path_buf(),
        drained: false,
        router: ConversationRouter::new(answers_tx),
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
    Ok((engine, answers_rx))
}

fn session_of(engine: &ReviewEngine) -> Option<AuthoringSessionId> {
    engine.ui().model.reviews.first().map(|r| r.session.clone())
}

fn run_tui(
    mut engine: ReviewEngine,
    answers: Receiver<ConversationAnswer>,
) -> std::io::Result<i32> {
    let Some(session) = session_of(&engine) else {
        eprintln!("cannot open review: the engine reported no open review");
        return Ok(1);
    };
    let mut terminal = ratatui::init();
    let mut viewport_rows = 0usize;
    // The text being typed (a question or a Fix-it Comment) and which box
    // owns it: TUI presentation state, not engine state.
    let mut input = String::new();
    let mut comment_input: Option<CommentInput> = None;
    // An instruction set (a Review, a commit, a conflict hand-off) is out
    // with the agent; its completed `claude -p` turn comes back on the
    // answers channel and means "the agent acted".
    let mut awaiting: Option<AwaitingTurn> = None;
    // The last flow failure, drawn in the header until the flow moves on.
    let mut notice: Option<String> = None;
    let result = loop {
        while let Ok(answer) = answers.try_recv() {
            if let Some(claude_session) = answer.claude_session {
                engine
                    .agent_mut()
                    .router
                    .record_claude_session(&answer.target, claude_session);
            }
            // The turn that applied the submitted Review finished: refresh
            // the diff instead of treating it as a conversation answer —
            // unless a live-session conversation is still owed one (then
            // the conversation wins and the next turn ends the Review).
            let live_conversation_awaiting = engine
                .ui()
                .model
                .reviews
                .first()
                .and_then(|review| review.conversation.as_ref())
                .is_some_and(|conversation| {
                    conversation.awaiting_answer
                        && conversation.routing == Some(ConversationRouting::LiveAuthoringSession)
                });
            if awaiting.is_some() && answer.target == session && !live_conversation_awaiting {
                awaiting = None;
                notice = None;
                let effects = engine.handle_event(Event::AgentFinished {
                    session: session.clone(),
                });
                apply_flow_effects(&effects, &mut awaiting, &mut notice);
                continue;
            }
            engine.handle_event(Event::ConversationAnswerReceived {
                session: session.clone(),
                target: answer.target,
                answer: answer.text,
            });
        }
        let Some(view) = engine.ui().model.reviews.first().cloned() else {
            break Ok(0);
        };
        let merge_offered = engine.ui().model.merge_offers.contains(&session);
        let mut status = awaiting
            .as_ref()
            .map(AwaitingTurn::label)
            .unwrap_or(if merge_offered {
                " — committed · merge into the default branch? y/n"
            } else {
                ""
            })
            .to_string();
        if let Some(notice) = notice.as_ref() {
            status.push_str(&format!(" — {notice}"));
        }
        let mut stream_rows = 0usize;
        if let Err(err) = terminal.draw(|frame| {
            stream_rows = draw(
                frame,
                &view,
                &input,
                comment_input.as_ref(),
                &status,
                merge_offered,
            );
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
        // An open conversation captures the keyboard for its question box.
        if let Some(conversation) = view.conversation.as_ref() {
            match key.code {
                KeyCode::Esc => {
                    input.clear();
                    engine.handle_event(Event::ConversationClosed {
                        session: session.clone(),
                    });
                }
                KeyCode::Enter if !conversation.awaiting_answer && !input.trim().is_empty() => {
                    let question = std::mem::take(&mut input);
                    // A failed ask joins the transcript engine-side; no
                    // effect handling needed here.
                    engine.handle_event(Event::ConversationAsked {
                        session: session.clone(),
                        question,
                    });
                }
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Char(c) if !ctrl => input.push(c),
                _ => {}
            }
            continue;
        }
        // An active Fix-it Comment box captures the keyboard likewise.
        if comment_input.is_some() {
            match key.code {
                KeyCode::Esc => {
                    input.clear();
                    comment_input = None;
                }
                KeyCode::Enter if !input.trim().is_empty() => {
                    let text = std::mem::take(&mut input);
                    match comment_input.take() {
                        Some(CommentInput::Edit(comment)) => {
                            engine.handle_event(Event::CommentEdited {
                                session: session.clone(),
                                comment,
                                text,
                            });
                        }
                        _ => {
                            engine.handle_event(Event::CommentDrafted {
                                session: session.clone(),
                                text,
                            });
                        }
                    }
                }
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Char(c) if !ctrl => input.push(c),
                _ => {}
            }
            continue;
        }
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
            KeyCode::Char('c') => {
                engine.handle_event(Event::ConversationOpened {
                    session: session.clone(),
                });
            }
            KeyCode::Char('f') => {
                // Drafting only anchors to a hunk; opening the box on a
                // file header would swallow a comment the engine drops.
                if !matches!(
                    view.stream.get(view.cursor),
                    None | Some(StreamRow::FileHeader { .. })
                ) {
                    comment_input = Some(CommentInput::New);
                }
            }
            KeyCode::Char('e') => {
                if let Some(StreamRow::Comment { comment, text, .. }) = view.stream.get(view.cursor)
                {
                    input = text.clone();
                    comment_input = Some(CommentInput::Edit(*comment));
                }
            }
            KeyCode::Char('x') => {
                if let Some(StreamRow::Comment { comment, .. }) = view.stream.get(view.cursor) {
                    engine.handle_event(Event::CommentDeleted {
                        session: session.clone(),
                        comment: *comment,
                    });
                }
            }
            KeyCode::Char('S') => {
                let effects = engine.handle_event(Event::ReviewSubmitted {
                    session: session.clone(),
                });
                apply_flow_effects(&effects, &mut awaiting, &mut notice);
            }
            KeyCode::Char('A') if awaiting.is_none() && !merge_offered => {
                let effects = engine.handle_event(Event::ChangesetApproved {
                    session: session.clone(),
                });
                apply_flow_effects(&effects, &mut awaiting, &mut notice);
            }
            KeyCode::Char('y') if merge_offered => {
                let effects = engine.handle_event(Event::MergeAccepted {
                    session: session.clone(),
                });
                apply_flow_effects(&effects, &mut awaiting, &mut notice);
            }
            KeyCode::Char('n') if merge_offered => {
                engine.handle_event(Event::MergeDeclined {
                    session: session.clone(),
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
fn draw(
    frame: &mut Frame,
    view: &ReviewView,
    input: &str,
    comment_input: Option<&CommentInput>,
    status: &str,
    merge_offered: bool,
) -> usize {
    let [header_area, mut body_area, footer_area] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .areas(frame.area());
    if let Some(conversation) = view.conversation.as_ref() {
        let height = (conversation.entries.len() as u16 * 2 + 5).clamp(7, body_area.height / 2);
        let [rest, conversation_area] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(height)])
            .areas(body_area);
        body_area = rest;
        draw_conversation(frame, conversation_area, conversation, input);
    }
    if let Some(mode) = comment_input {
        let [rest, comment_area] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(2)])
            .areas(body_area);
        body_area = rest;
        let title = match mode {
            CommentInput::New => "Fix-it Comment",
            CommentInput::Edit(_) => "edit Fix-it Comment",
        };
        frame.render_widget(
            Paragraph::new(format!("> {input}▏"))
                .block(Block::default().borders(Borders::TOP).title(title)),
            comment_area,
        );
    }
    let sidebar_width = (body_area.width / 3).clamp(20, 40).min(body_area.width);
    let [sidebar_area, stream_area] = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(sidebar_width), Constraint::Min(1)])
        .areas(body_area);

    let reviewed_files = view.files.iter().filter(|f| f.reviewed).count();
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(
                " Review: {} — {}/{} files reviewed{status}",
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

    let hints = if view.conversation.is_some() {
        " type your question · Enter ask · Esc close conversation"
    } else if comment_input.is_some() {
        " type your fix-it comment · Enter save draft · Esc cancel"
    } else if merge_offered {
        " y merge into the default branch · n keep the branch as-is · q quit"
    } else {
        " j/k move · C-d/C-u half page · [ ] hunk · { } file · m/M mark · c converse · f fix-it · e edit · x delete · S submit · A approve · q quit"
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            hints,
            Style::default().fg(Color::DarkGray),
        ))),
        footer_area,
    );
    stream_rows
}

/// Draw the open Hunk Conversation: the exchange so far and the question
/// box, anchored under the stream.
fn draw_conversation(frame: &mut Frame, area: Rect, conversation: &ConversationView, input: &str) {
    let routing = match conversation.routing {
        None => "unrouted",
        Some(ConversationRouting::LiveAuthoringSession) => "live Authoring Session",
        Some(ConversationRouting::SeededSession) => "fresh seeded session",
    };
    let title = format!(
        " conversation · {} {} · {routing} ",
        conversation.file, conversation.hunk_header
    );
    let mut lines: Vec<Line> = Vec::new();
    for entry in &conversation.entries {
        lines.push(match entry {
            ConversationEntry::Question(question) => Line::from(Span::styled(
                format!("you ▸ {question}"),
                Style::default().fg(Color::Cyan),
            )),
            ConversationEntry::Answer(answer) => Line::from(Span::styled(
                format!("agent ▸ {answer}"),
                Style::default().fg(Color::Yellow),
            )),
        });
    }
    if conversation.awaiting_answer {
        lines.push(Line::from(Span::styled(
            "agent ▸ thinking…",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            format!("> {input}▏"),
            Style::default().add_modifier(Modifier::BOLD),
        )));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::TOP).title(title)),
        area,
    );
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
                StreamRow::Comment { text, .. } => (
                    format!("✎ {text}"),
                    Style::default()
                        .fg(Color::Blue)
                        .add_modifier(Modifier::ITALIC),
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

        let (answers, _keep_alive) = channel();
        let mut port = StoreAgentPort {
            store,
            repo_root: repo.clone(),
            branch: "agent/pane-1".to_string(),
            worktree: repo,
            drained: false,
            router: ConversationRouter::new(answers),
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

    /// The seeded conversation path against the real CLI: one `claude -p`
    /// turn seeded with a hunk answers a question about it, and the
    /// reported session id resumes into a follow-up that still remembers
    /// the exchange. Opt in with `--ignored` — it needs the `claude` CLI
    /// and API access.
    #[test]
    #[ignore = "round-trips real claude -p turns; needs `claude` on PATH and API access"]
    fn a_real_seeded_claude_session_answers_and_resumes() {
        if Command::new("claude").arg("--version").output().is_err() {
            eprintln!("skipping: claude CLI not found on PATH");
            return;
        }
        let dir = std::env::temp_dir().join(format!("herdr-conversation-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let seed = SessionSeed {
            file: "src/lib.rs".to_string(),
            hunk_header: "@@ -1,1 +1,1 @@".to_string(),
            diff: "@@ -1,1 +1,1 @@\n-mod parser;\n+mod zephyr_parser;".to_string(),
            annotations: vec![review_engine::Annotation {
                file: "src/lib.rs".to_string(),
                line: 1,
                what: "rename the parser module".to_string(),
                why: "the old name collided with a dependency".to_string(),
            }],
        };
        let question = format!(
            "{}\n\nWhat is the exact name of the module the hunk adds?",
            seed_prompt(&seed)
        );
        let (answer, claude_session) =
            run_claude_print(&dir, None, &question).expect("the seeded turn should answer");
        assert!(
            answer.contains("zephyr_parser"),
            "the answer should come from the seeded hunk, got: {answer}"
        );

        let claude_session = claude_session.expect("claude should report a session_id to resume");
        let (follow_up, _) = run_claude_print(
            &dir,
            Some(&claude_session),
            "Answer again with exactly the module name from my previous question, nothing else.",
        )
        .expect("the resumed follow-up should answer");
        assert!(
            follow_up.contains("zephyr_parser"),
            "the follow-up should remember the conversation, got: {follow_up}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
