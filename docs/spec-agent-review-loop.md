# Spec: Agent Review Loop (v1)

> Draft for publication to the issue tracker with label `ready-for-agent`.

## Problem Statement

Developers running AI coding agents in the terminal have no unified place to supervise them. Watching several agents means tab-cycling with no status signal; understanding what an agent changed means squinting at a linear `git diff` with no explanation of intent; giving feedback means pasting fragments into a chat prompt; and nothing connects "this hunk" to "the agent that wrote it and why." The review step — the human's actual job in agentic coding — is scattered across tools that don't know about each other.

## Solution

A terminal multiplexer for AI agents (a tracking fork of herdr, all Rust, with native tmux keybinds) with a built-in, agent-aware review surface. Each agent pane gets its own Agent Worktree, so every changeset is attributable to one Authoring Session. As the agent works, its Harness Adapter captures per-hunk Annotations (what/why). When the agent finishes, its pane turns Ready-for-Review; one keybind opens the Review Pane — a continuous multi-file diff with Annotations inline and vim navigation. From any hunk the human can open a Hunk Conversation (routed to the live Authoring Session, or a fresh seeded session if it's gone) or draft Fix-it Comments, which submit as one batched Review the agent acts on. Approve instructs the agent to commit on its branch, then offers a one-keypress merge to the default branch, with conflicts handed back to the agent.

## User Stories

1. As an agent operator, I want to spawn agent panes inside a multiplexer with native tmux keybinds, so that my existing muscle memory works from day one.
2. As an agent operator, I want each agent pane to get its own Agent Worktree (worktree + branch) automatically, so that parallel agents never interleave edits in one working tree.
3. As an agent operator, I want to adopt an existing worktree/branch when spawning an agent pane, so that I can resume prior work instead of always starting fresh.
4. As an agent operator, I want a sidebar showing each agent's status (working / blocked / done), inherited from herdr, so that I can see where my attention is needed across the herd.
5. As an agent operator, I want a pane to enter Ready-for-Review state when its agent finishes, so that reviewable work is visible without stealing my focus.
6. As an agent operator, I want a keybind on a Ready-for-Review pane that opens the Review Pane on that agent's changeset, so that entering review is one action.
7. As a reviewer, I want the Review Pane to render the changeset as a continuous top-to-bottom stream across all files with a file sidebar, so that I read the change as one narrative instead of flipping per-file.
8. As a reviewer, I want vim keybindings (j/k, Ctrl-d/u, [ ] between hunks/files) in the Review Pane, so that navigation feels native to my hands.
9. As a reviewer, I want each hunk's Annotation (what the agent did and why) rendered inline above the hunk, so that I understand intent without asking.
10. As an agent, I want the Harness Adapter to prompt me to emit an Annotation per hunk as I author changes, so that my rationale is persisted with the diff rather than lost with my session.
11. As a reviewer, I want to open a Hunk Conversation on any hunk and ask questions ("why this way?", "what's the trade-off?"), so that I can learn from and interrogate the change in place.
12. As a reviewer, I want Hunk Conversations routed to the live Authoring Session when it still exists, so that answers come from the memory that actually made the decision.
13. As a reviewer, I want a fresh agent session spawned and seeded with the stored Annotation and diff context when the Authoring Session is gone, so that conversations are always available.
14. As a reviewer, I want to draft Fix-it Comments on any hunk (add, edit, delete before submit), so that I can mark up the whole changeset at my own pace.
15. As a reviewer, I want to mark files or hunks as reviewed with progress persisted, so that I can leave and resume a large review without losing my place.
16. As a reviewer, I want to submit all drafted Fix-it Comments as one batched Review with a single action, so that the agent receives a coherent instruction set instead of piecemeal edits.
17. As an agent, I want the submitted Review delivered as structured instructions with file/line anchors, so that I can locate and apply every fix precisely.
18. As a reviewer, I want the Review Pane to refresh the diff after the agent applies my Review, keeping my reviewed-marks where content is unchanged, so that re-review only costs me the parts that moved.
19. As a reviewer, I want to Approve the changeset when satisfied, so that the agent commits it on its branch with a commit message it authors.
20. As an agent operator, I want a one-keypress merge offer to the default branch after the commit lands, so that the loop can end with code on main without leaving the product.
21. As an agent operator, I want merge conflicts handed to the agent to resolve, so that mechanical git pain stays the agent's job.
22. As an agent operator, I want to decline the merge offer and leave the branch as-is, so that I can stack further work or PR it through my own workflow.
23. As an agent operator, I want to discard a changeset I don't want (reset the Agent Worktree), so that a bad direction dies cleanly instead of lingering.
24. As an agent operator, I want sessions (panes, worktree bindings, review progress, annotations) to survive detach/reattach, so that the product behaves like the multiplexer it is.
25. As a terminal user, I want the whole product distributed as a single binary, so that installation stays `brew install`-simple.

## Implementation Decisions

- The product is a tracking fork of herdr (Apache-2.0; license and NOTICE attribution retained), public OSS from day one. Upstream is merged regularly; all net-new code is additive (ADR-0002).
- All net-new logic lives in one headless module: the **Review Loop Engine** — an event-in/effect-out core owning the full loop state machine (worktree lifecycle → annotation ingest → review → conversation → batched submit → approve → commit → merge offer).
- The engine touches the world only through three ports:
  - **AgentPort** — the Harness Adapter seam: deliver instructions, receive Annotations, query Authoring Session liveness, spawn seeded sessions. v1 ships exactly one implementation: Claude Code (hooks + a bundled skill teaching the annotation protocol).
  - **GitPort** — worktree create/adopt/discard, diff computation, commit, merge, conflict detection.
  - **UiPort** — a declarative render model consumed by the Review Pane TUI; the pane draws state, the engine owns it.
- herdr's core is modified only at two attachment points: hosting the Review Pane as a pane type, and surfacing Ready-for-Review in the sidebar status model.
- Annotations are persisted in the product's own state store (alongside herdr session state), keyed by Agent Worktree and commit range — not as sidecar files inside the repo, which would pollute worktrees and diffs.
- Fix-it Comments are drafts local to the review until explicit submit; a submitted Review is delivered to the agent as one structured instruction set with file/line anchors (format also chosen to map 1:1 onto forge PR review submission in v2).
- Hunk Conversation routing: live Authoring Session first, fresh seeded session as fallback (ADR-0003).
- Worktree-per-agent is product-managed (ADR-0004); changeset→agent attribution is exact by construction.
- tmux keybinds govern the multiplexer layer (inherited from herdr); vim keybinds govern the Review Pane.

## Testing Decisions

- Tests assert external behavior at one seam: the Review Loop Engine's public API. Feed events, assert on emitted effects and observable state — never on internal structure.
- All three ports are faked in engine tests (scripted fake agent, in-memory fake git, captured render models). The full Agent Review Loop — spawn → annotate → review → converse → submit → approve → merge — must be executable as a pure engine test with no PTY, TUI, or real git.
- Real port implementations get thin integration tests only: the Claude Code adapter round-trips an Annotation and an instruction against a real session; GitPort operates on a real temporary repo; UiPort render models snapshot-render in a test terminal.
- Upstream herdr functionality is not re-tested; our suites cover only the two attachment points and everything behind the engine seam.
- Prior art: greenfield repo — these suites establish the house style; herdr's existing test conventions apply to any code living in its crates.

## Out of Scope

- Forge PR review (Loop B): opening PRs locally, posting comments to GitHub/GitLab, approve/reject on the forge — v2.
- Harness adapters beyond Claude Code (Codex, OpenCode, Devin, …) — the AgentPort seam admits them later.
- Auto-opening the Review Pane on agent completion (opt-in config, later).
- Instant per-comment send to the agent (batched submit only in v1).
- Product naming/branding — building under the working name `ai-workplace`.
- Upstreaming changes to herdr (desirable per ADR-0002, but no work item in v1).
- Windows support beyond whatever the herdr fork inherits.

## Further Notes

- Glossary: `CONTEXT.md` (Agent, Harness Adapter, Authoring Session, Annotation, Hunk Conversation, Fix-it Comment, Review, Approve, Ready-for-Review, Review Pane, Agent Worktree, Agent Review Loop). Use these terms in all tickets, code, and tests.
- Decisions on record: ADR-0001 (fork herdr), ADR-0002 (tracking fork, additive modules), ADR-0003 (annotation capture + conversation routing), ADR-0004 (worktree-per-agent).
- Known design work deliberately left to implementation: the concrete annotation protocol payload (what the Claude Code skill/hooks emit), reviewed-mark preservation heuristics across diff refreshes, and worktree cleanup policy after merge/discard.
