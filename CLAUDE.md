# ai-workplace

Tracking fork of [herdr](https://github.com/herdrdev/herdr) (upstream remote: `upstream`). For herdr development guidance (architecture principles, testing, `just` recipes), see `AGENTS.md`. Note: upstream's CLAUDE.md is a symlink to AGENTS.md; this fork keeps CLAUDE.md as a regular file — resolve future merge conflicts here by keeping this file and taking upstream's changes in AGENTS.md.

## Agent skills

### Issue tracker

Issues live in this repo's GitHub Issues via the `gh` CLI (remote not yet connected). See `docs/agents/issue-tracker.md`.

### Domain docs

Single-context: `CONTEXT.md` and `docs/adr/` at the repo root. See `docs/agents/domain.md`.
