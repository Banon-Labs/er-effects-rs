#!/usr/bin/env python3
"""Replay real past turns through the admission-with-defence guard and count how many it would halt.

The `no_admission_with_defence` Stop guard is only worth having if it stays quiet on ordinary turns.
A guard that cries wolf gets ignored, which is worse than no guard, and a false positive here blocks
a turn that was entitled to stop -- one that admitted an omission and then reported the substitute
work, one that answered a question the user actually asked, one whose reason for not acting was a
credential it does not hold. Unit tests prove the shapes the author thought of; this proves the
shapes the author did not, by running the real signal over the session transcripts this repo has
actually produced.

For every turn boundary in a transcript it builds a fixture from the preceding window of events,
runs `.cupcake/signals/last_assistant_admission_with_defence.sh` against it under a temporary home,
and applies the policy's own conjunction to the facts line. Read the hits: each one is either a real
instance of the defect (good) or a false positive to narrow away.

The denominator is real turn boundaries, counted the way
`scripts/audit-narrated-action-false-positives.py` counts them: an interrupted turn and a piece of
harness bookkeeping are both excluded, because the Stop hook fires on neither. See `audit` below.

This is how the guard was tuned rather than guessed. Over all 74 transcripts this repo has produced
it halts 1 of 1983 real turn boundaries (0.05%), and that one is the verbatim instance the rule was
written for. Counting every user event as a boundary gives 1 of 2774 over the 30 newest transcripts
against 1 of 1341 for the same set here -- the same single hit either way, so unlike the
narrated-action guard no phantom boundary inflates it.

Two things were measured out by reading the hits of earlier drafts, and both changed the classifier
rather than the audit:

  * dropping the omission-reference requirement from the justification arm -- letting any causal
    marker after an admission count -- took it to 3 of 1982 (0.15%). Two of those three were
    exemplary ownership: "So that commit was me re-litigating a path the design had already
    abandoned, which is why it made things worse for you" explains the consequence of the agent's
    own mistake, and convicting it would teach agents to admit less. The requirement stays.
  * a markdown table row was arriving as a sentence, so the header "| what I should have read |
    value | meaning |" was reported as a first-person admission and the halt would have quoted a
    column caption back as a confession.

Worth knowing when reading a zero: the admission fact alone fires on 20 of the 1744 turns that
ended on prose, so a quiet total is the conjunction being selective rather than the detector being
dead.

Usage:
    python3 scripts/audit-admission-with-defence-false-positives.py [--window=N] [--limit=N]
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
SIGNAL = REPO_ROOT / ".cupcake" / "signals" / "last_assistant_admission_with_defence.sh"

# The fixture handed to the signal is the last window events before a boundary: enough for the turn
# and its immediate history. Older events fall outside it, which can only make the guard fire more
# than it does in production -- the safe direction for a false-positive audit.
WINDOW = 400

FAKE_PROJECT = "/fake/project/er-quickload"
DEFAULT_TRANSCRIPT_COUNT = 10


# Harness bookkeeping that arrives as a "user" event and is not a prompt. Excluding these is what
# makes the denominator real turn boundaries rather than events: a `<task-notification>` does not
# end a turn, and slicing the transcript at one leaves a mid-turn message looking like a closing
# one.
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
    """The dilution kind that would halt, or None.

    The rule that turns facts into a verdict lives in
    `.cupcake/policies/claude/no_admission_with_defence.rego`, and it is restated here rather than
    guessed at: counting bare signal output instead would inflate the number with the very turns
    the exemptions exist to let through.
    """
    if not out.startswith("ADMISSIONFACTS|"):
        return None
    parsed = fields(out)
    if not parsed.get("admission") or not parsed.get("dilution"):
        return None
    if all(parsed.get(k, "0") == "0" for k in ("solicited", "blocked")):
        return parsed.get("kind", "dilution")
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
    event appeared". Two shapes are excluded and both were measured rather than assumed by the
    neighbouring narrated-action audit:

      * an interrupted turn. When the user hits Ctrl+C the transcript records
        `[Request interrupted by user]` and the assistant's last prose sits there unfinished -- but
        the Stop hook never fires on one, because the turn was aborted rather than stopped.
      * harness bookkeeping that arrives as a "user" event: a task notification, a slash command
        and its stdout, a system reminder. None of them ends a turn.
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
        print(f"  {name} line {boundary} [{which}]: {out[:400]}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
