# Tracking fork of upstream herdr

herdr is pre-1.0 and ships fast; its improvements (status detection, agent integrations, SSH/ConPTY work) are value we want to keep inheriting. We therefore maintain a tracking fork: upstream herdr is merged regularly, and all of our code — the entire review layer — lives in isolated, additive modules/crates with minimal edits to herdr's core, specifically so merges stay cheap.

## Consequences

- Any feature that would require invasive changes to herdr's core should first be attempted as an upstream contribution or an extension-point request, and only then as a carried patch.
- We accept living with herdr's core architecture decisions while tracking.
- Revisit if merge pain ever exceeds inherited value (e.g. post-1.0 divergence becomes affordable).
