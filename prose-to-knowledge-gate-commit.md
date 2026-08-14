# Prose-to-knowledge gate commit findings

Committed `78a57ea1` (`docs: add prose-to-knowledge gate`).

The gate in `AGENTS.md` is explicitly limited by "If and only if" to cases where the agent's own recent user-facing prose treated an entity, identifier, plan node, claim, or term as meaningful without enough information to explain it. It requires a plain user-visible admission of ignorance before every relevant clarifying search, lookup, or inspection in that turn.

Validation: `git diff --check` passed before staging; the staged file list contained only `AGENTS.md`; `git diff --cached --check` passed before commit. No tests were added. No interactive Pi smoke passed or was claimed: the prior smoke controller had no Pi/Kitty child and was terminated.

The required findings artifact is intentionally untracked and was not staged.
