#!/usr/bin/env python3
"""Replay real past turns through the unexecuted-promise guard and count how many it would halt.

The `no_unexecuted_promise` Stop guard is only worth having if it stays quiet on ordinary turns: a
guard that cries wolf gets ignored, which is worse than no guard. Unit tests prove the shapes the
author thought of; this proves the shapes the author did not think of, by running the real signal
over the session transcripts the agent has actually written.

For every turn boundary in a transcript it builds a fixture from the preceding window of events and
runs `.cupcake/signals/last_assistant_unexecuted_promise.sh` against it under a temporary home, then
reports every turn that would have been halted and the clause it would have quoted back. Read the
hits: each one is either a real instance of the defect (good) or a false positive to narrow away.

Usage:
    python3 scripts/audit-unexecuted-promise-false-positives.py [--window=N] [transcript.jsonl ...]

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
SIGNAL = REPO_ROOT / ".cupcake" / "signals" / "last_assistant_unexecuted_promise.sh"

# The fixture handed to the signal is the last window events before a boundary. Enough for the turn
# and its immediate history; older background launches fall outside it, which can only make the guard
# fire more than in production -- the safe direction for a false-positive audit.
WINDOW = 400

FAKE_PROJECT = "/fake/project/er-quickload"
DEFAULT_TRANSCRIPT_COUNT = 3


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
        if out:
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
