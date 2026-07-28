---
name: watch-workflow
description: Open a live, ANSI-colored kitty tab tailing a running dynamic-workflow (Workflow tool) run — its per-agent tool calls, results, and final structured outputs. Use when the user wants to observe/monitor a running workflow more richly than the condensed /workflows TUI, or asks to "watch/tail/see the workflow" in a terminal tab.
---

# watch-workflow

Spawns a new **kitty tab** that live-tails a dynamic-workflow run directory with color and formatting, so the user can watch a running Workflow's real activity (agent text, tool calls, tool results, per-agent final results) instead of the condensed `/workflows` tree.

## How to run it

Run the launcher, passing the run id if the user named one (else it picks the latest run under this project):

```bash
bash scripts/watch-workflow.sh [RUNID|latest] [--full] [--result-lines N]
```

- `RUNID` — a `wf_...` id (from a Workflow tool result). Omit or `latest` = newest run.
- `--full` — include agent reasoning text, not just tool calls + results.
- `--result-lines N` — cap each command result at N lines (default 24; `0` = uncapped/full). The cap is DISPLAY-ONLY — the sub-agent always read the full result from its transcript; this only shortens the terminal view. Truncated lines were never printed, so raise the cap (or use `0`) to see them; they are not recoverable from kitty scrollback.

The launcher resolves the run dir, opens a kitty tab (via `KITTY_LISTEN_ON` remote control, `--keep-focus` so it does not steal focus from Claude Code), and runs `scripts/watch-workflow.py` in follow mode there. If kitty remote control is off it opens a separate kitty window; with no kitty it runs inline.

## Notes

- The run dir is `~/.claude/projects/<proj>/<session>/subagents/workflows/wf_<id>/` (`journal.jsonl` = workflow-level started/result events; `agent-<id>.jsonl` = per-agent transcript). The renderer follows all of them live and prints `═══ RUN COMPLETE ═══` once every started agent has a result.
- This is read-only observability. To *steer* a running workflow, use the `.workflow-steering/<label>/steer.md` channel (see `scripts/steering/README.md`), which is separate.
- If the user wants to watch a run that is not the latest, get its `wf_...` id from the Workflow tool result and pass it as `RUNID`.
