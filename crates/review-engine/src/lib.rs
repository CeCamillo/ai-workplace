//! The Review Loop Engine: the headless core of the Agent Review Loop.
//!
//! The engine is event-in/effect-out. The host (herdr) feeds it [`Event`]s,
//! the engine returns [`Effect`]s and owns all loop state. It touches the
//! world only through three ports: [`AgentPort`] (the Harness Adapter seam),
//! [`GitPort`] (Agent Worktrees and diffs), and [`UiPort`] (the declarative
//! render model the Review Pane draws).
//!
//! Tests assert at exactly one seam — this crate's public API — with all
//! three ports faked ([`fakes`]).

mod annotation;
mod diff;
mod engine;
mod event;
pub mod fakes;
pub mod git;
mod ids;
mod ports;
mod review;
mod worktree;

pub use annotation::{Annotation, AnnotationsSnapshot};
pub use diff::{parse_unified_diff, Changeset, DiffLine, DiffLineKind, FileDiff, Hunk};
pub use engine::Engine;
pub use event::{Effect, Event, WorktreeSpawn};
pub use ids::{AgentWorktreeId, AuthoringSessionId};
pub use ports::{AgentPort, GitPort, RenderModel, UiPort};
pub use review::{
    FileSummary, MarkTarget, ReviewMotion, ReviewView, ReviewedMarks, ReviewedMarksSnapshot,
    StreamRow,
};
pub use worktree::AgentWorktree;
