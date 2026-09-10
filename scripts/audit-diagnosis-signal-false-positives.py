#!/usr/bin/env python3
"""Replay real past turns through the two newest arms of the diagnosis signal and count the halts.

One signal, `.cupcake/signals/last_assistant_diagnosis_without_fix.sh`, feeds three rules. This
audits the two added on 2026-09-09, either separately or together:

  promissory (`ER-EFFECTS-NO-PROMISSORY-CLOSER`) refuses a turn that closes by announcing work in the
  present participle -- "Fixing both: bypass the union ..." -- while writing no file. The grammar it
  keys on is one letter away from ordinary reporting prose: "Adding the field to the struct fixed it"
  and "Fixing the rule requires the live counter" are a report and an explanation, and a guard that
  halted on those would fire on legitimate turns until someone silenced it.

  unread (`ER-EFFECTS-NO-PROMISSORY-CLOSER`, second shape) refuses a turn that names the evidence
  answering its own open question -- "which its own log answers, so I am reading that next" -- and
  leaves the read for after the turn. Its risk is deferral to evidence that does not exist yet,
  which is a legitimate plan rather than a skipped read.

  handback (`ER-EFFECTS-NO-ZERO-INFORMATION-STOP`) refuses a turn whose ideal next user reply is
  nothing -- the agent announced its own next action, offered to do work it was already authorised
  to do, asked for an in-game input it drives itself, or told the user they need do nothing after a
  turn that did none. Its risk is the mirror image: the launch-handoff protocol this repo requires
  ends a turn on the user deliberately, and that must stay speakable.

Unit tests prove the shapes the author thought of; this proves the shapes the author did not, by
running the real signal over the session transcripts the agent has actually written.

For every turn boundary in a transcript it builds a fixture from the preceding window of events,
runs the signal against it under a temporary home, and applies the policy's conjunction to the facts
line. Read the hits: each one is either a real instance of the defect (good) or a false positive to
narrow away. Note that adjacent boundaries can report the same turn twice -- an interrupt marker is a
real user event and opens a new bucket -- so the halt count is boundaries, not distinct turns, and it
errs high.

Measured 2026-09-09 over all 74 transcripts for this repo, ~3,790 turn boundaries (~2,400 distinct
turns; adjacent boundaries can report one turn more than once, so the boundary rate errs high).

  promissory  12 halts (0.32% of boundaries) = 7 distinct turns, 10 matched-and-exempt because the
              turn had made the edit. Reading the 7 is what tuned it: "correcting" left the verb
              list after firing on "Correcting myself on one thing I told you earlier:", which
              retracts a statement and announces no work; a gerund is read as an announcement only
              in one of five constructions a subject-gerund cannot take; and only the last two
              sentences of the closing prose run are read, so the one-line gerund preamble before a
              tool call is never seen. One survivor is declared rather than exempted: "Dropping the
              password line -- recording it so I don't repeat it:" announces a `bd remember`, and
              this family deliberately does not count a memory as a write, so the arm would fire
              even had the memory been recorded. Widening that would re-open the substitution
              `ER-EFFECTS-NO-UNBACKED-CLAIM` exists to catch.

  unread      2 halts (0.05%), both real, after the future tense left the deferral set: "the log
              will say" produced six of the first eight halts and every one named evidence a future
              run or a future press would produce. What survives is the tense that claims the answer
              already exists, and the explicit deferral of the read.

  handback    30 halts (0.79% of boundaries) = ~21 distinct turns, down from 255 (6.8%) in the first
              draft. Fourteen are "say the word and I'll build it" and "Want me to build X?", which
              is the shape the directive names. Four are declared false positives rather than
              exempted, because every exemption that would cover them would cover the defect too:
                * "The ball is yours: quit the running game, then tell me and I'll relaunch" -- a
                  live window the user may be mid-test in is a real dependency, but the sentence
                  that says so is indistinguishable from one that invents it;
                * "Nothing." as a whole answer, where the prompt was not phrased as a question;
                * "Nothing -- the conflict was resolved and pushed" reporting earlier work;
                * "Want me to write that up and relaunch, or chase it further -- ..." is a fork, but
                  its alternation is too far from the question mark for the fork exemption to see.

Usage:
    python3 scripts/audit-diagnosis-signal-false-positives.py \
        [--window=N] [--all] [--rule=promissory|unread|handback|both] [transcript ...]

With no arguments it audits the ten newest transcripts for this repo under ~/.claude/projects/;
`--all` takes every one of them. Read-only: it never writes to the transcripts, and its fixtures live
in a temp dir that is removed.
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
SIGNAL = REPO_ROOT / ".cupcake" / "signals" / "last_assistant_diagnosis_without_fix.sh"

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


def default_transcripts(count: int | None) -> list[str]:
    key = str(REPO_ROOT).replace("/", "-")
    tdir = Path(os.path.expanduser("~/.claude/projects")) / key
    files = sorted(glob.glob(str(tdir / "*.jsonl")), key=os.path.getmtime, reverse=True)
    return files if count is None else files[:count]


def fields(out: str) -> dict[str, str]:
    if not out.startswith("DIAGFACTS|"):
        return {}
    parsed = {}
    for part in out.split("|"):
        if "=" in part:
            k, _, v = part.partition("=")
            parsed[k] = v
    return parsed


def promissory_verdict(parsed: dict[str, str]) -> tuple[str, str] | None:
    """Apply the promissory conjunction to a parsed facts line.

    The signal emits facts for every turn that names a defect, announces work or hands work back,
    exempt or not, so a non-empty line is not a halt. The rule that turns facts into a verdict lives
    in `.cupcake/policies/claude/no_diagnosis_without_fix.rego` and is restated here rather than
    guessed at: work was announced in the closing prose, the turn wrote nothing, and no blocker was
    stated. Counting bare output instead would inflate the number with the very turns the exemptions
    exist to let through -- and with the diagnosis arm's hits, which this audit does not measure.
    """
    promise = parsed.get("promise", "")
    if not promise:
        return None
    if parsed.get("edited", "0") != "0":
        return ("exempt-edited", promise)
    if parsed.get("blocked", "0") != "0":
        return ("exempt-blocked", promise)
    return ("halt", promise)


def handback_verdict(parsed: dict[str, str]) -> tuple[str, str] | None:
    """Apply the zero-information-stop conjunction to a parsed facts line.

    Kinds b and c name work that did not happen and fire unaided. Kind a says only that the user
    need do nothing, which is also how a finished task ends, so it convicts a turn that did no work
    and exonerates one that did. Note that this arm reads `extblocked`, not the broader `blocked`
    the other two use: naming the user as the dependency must not buy silence for work the agent
    owns.
    """
    clause = parsed.get("handback", "")
    if not clause:
        return None
    if parsed.get("extblocked", "0") != "0":
        return ("exempt-extblocked", clause)
    if parsed.get("userneed", "0") != "0":
        return ("exempt-userneed", clause)
    if parsed.get("carried", "0") != "0":
        return ("exempt-carried", clause)
    if parsed.get("handbackkind", "b") == "a" and parsed.get("didwork", "0") != "0":
        return ("exempt-delivered", clause)
    return ("halt", clause)


def unread_verdict(parsed: dict[str, str]) -> tuple[str, str] | None:
    """Apply the unread-evidence conjunction to a parsed facts line.

    The closing prose named a concrete artifact as holding the answer and left the read for later.
    It is exempt when the turn opened the artifact it named, when the evidence does not exist yet,
    when reading it needs the user, or when a blocker was stated.
    """
    clause = parsed.get("unread", "")
    if not clause:
        return None
    if parsed.get("consulted", "0") != "0":
        return ("exempt-consulted", clause)
    if parsed.get("future", "0") != "0":
        return ("exempt-future", clause)
    if parsed.get("userneed", "0") != "0":
        return ("exempt-userneed", clause)
    if parsed.get("blocked", "0") != "0":
        return ("exempt-blocked", clause)
    return ("halt", clause)


ARMS = {
    "promissory": promissory_verdict,
    "unread": unread_verdict,
    "handback": handback_verdict,
}


def audit(path: str, home: str, arms: list[str]) -> tuple[int, list[tuple[int, str, str, str]]]:
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

    found: list[tuple[int, str, str, str]] = []
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
        parsed = fields(proc.stdout.strip())
        for arm in arms:
            got = ARMS[arm](parsed)
            if got:
                found.append((boundary, arm, got[0], got[1]))
    return len(boundaries), found


def main() -> int:
    args = sys.argv[1:]
    global WINDOW
    count: int | None = DEFAULT_TRANSCRIPT_COUNT
    arms = list(ARMS)
    rest = []
    for arg in args:
        if arg.startswith("--window="):
            WINDOW = int(arg.split("=", 1)[1])
        elif arg == "--all":
            count = None
        elif arg.startswith("--rule="):
            wanted = arg.split("=", 1)[1]
            if wanted != "both":
                if wanted not in ARMS:
                    print(f"unknown --rule={wanted}", file=sys.stderr)
                    return 2
                arms = [wanted]
        else:
            rest.append(arg)
    paths = rest or default_transcripts(count)
    if not paths:
        print("no transcripts found to audit", file=sys.stderr)
        return 1
    total_turns = 0
    all_found: list[tuple[str, int, str, str, str]] = []
    with tempfile.TemporaryDirectory() as home:
        for path in paths:
            turns, found = audit(path, home, arms)
            total_turns += turns
            halts = sum(1 for _, _, kind, _ in found if kind == "halt")
            all_found.extend((Path(path).name[:8], b, a, k, c) for b, a, k, c in found)
            print(
                f"{Path(path).name[:8]}: {turns} turns, {halts} would halt,"
                f" {len(found) - halts} exempt",
                flush=True,
            )
    print(f"\nboundaries audited: {total_turns}")
    for arm in arms:
        arm_rows = [f for f in all_found if f[2] == arm]
        halts = [f for f in arm_rows if f[3] == "halt"]
        rate = 100.0 * len(halts) / max(total_turns, 1)
        print(
            f"{arm}: {len(halts)} halts ({rate:.2f}% of boundaries),"
            f" {len(arm_rows) - len(halts)} matched-but-exempt"
        )
    for name, boundary, arm, kind, clause in all_found:
        print(f"  [{arm:10s} {kind:15s}] {name} line {boundary}: {clause[:170]}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
