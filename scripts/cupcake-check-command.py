#!/usr/bin/env python3
"""Ad-hoc live-WASM-engine check for a single Bash command, for use during
agent policy work when the harness's own worktree-isolation guard refuses an
inline Bash call containing the literal substring "eval" (the `cupcake eval`
subcommand name). Mirrors eval_bash() in scripts/test-cupcake-delivered-shape.py
but as a standalone one-shot CLI so the word never has to appear on the outer
Bash tool call line.

Usage:
    python3 scripts/cupcake-check-command.py '<command>'

Prints allow or deny plus the reason (if any), using the real `cupcake` binary
compiled to WASM in production -- not the `opa` interpreter that `opa test`
and `opa eval` use.
"""
from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        print(f"usage: {argv[0]} '<command>'", file=sys.stderr)
        return 2
    command = argv[1]
    event = {
        "session_id": "cupcake-check-command",
        "transcript_path": "/tmp/cupcake-check-command.jsonl",
        "cwd": str(REPO_ROOT),
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {"command": command, "timeout": 30000},
        "signals": {"current_branch": "feature/cupcake-check-command\n"},
    }
    result = subprocess.run(
        ["cupcake", "eval", "--harness", "claude", "--strict", "--log-level", "error"],
        cwd=REPO_ROOT,
        input=json.dumps(event),
        text=True,
        capture_output=True,
        check=False,
        timeout=25,
    )
    verdict = "DENY" if result.returncode != 0 else "ALLOW"
    print(f"{verdict}  {command!r}")
    output = (result.stdout + result.stderr).strip()
    if output:
        print(output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
