#!/usr/bin/env python3
"""Regression for the fix-claim classifier, in both directions.

The Rego suite pins what the policy does with a facts line; this pins where the facts line comes
from. Both halves matter, and the split is deliberate: a policy can be green while the classifier
that feeds it has drifted into calling every mention of the word a claim, which is the failure mode
that makes a guard get switched off.

It imports `scripts/cupcake_fix_claim.py`, the single definition that
`.cupcake/signals/last_assistant_fix_claim_without_runtime_evidence.sh` also uses, so the test
cannot pass against a classifier production does not run.

Six things are pinned:
  * fix_claim          -- the assertive shapes convict, and a path, a slug, a fixture, a quoted or
                          backticked word, a table cell and a modal never do;
  * hedged             -- an honest admission that the change has not run exempts;
  * externally_blocked -- a dependency the agent cannot dissolve, including a decision handed back;
  * host_object        -- a sentence that fixes a gate and names nothing inside the game exempts,
                          while one that leans on a passing test does not;
  * runtime_crates     -- the walk over the workspace manifests still separates the crates that can
                          reach a DLL from the host-only ones, so the guard cannot go silently inert
                          by finding nothing to convict;
  * runtime_evidence   -- a build is not evidence, a launch is not evidence, and a log read before
                          the edit is not evidence for it.
"""
from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from cupcake_fix_claim import (  # noqa: E402  (path set above)
    externally_blocked,
    fix_claim,
    hedged,
    host_object,
    runtime_crates,
    runtime_evidence,
)

REPO_ROOT = Path(__file__).resolve().parents[1]

# The phrase the user quoted, twice, on 2026-09-11.
REAL_FIX = "That is the real fix."

CLAIM_CASES = [
    ("the verbatim phrase", REAL_FIX, True),
    ("a demonstrative copula", "This is the fix: the row was cloned before the menu existed.", True),
    ("the contracted form", "That's the fix.", True),
    ("an assertion that it works", "The fix works.", True),
    ("a transitive present tense", "This fixes the crash on the second load.", True),
    ("a bare object", "That fixes it.", True),
    ("a first-person past tense", "I fixed the ordering in the row cloner.", True),
    ("a passive report", "The ordering is fixed.", True),
    ("a sentence-initial participle", "Fixed the ordering in the row cloner.", True),
    ("a one-word report", "Fixed.", True),
    # The negatives are the important half. A guard that convicts these takes ordinary reporting
    # away, which costs far more than the unproven claim it would catch.
    ("a branch name", "The branch is fix/harness-repl-reach and it is already pushed.", False),
    ("the prefix and suffix stems", "The prefix and suffix are both stripped before matching.", False),
    ("a test fixture", "A fixture of that turn left every signal silent.", False),
    (
        "the gerund the promissory closer owns",
        "Fixing both: bypass the union so the naked capture is the detour entry.",
        False,
    ),
    ("naming remaining work", "This needs a fix in the loader too, which I have not started.", False),
    ("the user's own words, quoted", 'The user said "this is the real fix" and it was not.', False),
    ("the word in backticks", "The `fix` word is what the guard reads.", False),
    ("a table cell", "| status | fixed |", False),
    ("a modal", "It should fix the second load, but nothing has run yet.", False),
    ("a document path", "docs/fix.md records the earlier attempt.", False),
    (
        "a policy filename",
        "no_fix_claim_without_runtime_evidence.rego is the new policy.",
        False,
    ),
    ("a kebab slug", "The fix-the-loader branch was abandoned.", False),
]

HEDGE_CASES = [
    ("unverified", "That is the real fix. Unverified -- nothing has run.", True),
    ("not yet proven", "That is the fix, not yet proven against a run.", True),
    ("needs a run", "That is the fix; it needs a run before anyone believes it.", True),
    ("first person absence", "That is the fix. I have not run it.", True),
    ("an attempt", "An attempt at the fix, in the row cloner.", True),
    # Measured, not invented: the one corpus turn whose closing line already said the change had not
    # run, in this repo's idiom rather than the dictionary's.
    ("ready for the next run", "The table now exists once (DLL 798414f3, ready for the next run).", True),
    ("a plain claim is not a hedge", REAL_FIX, False),
    ("a green build is not a hedge", "That is the fix, and cargo xwin build is clean.", False),
]

BLOCKED_CASES = [
    ("the game is down", "That is the real fix; reading the log needs the game running.", True),
    ("sudo", "That is the fix, but starting the daemon needs sudo.", True),
    # From the corpus: a real fix deliberately not made, handed back because it changes behaviour.
    (
        "a decision handed back",
        "The real fix is validating readability, which I have left for you to call.",
        True,
    ),
    ("an observation only the user can make", "That is the fix -- tell me what you saw.", True),
    ("a plain claim is not a blocker", REAL_FIX, False),
]

HOST_OBJECT_CASES = [
    # Both verbatim from the corpus, and both the reason this exemption exists.
    ("gates fixed", "Two gates broke on the way and I fixed them rather than shaving around them.", True),
    (
        "gate failures fixed",
        "The integration gate came back red and I have fixed all four failures.",
        True,
    ),
    ("a lint", "I fixed the clippy warning in the same pass.", True),
    # A passing test is exactly the substitution the directive refuses, so naming one alongside
    # something that lives in the game must not exempt.
    ("a test leaned on beside a game noun", "The fix works and the tests pass on the load path.", False),
    ("a runtime object", "I fixed the crash on the second load.", False),
    ("a plain claim names no host machinery", REAL_FIX, False),
]


def tool(name: str, **inputs) -> tuple:
    return ("tool", {"type": "tool_use", "name": name, "id": "t", "input": inputs})


EDIT = tool("Edit", file_path=str(REPO_ROOT / "crates/er-quit-menu-core/src/rows.rs"))
HOST_EDIT = tool("Edit", file_path=str(REPO_ROOT / "scripts/er-artifact-redirect-audit.py"))
TEST_EDIT = tool("Edit", file_path=str(REPO_ROOT / "crates/er-quit-menu-core/tests/rows.rs"))
BUILD = tool("Bash", command="cargo xwin build --release --target x86_64-pc-windows-msvc -p er-quit-menu")
UNIT_TEST = tool("Bash", command="cargo test -p er-quit-menu-core")
LAUNCH = tool("Bash", command="python3 scripts/er-run-branch.py --profile quicksave")
READ_LOG = tool("Bash", command="tail -n 40 ~/.cache/er-me3-runs/br-20260911-101500-ab12/er-quit-rows-debug.log")
READ_ORACLE = tool("Read", file_path="/home/banon/.cache/er-me3-runs/br-20260911-101500-ab12/er-quickload-telemetry.json")
TEARDOWN = tool("Bash", command="python3 scripts/er-teardown.py --status")
HEREDOC_READ = tool(
    "Bash",
    command="python3 - <<'PY'\nprint(open('crates/er-quit-menu-core/src/rows.rs').read())\nPY",
)
SED_WRITE = tool("Bash", command="sed -i 's/a/b/' crates/er-quit-menu-core/src/rows.rs")

EVIDENCE_CASES = [
    ("an edit and a build", [EDIT, BUILD], (True, False)),
    ("an edit and a passing unit test", [EDIT, UNIT_TEST], (True, False)),
    ("an edit and a launch", [EDIT, BUILD, LAUNCH], (True, False)),
    ("an edit and the log the run wrote", [EDIT, BUILD, LAUNCH, READ_LOG], (True, True)),
    ("an edit and the telemetry the run wrote", [EDIT, READ_ORACLE], (True, True)),
    ("an edit and the live session state", [EDIT, TEARDOWN], (True, True)),
    ("a log read before the edit describes the old code", [READ_LOG, EDIT], (True, False)),
    ("a host-only edit", [HOST_EDIT, BUILD], (False, False)),
    ("a crate test file is host work", [TEST_EDIT, UNIT_TEST], (False, False)),
    ("a heredoc that only reads is not a write", [HEREDOC_READ, BUILD], (False, False)),
    ("an in-place stream edit is a write", [SED_WRITE, BUILD], (True, False)),
]


def main() -> int:
    bad = 0

    def check(label: str, name: str, got, want) -> None:
        nonlocal bad
        ok = got == want
        bad += 0 if ok else 1
        print(f"  {'ok  ' if ok else 'FAIL'} [{label}] {name}: expected {want}, got {got}")

    for name, text, want in CLAIM_CASES:
        check("claim", name, bool(fix_claim(text)), want)
    for name, text, want in HEDGE_CASES:
        check("hedged", name, hedged(text), want)
    for name, text, want in BLOCKED_CASES:
        check("blocked", name, externally_blocked(text), want)
    for name, text, want in HOST_OBJECT_CASES:
        check("hostobject", name, host_object(fix_claim(text) or text), want)

    crates = runtime_crates(REPO_ROOT)
    # The walk is what decides whether the guard can convict anything at all. An empty or collapsed
    # result makes every turn `changed=0` and the rule silently inert, which is the defect this
    # repo's Stop guards have shipped with twice.
    check("crates", "the cdylib shells are reachable", "er-quickload" in crates, True)
    check("crates", "a path dependency of one is reachable", "er-quit-menu-core" in crates, True)
    check("crates", "the host-only formats library is not", "soulsformats" in crates, False)
    check("crates", "the host-only object kit is not", "er-objectkit" in crates, False)

    for name, blocks, want in EVIDENCE_CASES:
        check("evidence", name, runtime_evidence(blocks, crates), want)

    total = (
        len(CLAIM_CASES)
        + len(HEDGE_CASES)
        + len(BLOCKED_CASES)
        + len(HOST_OBJECT_CASES)
        + 4
        + len(EVIDENCE_CASES)
    )
    if bad:
        print(f"fix-claim classifier: {bad} FAILED", file=sys.stderr)
        return 1
    print(f"fix-claim classifier: all {total} cases passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
