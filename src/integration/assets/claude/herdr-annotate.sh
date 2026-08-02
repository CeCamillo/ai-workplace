#!/bin/sh
# installed by herdr
# managed by herdr; reinstalling or updating the integration overwrites this file.
# add custom hooks beside this file instead of editing it.
# HERDR_INTEGRATION_ID=claude-annotate
# HERDR_INTEGRATION_VERSION=1

# SessionStart hook of the Claude Code Harness Adapter: when the session
# runs inside a herdr Agent Worktree, teach the agent the Annotation
# protocol via additionalContext. Everywhere else, stay silent.

set -eu

hook_input="$(cat 2>/dev/null || true)"

[ "${HERDR_ENV:-}" = "1" ] || exit 0
command -v herdr >/dev/null 2>&1 || exit 0
case "$hook_input" in
  *".herdr-agent-worktrees"*) ;;
  *) exit 0 ;;
esac

cat <<'JSON'
{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"This session runs inside a herdr Agent Worktree: a human will review your changeset in herdr's Review Pane, which renders your per-hunk Annotations inline above each hunk. Protocol: right after each cohesive edit, run `herdr annotate --file <repo-relative-path> --line <post-change-line> --what \"<what this hunk does>\" --why \"<why this way, incl. trade-offs>\"` — one command per hunk, at authoring time, never batched at the end. For the full protocol run `herdr annotate --skill`."}}
JSON
exit 0
