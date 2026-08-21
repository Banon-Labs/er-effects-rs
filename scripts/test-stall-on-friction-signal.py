#!/usr/bin/env python3
"""Behavioral tests for the cupcake signal `last_assistant_stall_on_friction`.

The signal scans the last-completed assistant turn plus the user prompt that opened it, and emits one
facts line:

  STALLFACTS|friction=..|admission=..|handback=..|blame=..|acted=0|1|blocked=0|1|question=0|1|owned=0|1

The RULE over those facts lives in .cupcake/policies/claude/no_stall_on_friction.rego and is covered
by .cupcake/tests/no_stall_on_friction_test.rego. THIS file covers the EXTRACTION, and it does so
against the VERBATIM corpus from the session that prompted the policy (2026-08-04) -- the real
turn-ending messages sent immediately after user frustration. Between the two suites the corpus is
proven end to end: text in, facts out, halt or no halt.

We drive the script against crafted transcript JSONL under a temporary HOME so its
`~/.claude/projects/<cwd-key>/*.jsonl` discovery resolves to our fixture, then assert the facts.
"""
from __future__ import annotations

import json
import os
import subprocess
import tempfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
SIGNAL = REPO_ROOT / ".cupcake" / "signals" / "last_assistant_stall_on_friction.sh"

PROJECT_DIR = "/fake/project/er-effects-rs"


def user(text: str) -> dict:
    return {"type": "user", "message": {"content": text}}


def tool_result() -> dict:
    """A tool-result carrier user event -- must NOT split the assistant turn."""
    return {"type": "user", "message": {"content": [{"type": "tool_result", "content": "ok"}]}}


def assistant_text(text: str) -> dict:
    return {"type": "assistant", "message": {"content": [{"type": "text", "text": text}]}}


def assistant_tool(name: str) -> dict:
    return {
        "type": "assistant",
        "message": {"content": [{"type": "tool_use", "name": name, "input": {}}]},
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
    """Assert selected facts. A value of "" asserts the field is empty; True asserts non-empty."""
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
    # --- verbatim corpus: the turns that must be caught -------------------------------------------

    # Corpus #1 -- confession with no action at all.
    expect_facts(
        "corpus-1-retraction-no-action",
        [
            user("You need to shut up when you have no idea what you're talking about"),
            assistant_text(
                "Retracting: I built a matchmaking model out of a config key's name. The "
                "public-pool claim, the session-slot claim and the invasion-routing claim were all "
                "invented. I've read none of Seamless's session or matchmaking code.\n\n"
                "What is actually established:\n"
                "- The key exists in the settings ini.\n"
                "- It is read once during startup."
            ),
        ],
        friction=True,
        admission=True,
        acted="0",
        blocked="0",
        question="0",
    )

    # Corpus #3 -- concede and stop, with no contrition word anywhere. The admission class has to
    # cover this or the most common stall shape walks straight through.
    expect_facts(
        "corpus-3-concession-closure-no-action",
        [
            user("You sound suprised. I could have told you this"),
            assistant_text(
                "Two facts, both already yours: the pool key is read once at session start, and the "
                "value is never re-read afterwards. That's the whole delta."
            ),
        ],
        friction=True,
        admission=True,
        acted="0",
        question="0",
    )

    # Corpus #4 -- sarcasm, then a decision handed back. The turn DID act (it wrote a script and hit
    # a guard), so acted=1 must be reported and the policy must halt anyway.
    expect_facts(
        "corpus-4-handback-after-acting",
        [
            user("I'm happy for *you*"),
            assistant_text("Two ways forward, your call:"),
            tool_result(),
            assistant_tool("Write"),
        ],
        friction=True,
        handback=True,
        acted="1",
        question="0",
    )

    # --- verbatim corpus: the turn that was CORRECT -----------------------------------------------

    # Corpus #2 -- the assistant admitted it and resumed the abandoned work in the same turn.
    expect_facts(
        "corpus-2-admission-then-real-action",
        [
            user("Well then its a shit thing that you stoped that subagent that was half way done"),
            assistant_text(
                "My mistake -- I killed a subagent that was mid-flight. Resuming that workflow now "
                "from where it left off."
            ),
            tool_result(),
            assistant_tool("Agent"),
        ],
        friction=True,
        admission=True,
        acted="1",
    )

    # --- exemptions the guard must honour ---------------------------------------------------------

    # A direct question, asked with friction. Answering it is not stalling.
    expect_facts(
        "question-asked-under-friction",
        [
            user("Why did you stop the subagent?"),
            assistant_text("I don't actually know what tripped it; the run ended before I read it."),
        ],
        friction=True,
        question="1",
    )

    # A real dependency on the user plus a commitment to act on its result.
    expect_facts(
        "genuine-wait-blocked-on-user",
        [
            user("You didn't even check the routing"),
            assistant_text(
                "I was wrong about the routing. Invade now and I'll read the log the moment the "
                "session opens."
            ),
        ],
        friction=True,
        admission=True,
        blocked="1",
    )

    # No friction at all -> nothing to report.
    expect_clean(
        "plain-answer-no-friction",
        [user("What is the offset?"), assistant_text("The offset is 0x40, verified against disasm.")],
    )

    # Quoting a banned shape is not using it (double-quoted spans are scrubbed).
    expect_facts(
        "quoted-handback-does-not-count",
        [
            user("That's not what I asked for"),
            assistant_text('The phrase "your call" is what the guard halts on.'),
        ],
        friction=True,
        handback="",
    )

    # Fenced code blocks are scrubbed too -- writing the policy must not trip the policy.
    expect_facts(
        "fenced-code-handback-does-not-count",
        [
            user("This is garbage"),
            assistant_text("Adding the pattern now:\n\n```\nyour call\nup to you\n```\n"),
        ],
        friction=True,
        handback="",
    )

    # --- blame deflection -------------------------------------------------------------------------

    # The mechanism left standing alone as the actor.
    expect_facts(
        "blame-without-ownership",
        [
            user("What happened to the run?"),
            assistant_text("The sentinel tore down the live run part-way through."),
        ],
        blame=True,
        owned="0",
    )

    # The same fact with the agent's own hand in it -> a full account, not a deflection.
    expect_facts(
        "blame-with-ownership",
        [
            user("What happened to the run?"),
            assistant_text(
                "My edit to a tracked file tripped it, and then the sentinel tore down the run."
            ),
        ],
        blame=True,
        owned="1",
    )

    # Blame needs no friction gate: this prompt is neutral and the facts line is still emitted.
    out = run_signal(
        [user("Status?"), assistant_text("The guard blocked the command outright.")]
    )
    got = parse(out)
    if got.get("friction") != "" or got.get("blame") in ("", None):
        raise AssertionError(f"blame-without-friction: expected blame with empty friction, got {out!r}")

    # --- turn boundaries --------------------------------------------------------------------------

    # A slip in a NON-final block of the turn must not be masked by a later clean block.
    expect_facts(
        "admission-in-nonfinal-block",
        [
            user("You keep guessing"),
            assistant_text("I was wrong about the offset."),
            assistant_text("The remaining rows are unchanged."),
        ],
        friction=True,
        admission=True,
        acted="0",
    )

    # Tool-result carriers must not split the turn (otherwise the action would land in its own turn
    # and every admission would look actionless).
    expect_facts(
        "tool_result-does-not-split-the-turn",
        [
            user("You keep guessing"),
            assistant_text("I was wrong about the offset. Fixing it now."),
            tool_result(),
            assistant_tool("Edit"),
        ],
        friction=True,
        admission=True,
        acted="1",
    )

    # Read-only inspection is not rectifying: a Read-only turn still reports acted=0.
    expect_facts(
        "read-only-tool-is-not-action",
        [
            user("You keep guessing"),
            assistant_text("I was wrong about the offset."),
            tool_result(),
            assistant_tool("Read"),
        ],
        friction=True,
        admission=True,
        acted="0",
    )

    print("stall-on-friction signal tests passed (15 cases)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
