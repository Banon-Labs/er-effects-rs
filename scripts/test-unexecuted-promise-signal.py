#!/usr/bin/env python3
"""Behavioral tests for the cupcake signal `last_assistant_unexecuted_promise`.

The signal scans the last-completed assistant turn of the session transcript and returns:
  * PROMISE:<clause>  -- the turn ended on a first-person promise to do concrete work, nothing in the
                         turn executed it, no background task or shell is carrying it, and the message
                         did not hand the obligation to the user.
  * ""                -- clean, which includes every one of those four facts being absent.

The false-positive cost is high (a guard that cries wolf gets ignored), so most of this file is the
negative side: the shapes that look like the defect and must stay silent.

We drive the real signal against crafted transcript JSONL under a temporary home so its
`~/.claude/projects/<cwd-key>/*.jsonl` discovery resolves to our fixture, then assert the tag.

Why `scripts/audit-selftest-vacuity.py` calls this file "ASSERTED", and why that is not a defect.
That tool blinds the `re` module of the process it runs and asks whether the selftest notices. The
subject here is a shell script run as a SUBPROCESS, whose Python is a separate interpreter the
blinding never reaches -- and this file resolves the signal through `Path(__file__).resolve()`, so
the tool's shadow-root subject-stub path cannot reach it either. The verdict is structural, not a
statement about these cases, and it is identical at every commit that has touched this file.
The question it stands in for is answered, and more sharply, by `scripts/prove-gate-positive-controls.py`:
`--only cupcake-signal-tests` stubs the signal silent and always-firing and requires this suite to go
red both ways, and `--only unexecuted-promise-openers` reverts each of the three fixes behind the
2026-09-01 present-continuous failure one at a time and requires the frozen case below to fail.
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
SIGNAL = REPO_ROOT / ".cupcake" / "signals" / "last_assistant_unexecuted_promise.sh"

sys.path.insert(0, str(REPO_ROOT / "scripts"))
from cupcake_turn_scan import RECENT_TURNS  # noqa: E402  (staleness bound, kept in one place)

PROJECT_DIR = "/fake/project/er-quickload"

# The verbatim turn-ending that prompted the guard (user report 2026-08-22).
THE_INSTANCE = "I'll re-record the directive with the shell metacharacters escaped rather than leave it unsaved."

# The verbatim turn-ending that walked through the guard (production failure 2026-09-01): OPENER_RE
# had no bare present continuous, so the opener never matched and the other three facts were never
# evaluated; no `gh pr close` ran and the PR stayed open until the user asked why. Frozen here as the
# regression case -- it is the real failure, not a synthetic one.
THE_PRESENT_CONTINUOUS_INSTANCE = (
    "I'm closing it rather than pushing an empty merge commit to make an empty PR green"
)


def user(text: str) -> dict:
    return {"type": "user", "message": {"content": text}}


def tool_result(tool_use_id: str = "toolu_x", content: str = "ok") -> dict:
    """A tool-result carrier user event -- must not split the assistant turn."""
    return {
        "type": "user",
        "message": {"content": [{"type": "tool_result", "tool_use_id": tool_use_id, "content": content}]},
    }


def assistant_text(text: str) -> dict:
    return {"type": "assistant", "message": {"content": [{"type": "text", "text": text}]}}


def assistant_bash(command: str, tool_use_id: str = "toolu_b", background: bool = False) -> dict:
    inp: dict = {"command": command}
    if background:
        inp["run_in_background"] = True
    return {
        "type": "assistant",
        "message": {"content": [{"type": "tool_use", "id": tool_use_id, "name": "Bash", "input": inp}]},
    }


def assistant_text_then_bash(text: str, command: str, tool_use_id: str = "toolu_b") -> dict:
    """One assistant message that says something and then acts on it in the same message."""
    return {
        "type": "assistant",
        "message": {
            "content": [
                {"type": "text", "text": text},
                {"type": "tool_use", "id": tool_use_id, "name": "Bash", "input": {"command": command}},
            ]
        },
    }


def assistant_edit(tool_use_id: str = "toolu_e") -> dict:
    return {
        "type": "assistant",
        "message": {
            "content": [
                {"type": "tool_use", "id": tool_use_id, "name": "Edit", "input": {"file_path": "/x/y.rs"}}
            ]
        },
    }


def assistant_agent(tool_use_id: str = "toolu_a") -> dict:
    return {
        "type": "assistant",
        "message": {
            "content": [{"type": "tool_use", "id": tool_use_id, "name": "Agent", "input": {"prompt": "go"}}]
        },
    }


def async_agent_result(tool_use_id: str = "toolu_a") -> dict:
    """What the harness returns for a backgrounded subagent launch."""
    return {
        "type": "user",
        "message": {
            "content": [
                {
                    "type": "tool_result",
                    "tool_use_id": tool_use_id,
                    "content": [
                        {
                            "type": "text",
                            "text": "Async agent launched successfully. (This tool result is internal "
                            "metadata.)\nagentId: a1b2c3d4",
                        }
                    ],
                }
            ]
        },
    }


def background_launch_result(tool_use_id: str = "toolu_bg") -> dict:
    """What the harness actually returns the instant a backgrounded Bash starts: an acknowledgement,
    not a result. The real output arrives later as a <task-notification>."""
    return {
        "type": "user",
        "message": {
            "content": [
                {
                    "type": "tool_result",
                    "tool_use_id": tool_use_id,
                    "content": "Command running in background with ID: bd12ab. Output is being written "
                    "to: /tmp/tasks/bd12ab.output. You will be notified when it completes.",
                }
            ]
        },
    }


def task_notification(tool_use_id: str = "toolu_a", status: str = "completed") -> dict:
    """The harness's completion notice for a background task."""
    return {
        "type": "user",
        "message": {
            "content": (
                "<task-notification>\n<task-id>a1b2c3d4</task-id>\n"
                f"<tool-use-id>{tool_use_id}</tool-use-id>\n"
                f"<status>{status}</status>\n<summary>Agent finished</summary>\n</task-notification>"
            )
        },
    }


def run_signal(events: list[dict]) -> str:
    """Write events to a fixture transcript under a temp home and return the signal's stdout."""
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


def fires(out: str) -> bool:
    return out.startswith("PROMISE:")


def silent(out: str) -> bool:
    return out == ""


def main() -> int:
    # ---- The defect ------------------------------------------------------------------------------

    # (1) The reported instance: a closing promise, no tool call, nothing running, no word to the user.
    expect(
        "true-positive-bare-promise",
        [user("Record that directive."), assistant_text(THE_INSTANCE)],
        fires,
        "expected PROMISE for a closing promise with no tool call and nothing running",
    )

    # (2) Work earlier in the turn does not excuse a promise made after it: the promise is still the
    # last thing said and still nothing is going to keep it.
    expect(
        "true-positive-promise-after-earlier-work",
        [
            user("Fix the offset."),
            assistant_text("Patching the constant now."),
            assistant_edit(),
            tool_result("toolu_e"),
            assistant_text("Offset corrected. I'll re-run the gate to confirm nothing else regressed."),
        ],
        fires,
        "expected PROMISE when the closing promise comes after the turn's tool work",
    )

    # (3) Other commitment openers an assistant actually uses.
    expect(
        "true-positive-let-me",
        [user("What is the offset?"), assistant_text("Let me check the offsets in the disassembly.")],
        fires,
        "expected PROMISE for 'let me <action>' with nothing executing it",
    )
    expect(
        "true-positive-going-to",
        [user("The mask is missing."), assistant_text("I'm going to patch the compositor gate.")],
        fires,
        "expected PROMISE for 'I'm going to <action>'",
    )

    # (4) A finished subagent does not cover a new promise: the notification resolved it.
    expect(
        "true-positive-after-agent-completed",
        [
            user("Investigate the crop."),
            assistant_agent(),
            async_agent_result(),
            task_notification(status="completed"),
            assistant_text("The investigator found the crop seed. I'll rewrite the envelope logging."),
        ],
        fires,
        "expected PROMISE once the background subagent has already reported",
    )

    # ---- The four false positives the rule must not commit ---------------------------------------

    # (F1) Contingent on the user doing something first -- not a violation.
    expect(
        "false-positive-contingent-on-user",
        [
            user("I'll run the probe myself."),
            assistant_text("I'll read the numbers once you've run it."),
        ],
        silent,
        "a promise contingent on the user must not fire",
    )

    # (F2) A directive to the user -- not a violation. Doubly excluded: "need" is a hedge, not a
    # commitment, and "need you to" hands the action over.
    expect(
        "false-positive-directive-to-user",
        [
            user("The menu is stuck."),
            assistant_text("I'll need you to press Escape so the dialog closes before anything else."),
        ],
        silent,
        "a directive to the user must not fire",
    )

    # (F2b) The harder shape: a real action promise sitting next to a directive to the user. Only the
    # hand-over clause can explain the silence here -- its control below fires on the same sentence
    # with the directive removed.
    expect(
        "false-positive-action-promise-plus-directive",
        [
            user("The menu is stuck."),
            assistant_text("I'll capture the frame, but I need you to press Escape first so the dialog closes."),
        ],
        silent,
        "an action promise alongside a directive to the user must not fire",
    )

    # (F3) Executed in the same turn -- the tool call follows the sentence.
    expect(
        "false-positive-executed-same-turn-separate-message",
        [
            user("Launch it."),
            assistant_text("I'll launch it now."),
            assistant_bash("python3 scripts/er-run-branch.py --no-fetch"),
        ],
        silent,
        "a promise the turn then executes must not fire",
    )
    expect(
        "false-positive-executed-same-message",
        [
            user("Launch it."),
            assistant_text_then_bash("I'll launch it now.", "python3 scripts/er-run-branch.py --no-fetch"),
        ],
        silent,
        "a promise executed by a tool_use in the same message must not fire",
    )

    # (F4) Covered by live background work: a backgrounded Bash whose result has not come back.
    expect(
        "false-positive-live-background-bash",
        [
            user("Build it."),
            assistant_bash("bash scripts/er-build-dlls.sh --all", "toolu_bg", background=True),
            assistant_text("I'll read the build log the moment it lands."),
        ],
        silent,
        "a promise covered by a live backgrounded Bash must not fire",
    )

    # (F4b) Same, for an async subagent that has not notified yet.
    expect(
        "false-positive-live-async-agent",
        [
            user("Investigate the crop."),
            assistant_agent(),
            async_agent_result(),
            assistant_text("I'll rewrite the envelope logging with whatever it finds."),
        ],
        silent,
        "a promise covered by a still-running subagent must not fire",
    )

    # (F4c) Same, for a shell the turn detached itself.
    expect(
        "false-positive-detached-shell",
        [
            user("Start the watcher."),
            assistant_bash("nohup python3 scripts/er-readiness-watch.py > target/watch.log 2>&1 &"),
            assistant_text("I'll read the watcher output as soon as the semaphore asserts."),
        ],
        silent,
        "a promise covered by a detached shell must not fire",
    )

    # (F5) A promise about a future turn the user must trigger, stated as such.
    expect(
        "false-positive-explicit-reinitiation",
        [
            user("Anything left?"),
            assistant_text(
                "I'll re-run the gate next session -- you'll need to re-initiate it, because the run "
                "needs your machine free and I cannot take it."
            ),
        ],
        silent,
        "an explicit 'you must re-initiate' must not fire",
    )

    # (F6) A user-dependency clause in any shape, including the one the real-transcript audit found:
    # the agent named why it is not acting now, and it is the user's run that blocks it.
    expect(
        "false-positive-not-while-you-are-mid-run",
        [
            user("Work around it for now."),
            assistant_text("I'll fix rather than work around -- but not while you're mid-run."),
        ],
        silent,
        "a 'not while you' dependency must not fire",
    )

    # (F7) "report" is not "re-" + "port". A derived re-form invented a promise nobody made; merged
    # re-forms are enumerated instead. Found by the real-transcript audit, kept pinned here.
    expect(
        "false-positive-report-is-not-re-port",
        [user("Kick off the last stage."), assistant_text("I'll report if that last stage fails.")],
        silent,
        "'report' must not be read as a re-prefixed 'port'",
    )

    # Hyphenated re-forms are the same verb, and must still fire -- that is the reported instance.
    expect(
        "true-positive-hyphenated-re-form",
        [user("Save it."), assistant_text("I'll re-record the directive.")],
        fires,
        "'re-record' must still be read as 'record'",
    )

    # (F8) A background job launched a couple of turns ago and still unresolved still covers a
    # promise -- work does outlive one turn.
    expect(
        "false-positive-background-launched-two-turns-ago",
        [
            user("Build it."),
            assistant_bash("bash scripts/er-build-dlls.sh --all", "toolu_bg", background=True),
            assistant_text("Build started."),
            user("Anything else?"),
            assistant_text("I'll read the build log the moment it lands."),
        ],
        silent,
        "a background job from a recent turn must still cover a promise",
    )

    # (F8b) ...but a launch that never reported and has gone stale must stop covering. A dropped
    # notification would otherwise disable the guard for the rest of the session: measured on a real
    # transcript, one un-notified subagent silenced it across the following 2,800 lines, including the
    # exact turn it exists to catch.
    expect(
        "stale-background-launch-stops-covering",
        [
            user("Build it."),
            assistant_bash("bash scripts/er-build-dlls.sh --all", "toolu_bg", background=True),
            assistant_text("Build started."),
            *[e for i in range(RECENT_TURNS + 1) for e in (user(f"q{i}"), assistant_text(f"a{i}"))],
            user("Anything else?"),
            assistant_text("I'll read the build log the moment it lands."),
        ],
        fires,
        "a background launch that never reported must go stale, not exempt forever",
    )

    # (F9) The real harness shape for a backgrounded Bash: an immediate "running in background"
    # acknowledgement, with the actual result arriving later. That acknowledgement is not a result,
    # and the job must keep covering the promise. (Measured miss: it was read as completion, and a
    # turn with a live background job was flagged.)
    expect(
        "false-positive-background-ack-is-not-a-result",
        [
            user("Build it."),
            assistant_bash("bash scripts/er-build-dlls.sh --all", "toolu_bg", background=True),
            background_launch_result("toolu_bg"),
            assistant_text("I'll read the build log the moment it lands."),
        ],
        silent,
        "a 'running in background' acknowledgement must not count as the job finishing",
    )

    # (F9b) ...and once the notification says it finished, it stops covering.
    expect(
        "control-F9-after-the-background-job-notified",
        [
            user("Build it."),
            assistant_bash("bash scripts/er-build-dlls.sh --all", "toolu_bg", background=True),
            background_launch_result("toolu_bg"),
            task_notification("toolu_bg", status="completed"),
            assistant_text("The build landed. I'll re-run the gate."),
        ],
        fires,
        "a notified background job must stop covering new promises",
    )

    # (5) A live background job the promise never mentions is not cover. The reported instance had a
    # game launch two lines earlier; the promise was to go re-record a directive, which that launch
    # was never going to do. Deferred behind it is not carried by it.
    expect(
        "true-positive-live-job-the-promise-does-not-wait-on",
        [
            user("Launch the run, then save that directive."),
            assistant_bash("python3 scripts/er-run-branch.py --no-fetch", "toolu_bg", background=True),
            background_launch_result("toolu_bg"),
            assistant_text(THE_INSTANCE),
        ],
        fires,
        "an unrelated live job must not excuse an unrelated promise",
    )

    # ---- CONTROLS: each carve-out above must be what silenced it, not luck -----------------------
    # Same sentences with only the exempting element removed. If a control ever goes silent, the
    # matching false-positive test above has stopped proving anything.

    expect(
        "control-F1-without-the-user-contingency",
        [user("I'll run the probe myself."), assistant_text("I'll read the numbers from the probe output.")],
        fires,
        "the contingency, not the verb, must be what silences F1",
    )
    expect(
        "control-F2b-without-the-directive",
        [user("The menu is stuck."), assistant_text("I'll capture the frame first so the dialog is out of the way.")],
        fires,
        "the hand-over clause, not the verb, must be what silences F2b",
    )
    expect(
        "control-F3-without-the-tool-call",
        [user("Launch it."), assistant_text("I'll launch it now.")],
        fires,
        "the tool call, not the wording, must be what silences F3",
    )
    expect(
        "control-F4-without-the-background-flag",
        [
            user("Build it."),
            assistant_bash("bash scripts/er-build-dlls.sh --all", "toolu_bg", background=False),
            assistant_text("I'll read the build log the moment it lands."),
        ],
        fires,
        "the live background job, not the wording, must be what silences F4",
    )
    expect(
        "control-F4b-once-the-background-job-reported",
        [
            user("Build it."),
            assistant_bash("bash scripts/er-build-dlls.sh --all", "toolu_bg", background=True),
            tool_result("toolu_bg", "EXIT=0"),
            assistant_text("I'll read the build log now that it has landed."),
        ],
        fires,
        "a finished background job must stop covering new promises",
    )
    expect(
        "control-F4c-without-the-detach",
        [
            user("Start the watcher."),
            assistant_bash("python3 scripts/er-readiness-watch.py"),
            assistant_text("I'll read the watcher output as soon as the semaphore asserts."),
        ],
        fires,
        "the detached shell, not the wording, must be what silences F4c",
    )
    expect(
        "control-F6-without-the-user-dependency",
        [user("Work around it for now."), assistant_text("I'll fix the drift rather than work around it.")],
        fires,
        "the 'while you' dependency, not the verb, must be what silences F6",
    )
    expect(
        "control-F5-without-the-reinitiation-note",
        [user("Anything left?"), assistant_text("I'll re-run the gate.")],
        fires,
        "the re-initiation note, not the verb, must be what silences F5",
    )

    # ---- NARROWINGS that keep the guard quiet ----------------------------------------------------

    # A question hands the turn back; the user knows the ball is theirs.
    expect(
        "silent-question",
        [user("Rebuild?"), assistant_text("I'll rebuild the DLL -- do you want it launched after?")],
        silent,
        "a closing question must not fire",
    )

    # A stated blocker is the honest version of the hand-back.
    expect(
        "silent-blocker-stated",
        [
            user("Prove it."),
            assistant_text("I'll capture the frame, but I am blocked on Steam being up first."),
        ],
        silent,
        "a stated blocker must not fire",
    )

    # Hedges and negations are not commitments.
    expect(
        "silent-hedge",
        [user("Anything else?"), assistant_text("I'll probably re-run the gate later.")],
        silent,
        "a hedged intention must not fire",
    )
    expect(
        "silent-negation",
        [user("Anything else?"), assistant_text("I'll never run that launch form again.")],
        silent,
        "a negation must not fire",
    )

    # Stance verbs and verbs the message itself fulfils are outside the action allowlist.
    expect(
        "silent-stance-verb",
        [user("Noted?"), assistant_text("I'll keep that constraint in mind for the next hook.")],
        silent,
        "a stance commitment must not fire",
    )
    expect(
        "silent-self-fulfilling-verb",
        [user("Summarise."), assistant_text("I'll summarise: the offset is 0x40 and the gate is green.")],
        silent,
        "a verb the message itself fulfils must not fire",
    )

    # Quoting the ban is not committing it.
    expect(
        "silent-quoted-promise",
        [
            user("Explain the guard."),
            assistant_text('The guard fires on "I\'ll re-run the gate" when no tool call follows.'),
        ],
        silent,
        "a double-quoted promise must not fire",
    )
    expect(
        "silent-backticked-promise",
        [
            user("Explain the guard."),
            assistant_text("The guard fires on `I'll re-run the gate` when no tool call follows."),
        ],
        silent,
        "a backticked promise must not fire",
    )

    # Only the closing prose is scanned: a mid-turn promise whose turn moved on is the normal shape.
    expect(
        "silent-promise-not-in-final-block",
        [
            user("What is the offset?"),
            assistant_text("I'll check the offsets."),
            assistant_text("The offset is 0x40, verified against the disassembly."),
        ],
        silent,
        "a promise superseded by a later closing block must not fire",
    )

    # Ordinary reporting prose with no commitment at all.
    expect(
        "silent-no-commitment",
        [
            user("What is the offset?"),
            assistant_text("The offset is 0x40; the gate is green and the branch is pushed."),
        ],
        silent,
        "prose with no commitment must not fire",
    )

    # A turn with no assistant text at all (nothing to judge).
    expect(
        "silent-no-assistant-text",
        [user("Run it."), assistant_bash("bash scripts/check.sh")],
        silent,
        "a turn with no closing prose must not fire",
    )

    # ---- Bare present continuous ("I'm closing it") ----------------------------------------------
    # The hole this guard shipped with, found by it failing in production 2026-09-01. OPENER_RE knew
    # "I'll" / "I'm going to" / "let me" and not the bare present continuous, so the turn below ended
    # on a commitment, nothing ran it, and the other three facts were never evaluated. Present
    # continuous is the more seductive way to make an unkept promise than "I'll", because it reads to
    # a human as already underway.

    # The instance, verbatim. Frozen: this exact sentence must halt.
    expect(
        "true-positive-the-present-continuous-instance",
        [
            user("What happened to the PR?"),
            assistant_text(THE_PRESENT_CONTINUOUS_INSTANCE),
        ],
        fires,
        "expected PROMISE for the verbatim 'I'm closing it ...' turn-ending",
    )

    # It took two fixes, and each must be load-bearing on its own. Controls for both live in
    # scripts/prove-gate-positive-controls.py (sens/opener-lacks-present-continuous and
    # sens/close-not-an-action), which revert one at a time and require this case to go green again.
    # Here: the same sentence with a verb that was already on the allowlist isolates the opener, and
    # an "I'll close it" isolates the actions entry.
    expect(
        "true-positive-present-continuous-already-listed-verb",
        [user("What about the branch?"), assistant_text("I'm deleting the stale branch.")],
        fires,
        "the opener alone must carry a verb that was always on the allowlist",
    )
    expect(
        "true-positive-close-with-the-old-opener",
        [user("What happened to the PR?"), assistant_text("I'll close it rather than push an empty merge commit.")],
        fires,
        "'close' must be a concrete action under the openers that already worked",
    )

    # The three inflections a gerund can take. Resolved against the same actions list, no second
    # vocabulary: dropped-e, doubled consonant, and the plain case.
    expect(
        "true-positive-gerund-dropped-e",
        [user("Where is the DLL?"), assistant_text("I'm staging the DLL into the profile.")],
        fires,
        "'staging' must resolve to 'stage'",
    )
    expect(
        "true-positive-gerund-doubled-consonant",
        [user("Is it saved?"), assistant_text("I'm committing the fix.")],
        fires,
        "'committing' must resolve to 'commit'",
    )
    expect(
        "true-positive-gerund-plain",
        [user("And the branch?"), assistant_text("I'm landing it on main.")],
        fires,
        "'landing' must resolve to 'land'",
    )
    expect(
        "true-positive-gerund-hyphenated-re-form",
        [user("Save it."), assistant_text("I'm re-recording the directive.")],
        fires,
        "'re-recording' must resolve to 're-record'",
    )
    expect(
        "true-positive-i-am-spelled-out",
        [user("And the constant?"), assistant_text("I am reverting the offset change.")],
        fires,
        "'I am <gerund>' must fire like \"I'm <gerund>\"",
    )

    # Ordering regression. The bare opener is last in the alternation on purpose: Python takes the
    # first alternative that matches at a position, not the longest, so a bare "i'm" placed ahead of
    # "i'm going to" would swallow its prefix and hand committed_verb the word "going" -- which is
    # deliberately not an action, silently killing the case above it.
    expect(
        "true-positive-going-to-still-fires-after-bare-opener-added",
        [user("The mask is missing."), assistant_text("I'm going to patch the compositor gate.")],
        fires,
        "'I'm going to <action>' must survive the bare present-continuous alternative",
    )
    expect(
        "true-positive-about-to-still-fires-after-bare-opener-added",
        [user("Ready?"), assistant_text("I'm about to run the gate.")],
        fires,
        "'I'm about to <action>' must survive the bare present-continuous alternative",
    )

    # ---- ...AND the STATIVE uses that must stay silent -------------------------------------------
    # Present continuous is ambiguous in a way "I'll" is not, and this repo's prose is full of the
    # ambiguous half. Every one of these is silenced by the pipeline that was already there: the verb
    # simply is not on the concrete-action allowlist, or it is a hedge or a negation.

    # Declining to act is the opposite of an unkept promise, and it is this repo's own reporting
    # convention ("report ... rather than fix"). If this ever fires the guard is unusable.
    expect(
        "false-positive-declining-to-act",
        [
            user("Fix the drift."),
            assistant_text("I'm reporting it rather than fixing it, because the fix needs a runtime run."),
        ],
        silent,
        "declining to act must not read as promising to act",
    )
    expect(
        "false-positive-stative-treating",
        [user("Is that proven?"), assistant_text("I'm treating that as unproven until the pixel diff lands.")],
        silent,
        "a stance verb in the present continuous must not fire",
    )
    expect(
        "false-positive-not-sure",
        [user("Which is it?"), assistant_text("I'm not certain which of the two producers is firing.")],
        silent,
        "a negated present continuous must not fire",
    )
    expect(
        "false-positive-about-the-message-itself",
        [user("What failed?"), assistant_text("I'm quoting the failure below, unmodified.")],
        silent,
        "present continuous about the message itself must not fire",
    )
    expect(
        "false-positive-present-continuous-hedge",
        [user("Next?"), assistant_text("I'm thinking the offset moved, but nothing proves it yet.")],
        silent,
        "a hedge in the present continuous must not fire",
    )
    expect(
        "false-positive-present-continuous-negation-verb",
        [user("Should I keep it?"), assistant_text("I'm leaving the branch exactly as it is.")],
        silent,
        "a negation-family verb in the present continuous must not fire",
    )

    # Narration of work the turn then does. This is the correct shape and the commonest one -- the
    # existing tool_after check must cover the new opener exactly as it covers "I'll".
    expect(
        "false-positive-present-continuous-then-executed",
        [
            user("Close the PR."),
            assistant_text_then_bash(THE_PRESENT_CONTINUOUS_INSTANCE, "gh pr close 371"),
        ],
        silent,
        "a present-continuous narration the turn then executes must not fire",
    )
    expect(
        "false-positive-present-continuous-executed-next-message",
        [
            user("Close the PR."),
            assistant_text(THE_PRESENT_CONTINUOUS_INSTANCE),
            assistant_bash("gh pr close 371"),
        ],
        silent,
        "a present-continuous narration executed by a later block must not fire",
    )

    # Ongoing background work described in the present continuous. Doubly covered, and the control
    # below separates the two: "watch" is not an action verb, and a live job the promise waits on
    # covers it anyway.
    expect(
        "false-positive-present-continuous-watching-live-work",
        [
            user("Build it."),
            assistant_bash("bash scripts/er-build-dlls.sh --all", "toolu_bg", background=True),
            background_launch_result("toolu_bg"),
            assistant_text("I'm watching the build run."),
        ],
        silent,
        "describing live background work must not fire",
    )
    expect(
        "false-positive-present-continuous-waiting-on-live-work",
        [
            user("Build it."),
            assistant_bash("bash scripts/er-build-dlls.sh --all", "toolu_bg", background=True),
            background_launch_result("toolu_bg"),
            assistant_text("I'm reading the build log the moment it lands."),
        ],
        silent,
        "a present-continuous promise waiting on live background work must not fire",
    )
    expect(
        "control-present-continuous-watching-without-live-work",
        [user("What now?"), assistant_text("I'm reading the build log.")],
        fires,
        "the allowlist and the live job, not the tense, must be what silences the two above",
    )

    # The halt quotes a clause back at the agent, so it must be the promise nearest where the turn
    # stopped. Broadening the opener set makes an incidental early match likelier -- measured on a
    # real halted turn, where the quote moved from the closing sentence back to a table cell.
    expect(
        "quotes-the-last-promise-not-the-first",
        [
            user("What now?"),
            assistant_text(
                "why I'm not guessing | I'm re-running it with the crash logger loaded.\n\n"
                "I'll revert the marker spawn to the wrapper rather than ship a coin-flip."
            ),
        ],
        lambda out: out.startswith("PROMISE:") and "revert" in out,
        "the halt must quote the promise nearest the end of the turn",
    )

    # A present-continuous commitment handed to the user is still a hand-off, not a broken promise.
    expect(
        "false-positive-present-continuous-handed-to-user",
        [
            user("Close it."),
            assistant_text("I'm closing it once you confirm the branch is not needed."),
        ],
        silent,
        "a present-continuous commitment contingent on the user must not fire",
    )

    if FAILURES:
        print(f"unexecuted-promise signal: {len(FAILURES)} FAILED: {', '.join(FAILURES)}")
        return 1
    print("unexecuted-promise signal tests passed (64 cases)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
