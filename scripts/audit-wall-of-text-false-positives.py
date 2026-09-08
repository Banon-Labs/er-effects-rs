#!/usr/bin/env python3
"""Replay real past turns through the one-paragraph rule and show what each counting scheme costs.

The wall-of-text guard is only worth having if it fires on walls of text and stays silent on ordinary
work. Unit tests prove the shapes the author thought of; this proves the shapes the author did not,
by counting paragraphs the way production does over the session transcripts the agent actually wrote.

It scores every turn three ways so the difference between them is a measured number rather than an
argument:

    whole-turn   every text block in the turn summed together (what the guard did until 2026-08-22)
    max-run      the longest contiguous run of prose -- text blocks with no tool call between them
                 (what it does now: the unit a reader actually experiences as one message)
    closing      only the prose after the last tool call (the final answer alone)

`whole-turn` counts a tool-heavy turn's one-line preambles as paragraphs: eleven "Now let me check X."
lines interleaved with eleven tool calls scored the same as an eleven-paragraph essay, which is how a
guard meant for walls of text ended up firing on ordinary work.

Usage:
    python3 scripts/audit-wall-of-text-false-positives.py [--limit=N] [transcript.jsonl ...]

With no arguments it audits the newest transcripts for this repo under ~/.claude/projects/.
Read-only: it opens transcripts and writes nothing.
"""
from __future__ import annotations

import glob
import os
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(REPO_ROOT / "scripts"))

import cupcake_turn_scan as scan  # noqa: E402  (path set above)

DEFAULT_TRANSCRIPT_COUNT = 6
DEFAULT_EXAMPLES = 6


def default_transcripts() -> list[str]:
    key = str(REPO_ROOT).replace("/", "-")
    tdir = Path(os.path.expanduser("~/.claude/projects")) / key
    files = sorted(glob.glob(str(tdir / "*.jsonl")), key=os.path.getmtime, reverse=True)
    return files[:DEFAULT_TRANSCRIPT_COUNT]


def score(turn: scan.Turn) -> tuple[int, int, int]:
    """(whole-turn, max-run, closing) paragraph counts for one turn."""
    runs = turn.text_runs
    if not runs:
        return 0, 0, 0
    whole = len(scan.prose_paragraphs("\n\n".join(runs)))
    max_run = max(len(scan.prose_paragraphs(r)) for r in runs)
    idx = turn.last_text_index
    closing = 0 if idx < 0 or turn.tool_after(idx) else len(scan.prose_paragraphs(turn.blocks[idx][1]))
    return whole, max_run, closing


def main(argv: list[str]) -> int:
    paths = [a for a in argv if not a.startswith("--")]
    limit = DEFAULT_EXAMPLES
    for arg in argv:
        if arg.startswith("--limit="):
            limit = int(arg.split("=", 1)[1])
    if not paths:
        paths = default_transcripts()
    if not paths:
        print("audit-wall-of-text: no transcripts found")
        return 0

    totals = {"turns": 0, "whole": 0, "run": 0, "closing": 0}
    only_narration: list[tuple[str, int, int]] = []
    still_fires: list[tuple[str, int, str]] = []

    for path in paths:
        events = scan.load_events(path)
        for turn in scan.split_turns(events):
            if not turn.texts:
                continue
            whole, max_run, closing = score(turn)
            totals["turns"] += 1
            totals["whole"] += 1 if whole > 1 else 0
            totals["run"] += 1 if max_run > 1 else 0
            totals["closing"] += 1 if closing > 1 else 0
            if whole > 1 and max_run <= 1 and len(only_narration) < 200:
                only_narration.append((os.path.basename(path), whole, len(turn.text_runs)))
            if max_run > 1 and len(still_fires) < 200:
                first = " ".join(turn.text_runs[0].split())[:70]
                still_fires.append((os.path.basename(path), max_run, first))

    turns = max(totals["turns"], 1)
    print(f"transcripts={len(paths)}  turns with prose={totals['turns']}")
    print(f"  whole-turn > 1 paragraph : {totals['whole']:5d}  ({100 * totals['whole'] / turns:.0f}%)")
    print(f"  max-run    > 1 paragraph : {totals['run']:5d}  ({100 * totals['run'] / turns:.0f}%)")
    print(f"  closing    > 1 paragraph : {totals['closing']:5d}  ({100 * totals['closing'] / turns:.0f}%)")
    print(
        f"\nturns the OLD whole-turn count flagged that are only interleaved narration: "
        f"{len(only_narration)}"
    )
    for row in only_narration[:limit]:
        print(f"   {row[0]}  whole={row[1]} across {row[2]} separate prose runs")
    print(f"\nturns max-run still flags (real multi-paragraph messages): {len(still_fires)}")
    for row in still_fires[:limit]:
        print(f"   {row[0]}  paragraphs={row[1]}  {row[2]!r}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
