#!/usr/bin/env python3
"""Behavioral tests for the cupcake signal `last_assistant_wall_of_text`.

The signal scans the last assistant turn that actually said something and returns:
  * WALLOFTEXT:<n>:<opener>  -- the turn's longest contiguous run of prose was n > 1 paragraphs;
  * ""                       -- clean.

Three properties matter and each has a wrong answer that already shipped:

  * Which turn. The consumer is UserPromptSubmit, where the new prompt may already be on disk. Taking
    `turns[-1]` there reads an empty turn and the guard silently never fires -- so the signal must
    pick the last turn with text, and this file drives both orderings.
  * What unit. The longest contiguous run, not the turn's prose summed. A tool-heavy turn emits a
    one-line preamble before each call; summing eleven of those scored the turn as an eleven-paragraph
    wall and halted ordinary work.
  * What it says. The tag has to carry the count and the opener, because the correction quotes them
    back. A correction that cannot name what it measured is a generic nag, and generic nags are what
    this guard already tried and lost with.

We drive the real signal against crafted transcript JSONL under a temporary home so its
`~/.claude/projects/<cwd-key>/*.jsonl` discovery resolves to our fixture, then assert the tag.
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
SIGNAL = REPO_ROOT / ".cupcake" / "signals" / "last_assistant_wall_of_text.sh"

PROJECT_DIR = "/fake/project/er-quickload"


def user(text: str) -> dict:
    return {"type": "user", "message": {"content": text}}


def tool_result(tool_use_id: str = "toolu_x") -> dict:
    """A tool-result carrier user event -- must not split the assistant turn."""
    return {
        "type": "user",
        "message": {"content": [{"type": "tool_result", "tool_use_id": tool_use_id, "content": "ok"}]},
    }


def assistant_text(text: str) -> dict:
    return {"type": "assistant", "message": {"content": [{"type": "text", "text": text}]}}


def assistant_text_then_bash(text: str, tool_use_id: str = "toolu_b") -> dict:
    return {
        "type": "assistant",
        "message": {
            "content": [
                {"type": "text", "text": text},
                {"type": "tool_use", "id": tool_use_id, "name": "Bash", "input": {"command": "echo hi"}},
            ]
        },
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


FAILURES: list[str] = []


def expect(name: str, events: list[dict], predicate, describe: str) -> None:
    out = run_signal(events)
    if predicate(out):
        print(f"  ok   {name}")
    else:
        print(f"  FAIL {name}: {describe} (got {out!r})")
        FAILURES.append(name)


def silent(out: str) -> bool:
    return out == ""


def fires(n: int):
    def check(out: str) -> bool:
        return out.startswith(f"WALLOFTEXT:{n}:")

    return check


FOUR_PARAGRAPHS = (
    "The gate is green after the change.\n\n"
    "I looked at the aggregator and it walks the policy tree recursively.\n\n"
    "The signal scripts are unchanged and still emit their tagged markers.\n\n"
    "Next I would look at the routing map to confirm the event names line up."
)


def main() -> int:
    # ---- The defect -------------------------------------------------------------------------------

    expect(
        "true-positive-four-paragraph-answer",
        [user("status?"), assistant_text(FOUR_PARAGRAPHS)],
        fires(4),
        "expected WALLOFTEXT:4 for a four-paragraph closing message",
    )

    # The opener is what the correction quotes back, so it must be the offending run's first line.
    expect(
        "tag-carries-the-opener",
        [user("status?"), assistant_text(FOUR_PARAGRAPHS)],
        lambda out: out.endswith("The gate is green after the change."),
        "expected the tag to end with the offending run's first line",
    )

    # ---- Which turn -------------------------------------------------------------------------------

    # At UserPromptSubmit the next prompt is often already on disk. Reading turns[-1] there sees an
    # empty turn and the guard goes silent forever, which is how a guard dies without anyone noticing.
    expect(
        "fires-when-the-next-prompt-is-already-on-disk",
        [user("status?"), assistant_text(FOUR_PARAGRAPHS), user("and now?")],
        fires(4),
        "expected the previous turn to still be judged once the new prompt has landed",
    )

    # ---- What unit --------------------------------------------------------------------------------

    # Six one-line preambles between six tool calls: six runs of one paragraph, not a six-paragraph
    # wall. This is the shape the old whole-turn sum halted on, measured at 15 real turns.
    narration: list[dict] = [user("go fix the offsets")]
    for i in range(6):
        narration.append(assistant_text_then_bash(f"Checking step {i}.", f"toolu_{i}"))
        narration.append(tool_result(f"toolu_{i}"))
    narration.append(assistant_text("Done: all six steps pass."))
    expect(
        "narration-between-tool-calls-is-silent",
        narration,
        silent,
        "expected silence for one-line preambles interleaved with tool calls",
    )

    # A genuine multi-paragraph run still fires even when it is mid-turn rather than the closer: the
    # user read it either way.
    expect(
        "mid-turn-wall-still-fires",
        [
            user("explain"),
            assistant_text_then_bash("First.\n\nSecond.\n\nThird.", "toolu_m"),
            tool_result("toolu_m"),
            assistant_text("Done."),
        ],
        fires(3),
        "expected a three-paragraph mid-turn run to be reported",
    )

    # ---- Structure is not prose -------------------------------------------------------------------

    expect(
        "one-paragraph-plus-table-is-silent",
        [user("status?"), assistant_text("Here is the answer.\n\nBefore:\n| a |\n|---|\n\nAfter:\n| b |\n|---|")],
        silent,
        "expected a paragraph plus two captioned tables to be one paragraph of prose",
    )

    expect(
        "one-paragraph-plus-code-is-silent",
        [user("status?"), assistant_text("Run this.\n\n```sh\nbash scripts/check.sh\n\ncargo test\n```")],
        silent,
        "expected fenced code (blank lines included) not to count as prose",
    )

    expect(
        "heading-plus-one-paragraph-is-silent",
        [user("status?"), assistant_text("## Root cause\n\nThe hook fires after the text is streamed.")],
        silent,
        "expected a heading to be a signpost rather than a paragraph",
    )

    # ---- Clean ------------------------------------------------------------------------------------

    expect(
        "single-paragraph-is-silent",
        [user("status?"), assistant_text("Fixed: the routing now targets UserPromptSubmit.")],
        silent,
        "expected silence for a one-paragraph answer",
    )

    expect(
        "turn-with-no-prose-is-silent",
        [user("go"), assistant_text_then_bash("", "toolu_q"), tool_result("toolu_q")],
        silent,
        "expected silence when the turn never emitted prose",
    )

    expect(
        "empty-transcript-is-silent",
        [],
        silent,
        "expected silence (fail open) on an empty transcript",
    )

    if FAILURES:
        print(f"\ntest-wall-of-text-signal: {len(FAILURES)} failure(s): {', '.join(FAILURES)}", file=sys.stderr)
        return 1
    print("test-wall-of-text-signal: all cases passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
