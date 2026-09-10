#!/usr/bin/env python3
"""Unit tests for the future-commitment classifier -- the half a Rego test cannot see.

`.cupcake/tests/no_future_tense_commitment_test.rego` proves the conjunction: given these facts,
does the policy halt. It says nothing about whether a real sentence ever produces those facts,
because it hands the policy a facts line somebody typed. The prose classification lives in
`scripts/cupcake_future_commitment.py`, and that is where a guard silently stops firing -- a pattern
that no longer matches leaves every layer green.

So each catchable phrasing from the 2026-09-09 directive is asserted here against the classifier
itself, together with the shapes that must stay silent. The verbatim sentences are kept verbatim on
purpose: a paraphrase would let the guard drift away from the instance it exists to refuse.

Run: python3 scripts/test-future-commitment-signal.py
"""
from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import cupcake_future_commitment as fc  # noqa: E402

VERBATIM = (
    "Next run I'll build this in and re-attach Frida to confirm zero DISCARDING lines across a "
    "full invade-reject-reinvade cycle."
)

DELEGATION_VERBATIM = (
    "It has the verbatim sentence, the exemptions that must not trip (a real blocker, the user "
    "owning the observation, work already started in-turn), and the same evidence bar the others "
    "got: measured false-positive rate over real transcripts, all four repo gates, and a cupcake "
    "eval returning decision:block on that exact line rather than a passing unit test."
)


def edit_tool() -> dict:
    return {"name": "Edit", "input": {"file_path": "crates/x/src/lib.rs", "old_string": "a", "new_string": "b"}}


def bash(cmd: str, background: bool = False) -> dict:
    return {"name": "Bash", "input": {"command": cmd, "run_in_background": background}}


CASES: list[tuple[str, bool]] = []


def check(name: str, condition: bool) -> None:
    CASES.append((name, condition))


def promised(text: str):
    return fc.promised_action(text)


def main() -> int:
    # --- one case per catchable phrasing ---------------------------------------------------------
    hit = promised(VERBATIM)
    check("the verbatim instance is caught", hit is not None)
    check("its class is the build it never ran", hit is not None and hit[1] == "build")
    check("its time marker is seen", hit is not None and hit[2] is True)

    for phrase, expected in [
        ("Next time I'll rebuild before launching.", "build"),
        ("Afterwards I'll re-run the gate.", "launch"),
        ("Then I'll commit the fix.", "vcs"),
        ("Once the DLL is staged, I'll relaunch and watch the log.", "launch"),
        ("I'll build the package.", "build"),
        ("I will re-attach Frida.", "attach"),
        ("I'm going to rerun the probe.", "launch"),
        ("I plan to push the branch.", "vcs"),
        ("I'll read the log.", "read"),
        # The trailing purpose clause: the only concrete verb sits past the object.
        ("I'll take another pass and confirm the counter reaches zero.", "measure"),
    ]:
        got = promised(phrase)
        check(f"caught: {phrase!r}", got is not None and got[1] == expected)

    # --- the shapes that must stay silent --------------------------------------------------------
    for phrase in [
        # A hedge is not a commitment.
        "I'll probably look at the offsets again.",
        # A negation is its opposite.
        "I'll not touch the profile until the run lands.",
        # No allowlisted verb in a verb position. `time` is in the measuring group and `land` in the
        # editing group, and both were measured firing on these two real sentences before the scan
        # required a verb position.
        "Since there's no colour in the log to retain, I'll colourise at display time.",
        "Use the Lynchpin whenever you're ready and I'll get the events as they land.",
        # A stance, not an action.
        "I'll keep that in mind for the next pass.",
    ]:
        check(f"silent: {phrase!r}", promised(phrase) is None)

    # Only the closing sentences are read: a promise made mid-message and kept later in the same
    # message is the correct shape.
    mid_turn = (
        "I'll read the filter first.\nThe early return is now unconditional. "
        "The gate is green and the provenance record is written."
    )
    check("a promise earlier in the message is not the closer", promised(mid_turn) is None)

    # --- one case per exemption ------------------------------------------------------------------
    check(
        "an edit keeps an edit-class promise",
        fc.action_taken("edit", [edit_tool()]),
    )
    check(
        "an edit does not keep a build-class promise",
        not fc.action_taken("build", [edit_tool()]),
    )
    check(
        "a build command keeps a build-class promise",
        fc.action_taken("build", [bash("bash scripts/er-build-dlls.sh er-invasion-warp")]),
    )
    check(
        "a memory note that mentions rebuilding is not a build",
        not fc.action_taken(
            "build", [bash("$HOME/.local/bin/bd remember --key k \"rebuild the DLL next\"")]
        ),
    )
    check(
        "an attach keeps an attach-class promise",
        fc.action_taken("attach", [bash("python3 scripts/er-frida-up.py")]),
    )
    check(
        "an edit alone does not keep a measure-class promise",
        not fc.action_taken("measure", [edit_tool()]),
    )
    check(
        "a blocker only the user can clear is seen",
        bool(fc.EXTERNAL_BLOCKER_RE.search("Tell me what you saw on screen and I'll rerun it.")),
    )
    check(
        "a plain closing sentence is not a blocker",
        not fc.EXTERNAL_BLOCKER_RE.search(VERBATIM),
    )
    check(
        "a request for the plan exempts",
        fc.plan_requested("what's the plan for the next run?"),
    )
    check(
        "an ordinary question does not exempt",
        not fc.plan_requested("did the DLL load?"),
    )
    check(
        "work started in this turn and waited on covers the promise",
        fc.deferred_to_started_work(
            "I'll read the findings once it reports.", [bash("bash scripts/probe.sh", True)]
        ),
    )
    check(
        "the same sentence with nothing started does not",
        not fc.deferred_to_started_work("I'll read the findings once it reports.", [edit_tool()]),
    )
    check(
        "a promise waiting on a subagent still running from an earlier turn is covered",
        fc.deferred_to_started_work(
            "I'll push that once the worktree subagent makes those two gates green.",
            [edit_tool()],
            live_background=True,
        ),
    )
    check(
        "live work the promise does not wait on covers nothing",
        not fc.deferred_to_started_work(
            "Next run I'll build this in and re-attach Frida.", [edit_tool()], live_background=True
        ),
    )
    check(
        "a watcher started in this turn covers the promise",
        fc.deferred_to_started_work("I'll get the events as they land.", [{"name": "Monitor", "input": {}}]),
    )
    check(
        "a heredoc body that ends a line on an ampersand is not a detached shell",
        not fc.started_watcher([bash("python3 - <<'PY'\ns = 'let x = &mut y;' &\nPY")]),
    )
    offer = promised("Say the word and I'll build it.")
    check("a conditional offer is flagged as conditional", offer is not None and offer[3] is True)
    contingency = promised("I'll relaunch them that way if either trips over the other.")
    check(
        "a contingency on an event is flagged as conditional",
        contingency is not None and contingency[3] is True,
    )
    check("the verbatim instance is unconditional", hit is not None and hit[3] is False)

    # --- the delegation arm ----------------------------------------------------------------------
    check(
        "the delegation instance is caught",
        fc.delegation_claim(DELEGATION_VERBATIM) != "",
    )
    check(
        "a future-tense claim about the delegate is caught",
        fc.delegation_claim("It will produce the policy, the signal, the tests, and a measured rate.")
        != "",
    )
    check(
        "a bare dispatch statement is not a claim",
        fc.delegation_claim("A subagent is working on the policy.") == "",
    )
    check(
        "describing the instruction is not a claim",
        fc.delegation_claim(
            "I asked it to measure the false-positive rate, run the gates, and report the number."
        )
        == "",
    )
    check(
        "the instruction framing is reported as its own fact",
        fc.delegation_instruction_framed(
            "I asked it to measure the false-positive rate, run the gates, and report the number."
        ),
    )
    dispatch = {"name": "Agent", "id": "t1", "input": {"prompt": "write the guard"}}
    launched = {
        "message": {
            "content": [
                {
                    "type": "tool_result",
                    "tool_use_id": "t1",
                    "content": [{"type": "text", "text": "Async agent launched successfully."}],
                }
            ]
        }
    }
    notified = {
        "message": {
            "content": "<task-notification><tool-use-id>t1</tool-use-id><status>completed</status>"
            "</task-notification>"
        }
    }
    check(
        "a dispatch with only the launch acknowledgement is unreported",
        fc.unreported_dispatch([dispatch], [launched]),
    )
    check(
        "a dispatch with a completion notification is reported",
        not fc.unreported_dispatch([dispatch], [launched, notified]),
    )
    check(
        "a turn that dispatched nothing has nothing to misreport",
        not fc.unreported_dispatch([edit_tool()], [launched]),
    )

    # --- quoting cannot trip either arm -----------------------------------------------------------
    check(
        "a backticked quotation of the instance is inert",
        promised("The rule refuses `Next run I'll build this in and re-attach Frida`.") is None,
    )

    failures = [name for name, ok in CASES if not ok]
    for name, ok in CASES:
        print(f"{'ok  ' if ok else 'FAIL'} {name}")
    if failures:
        print(f"\ntest-future-commitment-signal: {len(failures)} failure(s)", file=sys.stderr)
        return 1
    print(f"\ntest-future-commitment-signal: OK ({len(CASES)} cases)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
