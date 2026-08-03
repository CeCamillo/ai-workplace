//! Read-only review state: the open Review Pane's cursor and scroll, and
//! the reviewed-marks that outlive it. Marks are keyed by Agent Worktree
//! (not by pane) so they survive close/reopen and detach/reattach.

use std::collections::{HashMap, HashSet};

use crate::annotation::Annotation;
use crate::comment::FixItComment;
use crate::conversation::{Conversation, ConversationEntry, ConversationView, SessionSeed};
use crate::diff::{match_hunks, Changeset, DiffLineKind, FileDiff, Hunk};
use crate::ids::{AgentWorktreeId, AuthoringSessionId};
use crate::worktree::AgentWorktree;

/// A vim motion inside the Review Pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewMotion {
    LineDown,
    LineUp,
    HalfPageDown,
    HalfPageUp,
    NextHunk,
    PrevHunk,
    NextFile,
    PrevFile,
}

/// What a reviewed-mark toggle applies to: the hunk under the cursor, or
/// the whole file under the cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkTarget {
    Hunk,
    File,
}

/// Reviewed-marks for one Agent Worktree. A file counts as reviewed when
/// explicitly marked or when every one of its hunks is.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReviewedMarks {
    /// `(file path, hunk index)` pairs marked reviewed.
    pub hunks: HashSet<(String, usize)>,
    /// Files explicitly marked reviewed (covers hunkless files, e.g. binary).
    pub files: HashSet<String>,
}

/// All reviewed-marks, keyed by Agent Worktree — the unit the host
/// persists across server restarts.
pub type ReviewedMarksSnapshot = HashMap<AgentWorktreeId, ReviewedMarks>;

impl ReviewedMarks {
    /// The marks carried over a diff refresh: a hunk mark follows its hunk
    /// (by content, via [`match_hunks`]) to the hunk's new index; an
    /// explicit file mark survives only when every hunk of the file is
    /// unchanged. Marks on files or hunks that changed are dropped —
    /// re-review only costs the parts that moved.
    fn remapped(&self, old: &Changeset, new: &Changeset) -> ReviewedMarks {
        let mut remapped = ReviewedMarks::default();
        for new_file in &new.files {
            let path = &new_file.path;
            let Some(old_file) = old.files.iter().find(|f| f.path == *path) else {
                continue;
            };
            let pairs = match_hunks(old_file, new_file);
            for (old_idx, new_idx) in &pairs {
                if self.hunks.contains(&(path.clone(), *old_idx)) {
                    remapped.hunks.insert((path.clone(), *new_idx));
                }
            }
            let unchanged =
                old_file.hunks.len() == new_file.hunks.len() && pairs.len() == old_file.hunks.len();
            if unchanged && self.files.contains(path) {
                remapped.files.insert(path.clone());
            }
        }
        remapped
    }

    fn file_reviewed(&self, path: &str, hunk_count: usize) -> bool {
        self.files.contains(path)
            || (hunk_count > 0
                && (0..hunk_count).all(|i| self.hunks.contains(&(path.to_string(), i))))
    }

    fn toggle_file(&mut self, path: &str, hunk_count: usize) {
        if self.file_reviewed(path, hunk_count) {
            self.files.remove(path);
            self.hunks.retain(|(p, _)| p != path);
        } else {
            self.files.insert(path.to_string());
            for i in 0..hunk_count {
                self.hunks.insert((path.to_string(), i));
            }
        }
    }

    fn toggle_hunk(&mut self, path: &str, hunk: usize) {
        let key = (path.to_string(), hunk);
        if !self.hunks.remove(&key) {
            self.hunks.insert(key);
        } else {
            // A partially-reviewed file is no longer wholly reviewed.
            self.files.remove(path);
        }
    }
}

/// One open Review Pane: the changeset and Annotation snapshots it renders
/// plus cursor and scroll. Marks live outside this, keyed by worktree.
#[derive(Debug)]
pub struct ReviewState {
    pub worktree: AgentWorktree,
    pub changeset: Changeset,
    /// The commit range this review's changeset and Annotations sit on.
    pub base: String,
    /// The Annotations for `(worktree, base)`, in emission order.
    pub annotations: Vec<Annotation>,
    pub cursor: usize,
    pub scroll_top: usize,
    pub viewport_rows: usize,
    /// The open Hunk Conversation, at most one per review.
    pub conversation: Option<Conversation>,
    /// Drafted Fix-it Comments in draft order, local until submit.
    pub drafts: Vec<FixItComment>,
    /// How many in-flight answers each session still owes to conversations
    /// that were abandoned while awaiting one. Those answers are dropped on
    /// arrival instead of attaching to whatever hunk is open by then.
    stale_answers: HashMap<AuthoringSessionId, usize>,
}

impl ReviewState {
    pub fn new(
        worktree: AgentWorktree,
        changeset: Changeset,
        base: String,
        annotations: Vec<Annotation>,
    ) -> Self {
        Self {
            worktree,
            changeset,
            base,
            annotations,
            cursor: 0,
            scroll_top: 0,
            viewport_rows: 0,
            conversation: None,
            drafts: Vec::new(),
            stale_answers: HashMap::new(),
        }
    }

    /// Draft a Fix-it Comment on the hunk under the cursor; no-op when the
    /// cursor is not on a hunk.
    pub fn draft_comment(&mut self, text: String) {
        if let Some((file, hunk)) = self.hunk_under_cursor() {
            self.drafts.push(FixItComment { file, hunk, text });
        }
    }

    /// Replace draft `comment`'s text; no-op on an out-of-range index.
    pub fn edit_comment(&mut self, comment: usize, text: String) {
        if let Some(draft) = self.drafts.get_mut(comment) {
            draft.text = text;
        }
    }

    /// Delete draft `comment`; no-op on an out-of-range index.
    pub fn delete_comment(&mut self, comment: usize) {
        if comment < self.drafts.len() {
            self.drafts.remove(comment);
        }
    }

    /// Take the batched Review as one structured instruction set: every
    /// draft in changeset order, each anchored by file path, post-image
    /// line, and hunk header. The drafts leave with it — a submitted
    /// Review clears the pane. `None`, and no state change, when there
    /// are no drafts.
    pub fn take_review_instructions(&mut self) -> Option<String> {
        if self.drafts.is_empty() {
            return None;
        }
        // Changeset order (file, then hunk, then draft order), so the
        // agent reads the Review the way the reviewer read the diff.
        let mut ordered: Vec<(usize, &FixItComment)> = self
            .drafts
            .iter()
            .map(|draft| {
                let file = self
                    .changeset
                    .files
                    .iter()
                    .position(|f| f.path == draft.file)
                    .unwrap_or(usize::MAX);
                (file, draft)
            })
            .collect();
        ordered.sort_by_key(|(file, draft)| (*file, draft.hunk));
        let mut instructions = format!(
            "The reviewer submitted a Review of your changeset: {} fix-it comment(s). \
             Apply every one of them at its anchored location.",
            ordered.len()
        );
        for (idx, (file, draft)) in ordered.iter().enumerate() {
            let hunk = self
                .changeset
                .files
                .get(*file)
                .and_then(|f| f.hunks.get(draft.hunk));
            let line = hunk
                .and_then(Hunk::post_image_range)
                .map(|(start, _)| start)
                .unwrap_or(0);
            let header = hunk.map(|h| h.header.as_str()).unwrap_or("");
            instructions.push_str(&format!(
                "\n\n{}. {}:{line} ({header})\n{}",
                idx + 1,
                draft.file,
                draft.text
            ));
        }
        self.drafts.clear();
        Some(instructions)
    }

    /// Swap in the re-fetched changeset after the agent acted on the
    /// Review. `marks` (the worktree's reviewed-marks) and the drafts are
    /// remapped onto content-identical hunks; anything anchored to changed
    /// content is dropped. The conversation closes — its hunk may be gone —
    /// and the cursor clamps into the new stream.
    pub fn refresh(
        &mut self,
        changeset: Changeset,
        base: String,
        annotations: Vec<Annotation>,
        marks: &mut ReviewedMarks,
    ) {
        *marks = marks.remapped(&self.changeset, &changeset);
        self.drafts = std::mem::take(&mut self.drafts)
            .into_iter()
            .filter_map(|draft| {
                let old_file = self.changeset.files.iter().find(|f| f.path == draft.file)?;
                let new_file = changeset.files.iter().find(|f| f.path == draft.file)?;
                let (_, new_hunk) = match_hunks(old_file, new_file)
                    .into_iter()
                    .find(|(old_idx, _)| *old_idx == draft.hunk)?;
                Some(FixItComment {
                    hunk: new_hunk,
                    ..draft
                })
            })
            .collect();
        self.close_conversation();
        self.changeset = changeset;
        self.base = base;
        self.annotations = annotations;
        let rows = self.stream_positions().len();
        self.cursor = self.cursor.min(rows.saturating_sub(1));
        self.scroll_top = self.scroll_top.min(self.cursor);
        self.follow_cursor();
    }

    /// Open a Hunk Conversation anchored to the hunk under the cursor;
    /// no-op when the cursor is not on a hunk. Replacing a conversation
    /// abandons it like closing does.
    pub fn open_conversation(&mut self) {
        if let Some((file, hunk)) = self.hunk_under_cursor() {
            self.abandon_pending_answer();
            self.conversation = Some(Conversation {
                file,
                hunk,
                target: None,
                entries: Vec::new(),
                awaiting_answer: false,
            });
        }
    }

    pub fn close_conversation(&mut self) {
        self.abandon_pending_answer();
        self.conversation = None;
    }

    /// Deliver `target`'s answer to the open conversation — unless it was
    /// owed to an abandoned one, or no open conversation is awaiting an
    /// answer from that session; those answers are dropped.
    pub fn receive_answer(&mut self, target: &AuthoringSessionId, answer: String) {
        if let Some(pending) = self.stale_answers.get_mut(target) {
            *pending -= 1;
            if *pending == 0 {
                self.stale_answers.remove(target);
            }
            return;
        }
        if let Some(conversation) = self.conversation.as_mut() {
            if conversation.awaiting_answer
                && conversation
                    .target
                    .as_ref()
                    .is_some_and(|(routed, _)| routed == target)
            {
                conversation.entries.push(ConversationEntry::Answer(answer));
                conversation.awaiting_answer = false;
            }
        }
    }

    /// The open conversation is going away; if it still awaits an answer,
    /// remember to drop that answer when it arrives.
    fn abandon_pending_answer(&mut self) {
        if let Some(conversation) = self.conversation.as_ref() {
            if conversation.awaiting_answer {
                if let Some((target, _)) = conversation.target.clone() {
                    *self.stale_answers.entry(target).or_insert(0) += 1;
                }
            }
        }
    }

    /// The seed a fresh session needs to stand in for the gone Authoring
    /// Session on the open conversation's hunk (ADR-0003): the hunk's diff
    /// plus the stored Annotations placed on it.
    pub fn conversation_seed(&self) -> Option<SessionSeed> {
        let conversation = self.conversation.as_ref()?;
        let file_idx = self
            .changeset
            .files
            .iter()
            .position(|f| f.path == conversation.file)?;
        let hunk = self.changeset.files[file_idx]
            .hunks
            .get(conversation.hunk)?;
        let mut diff = hunk.header.clone();
        for line in &hunk.lines {
            let prefix = match line.kind {
                DiffLineKind::Added => '+',
                DiffLineKind::Removed => '-',
                DiffLineKind::Context => ' ',
            };
            diff.push('\n');
            diff.push(prefix);
            diff.push_str(&line.content);
        }
        let annotations = self
            .placed_annotations()
            .get(&(file_idx, conversation.hunk))
            .into_iter()
            .flatten()
            .map(|idx| self.annotations[*idx].clone())
            .collect();
        Some(SessionSeed {
            file: conversation.file.clone(),
            hunk_header: hunk.header.clone(),
            diff,
            annotations,
        })
    }

    /// Indices into `annotations` grouped by the `(file, hunk)` each one
    /// renders above: the hunk whose post-image range covers the annotated
    /// line, else the nearest hunk in the file. Annotations for files
    /// outside the changeset have nowhere to render and are dropped.
    fn placed_annotations(&self) -> HashMap<(usize, usize), Vec<usize>> {
        let mut placed: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
        for (idx, annotation) in self.annotations.iter().enumerate() {
            let Some(file_idx) = self
                .changeset
                .files
                .iter()
                .position(|f| f.path == annotation.file)
            else {
                continue;
            };
            if let Some(hunk_idx) = hunk_for_line(&self.changeset.files[file_idx], annotation.line)
            {
                placed.entry((file_idx, hunk_idx)).or_default().push(idx);
            }
        }
        placed
    }

    fn stream_positions(&self) -> Vec<StreamPosition> {
        let placed = self.placed_annotations();
        let mut rows = Vec::new();
        for (file_idx, file) in self.changeset.files.iter().enumerate() {
            rows.push(StreamPosition::FileHeader { file: file_idx });
            for (hunk_idx, hunk) in file.hunks.iter().enumerate() {
                let annotations = placed.get(&(file_idx, hunk_idx)).map(Vec::len).unwrap_or(0);
                for _ in 0..annotations {
                    rows.push(StreamPosition::Annotation {
                        file: file_idx,
                        hunk: hunk_idx,
                    });
                }
                rows.push(StreamPosition::HunkHeader {
                    file: file_idx,
                    hunk: hunk_idx,
                });
                for _ in &hunk.lines {
                    rows.push(StreamPosition::Line {
                        file: file_idx,
                        hunk: hunk_idx,
                    });
                }
                for _ in self.hunk_drafts(&file.path, hunk_idx) {
                    rows.push(StreamPosition::Comment {
                        file: file_idx,
                        hunk: hunk_idx,
                    });
                }
            }
        }
        rows
    }

    /// The draft indices anchored to `(path, hunk)`, in draft order.
    fn hunk_drafts(&self, path: &str, hunk: usize) -> Vec<usize> {
        self.drafts
            .iter()
            .enumerate()
            .filter(|(_, draft)| draft.file == path && draft.hunk == hunk)
            .map(|(idx, _)| idx)
            .collect()
    }

    pub fn apply_motion(&mut self, motion: ReviewMotion) {
        let positions = self.stream_positions();
        if positions.is_empty() {
            return;
        }
        let last = positions.len() - 1;
        let half_page = (self.viewport_rows / 2).max(1);
        self.cursor = match motion {
            ReviewMotion::LineDown => (self.cursor + 1).min(last),
            ReviewMotion::LineUp => self.cursor.saturating_sub(1),
            ReviewMotion::HalfPageDown => (self.cursor + half_page).min(last),
            ReviewMotion::HalfPageUp => self.cursor.saturating_sub(half_page),
            ReviewMotion::NextHunk => next_matching(&positions, self.cursor, is_hunk_header),
            ReviewMotion::PrevHunk => prev_matching(&positions, self.cursor, is_hunk_header),
            ReviewMotion::NextFile => next_matching(&positions, self.cursor, is_file_header),
            ReviewMotion::PrevFile => prev_matching(&positions, self.cursor, is_file_header),
        };
        self.follow_cursor();
    }

    pub fn resize_viewport(&mut self, rows: usize) {
        self.viewport_rows = rows;
        self.follow_cursor();
    }

    /// Keep the cursor inside the viewport window.
    fn follow_cursor(&mut self) {
        if self.viewport_rows == 0 {
            return;
        }
        if self.cursor < self.scroll_top {
            self.scroll_top = self.cursor;
        } else if self.cursor >= self.scroll_top + self.viewport_rows {
            self.scroll_top = self.cursor + 1 - self.viewport_rows;
        }
    }

    /// The `(path, hunk index)` under the cursor, if the cursor is on or
    /// inside a hunk (its Annotation rows included).
    fn hunk_under_cursor(&self) -> Option<(String, usize)> {
        match self.stream_positions().get(self.cursor)? {
            StreamPosition::Annotation { file, hunk }
            | StreamPosition::HunkHeader { file, hunk }
            | StreamPosition::Line { file, hunk }
            | StreamPosition::Comment { file, hunk } => {
                Some((self.changeset.files[*file].path.clone(), *hunk))
            }
            StreamPosition::FileHeader { .. } => None,
        }
    }

    /// The file under the cursor (any row belongs to exactly one file).
    fn file_under_cursor(&self) -> Option<usize> {
        match self.stream_positions().get(self.cursor)? {
            StreamPosition::FileHeader { file }
            | StreamPosition::Annotation { file, .. }
            | StreamPosition::HunkHeader { file, .. }
            | StreamPosition::Line { file, .. }
            | StreamPosition::Comment { file, .. } => Some(*file),
        }
    }

    pub fn toggle_mark(&self, marks: &mut ReviewedMarks, target: MarkTarget) {
        match target {
            MarkTarget::Hunk => {
                if let Some((path, hunk)) = self.hunk_under_cursor() {
                    marks.toggle_hunk(&path, hunk);
                }
            }
            MarkTarget::File => {
                if let Some(file) = self.file_under_cursor() {
                    let file = &self.changeset.files[file];
                    marks.toggle_file(&file.path, file.hunks.len());
                }
            }
        }
    }

    /// Build what the Review Pane draws.
    pub fn view(&self, session: &AuthoringSessionId, marks: &ReviewedMarks) -> ReviewView {
        let files = self
            .changeset
            .files
            .iter()
            .map(|file| {
                let hunks_reviewed = (0..file.hunks.len())
                    .filter(|i| marks.hunks.contains(&(file.path.clone(), *i)))
                    .count();
                FileSummary {
                    path: file.path.clone(),
                    reviewed: marks.file_reviewed(&file.path, file.hunks.len()),
                    hunks_total: file.hunks.len(),
                    hunks_reviewed,
                }
            })
            .collect();
        let placed = self.placed_annotations();
        let mut stream = Vec::new();
        for (file_idx, file) in self.changeset.files.iter().enumerate() {
            stream.push(StreamRow::FileHeader {
                file: file_idx,
                path: file.path.clone(),
                reviewed: marks.file_reviewed(&file.path, file.hunks.len()),
            });
            for (hunk_idx, hunk) in file.hunks.iter().enumerate() {
                for annotation_idx in placed.get(&(file_idx, hunk_idx)).into_iter().flatten() {
                    let annotation = &self.annotations[*annotation_idx];
                    stream.push(StreamRow::Annotation {
                        file: file_idx,
                        hunk: hunk_idx,
                        what: annotation.what.clone(),
                        why: annotation.why.clone(),
                    });
                }
                stream.push(StreamRow::HunkHeader {
                    file: file_idx,
                    hunk: hunk_idx,
                    header: hunk.header.clone(),
                    reviewed: marks.hunks.contains(&(file.path.clone(), hunk_idx)),
                });
                for line in &hunk.lines {
                    stream.push(StreamRow::Line {
                        kind: line.kind,
                        content: line.content.clone(),
                    });
                }
                for comment_idx in self.hunk_drafts(&file.path, hunk_idx) {
                    stream.push(StreamRow::Comment {
                        file: file_idx,
                        hunk: hunk_idx,
                        comment: comment_idx,
                        text: self.drafts[comment_idx].text.clone(),
                    });
                }
            }
        }
        ReviewView {
            session: session.clone(),
            worktree: self.worktree.id.clone(),
            files,
            stream,
            cursor: self.cursor,
            cursor_file: self.file_under_cursor(),
            scroll_top: self.scroll_top,
            viewport_rows: self.viewport_rows,
            conversation: self.conversation_view(),
        }
    }

    fn conversation_view(&self) -> Option<ConversationView> {
        let conversation = self.conversation.as_ref()?;
        let hunk_header = self
            .changeset
            .files
            .iter()
            .find(|f| f.path == conversation.file)
            .and_then(|f| f.hunks.get(conversation.hunk))
            .map(|hunk| hunk.header.clone())
            .unwrap_or_default();
        Some(ConversationView {
            file: conversation.file.clone(),
            hunk: conversation.hunk,
            hunk_header,
            routing: conversation.target.as_ref().map(|(_, routing)| *routing),
            entries: conversation.entries.clone(),
            awaiting_answer: conversation.awaiting_answer,
        })
    }
}

/// Row identity used for cursor motions; parallel to [`StreamRow`] without
/// the display payload.
enum StreamPosition {
    FileHeader { file: usize },
    Annotation { file: usize, hunk: usize },
    HunkHeader { file: usize, hunk: usize },
    Line { file: usize, hunk: usize },
    Comment { file: usize, hunk: usize },
}

/// The hunk of `file` whose post-image range covers `line`, else the
/// nearest hunk by line distance; `None` only when the file has no hunk
/// with a parseable range.
fn hunk_for_line(file: &FileDiff, line: usize) -> Option<usize> {
    let ranges: Vec<(usize, (usize, usize))> = file
        .hunks
        .iter()
        .enumerate()
        .filter_map(|(idx, hunk)| hunk.post_image_range().map(|range| (idx, range)))
        .collect();
    ranges
        .iter()
        .find(|(_, (start, count))| line >= *start && line < start + (*count).max(1))
        .or_else(|| {
            ranges.iter().min_by_key(|(_, (start, count))| {
                let end = start + (*count).max(1) - 1;
                if line < *start {
                    *start - line
                } else {
                    line - end
                }
            })
        })
        .map(|(idx, _)| *idx)
}

fn is_hunk_header(row: &StreamPosition) -> bool {
    matches!(row, StreamPosition::HunkHeader { .. })
}

fn is_file_header(row: &StreamPosition) -> bool {
    matches!(row, StreamPosition::FileHeader { .. })
}

fn next_matching(
    positions: &[StreamPosition],
    cursor: usize,
    matches: fn(&StreamPosition) -> bool,
) -> usize {
    positions
        .iter()
        .enumerate()
        .skip(cursor + 1)
        .find(|(_, row)| matches(row))
        .map(|(idx, _)| idx)
        .unwrap_or(cursor)
}

fn prev_matching(
    positions: &[StreamPosition],
    cursor: usize,
    matches: fn(&StreamPosition) -> bool,
) -> usize {
    positions
        .iter()
        .enumerate()
        .take(cursor)
        .rev()
        .find(|(_, row)| matches(row))
        .map(|(idx, _)| idx)
        .unwrap_or(cursor)
}

/// What the Review Pane draws: file sidebar, continuous multi-file stream,
/// cursor and scroll. The engine owns this; the pane only renders it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewView {
    pub session: AuthoringSessionId,
    pub worktree: AgentWorktreeId,
    pub files: Vec<FileSummary>,
    pub stream: Vec<StreamRow>,
    pub cursor: usize,
    /// Index into `files` of the file the cursor row belongs to, so the
    /// pane can highlight the sidebar without re-deriving it.
    pub cursor_file: Option<usize>,
    pub scroll_top: usize,
    pub viewport_rows: usize,
    /// The open Hunk Conversation, drawn anchored to its hunk.
    pub conversation: Option<ConversationView>,
}

/// One file sidebar row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSummary {
    pub path: String,
    pub reviewed: bool,
    pub hunks_total: usize,
    pub hunks_reviewed: usize,
}

/// One row of the continuous diff stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamRow {
    FileHeader {
        file: usize,
        path: String,
        reviewed: bool,
    },
    /// An agent Annotation, rendered inline above the hunk it explains.
    Annotation {
        file: usize,
        hunk: usize,
        what: String,
        why: String,
    },
    HunkHeader {
        file: usize,
        hunk: usize,
        header: String,
        reviewed: bool,
    },
    Line {
        kind: DiffLineKind,
        content: String,
    },
    /// A drafted Fix-it Comment, rendered inline below the hunk it is
    /// anchored to. `comment` is the draft's index in draft order — the
    /// handle [`crate::Event::CommentEdited`] and
    /// [`crate::Event::CommentDeleted`] take.
    Comment {
        file: usize,
        hunk: usize,
        comment: usize,
        text: String,
    },
}
