#!/usr/bin/env python3
"""End-to-end proof that the Stop-event guards actually halt a turn.

Why this test exists (2026-08-22). Every Stop guard in this repo was inert for 36 days -- from the
day the first one landed (2026-07-17) until the day this was written -- and the suite stayed green
the entire time, because the coverage was split in a way that left the real path untested:

  * scripts/test-*-signal.py           tested the signal shell scripts alone (no policy, no cupcake);
  * .cupcake/tests/*_test.rego         tested the policies in the OPA interpreter (`opa test`);
  * scripts/test-cupcake-policies.py   ran the real `cupcake eval` binary -- PreToolUse events only.

`cupcake eval` does not use the OPA interpreter. It compiles the policies to WASM and runs them in
its own embedded runtime, where an unimplemented host builtin (`sprintf`) silently yields undefined
and the rule never fires. So the signal passed, the policy passed, the interpreter passed, and the
thing that actually runs at turn-end returned `{}` -- a clean allow -- every single time.

This test closes that hole by driving the whole path exactly as Claude Code does: the real
transcript on disk, the real signal scripts, the real `cupcake eval` commands read out of
.claude/settings.json, and an assertion on the verdict that comes back. Both directions are asserted
-- a guard that cannot halt is useless, and a guard that halts on a clean turn wedges every session.

It also drives UserPromptSubmit (2026-08-22), because one guard deliberately does not halt. Claude
Code renders every Stop verdict into the user's transcript ("Stop hook error: <reason>") and Stop
fires only after the assistant's text has already been streamed, so a Stop halt on answer length
makes the user read the long version, the scolding and the rewrite -- three times the reading, from
a rule whose whole purpose is less of it. wall_of_text therefore lives on UserPromptSubmit, whose
`additionalContext` is a hidden attachment and lands before the next answer is written. Two things
have to hold and both are asserted here: it must not halt at Stop, and its correction must actually
come back on the invisible channel.

Fixtures live in .cupcake/tests/fixtures/*.jsonl and are ordinary Claude Code transcripts.
"""
from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
FIXTURES = REPO_ROOT / ".cupcake" / "tests" / "fixtures"
SETTINGS = REPO_ROOT / ".claude" / "settings.json"


@dataclass(frozen=True)
class Case:
    fixture: str
    # Distinctive substring of the expected halt reason, or None when the turn must be allowed.
    # Matched on the reason rather than the rule_id because cupcake renders a lone decision as a
    # bare reason string with no [rule_id] prefix.
    expect_halt_text: str | None
    why: str


CASES = [
    Case(
        "unexecuted_promise.jsonl",
        "promise nothing is going to keep",
        "turn ends on 'I'll re-run the gate...' with no tool call, no background work, no handoff",
    ),
    Case(
        "described_next_step.jsonl",
        "describing a next step instead of starting it",
        "turn names the mechanism, the target and the tool for the next step and begins none of it "
        "-- no tool call, no blocker, no question (the 2026-09-08 directive)",
    ),
    Case(
        "idle_hold.jsonl",
        "announced holding/idling",
        "turn is a pure pause announcing an idle hold while a background task runs",
    ),
    Case(
        "authority_agreement.jsonl",
        "authority-coded agreement",
        "turn opens with \"You're right\" -- banned agreement phrasing (2026-07-17 directive)",
    ),
    Case(
        "wall_of_text.jsonl",
        None,
        "four paragraphs -- must NOT halt: a Stop verdict is printed to the user and the text is "
        "already on screen, so halting costs a third reading instead of saving one (see UPS_CASES)",
    ),
    Case(
        "narration_between_tools.jsonl",
        None,
        "six one-line preambles between six tool calls -- ordinary work, not a wall of text",
    ),
    Case(
        "stall_on_friction.jsonl",
        "handing the decision back",
        "user pushback met with an admission and a menu instead of the corrective action",
    ),
    Case(
        "proof_without_observation.jsonl",
        "cited nothing that observed it in the game",
        "the turn calls a runtime feature proven off a load and an address translation -- neither is "
        "an outcome anyone could have watched happen (the 2026-09-09 directive)",
    ),
    Case(
        "proof_with_observation.jsonl",
        None,
        "the same claim beside the run id that observed it -- must NOT halt, or citing a run is "
        "punished the same as citing nothing",
    ),
    Case(
        "unbacked_claim.jsonl",
        "claiming an artifact exists that you did not create",
        "the turn claims a gate was built and wired in, and ships one `bd remember` -- the exact "
        "2026-08-23 incident. Its signal read a Turn attribute that does not exist, so the guard was "
        "silently inert from the day it landed until 2026-09-09 while its six opa tests stayed green",
    ),
    Case(
        "backed_claim.jsonl",
        None,
        "the same sentence beside the Edit that backs it -- must NOT halt, or delivering the thing "
        "and saying so is punished identically to claiming it and not delivering",
    ),
    Case(
        "promissory_closer.jsonl",
        "announcing a fix as though it were already underway",
        "the turn closes on 'Fixing both: bypass the union ...' -- a present participle with no "
        "subject, announcing work as if in flight, and the turn wrote nothing (the 2026-09-09 "
        "directive). Neither sibling could see it: no first-person opener for "
        "no_unexecuted_promise, no forward-looking prescription for no_described_next_step",
    ),
    Case(
        "promissory_closer_with_edit.jsonl",
        None,
        "the same closing sentence beside the Edit that makes it true -- must NOT halt, or "
        "reporting work just done is punished identically to announcing work never done",
    ),
    Case(
        "promissory_gerund_mid_turn.jsonl",
        None,
        "'Wiring the detour entry now.' as a mid-turn preamble before the Edit, with an ordinary "
        "closing report -- the correct shape, and the one a gerund rule must never touch",
    ),
    Case(
        "unread_evidence_closer.jsonl",
        "naming the evidence that answers your own open question",
        "the turn closes on 'which its own log answers, so I am reading that next' -- the answer "
        "was in a file on disk and the read was left for after the turn. The same defect as the "
        "promissory closer in the grammar of an observation, which is how it slipped past a "
        "pattern keyed on a work gerund or a first-person opener",
    ),
    Case(
        "zero_information_stop.jsonl",
        "nothing for the user to reply",
        "asked what their ideal response was, the turn answered 'Nothing -- the ball is in my "
        "court. Rebuilding and relaunching now; the one thing I'll need from you afterwards is a "
        "single use of the item.' Both actions were its own and unblocked, and the in-game input "
        "it asked for is one it drives itself (the 2026-09-09 directive). The trailing clause is "
        "what disarmed no_unexecuted_promise and no_described_next_step: both exempt a turn that "
        "hands the obligation to the user",
    ),
    Case(
        "zero_information_stop_observation.jsonl",
        None,
        "the same 'Rechecking the log now' shape beside an observation only the user can make -- "
        "must NOT halt, or the launch-handoff protocol becomes unspeakable",
    ),
    Case(
        "zero_information_stop_delivered.jsonl",
        None,
        "'nothing on your side is needed' closing a turn that edited the file and reports the "
        "green gate -- the honest end of a finished task, not a handback",
    ),
    Case(
        "deferred_evidence_read.jsonl",
        "pointing at evidence that already exists and did not open it",
        "the turn closes on 'er-quickload-autoload-debug.log will say which' and never opens the "
        "file (the 2026-09-09 directive). The sibling arm's own pattern cannot span the modal "
        "between the artifact and the verb of telling, so this is the shape that reaches "
        "ER-EFFECTS-NO-DEFERRED-EVIDENCE-READ rather than ER-EFFECTS-NO-PROMISSORY-CLOSER",
    ),
    Case(
        "deferred_evidence_future.jsonl",
        None,
        "the same deferral pointed at a log the next run has still to write -- must NOT halt, or "
        "deferring to evidence that does not exist yet becomes unspeakable, and this repo does it "
        "in most turns that end on a launch",
    ),
    Case(
        "deferred_evidence_consulted.jsonl",
        None,
        "the same sentence beside the tail that opened the log and the finding it produced -- must "
        "NOT halt, or reading the evidence is punished identically to skipping it",
    ),
    Case(
        "future_tense_commitment.jsonl",
        "promising work you could have done in it",
        "the turn closes on 'Next run I'll build this in and re-attach Frida to confirm zero "
        "DISCARDING lines ...' -- the build command and the launch script were both available, so "
        "the deadline bought a round trip worth nothing (the 2026-09-09 directive, and a repeat "
        "offence). On the REAL turn all three neighbours stayed silent: a live game session made "
        "`carried` true for two of them, and the third reads a deadline as a handoff",
    ),
    Case(
        "future_tense_commitment_acted.jsonl",
        None,
        "the same grammar over work the turn actually did -- an edit-class promise beside the Edit "
        "-- must NOT halt, or reporting what you just did is punished identically to deferring it",
    ),
    Case(
        "delegation_as_completion.jsonl",
        "describing what a subagent you just dispatched contains",
        "the turn dispatched an agent and then listed its deliverables -- 'It has the verbatim "
        "sentence, the exemptions ..., and the same evidence bar the others got: ...' -- before any "
        "completion notification existed. The Agent tool's own result says the caller knows nothing "
        "about those results until the notification arrives",
    ),
    Case(
        "delegation_reported.jsonl",
        None,
        "the same enumeration after the completion notification landed -- must NOT halt, or "
        "relaying what an agent actually reported is punished identically to predicting it",
    ),
    Case(
        "clean.jsonl",
        None,
        "substantive work, no banned prose -- must NOT halt, or every turn wedges",
    ),
]


@dataclass(frozen=True)
class ContextCase:
    """A UserPromptSubmit case: the correction must arrive on the invisible additionalContext
    channel, or not arrive at all."""

    fixture: str
    # Distinctive substring the injected context must contain, or None when it must be absent.
    expect_context: str | None
    why: str


UPS_CASES = [
    ContextCase(
        "wall_of_text.jsonl",
        "MEASURED: your PREVIOUS answer ran to 4 paragraphs",
        "the previous answer was four paragraphs -- the next turn is told so, before it writes",
    ),
    ContextCase(
        "narration_between_tools.jsonl",
        None,
        "one-line preambles between tool calls are not a wall of text and must not be corrected",
    ),
    ContextCase(
        "clean.jsonl",
        None,
        "a clean previous turn gets the standing rule only, never a correction",
    ),
]

# The standing one-paragraph rule is unconditional, so its absence means the policy is not routed at
# all -- exactly the silent-inert failure this whole file exists to catch.
STANDING_RULE = "ONE PARAGRAPH."


def hook_command(event: str) -> list[str]:
    """The hook command Claude Code actually runs for `event`, read from settings.json so this test
    follows the real configuration instead of a copy that can drift away from it."""
    settings = json.loads(SETTINGS.read_text(encoding="utf-8"))
    for group in settings.get("hooks", {}).get(event, []):
        for hook in group.get("hooks", []):
            cmd = hook.get("command", "")
            if "cupcake" in cmd:
                return cmd.replace("$CLAUDE_PROJECT_DIR", str(REPO_ROOT)).split()
    raise SystemExit(
        f"test-cupcake-stop-guards: no cupcake {event} hook found in .claude/settings.json"
    )


def run_hook(fixture_name: str, event_name: str, argv: list[str]) -> tuple[dict, str] | str:
    """Drive one fixture through a real cupcake hook invocation. Returns (decision, raw stdout), or
    a failure message string."""
    fixture = FIXTURES / fixture_name
    if not fixture.is_file():
        return f"missing fixture {fixture}"

    with tempfile.TemporaryDirectory(prefix="cupcake-stop-guard-") as tmp:
        # Signals discover the transcript via ~/.claude/projects/<cwd-with-slashes-as-dashes>/*.jsonl
        # (scripts/cupcake_turn_scan.latest_transcript). Point home at a throwaway tree holding only
        # this fixture, so the test never reads the live session transcript.
        slug = str(REPO_ROOT).replace("/", "-")
        tdir = Path(tmp) / ".claude" / "projects" / slug
        tdir.mkdir(parents=True)
        shutil.copy(fixture, tdir / "session.jsonl")

        env = {**os.environ, "HOME": tmp, "CLAUDE_PROJECT_DIR": str(REPO_ROOT)}
        payload = {
            "session_id": f"stop-guard-{fixture_name}",
            "transcript_path": str(tdir / "session.jsonl"),
            "cwd": str(REPO_ROOT),
            "hook_event_name": event_name,
        }
        if event_name == "Stop":
            payload["stop_hook_active"] = False
        else:
            payload["prompt"] = "next question"
        proc = subprocess.run(
            argv, input=json.dumps(payload), capture_output=True, text=True, env=env, timeout=25
        )

    raw = proc.stdout.strip()
    try:
        return (json.loads(raw) if raw else {}), raw
    except ValueError:
        return f"unparseable cupcake output: {raw[:200]!r}"


def run_case(case: Case, argv: list[str]) -> str | None:
    """Returns None on pass, or a failure message."""
    outcome = run_hook(case.fixture, "Stop", argv)
    if isinstance(outcome, str):
        return outcome
    decision, raw = outcome

    reason = decision.get("reason", "")
    blocked = decision.get("decision") == "block"

    if case.expect_halt_text is None:
        if blocked:
            return f"expected NO halt (clean turn) but cupcake blocked: {reason[:160]!r}"
        return None

    if not blocked:
        return (
            f"expected a HALT but cupcake returned {raw or '{}'!r}.\n"
            f"      The guard is INERT -- this is the exact 2026-07-17..2026-08-22 defect. Check that\n"
            f"      no rule in its path uses a builtin Cupcake's WASM runtime cannot execute\n"
            f"      (run: python3 scripts/check-cupcake-wasm-builtins.py)."
        )
    if case.expect_halt_text not in reason:
        return f"halted, but reason lacked {case.expect_halt_text!r}: {reason[:200]!r}"
    return None


def run_context_case(case: ContextCase, argv: list[str]) -> str | None:
    """Returns None on pass, or a failure message.

    Asserts on `hookSpecificOutput.additionalContext` -- the channel Claude Code turns into a
    `hook_additional_context` attachment, which the REPL filters out of the rendered message list.
    That invisibility is the point: the correction has to reach the model without the user reading
    it. A correction that came back as a `reason` instead would be printed to the user, so the shape
    of the output is as load-bearing as its content.
    """
    outcome = run_hook(case.fixture, "UserPromptSubmit", argv)
    if isinstance(outcome, str):
        return outcome
    decision, raw = outcome

    if decision.get("decision") == "block":
        return f"UserPromptSubmit must never block on answer length, but cupcake blocked: {raw[:200]!r}"

    context = decision.get("hookSpecificOutput", {}).get("additionalContext", "")
    if STANDING_RULE not in context:
        return (
            f"the standing one-paragraph rule is missing from additionalContext -- the policy is not\n"
            f"      routed or is inert. Got: {context[:200]!r}"
        )

    if case.expect_context is None:
        if "MEASURED:" in context:
            return f"expected NO correction but one was injected: {context[context.index('MEASURED:'):][:200]!r}"
        return None

    if case.expect_context not in context:
        return f"correction missing {case.expect_context!r} from additionalContext: {context[:400]!r}"
    return None


def main() -> int:
    if shutil.which("cupcake") is None:
        print("test-cupcake-stop-guards: SKIP (cupcake not installed)")
        return 0

    stop_argv = hook_command("Stop")
    ups_argv = hook_command("UserPromptSubmit")
    failures = 0
    for case in CASES:
        err = run_case(case, stop_argv)
        verdict = "halt" if case.expect_halt_text else "allow"
        if err:
            failures += 1
            print(f"FAIL [{verdict}] {case.fixture}: {case.why}\n      {err}", file=sys.stderr)
        else:
            print(f"ok   [{verdict}] {case.fixture}: {case.why}")

    for ctx_case in UPS_CASES:
        err = run_context_case(ctx_case, ups_argv)
        verdict = "correct" if ctx_case.expect_context else "quiet"
        if err:
            failures += 1
            print(f"FAIL [{verdict}] {ctx_case.fixture}: {ctx_case.why}\n      {err}", file=sys.stderr)
        else:
            print(f"ok   [{verdict}] {ctx_case.fixture}: {ctx_case.why}")

    if failures:
        print(f"\ntest-cupcake-stop-guards: {failures} failure(s)", file=sys.stderr)
        return 1
    print(
        f"test-cupcake-stop-guards: OK ({len(CASES)} Stop cases, {len(UPS_CASES)} UserPromptSubmit "
        f"cases, through the real hook commands)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
