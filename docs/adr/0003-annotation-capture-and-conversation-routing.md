# Annotations captured at authoring time; hunk conversations routed live-first

Per-hunk explanations cannot rely on the authoring agent's session still existing when the human reviews. We therefore split the mechanic: the harness adapter has the agent emit an Annotation (what/why) per hunk at authoring time, persisted with the diff; a Hunk Conversation routes to the live Authoring Session when it still exists, and otherwise spawns a fresh agent session seeded with the stored Annotation plus diff context.

## Considered Options

- **Always inject into the live authoring session** — rejected: history is lost when the session closes/compacts, and mid-task injection derails a working agent.
- **Always spawn a fresh scoped session** — rejected: "why did you build it this way" becomes reconstruction, not memory.
- **Hybrid (chosen)** — best answer in every case; the cost is two routing paths and an annotation persistence layer, accepted deliberately.
