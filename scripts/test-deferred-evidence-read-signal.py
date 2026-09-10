#!/usr/bin/env python3
"""Behavioral tests for the cupcake signal `last_assistant_deferred_evidence_read`.

The signal scans the last completed assistant turn and emits one facts line:

  DEFERFACTS|deferral=..|consulted=0|1|future=0|1|userneed=0|1|blocked=0|1|carried=0|1

The rule over those facts lives in `.cupcake/policies/claude/no_deferred_evidence_read.rego` and is
covered by `.cupcake/tests/no_deferred_evidence_read_test.rego`. This file covers the extraction --
the half a rego suite cannot reach, because a facts line handed to `opa test` proves nothing about
whether the shell would ever produce it. Between the two suites the corpus is proven end to end:
text in, facts out, halt or no halt.

Every phrasing the rule is meant to catch gets a case here, and so does every exemption. The
verbatim turn that prompted the rule (2026-09-09) is the first of them.
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
SIGNAL = REPO_ROOT / ".cupcake" / "signals" / "last_assistant_deferred_evidence_read.sh"

PROJECT_DIR = "/fake/project/er-quickload"

# The closing message of 2026-09-09, verbatim. The answer was in a log file on disk, and the turn
# named the read instead of doing it.
VERBATIM = (
    "The detour now captures r14 at the naked entry, and the next measurement is whether the DLL is "
    "using the menu object it now captures (r14) for the re-invade gate or still falling back to "
    "the scan -- which its own log answers, so I am reading that next."
)

# A source read that is not the artifact the closing sentence points at. Present in most cases so
# the turn looks like real work rather than a bare paragraph.
SOURCE_READ = ("Bash", {"command": "sed -n '1,40p' crates/er-invasion-warp/src/local_invasion_filter.rs"})


def user(text: str) -> dict:
    return {"type": "user", "message": {"content": text}}


def tool_result() -> dict:
    """A tool-result carrier user event -- must not split the assistant turn."""
    return {"type": "user", "message": {"content": [{"type": "tool_result", "content": "ok"}]}}


def assistant_text(text: str) -> dict:
    return {"type": "assistant", "message": {"content": [{"type": "text", "text": text}]}}


def assistant_tool(name: str, inp: dict) -> dict:
    return {
        "type": "assistant",
        "message": {"content": [{"type": "tool_use", "name": name, "id": "t1", "input": inp}]},
    }


def run_signal(events: list[dict]) -> str:
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


def parse(out: str) -> dict[str, str]:
    fields: dict[str, str] = {}
    for part in out.split("|"):
        if "=" in part:
            k, _, v = part.partition("=")
            fields[k] = v
    return fields


def turn(closing: str, tools=(SOURCE_READ,)) -> dict[str, str]:
    events: list[dict] = [user("why did the re-invade not fire")]
    for name, inp in tools:
        events.append(assistant_tool(name, inp))
        events.append(tool_result())
    events.append(assistant_text(closing))
    return parse(run_signal(events))


FAILURES: list[str] = []


def check(name: str, ok: bool, detail: str = "") -> None:
    if ok:
        print(f"ok   {name}")
    else:
        FAILURES.append(name)
        print(f"FAIL {name}: {detail}", file=sys.stderr)


def expect_deferral(name: str, closing: str, tools=(SOURCE_READ,)) -> None:
    """The phrasing is recognised and none of the five exemptions is set."""
    f = turn(closing, tools)
    clear = all(f.get(k) == "0" for k in ("consulted", "future", "userneed", "blocked", "carried"))
    check(name, bool(f.get("deferral")) and clear, f"got {f or 'no output'}")


def expect_exempt(name: str, closing: str, field: str, tools=(SOURCE_READ,)) -> None:
    """The phrasing is recognised and the named exemption is set, so the policy stays quiet."""
    f = turn(closing, tools)
    check(name, bool(f.get("deferral")) and f.get(field) == "1", f"got {f or 'no output'}")


def main() -> int:
    # --- the phrasings the rule is meant to catch -------------------------------------------------
    expect_deferral("verbatim: the next measurement is ... which its own log answers", VERBATIM)
    expect_deferral(
        "the next step is ...",
        "The next step is to read er-quickload-autoload-debug.log and see which branch ran.",
    )
    expect_deferral(
        "what remains is to ...",
        "What remains is to read the newest er-invasion-warp log and see which branch the gate took.",
    )
    expect_deferral(
        "the remaining question is ...",
        "The remaining question is which branch the gate took, and its own log records that.",
    )
    expect_deferral(
        "which its own log answers",
        "The gate either adopted the object or fell back to the scan -- which its own log answers.",
    )
    expect_deferral(
        "the log will say",
        "The gate either adopted the captured object or fell back to the scan, and "
        "er-quickload-autoload-debug.log will say which.",
    )
    expect_deferral(
        "that will tell us",
        "The counter is incremented in both branches; the run artifact under er-me3-runs will tell "
        "us which one ran.",
    )
    expect_deferral(
        "its own log records that",
        "The scan ran on both attempts, and the trace at 0x140d39df0 already holds that.",
    )
    expect_deferral(
        "so I am reading that next",
        "The adopted pointer is either used or ignored, and I am reading "
        "target/er-me3-runs/latest.log next.",
    )
    expect_deferral(
        "so that is what I check next",
        "The disassembly at 0x140d39df0 settles whether the branch is taken, so that is what I read "
        "next.",
    )
    expect_deferral(
        "reading that now",
        "The re-invade gate either used the adopted object or the scan; reading that log now.",
    )

    # --- the exemptions ---------------------------------------------------------------------------
    expect_exempt(
        "consulted: the turn opened the artifact and is reporting it",
        "The remaining question is which branch the gate took, and er-quickload-autoload-debug.log "
        "records that: the adopted pointer was null on both attempts, so the scan ran.",
        "consulted",
        tools=(("Bash", {"command": "tail -40 /home/banon/Elden/Game/er-quickload-autoload-debug.log"}),),
    )
    expect_exempt(
        "future: the evidence does not exist yet",
        "The gate cannot say anything until it runs again; the next run's log will say whether the "
        "adopted object was used.",
        "future",
    )
    expect_exempt(
        "userneed: the read needs an observation only the user can make",
        "The remaining question is which banner rendered, and only the screen capture holds that -- "
        "tell me what you saw, because there is no pixel oracle for that crop yet.",
        "userneed",
    )
    expect_exempt(
        "blocked: a real blocker was stated",
        "The trace would settle it, but the dump is not readable without sudo, so I cannot open it "
        "from this session.",
        "blocked",
    )

    # --- shapes that must stay silent -------------------------------------------------------------
    check(
        "an ordinary report emits nothing",
        turn("The detour installs at 0x140d39df0 and the counter incremented twice, so the adopted "
             "object is in use.") == {},
        "expected no output",
    )
    check(
        "quoting the ban is not committing it",
        turn('The guard fires on "which its own log answers" and on `reading that log now`, which '
             "is why this paragraph does not trip it.") == {},
        "expected no output: backticked and double-quoted spans are blanked before matching",
    )
    check(
        "a deferral with no artifact named emits nothing",
        turn("The next step is obvious once the shape is clear.") == {},
        "expected no output",
    )

    if FAILURES:
        print(f"\ntest-deferred-evidence-read-signal: {len(FAILURES)} failure(s)", file=sys.stderr)
        return 1
    print("\ntest-deferred-evidence-read-signal: OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
