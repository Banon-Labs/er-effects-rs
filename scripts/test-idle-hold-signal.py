#!/usr/bin/env python3
"""Behavioral tests for the cupcake signal `last_assistant_idle_hold`.

The signal script scans the last-completed assistant turn of the session transcript and returns a
TAGGED marker:
  * IDLEHOLD:<phrase>  -- an unjustified idle/hold announcement while a background task runs
  * ""                 -- clean (no hold language, OR the hold is justified / accompanied by
                          substantive non-overlapping work / blocked on the user)

We drive it against crafted transcript JSONL under a temporary HOME so the script's
`~/.claude/projects/<cwd-key>/*.jsonl` discovery resolves to our fixture, then assert the tag.
"""
from __future__ import annotations

import json
import os
import subprocess
import tempfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
SIGNAL = REPO_ROOT / ".cupcake" / "signals" / "last_assistant_idle_hold.sh"

PROJECT_DIR = "/fake/project/er-effects-rs"


def user(text: str) -> dict:
    return {"type": "user", "message": {"content": text}}


def tool_result() -> dict:
    """A tool-result carrier user event -- must NOT split the assistant turn."""
    return {"type": "user", "message": {"content": [{"type": "tool_result", "content": "ok"}]}}


def assistant_text(text: str) -> dict:
    return {"type": "assistant", "message": {"content": [{"type": "text", "text": text}]}}


def assistant_bash(command: str) -> dict:
    return {
        "type": "assistant",
        "message": {"content": [{"type": "tool_use", "name": "Bash", "input": {"command": command}}]},
    }


def assistant_edit() -> dict:
    return {
        "type": "assistant",
        "message": {
            "content": [{"type": "tool_use", "name": "Edit", "input": {"file_path": "/x/y.rs"}}]
        },
    }


def assistant_agent() -> dict:
    return {
        "type": "assistant",
        "message": {"content": [{"type": "tool_use", "name": "Agent", "input": {"prompt": "go"}}]},
    }


def run_signal(events: list[dict]) -> str:
    """Write events to a fixture transcript under a temp HOME and return the signal's stdout."""
    with tempfile.TemporaryDirectory() as home:
        key = PROJECT_DIR.replace("/", "-")
        tdir = Path(home) / ".claude" / "projects" / key
        tdir.mkdir(parents=True, exist_ok=True)
        with (tdir / "session.jsonl").open("w", encoding="utf-8") as fh:
            for ev in events:
                fh.write(json.dumps(ev) + "\n")
        proc = subprocess.run(
            ["bash", str(SIGNAL)],
            cwd=REPO_ROOT,
            text=True,
            capture_output=True,
            timeout=25,
            env={**os.environ, "HOME": home, "CLAUDE_PROJECT_DIR": PROJECT_DIR},
        )
        return proc.stdout.strip()


def expect(name: str, events: list[dict], predicate, describe: str) -> None:
    out = run_signal(events)
    if not predicate(out):
        raise AssertionError(f"{name}: {describe} (got {out!r})")


def main() -> int:
    # (1) A bare holding announcement with no work -> IDLEHOLD.
    expect(
        "hold-alone",
        [user("Kick off the RE."), assistant_text("I'm holding for the RE subagent to finish.")],
        lambda o: o.startswith("IDLEHOLD:"),
        "expected IDLEHOLD for a bare hold announcement with no work",
    )

    # (2a) Same hold + a substantive Bash tool_use in the turn -> not flagged.
    expect(
        "hold-plus-substantive-bash",
        [
            user("Kick off the RE."),
            assistant_text("I'm holding for the RE subagent. Meanwhile, checking the disassembly."),
            tool_result(),
            assistant_bash("cargo xwin build --release --target x86_64-pc-windows-msvc"),
        ],
        lambda o: o == "",
        "expected empty when the hold turn also runs substantive work",
    )

    # (2b) Same hold + an Edit tool_use in the turn -> not flagged.
    expect(
        "hold-plus-edit",
        [
            user("Kick off the RE."),
            assistant_text("Standing by for the subagent; prepping the next fix now."),
            tool_result(),
            assistant_edit(),
        ],
        lambda o: o == "",
        "expected empty when the hold turn also edits a file",
    )

    # (2c) Same hold + an Agent tool_use in the turn -> not flagged.
    expect(
        "hold-plus-agent",
        [
            user("Kick off the RE."),
            assistant_text("I'll wait for the build. Launching a parallel investigation."),
            tool_result(),
            assistant_agent(),
        ],
        lambda o: o == "",
        "expected empty when the hold turn also launches a subagent",
    )

    # (3) Same hold + justification prose -> not flagged.
    expect(
        "hold-plus-justification",
        [
            user("Kick off the RE."),
            assistant_text(
                "I'm holding for the RE subagent. I would normally have prepped the next fix "
                "but the fix depends entirely on the subagent's finding, so there is nothing to pull forward."
            ),
        ],
        lambda o: o == "",
        "expected empty when the hold is accompanied by justification prose",
    )

    # (4) A status-peek-only Bash (tail/wc of a log) does NOT count as substantive -> still flagged.
    expect(
        "hold-plus-status-peek-only",
        [
            user("Kick off the RE."),
            assistant_text("I'm holding for the subagent; just peeking at the log."),
            tool_result(),
            assistant_bash("tail -n 40 target/run.log | wc -l"),
        ],
        lambda o: o.startswith("IDLEHOLD:"),
        "expected IDLEHOLD when the only Bash call is a status/log peek",
    )

    # (5) No holding language at all -> clean.
    expect(
        "no-hold-language",
        [
            user("What is the offset?"),
            assistant_text("The offset is 0x40; I verified it against the disassembly."),
        ],
        lambda o: o == "",
        "expected empty for ordinary technical prose with no hold language",
    )

    # (6) A wait genuinely BLOCKED ON THE USER is legitimate -> not flagged.
    expect(
        "wait-blocked-on-user",
        [
            user("I'll drive the menu."),
            assistant_text("I'll wait for the user to confirm the load finished before I continue."),
        ],
        lambda o: o == "",
        "expected empty when the wait is blocked on the user",
    )

    # (7) A hold phrase only inside a double-quoted span -> stripped -> clean.
    expect(
        "quoted-only-hold",
        [
            user("Explain the ban."),
            assistant_text('The phrase "I\'m holding" is banned unless the turn also does real work.'),
        ],
        lambda o: o == "",
        "expected empty when the hold phrase appears only inside double quotes",
    )

    # (8) Hold in a NON-final block of the turn (a later clean block must not mask it) -> detected.
    expect(
        "hold-in-nonfinal-block",
        [
            user("Kick off the RE."),
            assistant_text("I'm holding for the subagent."),
            assistant_text("The subagent will report the function signature soon."),
        ],
        lambda o: o.startswith("IDLEHOLD:"),
        "expected IDLEHOLD from a whole-turn scan when the hold is not the last block",
    )

    # (9) Interrupted turn: a NEW user prompt after the hold -> the prior turn is still detected.
    expect(
        "interrupted-turn",
        [
            user("Kick off the RE."),
            assistant_text("Standing by for the results."),
            user("Actually, also do Y."),
        ],
        lambda o: o.startswith("IDLEHOLD:"),
        "expected IDLEHOLD on the prior (interrupted) turn",
    )

    print("idle-hold signal tests passed (9 cases)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
