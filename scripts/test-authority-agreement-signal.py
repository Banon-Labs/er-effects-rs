#!/usr/bin/env python3
"""Behavioral tests for the cupcake signal `last_assistant_authority_agreement`.

The signal script scans the last-completed assistant turn of the session transcript and returns a
TAGGED marker:
  * AUTH:<phrase>         -- Category A authority-coded agreement (banned outright)
  * ACKUNBACKED:<phrase>  -- Category B feedback-acknowledgement prose with NO bd-memory in the turn
  * ""                    -- clean (Category A absent; Category B absent OR backed by a bd memory)

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
SIGNAL = REPO_ROOT / ".cupcake" / "signals" / "last_assistant_authority_agreement.sh"

PROJECT_DIR = "/fake/project/er-effects-rs"


def user(text: str) -> dict:
    return {"type": "user", "message": {"content": text}}


def tool_result() -> dict:
    """A tool-result carrier user event -- must NOT split the assistant turn."""
    return {"type": "user", "message": {"content": [{"type": "tool_result", "content": "ok"}]}}


def assistant_text(text: str) -> dict:
    return {"type": "assistant", "message": {"content": [{"type": "text", "text": text}]}}


def assistant_bd_remember() -> dict:
    return {
        "type": "assistant",
        "message": {
            "content": [
                {
                    "type": "tool_use",
                    "name": "Bash",
                    "input": {"command": "/home/banon/.local/bin/bd remember --key k 'lesson x'"},
                }
            ]
        },
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
    # (1) Ack phrase in a turn WITHOUT a bd-memory recording -> ACKUNBACKED.
    expect(
        "ack-unbacked",
        [user("Stop doing X."), assistant_text("Point taken. I'll adjust the approach.")],
        lambda o: o.startswith("ACKUNBACKED:"),
        "expected ACKUNBACKED for an unbacked acknowledgement",
    )

    # (2) Same ack WITH a bd-memory recording in the same turn -> empty (allowed).
    expect(
        "ack-backed",
        [
            user("Stop doing X."),
            assistant_text("Point taken. Recording it."),
            tool_result(),
            assistant_bd_remember(),
        ],
        lambda o: o == "",
        "expected empty when the ack turn recorded a bd memory",
    )

    # (3) Category A authority-coded agreement -> AUTH regardless of a bd-memory recording.
    expect(
        "auth-wins-over-bd-memory",
        [
            user("The offset is 0x40."),
            assistant_text("You're right, the offset is 0x40."),
            tool_result(),
            assistant_bd_remember(),
        ],
        lambda o: o.startswith("AUTH:"),
        "expected AUTH even when a bd memory was recorded (Category A is outright)",
    )

    # (4) Ack phrase only inside a double-quoted span -> stripped -> empty.
    expect(
        "quoted-only-ack",
        [
            user("Explain the ban."),
            assistant_text('The phrase "Point taken" is banned unless it is backed by a recording.'),
        ],
        lambda o: o == "",
        "expected empty when the ack appears only inside double quotes",
    )

    # (5) Ack phrase in a NON-final block of the turn (later clean block must not mask it) -> detected.
    expect(
        "ack-in-nonfinal-block",
        [
            user("Stop doing X."),
            assistant_text("Point taken."),
            assistant_text("Here is the analysis of the offset table."),
        ],
        lambda o: o.startswith("ACKUNBACKED:"),
        "expected ACKUNBACKED from a whole-turn scan when the ack is not the last block",
    )

    # (6) Interrupted turn: a NEW user prompt after the ack -> the prior turn is still detected.
    expect(
        "interrupted-turn",
        [
            user("Stop doing X."),
            assistant_text("Got it. Proceeding."),
            user("Actually, also do Y."),
        ],
        lambda o: o.startswith("ACKUNBACKED:"),
        "expected ACKUNBACKED on the prior (interrupted) turn",
    )

    # (7) Clean technical prose with incidental words -> empty (no false positive).
    expect(
        "clean-incidental-words",
        [
            user("What is the offset?"),
            assistant_text(
                "As noted above, the correct offset is 0x40; get everyone on board with that value."
            ),
        ],
        lambda o: o == "",
        "expected empty for incidental 'noted'/'correct'/'on board' in technical prose",
    )

    print("authority-agreement signal tests passed (7 cases)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
