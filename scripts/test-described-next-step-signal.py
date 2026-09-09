#!/usr/bin/env python3
"""Behavioral tests for the cupcake signal `last_assistant_described_next_step`.

The signal scans the last completed assistant turn and emits one facts line:

  NEXTSTEPFACTS|nextstep=..|acted=0|1|blocked=0|1|handoff=0|1|carried=0|1

The rule over those facts lives in `.cupcake/policies/claude/no_described_next_step.rego` and is
covered by `.cupcake/tests/no_described_next_step_test.rego`. This file covers the extraction, and it
does so against the verbatim turn that prompted the policy (2026-09-08) -- the closing message sent
after seven runs had disproven the place the agent kept searching. Between the two suites the corpus
is proven end to end: text in, facts out, halt or no halt.

The four cases the user asked for are asserted here as prose, and again as facts in the rego suite:
  * the verbatim closing message           -> a next step is extracted, with every exemption clear;
  * describing a step and then starting it -> `acted=1`;
  * waiting on the user                    -> `handoff=1`;
  * reporting a genuine blocker            -> `blocked=1`.

We drive the script against crafted transcript JSONL under a temporary home so its
`~/.claude/projects/<cwd-key>/*.jsonl` discovery resolves to our fixture, then assert the facts.
"""
from __future__ import annotations

import json
import os
import subprocess
import tempfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
SIGNAL = REPO_ROOT / ".cupcake" / "signals" / "last_assistant_described_next_step.sh"

PROJECT_DIR = "/fake/project/er-quickload"

# The closing message of 2026-09-08, verbatim. Seven runs had proven that the writable data of
# ersc.dll does not hold the live Seamless session pointer; this turn named the mechanism, the target
# and the tool, and then built none of it.
VERBATIM = (
    "The next approach follows from the disassembly rather than from hope: the session identifies "
    "itself by holding an active state at +0x150 with a std::mutex at +0x100 during a join, and "
    "that signature does not require the pointer to live in Seamless's data at all -- the "
    "address-space walk I deleted an hour ago is exactly the right tool pointed at the right object "
    "this time. I'd rather say that plainly than dress up a seventh variation on searching the one "
    "place we now know it isn't."
)


def user(text: str) -> dict:
    return {"type": "user", "message": {"content": text}}


def tool_result() -> dict:
    """A tool-result carrier user event -- must not split the assistant turn."""
    return {"type": "user", "message": {"content": [{"type": "tool_result", "content": "ok"}]}}


def assistant_text(text: str) -> dict:
    return {"type": "assistant", "message": {"content": [{"type": "text", "text": text}]}}


def assistant_tool(name: str, **inp: object) -> dict:
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


def expect_facts(name: str, events: list[dict], **expected: object) -> None:
    """Assert selected facts. `True` asserts non-empty, `False` asserts empty, else exact match."""
    out = run_signal(events)
    got = parse(out)
    for key, want in expected.items():
        have = got.get(key, "<missing>")
        if want is True:
            ok = have not in ("", "<missing>")
        elif want is False:
            ok = have == ""
        else:
            ok = have == want
        if not ok:
            raise AssertionError(f"{name}: {key} expected {want!r}, got {have!r} (line: {out!r})")


def expect_clean(name: str, events: list[dict]) -> None:
    out = run_signal(events)
    if out != "":
        raise AssertionError(f"{name}: expected an empty signal, got {out!r}")


def main() -> int:
    # --- the four cases the user asked for --------------------------------------------------------

    # 1. The verbatim failure. A concrete next step is named, nothing began it, no exemption applies.
    expect_facts(
        "verbatim-2026-09-08",
        [user("keep going"), assistant_text(VERBATIM)],
        nextstep=True,
        acted="0",
        blocked="0",
        handoff="0",
        carried="0",
    )

    # 2. The same sentence, followed by the tool call that begins it, and then the report of what it
    # found. Narrating what you are about to do and then doing it is ordinary good work and must
    # always pass. `acted=1` is the fact that says the step was begun.
    expect_facts(
        "described-then-started",
        [
            user("keep going"),
            assistant_text(VERBATIM),
            assistant_tool("Bash", command="python3 scripts/walk-address-space.py"),
            tool_result(),
            assistant_text("The walk found the state at +0x150 in the third region."),
        ],
        nextstep=True,
        acted="1",
    )

    # A status peek is not starting it. Tailing a log after describing the walk leaves the walk
    # undone, and the shared `block_is_substantive` says so.
    expect_facts(
        "peek-is-not-starting-it",
        [
            user("keep going"),
            assistant_text(VERBATIM),
            assistant_tool("Bash", command="tail -20 /tmp/run.log"),
            tool_result(),
            assistant_text("Nothing new in the log."),
        ],
        acted="0",
    )

    # 3. Waiting on the user for something only they can do. A stated dependency on a user action plus
    # a commitment to act on its result is a real wait, not a stall.
    expect_facts(
        "blocked-on-the-user",
        [
            user("keep going"),
            assistant_text(VERBATIM + " Invade now and I'll read the log."),
        ],
        handoff="1",
    )

    # A genuine fork put to the user. The question mark alone exempts, deliberately: a real fork is
    # exactly what an agent is allowed to stop for, and this guard must never gag one.
    expect_facts(
        "genuine-fork-question",
        [
            user("keep going"),
            assistant_text(VERBATIM + " Do you want the walk scoped to the join thread, or all of them?"),
        ],
        handoff="1",
    )

    # 4. A genuine blocker. Naming what stops you is required behaviour in this repo.
    expect_facts(
        "genuine-blocker",
        [
            user("keep going"),
            assistant_text("The next step is running the probe, which requires sudo I do not have."),
        ],
        blocked="1",
    )

    # A step contingent on something landing is not a step that could have been begun.
    expect_facts(
        "contingent-on-a-merge",
        [
            user("keep going"),
            assistant_text("Both branches are clean, and when they land the next move is one run against the tip."),
        ],
        blocked="1",
    )

    # --- turns that must produce no facts at all --------------------------------------------------

    # Ordinary work with no prescription in it.
    expect_clean(
        "clean-turn",
        [user("rename the constant"), assistant_text("Renamed it."), assistant_tool("Edit")],
    )

    # A prescription with no work word in it names no action a tool call could begin.
    expect_clean(
        "prescription-without-an-action",
        [user("so?"), assistant_text("The next step is obvious from the table above.")],
    )

    # Quoting the ban is not committing it. This is what lets a report about the guard, or this file,
    # describe the trigger without tripping it.
    expect_clean(
        "quoted-trigger-does-not-count",
        [
            user("what does it catch?"),
            assistant_text('It halts on "the next step is reading the vtable" when nothing began it.'),
        ],
    )

    expect_clean(
        "backticked-trigger-does-not-count",
        [user("what does it catch?"), assistant_text("It halts on `the next move is reading X`.")],
    )

    # An object, not a prescription: the phrase sits after a preposition or inside a subordinate
    # clause, and prescribes nothing. Both shapes were measured on real transcripts.
    expect_clean(
        "demoted-by-a-preposition",
        [
            user("anything else?"),
            assistant_text("One behaviour delta to watch on the next run is that it resolves through the call map."),
        ],
    )

    expect_clean(
        "demoted-inside-a-subordinate-clause",
        [
            user("anything else?"),
            assistant_text("It is a live suspect that the next run will confirm or clear by name."),
        ],
    )

    # A turn that ended on a tool call did not end on its prose, so it is out of scope.
    expect_clean(
        "turn-ended-on-a-tool-call",
        [
            user("keep going"),
            assistant_text(VERBATIM),
            tool_result(),
            assistant_tool("Bash", command="python3 scripts/walk-address-space.py"),
        ],
    )

    # The step named in an earlier block and begun before the turn ended is covered by the tool call
    # that follows it, even though the turn closes on other prose.
    expect_facts(
        "named-early-then-begun",
        [
            user("keep going"),
            assistant_text("The next step is reading the join path."),
            assistant_tool("Bash", command="python3 scripts/disas-deobf.sh 0x140abc000"),
            tool_result(),
            assistant_text("It dereferences a list head, so the state lives one level in."),
        ],
        acted="1",
    )

    print("test-described-next-step-signal: OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
