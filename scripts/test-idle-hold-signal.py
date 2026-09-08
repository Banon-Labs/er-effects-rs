#!/usr/bin/env python3
"""Behavioral tests for the cupcake signal `last_assistant_idle_hold`.

The signal script scans the last-completed assistant turn of the session transcript and returns a
tagged marker:
  * IDLEHOLD:<phrase>  -- an unjustified idle/hold announcement while a background task runs
  * ""                 -- clean (no hold language, or the hold is justified / accompanied by
                          substantive non-overlapping work / blocked on the user)

We drive it against crafted transcript JSONL under a temporary home so the script's
`~/.claude/projects/<cwd-key>/*.jsonl` discovery resolves to our fixture, then assert the tag.
"""
from __future__ import annotations

import json
import os
import subprocess
import tempfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
SIGNAL = REPO_ROOT / ".cupcake" / "signals" / "last_assistant_idle_hold.sh"

PROJECT_DIR = "/fake/project/er-quickload"


def user(text: str) -> dict:
    return {"type": "user", "message": {"content": text}}


def tool_result() -> dict:
    """A tool-result carrier user event -- must not split the assistant turn."""
    return {"type": "user", "message": {"content": [{"type": "tool_result", "content": "ok"}]}}


def assistant_text(text: str) -> dict:
    return {"type": "assistant", "message": {"content": [{"type": "text", "text": text}]}}


def assistant_bash(command: str) -> dict:
    return {
        "type": "assistant",
        "message": {"content": [{"type": "tool_use", "name": "Bash", "input": {"command": command}}]},
    }


def assistant_edit() -> dict:
    return {
        "type": "assistant",
        "message": {
            "content": [{"type": "tool_use", "name": "Edit", "input": {"file_path": "/x/y.rs"}}]
        },
    }


def assistant_agent() -> dict:
    return {
        "type": "assistant",
        "message": {"content": [{"type": "tool_use", "name": "Agent", "input": {"prompt": "go"}}]},
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


def expect(name: str, events: list[dict], predicate, describe: str) -> None:
    out = run_signal(events)
    if not predicate(out):
        raise AssertionError(f"{name}: {describe} (got {out!r})")


def main() -> int:
    # (1) A bare holding announcement with no work -> IDLEHOLD.
    expect(
        "hold-alone",
        [user("Kick off the RE."), assistant_text("I'm holding for the RE subagent to finish.")],
        lambda o: o.startswith("IDLEHOLD:"),
        "expected IDLEHOLD for a bare hold announcement with no work",
    )

    # (2a) Same hold + a substantive Bash tool_use in the turn -> not flagged.
    expect(
        "hold-plus-substantive-bash",
        [
            user("Kick off the RE."),
            assistant_text("I'm holding for the RE subagent. Meanwhile, checking the disassembly."),
            tool_result(),
            assistant_bash("cargo xwin build --release --target x86_64-pc-windows-msvc"),
        ],
        lambda o: o == "",
        "expected empty when the hold turn also runs substantive work",
    )

    # (2b) Same hold + an Edit tool_use in the turn -> not flagged.
    expect(
        "hold-plus-edit",
        [
            user("Kick off the RE."),
            assistant_text("Standing by for the subagent; prepping the next fix now."),
            tool_result(),
            assistant_edit(),
        ],
        lambda o: o == "",
        "expected empty when the hold turn also edits a file",
    )

    # (2c) Same hold + an Agent tool_use in the turn -> not flagged.
    expect(
        "hold-plus-agent",
        [
            user("Kick off the RE."),
            assistant_text("I'll wait for the build. Launching a parallel investigation."),
            tool_result(),
            assistant_agent(),
        ],
        lambda o: o == "",
        "expected empty when the hold turn also launches a subagent",
    )

    # (3) Same hold + justification prose -> not flagged.
    expect(
        "hold-plus-justification",
        [
            user("Kick off the RE."),
            assistant_text(
                "I'm holding for the RE subagent. I would normally have prepped the next fix "
                "but the fix depends entirely on the subagent's finding, so there is nothing to pull forward."
            ),
        ],
        lambda o: o == "",
        "expected empty when the hold is accompanied by justification prose",
    )

    # (4) A status-peek-only Bash (tail/wc of a log) does not count as substantive -> still flagged.
    expect(
        "hold-plus-status-peek-only",
        [
            user("Kick off the RE."),
            assistant_text("I'm holding for the subagent; just peeking at the log."),
            tool_result(),
            assistant_bash("tail -n 40 target/run.log | wc -l"),
        ],
        lambda o: o.startswith("IDLEHOLD:"),
        "expected IDLEHOLD when the only Bash call is a status/log peek",
    )

    # (5) No holding language at all -> clean.
    expect(
        "no-hold-language",
        [
            user("What is the offset?"),
            assistant_text("The offset is 0x40; I verified it against the disassembly."),
        ],
        lambda o: o == "",
        "expected empty for ordinary technical prose with no hold language",
    )

    # (6) A wait genuinely blocked on the user is legitimate -> not flagged.
    expect(
        "wait-blocked-on-user",
        [
            user("I'll drive the menu."),
            assistant_text("I'll wait for the user to confirm the load finished before I continue."),
        ],
        lambda o: o == "",
        "expected empty when the wait is blocked on the user",
    )

    # (7) A hold phrase only inside a double-quoted span -> stripped -> clean.
    expect(
        "quoted-only-hold",
        [
            user("Explain the ban."),
            assistant_text('The phrase "I\'m holding" is banned unless the turn also does real work.'),
        ],
        lambda o: o == "",
        "expected empty when the hold phrase appears only inside double quotes",
    )

    # (8) Hold in a non-final block of the turn (a later clean block must not mask it) -> detected.
    expect(
        "hold-in-nonfinal-block",
        [
            user("Kick off the RE."),
            assistant_text("I'm holding for the subagent."),
            assistant_text("The subagent will report the function signature soon."),
        ],
        lambda o: o.startswith("IDLEHOLD:"),
        "expected IDLEHOLD from a whole-turn scan when the hold is not the last block",
    )

    # (9) Interrupted turn: a new user prompt after the hold -> the prior turn is still detected.
    expect(
        "interrupted-turn",
        [
            user("Kick off the RE."),
            assistant_text("Standing by for the results."),
            user("Actually, also do Y."),
        ],
        lambda o: o.startswith("IDLEHOLD:"),
        "expected IDLEHOLD on the prior (interrupted) turn",
    )

    # ---- VERBOSEPAUSE (tightened rule 2026-07-17) -------------------------------------------------

    # A long, multi-topic status dump used to end a paused turn (no substantive work, headings +
    # bullets + numbered list + >450 chars, not blocked on the user).
    long_pause_message = (
        "The build subagent is still compiling; here is where things stand while it runs.\n\n"
        "Progress so far:\n"
        "- Resolved the SpEffect call list against SpEffectParam and confirmed all 42 rows.\n"
        "- Rewrote the title-cover oracle to read loadstate the way the game does.\n"
        "- Verified the +0x35 field stalls below 0x0a in the stale dump.\n\n"
        "Next steps once the build lands:\n"
        "1. Re-run the direct offline probe with the freshly built DLL.\n"
        "2. Capture the loading-screen-portrait moment and pixel-diff the input extremes.\n"
        "3. Update the semaphore-progress teardown if the switch is still non-deterministic.\n\n"
        "I expect the compile to finish shortly and will pick this back up then."
    )

    # (10) Pure pause + long message, not blocked on the user -> VERBOSEPAUSE.
    expect(
        "verbose-pure-pause-long",
        [user("Kick off the build."), assistant_text(long_pause_message)],
        lambda o: o.startswith("VERBOSEPAUSE:"),
        "expected VERBOSEPAUSE for a pure pause whose message is long/multi-topic",
    )

    # (11) Pure pause + short, precise blocked note (no idle phrase) -> clean.
    expect(
        "verbose-pure-pause-short",
        [
            user("Kick off the build."),
            assistant_text(
                "Blocked on the release build finishing; nothing non-overlapping remains, "
                "so I'll resume the moment it returns."
            ),
        ],
        lambda o: o == "",
        "expected empty for a short, precise blocked-pause note",
    )

    # (12) long message but the turn also does substantive Edit work -> clean (may report results).
    expect(
        "verbose-long-with-edit",
        [
            user("Kick off the build."),
            assistant_text(long_pause_message),
            tool_result(),
            assistant_edit(),
        ],
        lambda o: o == "",
        "expected empty when a long message accompanies substantive Edit work",
    )

    # (12b) long message but the turn also launches a subagent -> clean.
    expect(
        "verbose-long-with-agent",
        [
            user("Kick off the build."),
            assistant_text(long_pause_message),
            tool_result(),
            assistant_agent(),
        ],
        lambda o: o == "",
        "expected empty when a long message accompanies an Agent launch",
    )

    # (13) long message that is genuinely blocked on the user -> exempt -> clean.
    expect(
        "verbose-long-blocked-on-user",
        [
            user("Let's validate the autoload."),
            assistant_text(
                "I've reached the point where only a live run can settle this, and that run needs "
                "your hands on the launcher.\n\n"
                "Here is exactly what I need you to do:\n"
                "- Start Steam and log in.\n"
                "- Run ~/Elden/launch.sh with the quicksave profile and the default APPDATA save.\n"
                "- Tell me what the loading-screen portrait shows when PRESS ANY BUTTON appears.\n\n"
                "I cannot proceed without that observation because the semaphore only asserts on a "
                "real character load, so I am blocked on you until you can run it."
            ),
        ],
        lambda o: o == "",
        "expected empty for a long message that is genuinely blocked on the user",
    )

    # (14) long message but only a status-peek Bash (tail) -> still a pure pause -> VERBOSEPAUSE.
    expect(
        "verbose-long-with-status-peek",
        [
            user("Kick off the build."),
            assistant_text(long_pause_message),
            tool_result(),
            assistant_bash("tail -n 40 target/build.log"),
        ],
        lambda o: o.startswith("VERBOSEPAUSE:"),
        "expected VERBOSEPAUSE when a long pause turn's only Bash is a status peek",
    )

    # (15) regression -- a long prose answer is not a pause. Nothing is running, the message announces
    # no hold, the user simply asked a question that prose answers. Before 2026-08-22 this fired
    # VERBOSEPAUSE, which halted three consecutive real answers -- twice on the corrective rewrite the
    # wall_of_text guard demanded back when it still halted at Stop, a turn that has no tool_use by
    # construction. Length alone is wall_of_text's jurisdiction, not this rule's.
    # >450 chars on purpose: at 411 the length heuristic would not fire and case (15) would pass
    # trivially, testing nothing. This is a plain answer -- no idle phrasing, no pending-work
    # phrasing, nothing running in the transcript.
    answer_no_pending = (
        "Yes -- intended. The build I launched before you asked for the relaunch already contained "
        "the fade fix, I said so in the handoff and gave you the two Escape presses precisely so you "
        "would try to reproduce it; the relaunch you asked for was the same DLL, byte-identical, so "
        "what you just failed to reproduce is the fixed build behaving as designed rather than a "
        "lucky run that happened to miss the timing window. The oracle table in the commit records "
        "the press landing inside the fade and the hold being refused eight times, which is why the "
        "absence of the symptom counts as proof here rather than as a run that simply missed it."
    )
    expect(
        "verbose-long-prose-answer-nothing-pending",
        [
            user("Tell me before you investigate if you intended for me to notice it was fixed."),
            assistant_text(answer_no_pending),
        ],
        lambda o: o == "",
        "expected empty for a long prose ANSWER with nothing pending -- it is not a pause",
    )

    # (16) control for (15): the identical message becomes a VERBOSEPAUSE the moment something really
    # is running, because then the turn is stopping with work pending and owes a terse blocked-note.
    # The job is launched in an earlier turn -- launching it in this one would itself be substantive
    # work and exempt the turn for a different reason, which would not test the pause gate at all.
    expect(
        "verbose-long-prose-answer-with-live-background-job",
        [
            user("Kick off the build."),
            {
                "type": "assistant",
                "message": {
                    "content": [
                        {
                            "type": "tool_use",
                            "id": "toolu_bg",
                            "name": "Bash",
                            "input": {"command": "cargo build", "run_in_background": True},
                        }
                    ]
                },
            },
            {
                "type": "user",
                "message": {
                    "content": [
                        {
                            "type": "tool_result",
                            "tool_use_id": "toolu_bg",
                            "content": "Command running in background with ID: bx1",
                        }
                    ]
                },
            },
            user("Tell me before you investigate if you intended for me to notice it was fixed."),
            assistant_text(answer_no_pending),
        ],
        lambda o: o.startswith("VERBOSEPAUSE:"),
        "expected VERBOSEPAUSE for the same long message when a background job is still live",
    )

    print("idle-hold signal tests passed (17 cases)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
