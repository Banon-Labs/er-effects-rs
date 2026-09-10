#!/usr/bin/env python3
"""Replay real past turns through the challenged-convention guard and count how many it would halt.

The `no_explanation_instead_of_correction` Stop guard is only worth having if it stays quiet on
ordinary turns. A guard that cries wolf gets ignored, which is worse than no guard, and a false
positive here blocks a turn that was entitled to stop -- one answering a direct question about the
binary, one that made the change and then said why, one waiting on something only the user has.
Unit tests prove the shapes the author thought of; this proves the shapes the author did not, by
running the real signal over the session transcripts this repo has actually produced.

For every turn boundary in a transcript it builds a fixture from the preceding window of events,
runs `.cupcake/signals/last_assistant_challenged_convention.sh` against it under a temporary home,
and applies the policy's own conjunction to the facts line. Read the hits: each one is either a
real instance of the defect (good) or a false positive to narrow away.

This is how the guard was tuned rather than guessed. The first draft halted 49 of 2,696 real turn
boundaries (1.82%); reading them suppressed "why do you think ..." as a request for the assistant's
diagnosis rather than a challenge to its choice, and cut a bare "yes"/"no"/"right"/"true"/"correct"
out of the concession heads -- 46 of the 49 were a direct answer to a yes-or-no question, which is
the answer-first shape this repo asks for. The tuned guard halts 3 of 2,699 (0.11%), and all three
sit in the one transcript that carries the verbatim corpus.

Both arms are counted separately, because they fail differently:
  * `defence` -- the user challenged a choice the assistant made, and the turn justified it instead
                 of changing anything;
  * `pivot`   -- a short concession followed by a long defensive elaboration, in a turn that
                 changed nothing.

Usage:
    python3 scripts/audit-challenged-convention-false-positives.py [--window=N] [--limit=N]
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
SIGNAL = REPO_ROOT / ".cupcake" / "signals" / "last_assistant_challenged_convention.sh"

# The fixture handed to the signal is the last window events before a boundary: enough for the turn
# and its immediate history. Older events fall outside it, which can only make the guard fire more
# than it does in production -- the safe direction for a false-positive audit.
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
    `.cupcake/policies/claude/no_explanation_instead_of_correction.rego`, and it is restated here
    rather than guessed at: counting bare signal output instead would inflate the number with the
    very turns the exemptions exist to let through.
    """
    if not out.startswith("CHALLENGEFACTS|"):
        return None
    parsed = fields(out)
    clear = all(parsed.get(k, "0") == "0" for k in ("changed", "blocked", "asked"))
    if parsed.get("defence") and parsed.get("challenge") and clear:
        return "defence"
    if parsed.get("pivot") and clear:
        return "pivot"
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
    defences = sum(1 for _, _, which, _ in all_fires if which == "defence")
    pivots = len(all_fires) - defences
    rate = (len(all_fires) / total_turns * 100) if total_turns else 0.0
    print(
        f"\nTOTAL: {len(all_fires)} halts across {total_turns} real turns ({rate:.2f}%) -- "
        f"{defences} explanation-instead-of-correction, {pivots} concession-pivot"
    )
    for name, boundary, which, out in all_fires:
        print(f"  {name} line {boundary} [{which}]: {out[:300]}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
