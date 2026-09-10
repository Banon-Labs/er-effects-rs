#!/usr/bin/env python3
"""Replay real past turns through the narrated-action guard and count how many it would halt.

The `no_narrated_action` Stop guard is only worth having if it stays quiet on ordinary turns. A
guard that cries wolf gets ignored, which is worse than no guard, and a false positive here blocks
a turn that was entitled to stop -- one that reported a measurement, one carrying the launch banner
`AGENTS.md` mandates, one waiting on something only the user has. Unit tests prove the shapes the
author thought of; this proves the shapes the author did not, by running the real signal over the
session transcripts this repo has actually produced.

For every turn boundary in a transcript it builds a fixture from the preceding window of events,
runs `.cupcake/signals/last_assistant_narrated_action.sh` against it under a temporary home, and
applies the policy's own conjunction to the facts line. Read the hits: each one is either a real
instance of the defect (good) or a false positive to narrow away.

This is how the guard was tuned rather than guessed, and both halves of the tuning are in the
numbers. The first draft, over the 6 newest transcripts, halted 25 of 763 boundaries (3.28%).
Reading every one changed two things and corrected this script:

  * the first-person progressive was matched anywhere in the closing sentence, so a report ending
    "..., er-npc-possess compiles again, and I'm restarting the full 26-shell relink" was read as a
    narration. It is anchored to the start of the sentence now, which is where the shape the
    directive names actually sits.
  * `reported` did not recognise a bare measured pair -- "repinned to measured values (2800 and
    1336)" -- so a sentence full of results still counted as an announcement.
  * fifteen of the twenty-five were one interrupted turn counted fifteen times, at boundaries that
    were task notifications rather than prompts. See `audit` below: a boundary now counts only when
    the turn before it actually ended, because the Stop hook fires on nothing else.

Over all 74 transcripts this repo has produced the tuned guard halts 2 of 1973 real turn
boundaries (0.10%), and both were read:

  * "The wording needed explaining; rebuilding and relaunching with it." -- the turn built, did not
    relaunch, and closed announcing both. The user's next message went somewhere else entirely and
    the relaunch never happened, which is the sentence being read as done.
  * "I'm instrumenting the child teardown next, not the evaluator." -- the user's very next message
    was "What's next?", which is the zero-information round trip this rule exists to prevent.

Counting every user event as a boundary the way the neighbouring audits do gives 10 of 1845
(0.54%) over the 30 newest transcripts, against 2 of 1331 for the same set here. The extra eight
all sit at boundaries the Stop hook never reaches: seven follow `[Request interrupted by user]`,
and three of those are one sentence counted once per slash-command bookkeeping event.

Usage:
    python3 scripts/audit-narrated-action-false-positives.py [--window=N] [--limit=N]
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
SIGNAL = REPO_ROOT / ".cupcake" / "signals" / "last_assistant_narrated_action.sh"

# The fixture handed to the signal is the last window events before a boundary: enough for the turn
# and its immediate history. Older events fall outside it, which can only make the guard fire more
# than it does in production -- the safe direction for a false-positive audit.
WINDOW = 400

FAKE_PROJECT = "/fake/project/er-quickload"
DEFAULT_TRANSCRIPT_COUNT = 10


# Harness bookkeeping that arrives as a "user" event and is not a prompt. Excluding these is what
# makes the denominator real turn boundaries rather than events, and it is not a cosmetic choice:
# with them counted, one interrupted turn produced fifteen identical halts at fifteen phantom
# boundaries, because slicing the transcript there leaves a mid-turn narration looking like a
# closing one. Neither shape ever reaches the Stop hook in production -- a `<task-notification>`
# does not end a turn, and an interrupted turn is aborted rather than stopped.
NON_PROMPT_MARKERS = (
    "<task-notification>",
    "[Request interrupted by user]",
    "<system-reminder>",
    "<command-name>",
    "<command-message>",
    "<local-command-stdout>",
    "A session-scoped Stop hook is now active",
)

INTERRUPT_MARKER = "[Request interrupted by user]"


def is_real_user_prompt(ev: dict) -> bool:
    if ev.get("type") != "user":
        return False
    content = ev.get("message", {}).get("content")
    if isinstance(content, str):
        text = content.strip()
    elif isinstance(content, list):
        if any(isinstance(b, dict) and b.get("type") == "tool_result" for b in content):
            return False
        text = "\n".join(
            b.get("text") or ""
            for b in content
            if isinstance(b, dict) and b.get("type") == "text"
        ).strip()
    else:
        return False
    if not text:
        return False
    return not any(text.startswith(marker) for marker in NON_PROMPT_MARKERS)


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
    """The action class that would halt, or None.

    The rule that turns facts into a verdict lives in
    `.cupcake/policies/claude/no_narrated_action.rego`, and it is restated here rather than guessed
    at: counting bare signal output instead would inflate the number with the very turns the
    exemptions exist to let through.
    """
    if not out.startswith("NARRATIONFACTS|"):
        return None
    parsed = fields(out)
    if not parsed.get("narration"):
        return None
    if all(parsed.get(k, "0") == "0" for k in ("reported", "banner", "blocked")):
        return parsed.get("actionclass", "work")
    return None


def event_text(ev: dict) -> str:
    content = ev.get("message", {}).get("content")
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "\n".join(
            b.get("text") or ""
            for b in content
            if isinstance(b, dict) and b.get("type") == "text"
        )
    return ""


def audit(path: str, home: str) -> tuple[int, list[tuple[int, str, str]]]:
    """Turn boundaries in this transcript, and the ones the guard would halt.

    A boundary counts only when the turn before it actually ended, which is narrower than "a user
    event appeared". Two shapes are excluded and both were measured rather than assumed:

      * an interrupted turn. When the user hits Ctrl+C the transcript records
        `[Request interrupted by user]` and the assistant's last prose sits there unfinished -- but
        the Stop hook never fires on one, because the turn was aborted rather than stopped. Seven of
        the first draft's ten hits were this shape, including three copies of one sentence.
      * harness bookkeeping that arrives as a "user" event: a task notification, a slash command
        and its stdout, a system reminder. None of them ends a turn, and counting them both inflated
        the denominator and manufactured phantom turn-ends mid-turn.
    """
    lines = Path(path).read_text(encoding="utf-8", errors="replace").splitlines()
    boundaries = []
    last_assistant_text = -1
    last_interrupt = -1
    for i, line in enumerate(lines):
        try:
            ev = json.loads(line)
        except ValueError:
            continue
        if not isinstance(ev, dict):
            continue
        if ev.get("type") == "assistant" and event_text(ev).strip():
            last_assistant_text = i
        if ev.get("type") == "user" and INTERRUPT_MARKER in event_text(ev):
            last_interrupt = i
        if is_real_user_prompt(ev) and last_interrupt < last_assistant_text:
            boundaries.append(i)
    if last_interrupt < last_assistant_text:
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
    rate = (len(all_fires) / total_turns * 100) if total_turns else 0.0
    classes: dict[str, int] = {}
    for _, _, which, _ in all_fires:
        classes[which] = classes.get(which, 0) + 1
    breakdown = ", ".join(f"{n} {name}" for name, n in sorted(classes.items()))
    print(
        f"\nTOTAL: {len(all_fires)} halts across {total_turns} real turns ({rate:.2f}%)"
        + (f" -- {breakdown}" if breakdown else "")
    )
    for name, boundary, which, out in all_fires:
        print(f"  {name} line {boundary} [{which}]: {out[:300]}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
