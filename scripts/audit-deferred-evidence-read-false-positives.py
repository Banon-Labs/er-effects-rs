#!/usr/bin/env python3
"""Replay real past turns through the deferred-evidence guard and count how many it would halt.

`no_deferred_evidence_read` is only worth having if it stays quiet on ordinary turns. A guard that
cries wolf gets ignored, which is worse than no guard, and a false positive here blocks a turn that
was allowed to stop: a genuine fork, a real blocker, a deferral to evidence a run has still to write.
Unit tests prove the shapes the author thought of; this proves the shapes the author did not, by
running the real signal over the session transcripts the agent has actually written.

For every turn boundary it builds a fixture from the preceding window of events, runs
`.cupcake/signals/last_assistant_deferred_evidence_read.sh` against it under a temporary home, and
applies the policy's conjunction. Two numbers come out, because this rule yields to a sibling:

  raw       -- the deferrals this signal recognises with none of its five exemptions set.
  effective -- the same, minus the ones `.cupcake/signals/last_assistant_diagnosis_without_fix.sh`
               already recognises through its `unread` fact, which is what the policy's
               `sibling_unread` conjunct removes. This is the rate a session actually sees from this
               rule, and the raw number is what it would be if that sibling arm were removed.

`--raw-only` skips the sibling signal and halves the runtime when only the upper bound is wanted.

Measured 2026-09-09 over 1,836 turn boundaries from the 20 newest transcripts: 2 raw halts (0.11%),
both yielded, 0 effective. The first draft fired on 10, and reading all ten is what produced the
adverb anchor, the clause-end anchor on the object of "the log says ...", five further future
spellings, and the discovery that a dot inside a filename was being read as a sentence break. Each is
recorded beside the pattern it constrains in the signal.

Usage:
    python3 scripts/audit-deferred-evidence-read-false-positives.py [--window=N] [--raw-only]
                                                                   [transcript.jsonl ...]

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
SIGNAL = REPO_ROOT / ".cupcake" / "signals" / "last_assistant_deferred_evidence_read.sh"
SIBLING = REPO_ROOT / ".cupcake" / "signals" / "last_assistant_diagnosis_without_fix.sh"

# The fixture handed to the signal is the last window of events before a boundary: enough for the
# turn and its immediate history. Older background launches fall outside it, which can only make the
# guard fire more often than in production -- the safe direction for a false-positive audit.
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


def fields(out: str, tag: str) -> dict[str, str]:
    if not out.startswith(tag):
        return {}
    parsed = {}
    for part in out.split("|"):
        if "=" in part:
            k, _, v = part.partition("=")
            parsed[k] = v
    return parsed


def would_halt(out: str) -> bool:
    """The policy's conjunction, restated rather than guessed at.

    The signal emits facts for every deferral it recognises, exempt or not, so a non-empty line is
    not a halt. The verdict lives in `.cupcake/policies/claude/no_deferred_evidence_read.rego`:
    evidence was named, nothing opened it, it exists, the user is not needed, no blocker was stated,
    and nothing is carrying the read. Counting bare output instead would inflate the number with the
    very turns the exemptions exist to let through.
    """
    parsed = fields(out, "DEFERFACTS|")
    return bool(parsed.get("deferral")) and all(
        parsed.get(k, "0") == "0"
        for k in ("consulted", "future", "userneed", "blocked", "carried")
    )


def run_signal(script: Path, home: str) -> str:
    proc = subprocess.run(
        ["bash", str(script)],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        timeout=25,
        env={**os.environ, "HOME": home, "CLAUDE_PROJECT_DIR": FAKE_PROJECT},
    )
    return proc.stdout.strip()


def audit(path: str, home: str, raw_only: bool) -> tuple[int, list[tuple[int, str, bool]]]:
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

    fires: list[tuple[int, str, bool]] = []
    for boundary in boundaries:
        chunk = lines[max(0, boundary - WINDOW):boundary]
        if not chunk:
            continue
        fixture.write_text("\n".join(chunk) + "\n", encoding="utf-8")
        out = run_signal(SIGNAL, home)
        if not would_halt(out):
            continue
        yielded = False
        if not raw_only and SIBLING.is_file():
            yielded = bool(fields(run_signal(SIBLING, home), "DIAGFACTS|").get("unread"))
        fires.append((boundary, out, yielded))
    return len(boundaries), fires


def main() -> int:
    args = sys.argv[1:]
    global WINDOW
    raw_only = False
    while args and args[0].startswith("--"):
        if args[0].startswith("--window="):
            WINDOW = int(args[0].split("=", 1)[1])
        elif args[0] == "--raw-only":
            raw_only = True
        else:
            print(f"unknown option {args[0]}", file=sys.stderr)
            return 2
        args = args[1:]
    paths = args or default_transcripts()
    if not paths:
        print("no transcripts found to audit", file=sys.stderr)
        return 1
    total_turns = 0
    all_fires: list[tuple[str, int, str, bool]] = []
    with tempfile.TemporaryDirectory() as home:
        for path in paths:
            turns, fires = audit(path, home, raw_only)
            total_turns += turns
            all_fires.extend((Path(path).name[:8], b, o, y) for b, o, y in fires)
            print(f"{Path(path).name[:8]}: {turns} turns, {len(fires)} raw halts", flush=True)
    effective = [f for f in all_fires if not f[3]]
    print(f"\nRAW: {len(all_fires)} halts across {total_turns} real turns")
    if not raw_only:
        print(f"EFFECTIVE (after the sibling arm takes its own): {len(effective)}")
    for name, boundary, out, yielded in all_fires:
        mark = "yielded" if yielded else "halts  "
        print(f"  [{mark}] {name} line {boundary}: {out[:220]}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
