#!/usr/bin/env python3
"""Regression for the narrated-action classifier, in both directions.

The Rego suite pins what the policy does with a facts line; this pins where the facts line comes
from. Both halves matter, and the split is deliberate: a policy can be green while the classifier
that feeds it has drifted into calling every gerund a narration, which is the failure mode that
makes a guard get switched off.

It imports `scripts/cupcake_narrated_action.py`, the single definition that
`.cupcake/signals/last_assistant_narrated_action.sh` and
`scripts/audit-narrated-action-false-positives.py` also use, so the test cannot pass against a
classifier production does not run.

Five things are pinned:
  * narrated_action    -- the five verbatim closers convict, and a gerund that is the subject of an
                          ordinary sentence never does;
  * reported_outcome   -- a measured outcome beside the narration exempts;
  * launch_banner      -- the banner `AGENTS.md` mandates, in both spellings this repo writes it in;
  * externally_blocked -- a dependency the agent cannot dissolve;
  * the verb boundary  -- the gerunds `ER-EFFECTS-NO-PROMISSORY-CLOSER` owns are read out of its own
                          signal file and must not appear in this classifier's table, or one closing
                          sentence would be charged twice with two different corrections.
"""
from __future__ import annotations

import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from cupcake_narrated_action import (  # noqa: E402  (path set above)
    ACTION_CLASS,
    externally_blocked,
    launch_banner,
    narrated_action,
    reported_outcome,
)

REPO_ROOT = Path(__file__).resolve().parents[1]
PROMISSORY_SIGNAL = REPO_ROOT / ".cupcake" / "signals" / "last_assistant_diagnosis_without_fix.sh"

# The five verbatim closers, one session, 2026-09-10.
RERUN = "Re-running it now without the cap."
REBUILD = "Rebuilding and relaunching now."
BRINGING = (
    "Bringing it up to read the pointer chain live rather than guessing another offset:"
)
DIAGNOSABLE = (
    "Making the empty read diagnosable - logging each of the three hops so the next invasion says "
    "*which* one returned zero instead of just that one did:"
)
DISPATCH = (
    "Dispatching a subagent to enumerate the menu builder's rows properly, and unblocking you now "
    "by narrowing the guard to the case where the engine has no session to protect:"
)

NARRATION_CASES = [
    ("the rerun closer", RERUN, True),
    ("the rebuild closer", REBUILD, True),
    ("the bringing-up closer", BRINGING, True),
    ("the diagnosable closer", DIAGNOSABLE, True),
    ("the dispatch closer", DISPATCH, True),
    ("a bare fragment", "Measuring it now.", True),
    ("a two-word dispatch", "Dispatching on it.", True),
    ("a first-person progressive", "I'm instrumenting the child teardown next.", True),
    ("a lead adverb in front of the participle", "Now checking the three call sites:", True),
    (
        "a semicolon clause is its own sentence",
        "The wording needed explaining; rebuilding and relaunching with it.",
        True,
    ),
    # The negatives are the important half. A guard that convicts these takes ordinary reporting
    # away, which costs far more than the stall it would catch.
    (
        "a gerund that is the subject of a sentence",
        "Reading the binary first is a rule this repo already has, and I have now paid the tuition.",
        False,
    ),
    (
        "a gerund subject with a finite verb and a colon",
        "Making it worse: the second hop also returns zero on the reject path.",
        False,
    ),
    (
        "a report that happens to open on a participle",
        "Running the whole suite takes nine seconds, which is cheaper than the four lines above.",
        False,
    ),
    ("a hedge is not an announcement", "Worth re-running it without the cap.", False),
    (
        "a subordinate clause is not an announcement",
        "Without rebuilding it first, the launch would load the previous artifact.",
        False,
    ),
    ("a code-change gerund belongs to the promissory closer", "Wiring the detour entry now.", False),
    ("a plain report", "The ledger picked up 26 rows and the newest entry is br-20260910.", False),
    ("an empty closing", "", False),
    (
        "a fenced block after the narration is the answer, not an announcement",
        "Checking the three offsets:\n\n```\n0x140749e20  0x140749e90  0x14074a970\n```",
        False,
    ),
]

REPORTED_CASES = [
    ("an exit code beside the narration", "Re-running it now - exit 0, 26 rows.", True),
    ("a bare measured pair", "Rebuilding now - 41s, 26 shells relinked.", True),
    ("a path", "Reading /home/banon/Elden/quicksave.me3 now.", True),
    ("a file name", "Reading er-quickload-autoload-debug.log now.", True),
    ("an address", "Bringing it up at 0x14074a970 now.", True),
    ("the five verbatim closers carry nothing measured", RERUN, False),
    ("nor does the dispatch closer", DISPATCH, False),
    ("a single digit is a count of work, not a result", "Checking the 3 call sites now.", False),
]

BOXED_BANNER = (
    "Same character will come back: the live run is Stink Bean, level 90.\n\n"
    "```\n"
    "╔══════════╗\n"
    "║  ⚠  TEARING DOWN YOUR ELDEN RING SESSION NOW  ⚠  ║\n"
    "╚══════════╝\n"
    "```\n\n"
    "Relaunching Stink Bean now."
)

INLINE_BANNER = (
    "**LAUNCHING ELDEN RING NOW** - same character and save as the crashed run, freshly built "
    "er_invasion_warp.dll with the null-repository gate. Relaunching Stink Bean now."
)

BANNER_CASES = [
    ("the box-drawn teardown banner", BOXED_BANNER, True),
    ("the bold inline launch banner", INLINE_BANNER, True),
    ("an ordinary narration is not a banner", RERUN, False),
    ("a lowercase mention of launching is not a banner", "Launching the audit now.", False),
]

BLOCKED_CASES = [
    ("an observation only the user has", "Bringing it up once you tell me what you saw.", True),
    ("sudo", "Starting the daemon requires sudo.", True),
    ("a guard that refused the write", "The guard refused the write, so the row is unchanged.", True),
    ("a live game", "Reading the chain needs the game running.", True),
    ("an ordinary narration names no blocker", DISPATCH, False),
    ("nor does the rerun closer", RERUN, False),
]


def promissory_closer_verbs() -> set[str]:
    """The gerunds the neighbouring rule owns, read out of its own signal rather than copied.

    Reading them keeps the boundary enforced when that file changes: a verb added there and left
    here would put two rules on one sentence.
    """
    source = PROMISSORY_SIGNAL.read_text(encoding="utf-8", errors="replace")
    match = re.search(r"CLOSER_VERBS\s*=\s*\(\s*(.*?)\)\s*\n", source, re.DOTALL)
    if not match:
        raise SystemExit(
            "test-narrated-action-classifier: could not find CLOSER_VERBS in "
            f"{PROMISSORY_SIGNAL} -- the verb boundary between the two rules cannot be checked"
        )
    return set(re.findall(r"[a-z]+ing", match.group(1)))


def main() -> int:
    bad = 0

    def check(label: str, name: str, got, want) -> None:
        nonlocal bad
        ok = got == want
        bad += 0 if ok else 1
        print(f"  {'ok  ' if ok else 'FAIL'} [{label}] {name}: expected {want}, got {got}")

    for name, text, want in NARRATION_CASES:
        check("narration", name, narrated_action(text) is not None, want)
    for name, text, want in REPORTED_CASES:
        check("reported", name, reported_outcome(text), want)
    for name, text, want in BANNER_CASES:
        check("banner", name, launch_banner(text), want)
    for name, text, want in BLOCKED_CASES:
        check("blocked", name, externally_blocked(text), want)

    overlap = promissory_closer_verbs() & set(ACTION_CLASS)
    check(
        "boundary",
        "no gerund is owned by both this rule and the promissory closer",
        sorted(overlap),
        [],
    )

    total = (
        len(NARRATION_CASES)
        + len(REPORTED_CASES)
        + len(BANNER_CASES)
        + len(BLOCKED_CASES)
        + 1
    )
    if bad:
        print(f"narrated-action classifier: {bad} FAILED", file=sys.stderr)
        return 1
    print(f"narrated-action classifier: all {total} cases passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
