#!/usr/bin/env python3
"""Replay real past turns through the future-commitment guard and count how many it would halt.

The `no_future_tense_commitment` Stop guard is only worth having if it stays quiet on ordinary
turns. A guard that cries wolf gets ignored, which is worse than no guard, and a false positive here
blocks a turn that was entitled to stop -- one waiting on an observation only the user can make, one
whose work is genuinely carried by a run it started, one answering a direct question about the plan.
Unit tests prove the shapes the author thought of; this proves the shapes the author did not, by
running the real signal over the session transcripts this repo has actually produced.

For every turn boundary in a transcript it builds a fixture from the preceding window of events,
runs `.cupcake/signals/last_assistant_future_commitment.sh` against it under a temporary home, and
applies the policy's own conjunction to the facts line. Read the hits: each one is either a real
instance of the defect (good) or a false positive to narrow away.

This is how the guard was tuned rather than guessed. The first draft halted 23 of 2,643 real turn
boundaries (0.88%); reading them moved the verb scan to verb positions only, took `pull` out of the
version-control group, widened the offer exemption into a conditional one, and made the delegation
arm require a clause-initial subject and two named deliverables. The tuned guard halts 2 of the same
2,643 (0.08%), and both are the verbatim instances it was written for.

Both arms are counted separately, because they fail differently:
  * `commit`     -- a first-person future-tense promise closing the turn, of a class the turn never
                    performed;
  * `delegation` -- a claim about what a just-dispatched subagent contains or will produce, made
                    before its completion notification exists.

Usage:
    python3 scripts/audit-future-commitment-false-positives.py [--window=N] [--limit=N]
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
SIGNAL = REPO_ROOT / ".cupcake" / "signals" / "last_assistant_future_commitment.sh"

# The fixture handed to the signal is the last window events before a boundary: enough for the turn
# and its immediate history. Older dispatches fall outside it, which can only make the guard fire
# more than it does in production -- the safe direction for a false-positive audit.
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


def default_transcripts(count: int = DEFAULT_TRANSCRIPT_COUNT) -> list[str]:
    key = str(REPO_ROOT).replace("/", "-")
    tdir = Path(os.path.expanduser("~/.claude/projects")) / key
    files = sorted(glob.glob(str(tdir / "*.jsonl")), key=os.path.getmtime, reverse=True)
    return files[:count]


def fields(out: str) -> dict[str, str]:
    parsed: dict[str, str] = {}
    for part in out.split("|"):
        if "=" in part:
            key, _, value = part.partition("=")
            parsed[key] = value
    return parsed


def verdict(out: str) -> str | None:
    """Which arm would halt, or None.

    The rule that turns facts into a verdict lives in
    `.cupcake/policies/claude/no_future_tense_commitment.rego`, and it is restated here rather than
    guessed at: counting bare signal output instead would inflate the number with the very turns the
    exemptions exist to let through.
    """
    if not out.startswith("FUTUREFACTS|"):
        return None
    parsed = fields(out)
    if parsed.get("commit") and all(
        parsed.get(k, "0") == "0" for k in ("acted", "blocked", "planasked", "deferred", "conditional")
    ):
        return "commit"
    if parsed.get("delegation") and all(
        parsed.get(k, "0") == "0" for k in ("reported", "instructed")
    ):
        return "delegation"
    return None


def audit(path: str, home: str) -> tuple[int, list[tuple[int, str, str]]]:
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

    fires: list[tuple[int, str, str]] = []
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
        which = verdict(out)
        if which:
            fires.append((boundary, which, out))
    return len(boundaries), fires


def main() -> int:
    args = sys.argv[1:]
    global WINDOW
    count = DEFAULT_TRANSCRIPT_COUNT
    rest = []
    for arg in args:
        if arg.startswith("--window="):
            WINDOW = int(arg.split("=", 1)[1])
        elif arg.startswith("--limit="):
            count = int(arg.split("=", 1)[1])
        else:
            rest.append(arg)
    paths = rest or default_transcripts(count)
    if not paths:
        print("no transcripts found to audit", file=sys.stderr)
        return 1
    total_turns = 0
    all_fires: list[tuple[str, int, str, str]] = []
    with tempfile.TemporaryDirectory() as home:
        for path in paths:
            turns, fires = audit(path, home)
            total_turns += turns
            all_fires.extend((Path(path).name[:8], b, w, o) for b, w, o in fires)
            print(f"{Path(path).name[:8]}: {turns} turns, {len(fires)} would halt", flush=True)
    commits = sum(1 for _, _, which, _ in all_fires if which == "commit")
    delegations = len(all_fires) - commits
    rate = (len(all_fires) / total_turns * 100) if total_turns else 0.0
    print(
        f"\nTOTAL: {len(all_fires)} halts across {total_turns} real turns ({rate:.2f}%) -- "
        f"{commits} future-tense commitment, {delegations} delegation-as-completion"
    )
    for name, boundary, which, out in all_fires:
        print(f"  {name} line {boundary} [{which}]: {out[:240]}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
