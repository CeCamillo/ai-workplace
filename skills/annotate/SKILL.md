---
name: herdr-annotate
description: "Emit per-hunk Annotations while authoring changes inside a herdr Agent Worktree, so the human reviewer sees what each change does and why, inline in herdr's Review Pane. Use whenever you are editing files in a pane managed by herdr (HERDR_ENV=1) whose working directory is a herdr Agent Worktree."
---

# Annotating your changes for herdr's Review Pane

You are working inside a herdr **Agent Worktree**: a dedicated git worktree
herdr created for this session. When you finish, a human reviews your
changeset in herdr's Review Pane — a continuous diff with your
**Annotations** rendered inline above each hunk. An Annotation explains one
hunk: **what** you did and **why** you did it that way. Annotations are your
rationale persisted with the diff; they outlive this session, and the
reviewer relies on them instead of asking you.

## When to annotate

Emit one Annotation per hunk you author, at authoring time — right after
each cohesive edit, while the reasoning is fresh. Do not batch them all at
the end of the task, and never skip hunks: an unannotated hunk is a hunk
the reviewer has to reverse-engineer.

## How to annotate

Run, from anywhere inside the worktree:

```bash
herdr annotate --file <repo-relative-path> --line <line> --what "<what>" --why "<why>"
```

- `--file`: the changed file's path, relative to the repository root, as it
  is named after your change.
- `--line`: any line number inside the changed region, counted in the
  post-change file (the line numbers you see after your edit).
- `--what`: what this hunk does, one sentence, imperative ("swap old() for
  new()", "add retry around the flaky call").
- `--why`: the reason and the trade-off, one or two sentences ("old() is
  deprecated in 2.0; new() keeps the same semantics"). This is the part the
  reviewer cannot see in the diff — never restate the diff here.

One command per hunk. If one edit produces several hunks in a file,
annotate each hunk separately with its own `--line`.

## What good Annotations look like

- `--what "guard config reload against empty paths" --why "a watcher event can fire before the file exists; reloading then wiped the live config"`
- `--what "extract parse_range() from the header parser" --why "the review stream needs the same parsing; duplicating it drifted once already"`

Weak (do not emit these): restating the diff ("change line 12"), vague
intent ("improve code"), or missing trade-offs when you made a real choice.

## Failure handling

If `herdr annotate` fails (not a git worktree, detached HEAD), continue
your task and mention the failure in your final summary. Do not create any
annotation files inside the repository yourself — annotations live in
herdr's own state store, never in the worktree.
