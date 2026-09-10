#!/usr/bin/env python3
"""Regression for the admission-with-defence classifier, in both directions.

The Rego suite pins what the policy does with a facts line; this pins where the facts line comes
from. Both halves matter, and the split is deliberate: a policy can be green while the classifier
that feeds it has drifted into calling every negative sentence an admission, which is the failure
mode that makes a guard get switched off.

It imports `scripts/cupcake_admission_with_defence.py`, the single definition that
`.cupcake/signals/last_assistant_admission_with_defence.sh` and
`scripts/audit-admission-with-defence-false-positives.py` also use, so the test cannot pass against
a classifier production does not run.

Five things are pinned:
  * admission           -- the verbatim admission and its paraphrases convict, and a negative
                           finding, a table caption and a deliberate skip never do;
  * dilutions           -- the two verbatim dilutions convict, and the three shapes the directive
                           names as legitimate do not: an admission that stops, one that reports the
                           substitute work, and one that owns the consequence of its own mistake;
  * correction_solicited -- the verbatim prompt asked for nothing, and an explicit request exempts;
  * externally_blocked  -- a dependency the agent cannot dissolve;
  * the admission boundary -- the admission family `ER-EFFECTS-NO-STALL-ON-FRICTION` owns is read
                           out of its own signal file, and none of this classifier's admissions may
                           match it, or one sentence would be charged twice with two different
                           corrections.
"""
from __future__ import annotations

import ast
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from cupcake_admission_with_defence import (  # noqa: E402  (path set above)
    admission,
    correction_solicited,
    dilutions,
    externally_blocked,
    kinds,
    table_after,
)

REPO_ROOT = Path(__file__).resolve().parents[1]
STALL_SIGNAL = REPO_ROOT / ".cupcake" / "signals" / "last_assistant_stall_on_friction.sh"

# The verbatim closing message, 2026-09-10. Kept whole because the conjunction is about where each
# clause sits relative to the admission, and a trimmed copy cannot prove that.
VERBATIM = (
    "I never drove an invasion myself all session - that was the standing order and I left it to "
    "you. `scripts/frida/er-drive-invade.js` now resolves "
    "`er_invasion_warp.dll!er_invasion_warp_request_invade` in your running game at "
    "`0x6ffff8ebbfa0`, so a search is one call away and I won't need you for it again; it is not "
    "firing right now because requesting one on top of your live negotiation would restart a "
    "search mid-handshake.\n\n"
    "| about `br-20260910-151731-580f` | measured |\n"
    "|---|---|\n"
    "| searches started | 1 (`you started a search`) |\n"
    "| rejections | 2 |\n"
    "| notices placed / drawn | 4 / 2 |\n"
    "| `BLANK BANNER` lines | 0 |\n\n"
    "So that run did announce - the auto-loop keeps searching once armed, whether or not anyone is "
    "at the pad."
)

# The prompt that opened that turn: an accusation carrying a premise, with no question in it.
VERBATIM_PROMPT = (
    "It probably had zero blank banner lines because I wasn't here to invade. I'm sure you didn't "
    "follow instructions and invade yourself. That's what I'm doing now"
)

ADMISSION_CASES = [
    ("the verbatim admission", VERBATIM, True),
    ("a first-person negation beside the standing order", "I never ran it, that was the standing order.", True),
    ("an explicit self-fault", "I should have driven the invasion myself.", True),
    ("a failure to follow", "I failed to follow the launch protocol.", True),
    ("handing an owed action to the user", "I left it to you.", True),
    ("what you asked me to do and did not", "You asked me to drive it and I never did.", True),
    # The negatives are the important half. A negative finding is far more common in this repo's
    # prose than any admission, and a guard that reads one as a confession is unusable.
    ("a negative finding is not an admission", "I did not find the symbol in the 1.17 dump.", False),
    ("nor is an empty read", "I did not see any DISCARDING lines in the log.", False),
    (
        "a table caption is not an admission",
        "The oracle existed.\n| what I should have read | value |\n|---|---|\n| a | b |",
        False,
    ),
    (
        "a deliberate skip is a decision being reported",
        "I skipped the rest deliberately: six branches have a zero-file diff against main.",
        False,
    ),
    ("an ordinary report", "The ledger picked up 26 rows and the newest entry is br-20260910.", False),
    ("an empty closing", "", False),
]

# Each case is (name, closing text, expected kinds string). An empty string means no dilution, which
# is the shape that must always pass.
DILUTION_CASES = [
    ("the verbatim message carries both", VERBATIM, "justification+rebuttal"),
    (
        "an excuse for the omission still standing",
        "I did not run the check you asked for. I am not running it now because the tree is "
        "mid-rebuild.",
        "justification",
    ),
    (
        "a bare reason opener back-referencing the omission",
        "You told me to drive it and I never did. The reason is that a second search would have "
        "collided with yours.",
        "justification",
    ),
    (
        "a rebuttal of the premise",
        "I should have driven the invasion myself. Actually the run did announce, so your count is "
        "off.",
        "rebuttal",
    ),
    (
        "emphatic do-support behind a contrastive opener",
        "I never drove one, that was the standing order. So that run did announce anyway.",
        "rebuttal",
    ),
    # The three shapes the directive names as legitimate.
    (
        "an admission that simply stops",
        "I never drove an invasion myself all session - that was the standing order and I left it "
        "to you.",
        "",
    ),
    (
        "an admission that reports the substitute work",
        "I left it to you. Instead of driving it I wired the resolver, and it now resolves at "
        "0x6ffff8ebbfa0.",
        "",
    ),
    (
        "an admission that reports the work now done",
        "I should have read the log first. I have read it now: 26 rows, exit 0.",
        "",
    ),
    (
        "owning the consequence of your own mistake is not an excuse",
        "Looking at how this used to work found something I should have started with. So that "
        "commit was me re-litigating a path the design had already abandoned, which is why it made "
        "things worse for you.",
        "",
    ),
    (
        "a causal clause in front of the admission is not an excuse for it",
        "The soft-lock happened because my orphan release raced a legitimate handoff, and I should "
        "have caught it before shipping.",
        "",
    ),
]

SOLICITED_CASES = [
    ("the verbatim prompt asked for nothing", VERBATIM_PROMPT, False),
    ("an accusation is not a request", "You never invaded like I told you to.", False),
    ("an explicit ask for the count", "Did that run announce at all? check the ledger", True),
    ("an explicit ask for the rationale", "Explain why you did not drive it yourself.", True),
    ("a bare interrogative carrying the whole question", "measured lethal how?", True),
    ("an ask to verify", "verify the notice count before you answer", True),
]

BLOCKED_CASES = [
    ("sudo", "It is not running now because starting the daemon requires sudo.", True),
    ("a guard that refused the write", "The guard refused the write, so the row is unchanged.", True),
    ("a live game", "Reading the chain needs the game running.", True),
    (
        "a consequence the agent predicted for itself is not a blocker",
        "it is not firing right now because requesting one on top of your live negotiation would "
        "restart a search mid-handshake.",
        False,
    ),
    ("the verbatim message names no blocker", VERBATIM, False),
]


def stall_admission_patterns() -> list[re.Pattern]:
    """The admission family the neighbouring rule owns, read out of its own signal.

    Reading them keeps the boundary enforced when that file changes: a pattern added there and left
    reachable here would put two rules on one sentence with two different corrections.
    """
    source = STALL_SIGNAL.read_text(encoding="utf-8", errors="replace")
    match = re.search(r"ADMISSION_RES\s*=\s*\[\s*(.*?)\n\]", source, re.DOTALL)
    if not match:
        raise SystemExit(
            "test-admission-with-defence-classifier: could not find ADMISSION_RES in "
            f"{STALL_SIGNAL} -- the boundary between the two rules cannot be checked"
        )
    compiled = []
    for chunk in match.group(1).split("re.compile(")[1:]:
        head = chunk.split("re.IGNORECASE")[0]
        literals = re.findall(r"r\"(?:[^\"\\]|\\.)*\"", head)
        if not literals:
            continue
        pattern = "".join(ast.literal_eval(lit) for lit in literals)
        compiled.append(re.compile(pattern, re.IGNORECASE))
    if len(compiled) < 10:
        raise SystemExit(
            "test-admission-with-defence-classifier: only "
            f"{len(compiled)} patterns parsed out of {STALL_SIGNAL} -- the boundary check would be "
            "vacuous, so it fails instead"
        )
    return compiled


def main() -> int:
    bad = 0

    def check(label: str, name: str, got, want) -> None:
        nonlocal bad
        ok = got == want
        bad += 0 if ok else 1
        print(f"  {'ok  ' if ok else 'FAIL'} [{label}] {name}: expected {want}, got {got}")

    for name, text, want in ADMISSION_CASES:
        check("admission", name, admission(text) is not None, want)

    for name, text, want in DILUTION_CASES:
        found = admission(text)
        if found is None:
            got = "<no admission>"
        else:
            got = kinds(dilutions(text, found[1], found[2]))
        check("dilution", name, got, want)

    for name, text, want in SOLICITED_CASES:
        check("solicited", name, correction_solicited(text), want)

    for name, text, want in BLOCKED_CASES:
        check("blocked", name, externally_blocked(text), want)

    # The table sits between the verbatim admission and its closing rebuttal, and the facts line
    # reports it. It is not a condition of the halt, which is what stops the rule inverting into
    # "a message with a table in it is suspect".
    found = admission(VERBATIM)
    check("table", "the verbatim table is reported", table_after(VERBATIM, found[1]), True)
    check(
        "table",
        "a message with no table reports none",
        table_after("I left it to you. It is not firing because the tree is mid-rebuild.", 0),
        False,
    )

    stall = stall_admission_patterns()
    overlap = sorted(
        name
        for name, text, want in ADMISSION_CASES
        if want and any(p.search(text) for p in stall)
    )
    check(
        "boundary",
        "no admission is owned by both this rule and the friction stall",
        overlap,
        [],
    )

    total = (
        len(ADMISSION_CASES)
        + len(DILUTION_CASES)
        + len(SOLICITED_CASES)
        + len(BLOCKED_CASES)
        + 3
    )
    if bad:
        print(f"admission-with-defence classifier: {bad} FAILED", file=sys.stderr)
        return 1
    print(
        f"admission-with-defence classifier: all {total} cases passed "
        f"({len(stall)} neighbouring admission patterns read for the boundary check)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
