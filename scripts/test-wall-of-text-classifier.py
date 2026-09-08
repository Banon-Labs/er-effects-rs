#!/usr/bin/env python3
"""Regression for the one-paragraph rule: the classifier the signal actually runs, both directions.

This file used to carry its own copy of the classifier, which meant it could pass while production
counted differently -- the failure mode the shared module exists to end. It now imports
`scripts/cupcake_turn_scan.prose_paragraphs`, the single definition that
`.cupcake/signals/last_assistant_wall_of_text.sh` and
`scripts/audit-wall-of-text-false-positives.py` also use.

Two things are pinned here:
  * prose_paragraphs -- what counts as a paragraph, and what is exempt structure (scanned, not read);
  * Turn.text_runs   -- the unit the rule measures. A tool-heavy turn's one-line preambles are
                        separate runs of one paragraph each, not an N-paragraph wall; scoring them
                        as one lump is what made the old guard fire on ordinary work.
"""
from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from cupcake_turn_scan import Turn, prose_paragraphs  # noqa: E402  (path set above)


def paragraphs(text: str) -> int:
    return len(prose_paragraphs(text))


CASES = [
    ("single paragraph passes", "This is the case.", 1),
    ("two paragraphs halt", "First.\n\nSecond.", 2),
    ("paragraph plus fenced code is structure", "Prose.\n\n```\ncode\n```", 1),
    ("paragraph plus table is structure", "Prose.\n\n| a | b |\n|---|---|\n| 1 | 2 |", 1),
    ("paragraph plus list is structure", "Prose.\n\n- one\n- two", 1),
    ("prose after a list still halts", "Prose.\n\n- one\n\nMore prose.", 2),
    ("code containing blank lines does not split", "Prose.\n\n```\na\n\nb\n```", 1),
    ("numbered list is structure", "Prose.\n\n1. one\n2. two", 1),
    # --- shapes the per-block rule got wrong, which is most of what an answer actually looks like ---
    (
        "caption plus its table is one scannable unit, not prose",
        "Findings:\n| a | b |\n|---|---|\n| 1 | 2 |",
        0,
    ),
    (
        "bold caption plus its list is not prose",
        "**Changed:**\n- the signal\n- the policy",
        0,
    ),
    (
        "prose, then two captioned tables, is still ONE paragraph",
        "Here is the answer.\n\nBefore:\n| a |\n|---|\n\nAfter:\n| b |\n|---|",
        1,
    ),
    ("a heading is a signpost, not a paragraph", "## Root cause\n\nOne paragraph.", 1),
    ("headings alone are not prose at all", "## One\n\n## Two\n\n## Three", 0),
    ("a horizontal rule is not a paragraph", "Prose.\n\n---\n\nMore prose.", 2),
    (
        "indented list continuations do not turn a list into prose",
        "Prose.\n\n- one\n  continued here\n- two\n  also continued",
        1,
    ),
    (
        "an unterminated fence does not leak its contents into the count",
        "Prose.\n\n```\nline one\n\nline two\n\nline three",
        1,
    ),
    ("empty text has no paragraphs", "", 0),
    ("whitespace-only text has no paragraphs", "   \n\n  \n", 0),
]


def text_block(value: str) -> tuple:
    return ("text", value)


def tool_block(name: str = "Bash") -> tuple:
    return ("tool", {"type": "tool_use", "name": name, "input": {"command": "echo hi"}})


RUN_CASES = [
    (
        "eleven one-line preambles between tool calls are eleven runs of one paragraph",
        Turn(blocks=[b for i in range(11) for b in (text_block(f"Step {i}."), tool_block())]),
        11,  # runs
        1,  # worst run's paragraph count
    ),
    (
        "a genuine multi-paragraph closer is one run of many paragraphs",
        Turn(blocks=[text_block("Preamble."), tool_block(), text_block("One.\n\nTwo.\n\nThree.")]),
        2,
        3,
    ),
    (
        "consecutive text blocks with no tool between them are ONE run",
        Turn(blocks=[text_block("One."), text_block("Two.")]),
        1,
        2,
    ),
    (
        "a turn that never spoke has no runs",
        Turn(blocks=[tool_block(), tool_block()]),
        0,
        0,
    ),
]


def main() -> int:
    bad = 0
    for name, text, want in CASES:
        got = paragraphs(text)
        ok = got == want
        bad += 0 if ok else 1
        print(f"  {'ok  ' if ok else 'FAIL'} {name}: expected {want}, got {got}")

    for name, turn, want_runs, want_worst in RUN_CASES:
        runs = turn.text_runs
        worst = max((paragraphs(r) for r in runs), default=0)
        ok = len(runs) == want_runs and worst == want_worst
        bad += 0 if ok else 1
        print(
            f"  {'ok  ' if ok else 'FAIL'} {name}: expected {want_runs} run(s)/worst {want_worst}, "
            f"got {len(runs)} run(s)/worst {worst}"
        )

    if bad:
        print(f"wall-of-text classifier: {bad} FAILED", file=sys.stderr)
        return 1
    print(f"wall-of-text classifier: all {len(CASES) + len(RUN_CASES)} cases passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
