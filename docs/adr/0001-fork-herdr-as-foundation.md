# Fork herdr as the product foundation

We are building a unified terminal product: an agent multiplexer with an integrated, agent-aware code-review surface (per-hunk agent explanations and conversation, review comments routed back to agents, and local PR review with forge submission). Rather than gluing the existing tools (herdr + hunk + tuicr) together or building from scratch, we fork herdr (Apache-2.0, verified 2026-08-02) and build the review layer natively on top of it, all in Rust.

## Considered Options

- **Glue/orchestration layer over the three real binaries** — rejected: cannot deliver the per-hunk agent conversation or a unified agent ↔ diff ↔ comment data model; only pane choreography.
- **Ground-up unified product** — rejected: 6–12 months before anything usable; re-implements a multiplexer core (PTY management, sessions, detach/reattach, SSH) that herdr has already built and battle-tested at ~23k-star scale.
- **Fork herdr (chosen)** — inherits the multiplexer core, tmux keybinds, session persistence, and agent-status detection for free; hunk's and tuicr's review experiences are reimplemented natively in Rust inside it. hunk is TypeScript so its UI could never be embedded anyway — its *ideas* (inline agent annotations, live watch) are what we take.

## Consequences

- We inherit herdr's architecture and pre-1.0 protocol churn; we must decide how (and whether) to track upstream.
- The review surface (hunk/tuicr functionality) is net-new Rust code — no code reuse from those two projects, UX reference only.
