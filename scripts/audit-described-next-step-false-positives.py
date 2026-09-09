#!/usr/bin/env python3
"""Replay real past turns through the described-next-step guard and count how many it would halt.

The `no_described_next_step` Stop guard is only worth having if it stays quiet on ordinary turns: a
guard that cries wolf gets ignored, which is worse than no guard, and a false positive here blocks a
turn that was allowed to stop -- a genuine fork, a real blocker, a destructive step awaiting
approval. Unit tests prove the shapes the author thought of; this proves the shapes the author did
not, by running the real signal over the session transcripts the agent has actually written.

For every turn boundary in a transcript it builds a fixture from the preceding window of events and
runs `.cupcake/signals/last_assistant_described_next_step.sh` against it under a temporary home, then
reports every turn that would have been halted and the clause it would have quoted back. Read the
hits: each one is either a real instance of the defect (good) or a false positive to narrow away.

This is how the guard was tuned rather than guessed. The first draft fired on 18 of 2,142 turns, and
reading them deleted two whole pattern families and added the demotion rule:
  * "what is left is ..." reads as a definition, not a prescription;
  * "... is the right move" reads as justification for a move being made now;
  * a phrase after a preposition or inside a subordinate clause prescribes nothing -- "on the next
    run:", "the part that decides the next step:", "a suspect that the next run will clear".
The tuned guard fires on 9 of the same 2,142 turns (0.4%), and each quoted sentence is a next step
that was described and then abandoned.

Usage:
    python3 scripts/audit-described-next-step-false-positives.py [--window=N] [transcript.jsonl ...]

With no arguments it audits the newest transcripts for this repo under ~/.claude/projects/.
Read-only: it never writes to the transcripts, and its fixtures live in a temp dir that is removed.
"""
from __future__ import annotations

import glob
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
SIGNAL = REPO_ROOT / ".cupcake" / "signals" / "last_assistant_described_next_step.sh"

# The fixture handed to the signal is the last window events before a boundary. Enough for the turn
# and its immediate history; older background launches fall outside it, which can only make the guard
# fire more than in production -- the safe direction for a false-positive audit.
WINDOW = 400

FAKE_PROJECT = "/fake/project/er-quickload"
DEFAULT_TRANSCRIPT_COUNT = 10


def is_real_user_prompt(ev: dict) -> bool:
    if ev.get("type") != "user":
        return False
    content = ev.get("message", {}).get("content")
    if isinstance(content, str):
        return content.strip() != ""
    if isinstance(content, list):
        return not any(isinstance(b, dict) and b.get("type") == "tool_result" for b in content)
    return False


def default_transcripts() -> list[str]:
    key = str(REPO_ROOT).replace("/", "-")
    tdir = Path(os.path.expanduser("~/.claude/projects")) / key
    files = sorted(glob.glob(str(tdir / "*.jsonl")), key=os.path.getmtime, reverse=True)
    return files[:DEFAULT_TRANSCRIPT_COUNT]


def would_halt(out: str) -> bool:
    """Apply the policy's rule to a facts line.

    The signal emits facts on every turn that names a next step, exempt or not, so a non-empty line
    is not a halt. The rule that turns facts into a verdict lives in
    `.cupcake/policies/claude/no_described_next_step.rego`, and it is restated here rather than
    guessed at: a next step was named, nothing began it, the user was not asked, no blocker was
    stated, and nothing is carrying it. Counting bare output instead would inflate the false-positive
    number with the very turns the exemptions exist to let through.
    """
    if not out.startswith("NEXTSTEPFACTS|"):
        return False
    fields = {}
    for part in out.split("|"):
        if "=" in part:
            k, _, v = part.partition("=")
            fields[k] = v
    return bool(fields.get("nextstep")) and all(
        fields.get(k, "0") == "0" for k in ("acted", "blocked", "handoff", "carried")
    )


def audit(path: str, home: str) -> tuple[int, list[tuple[int, str]]]:
    lines = Path(path).read_text(encoding="utf-8", errors="replace").splitlines()
    boundaries = []
    for i, line in enumerate(lines):
        try:
            ev = json.loads(line)
        except ValueError:
            continue
        if isinstance(ev, dict) and is_real_user_prompt(ev):
            boundaries.append(i)
    boundaries.append(len(lines))  # the turn still open at the end of the transcript

    fixture_dir = Path(home) / ".claude" / "projects" / FAKE_PROJECT.replace("/", "-")
    fixture_dir.mkdir(parents=True, exist_ok=True)
    fixture = fixture_dir / "session.jsonl"

    fires: list[tuple[int, str]] = []
    for boundary in boundaries:
        chunk = lines[max(0, boundary - WINDOW):boundary]
        if not chunk:
            continue
        fixture.write_text("\n".join(chunk) + "\n", encoding="utf-8")
        proc = subprocess.run(
            ["bash", str(SIGNAL)],
            cwd=REPO_ROOT,
            text=True,
            capture_output=True,
            timeout=25,
            env={**os.environ, "HOME": home, "CLAUDE_PROJECT_DIR": FAKE_PROJECT},
        )
        out = proc.stdout.strip()
        if would_halt(out):
            fires.append((boundary, out))
    return len(boundaries), fires


def main() -> int:
    args = sys.argv[1:]
    global WINDOW
    if args and args[0].startswith("--window="):
        WINDOW = int(args[0].split("=", 1)[1])
        args = args[1:]
    paths = args or default_transcripts()
    if not paths:
        print("no transcripts found to audit", file=sys.stderr)
        return 1
    total_turns = 0
    all_fires: list[tuple[str, int, str]] = []
    with tempfile.TemporaryDirectory() as home:
        for path in paths:
            turns, fires = audit(path, home)
            total_turns += turns
            all_fires.extend((Path(path).name[:8], b, o) for b, o in fires)
            print(f"{Path(path).name[:8]}: {turns} turns, {len(fires)} would halt", flush=True)
    print(f"\nTOTAL: {len(all_fires)} halts across {total_turns} real turns")
    for name, boundary, out in all_fires:
        print(f"  {name} line {boundary}: {out[:200]}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
