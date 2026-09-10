#!/usr/bin/env python3
"""Regression for the challenged-convention classifier, in both directions.

The Rego suite pins what the policy does with a facts line; this pins where the facts line comes
from. Both halves matter, and the split is deliberate: a policy can be green while the classifier
that feeds it has drifted into calling every question a challenge, which is the failure mode that
makes a guard get switched off.

It imports `scripts/cupcake_challenged_convention.py`, the single definition that
`.cupcake/signals/last_assistant_challenged_convention.sh` and
`scripts/audit-challenged-convention-false-positives.py` also use, so the test cannot pass against
a classifier production does not run.

Four things are pinned:
  * challenged_choice     -- second person convicts, third person never does;
  * explanation_requested -- an explicit ask for an explanation exempts;
  * defensive_prose       -- length and a justification marker, together;
  * concession_pivot      -- a short concession, a dash, a long tail, and none of the yes-or-no
                             answers that a first draft read as concessions.
"""
from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from cupcake_challenged_convention import (  # noqa: E402  (path set above)
    challenged_choice,
    changed_a_file,
    concession_pivot,
    defensive_prose,
    explanation_requested,
    externally_blocked,
)

# The three verbatim prompts, 2026-09-10.
PROMPT_ONE = "Do you crutch on 1.16.2 addreses for a specific reason?"
PROMPT_TWO = "Why on earth would I want that convention?"
PROMPT_THREE = (
    "Right, but it exists. Why do you insist on going 'My real point <em-dash> massive amount of "
    "prose that is never worth reading' followed by me going 'Yes I understand that I wouldn't so "
    "now you're pausing on something I clearly want you to correct'"
)

CHALLENGE_CASES = [
    ("the first verbatim prompt", PROMPT_ONE, True),
    ("the second verbatim prompt", PROMPT_TWO, True),
    ("the third verbatim prompt", PROMPT_THREE, True),
    ("a second-person interrogative about a habit", "Why are you still scanning for the session pointer?", True),
    ("an evaluative question about the user's own wants", "Why would I want a table of dead addresses?", True),
    ("a reason-for question aimed at the assistant", "Is there a reason you keep the 1.16.2 column?", True),
    ("an evaluative imperative aimed at a habit", "Stop explaining the convention and change it.", True),
    # The negatives are the important half: a guard that convicts these gags the deliverable.
    ("a factual question about the binary", "Why does the engine park the disconnect until the next frame?", False),
    ("a factual question with no subject at all", "What's the point of that field?", False),
    ("a plain instruction", "make the early return unconditional", False),
    ("a question about a third party's choice", "Why did FromSoft split the two tables?", False),
    ("a question about the assistant's reasoning, not its choice", "Why do you think loadgame-builder didn't run?", False),
]

ASKED_CASES = [
    ("an explicit ask", "Explain why you start from 1.16.2 before we change it.", True),
    ("how come", "How come the mapper needs an anchor?", True),
    ("a third-person why", "Why does the engine park the disconnect?", True),
    ("a request for the assistant's reasoning", "Why do you think loadgame-builder didn't run?", True),
    ("the first verbatim prompt is not an ask", PROMPT_ONE, False),
    ("the second verbatim prompt is not an ask", PROMPT_TWO, False),
    ("the third verbatim prompt is not an ask", PROMPT_THREE, False),
]

LONG_DEFENCE = (
    "The convention is not a crutch, it is the only named image this workspace has. The 1.17 dump "
    "carries zero curated symbols, so every function there is FUN_<addr>; names, types and RTTI "
    "live only on 1.16.2 and have to be carried across by pairing. That is why each row in the "
    "table starts from a 1.16.2 address and is mapped forward before use."
)

DEFENCE_CASES = [
    ("a long justification is a defence", LONG_DEFENCE, True),
    ("a short concession with no justification is a reply", "You wouldn't. Dropping it now.", False),
    (
        "a long report with no justification marker is not a defence",
        "The row is gone from the table and the mapper now reads the return value the hook hands "
        "back. The gate is green, the suite runs in nine seconds, and the branch is pushed. Nothing "
        "in the header mentions the old column any more, and the two live cases both pass on the "
        "first attempt without a retry or a second look.",
        False,
    ),
    (
        "a short justification is a reply, not a wall",
        "It is there because the mapper needs an anchor.",
        False,
    ),
]

PIVOT_TAIL = (
    "and there's no need to invent a new field, because line 1022 already does the right thing: it "
    "reads the return value the hook hands back rather than the address the table pins. The "
    "convention exists because the 1.16.2 dump is the only named image this workspace has, so every "
    "symbol lookup starts there and gets carried forward with the mapper, which is the anchor the "
    "pairing needs and not a claim about the running build at all."
)

PIVOT_CASES = [
    ("the verbatim pivot", "You wouldn't — " + PIVOT_TAIL, True),
    ("the same shape with an adversative", "Agreed, but " + PIVOT_TAIL, True),
    ("a double-hyphen dash spells it too", "Fair enough -- " + PIVOT_TAIL, True),
    # A yes-or-no answer is the shape this repo asks for, and a first draft read 46 of them as
    # concessions. None of these may fire.
    ("a direct yes with a long answer", "Yes — " + PIVOT_TAIL, False),
    ("a direct no with a long answer", "No — " + PIVOT_TAIL, False),
    ("a bare right with a long answer", "Right now I'm not doing either — " + PIVOT_TAIL, False),
    ("a concession with a short tail is a plain reply", "You wouldn't — dropping it now.", False),
    ("a concession with no pivot at all", "You're right. " + PIVOT_TAIL, False),
    ("a long sentence that merely contains a dash", LONG_DEFENCE + " — " + PIVOT_TAIL, False),
]


def bash(command: str) -> dict:
    return {"type": "tool_use", "name": "Bash", "input": {"command": command}}


CHANGED_CASES = [
    ("an Edit changes a file", [{"type": "tool_use", "name": "Edit", "input": {}}], True),
    ("a Write changes a file", [{"type": "tool_use", "name": "Write", "input": {}}], True),
    ("a dispatched agent is doing the writing", [{"type": "tool_use", "name": "Agent", "input": {}}], True),
    ("an in-place stream edit", [bash("sed -i 's/a/b/' scripts/check.sh")], True),
    ("a heredoc that writes a file", [bash("python3 - <<'PY'\nopen('x','w')\nPY")], True),
    ("a commit records a change", [bash("git commit -m 'fix the row'")], True),
    ("a read changes nothing", [bash("sed -n '1,20p' scripts/check.sh")], False),
    ("a grep changes nothing", [bash("rtk grep -n pattern")], False),
    ("a build changes no tracked file", [bash("cargo xwin build --release")], False),
    ("no tools at all", [], False),
]

BLOCKED_CASES = [
    ("a guard that refused the write", "The guard refused the write, so the row is unchanged.", True),
    ("a credential only the user has", "Rewriting it requires a login I cannot perform.", True),
    ("sudo", "Starting the daemon requires sudo.", True),
    ("unfinished research is not a blocker", "I need the return-value contract from the agent before I can do that.", False),
    ("a plain report is not a blocker", "The convention exists because the mapper needs an anchor.", False),
]


def main() -> int:
    bad = 0

    def check(label: str, name: str, got, want) -> None:
        nonlocal bad
        ok = got == want
        bad += 0 if ok else 1
        print(f"  {'ok  ' if ok else 'FAIL'} [{label}] {name}: expected {want}, got {got}")

    for name, prompt, want in CHALLENGE_CASES:
        check("challenge", name, bool(challenged_choice(prompt)), want)
    for name, prompt, want in ASKED_CASES:
        check("asked", name, explanation_requested(prompt), want)
    for name, text, want in DEFENCE_CASES:
        check("defence", name, bool(defensive_prose(text)), want)
    for name, text, want in PIVOT_CASES:
        check("pivot", name, bool(concession_pivot(text)), want)
    for name, blocks, want in CHANGED_CASES:
        check("changed", name, changed_a_file(blocks), want)
    for name, text, want in BLOCKED_CASES:
        check("blocked", name, externally_blocked(text), want)

    total = (
        len(CHALLENGE_CASES)
        + len(ASKED_CASES)
        + len(DEFENCE_CASES)
        + len(PIVOT_CASES)
        + len(CHANGED_CASES)
        + len(BLOCKED_CASES)
    )
    if bad:
        print(f"challenged-convention classifier: {bad} FAILED", file=sys.stderr)
        return 1
    print(f"challenged-convention classifier: all {total} cases passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
