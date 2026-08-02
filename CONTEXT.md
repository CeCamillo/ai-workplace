# AI Workplace

A terminal multiplexer for AI coding agents (a tracking fork of herdr) with a native, agent-aware code-review surface: agents explain their diffs inline, the human converses per hunk and sends fix-it comments back to the agent.

## Language

**Agent**:
An AI coding CLI (v1: Claude Code) running in a pane, supervised by the human.
_Avoid_: bot, assistant

**Harness Adapter**:
The integration layer for one agent product (hooks, prompt injection, annotation protocol). v1 ships the Claude Code adapter only, behind a seam that admits others later.
_Avoid_: plugin, integration (unqualified)

**Authoring Session**:
The specific agent session that produced a given changeset. May be live in a pane or already gone.

**Annotation**:
A per-hunk explanation (what was done and why) emitted by the agent at authoring time and persisted with the diff.
_Avoid_: comment (reserved for human input), note

**Hunk Conversation**:
An interactive exchange between the human and an agent about one specific hunk. Routed to the live Authoring Session when it exists; otherwise a fresh session seeded with the Annotation and diff context.

**Fix-it Comment**:
A human-authored instruction attached to a hunk, destined for an agent to act on.
_Avoid_: annotation, feedback

**Review**:
The batch of Fix-it Comments accumulated as drafts while reading a changeset and submitted to the agent as one structured instruction set on explicit submit.
_Avoid_: feedback round

**Approve**:
The exit of the Agent Review Loop: the human accepts the changeset and the product instructs the agent to commit it on its Agent Worktree's branch (the agent authors the commit message). After the commit, the product offers a one-keypress merge to the default branch; conflicts are handed to the agent to resolve.
_Avoid_: merge, accept

**Agent Worktree**:
The dedicated git worktree + branch the product creates (or adopts) for each agent pane. The Review Pane is scoped to it, making changeset→agent attribution exact.
_Avoid_: workspace, sandbox

**Ready-for-Review**:
A pane state (surfaced in the sidebar, like herdr's agent statuses) set when an agent finishes work; a keybind on that pane opens the Review Pane. Review is never auto-opened in v1.

**Review Pane**:
The pane rendering a changeset as a continuous multi-file diff stream with Annotations inline and conversation/comment affordances per hunk.
_Avoid_: diff viewer

**Agent Review Loop** (Loop A — v1 scope):
Agent finishes work → Review Pane opens on its changeset → human reads Annotations, holds Hunk Conversations, posts Fix-it Comments → agent acts on them.

**Forge PR Review** (Loop B — deferred to v2):
Opening a GitHub/GitLab PR locally in the Review Pane, commenting, and submitting approve/reject to the forge.
