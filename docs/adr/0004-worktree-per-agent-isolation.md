# Product-managed worktree per agent

Parallel agents sharing one working tree make "this agent's changeset" undefined — hunks cannot be attributed to an Authoring Session. Spawning an agent pane therefore creates (or adopts) a dedicated git worktree and branch for that agent; the Review Pane is scoped to the pane's worktree, so changeset→agent attribution is exact and Approve→commit lands on the agent's branch.

## Considered Options

- **User-managed isolation by convention** — rejected: attribution by hope; two agents colliding in one tree corrupts the review model silently.
- **Shared tree with snapshot-based attribution** — rejected: heuristic attribution with ugly edge cases (two agents editing one file).
- **Product-managed worktrees (chosen)** — most product responsibility (creation, cleanup, branch naming), but the only model where the agent ↔ diff ↔ comment chain is exact. Follows where herdr and tuicr are already heading.
