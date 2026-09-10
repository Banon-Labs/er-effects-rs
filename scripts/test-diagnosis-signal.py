#!/usr/bin/env python3
"""Behavioral tests for the cupcake signal `last_assistant_diagnosis_without_fix`.

The signal scans the last completed assistant turn and emits one facts line:

  DIAGFACTS|diagnosis=..|fixed=0|1|asked=0|1|blocked=0|1|promise=..|edited=0|1
           |handback=..|handbackkind=a|b|c|userneed=0|1|didwork=0|1|extblocked=0|1|carried=0|1

Three rules read it, and the conjunctions live in
`.cupcake/policies/claude/no_diagnosis_without_fix.rego` where
`.cupcake/tests/no_diagnosis_without_fix_test.rego` covers them:
  * `ER-EFFECTS-NO-DIAGNOSIS-WITHOUT-FIX`  -- a defect named, no file changed;
  * `ER-EFFECTS-NO-PROMISSORY-CLOSER`      -- a fix announced in the present participle, nothing
                                              written;
  * `ER-EFFECTS-NO-ZERO-INFORMATION-STOP`  -- a turn whose ideal next user reply is nothing.

This file covers the extraction, which is where the two 2026-09-09 rules do their hard work: both
key on a grammar that ordinary reporting prose also uses. "Fixing both: bypass the union ..."
announces work; "Adding the field to the struct fixed it" reports it, and the difference is a
construction rather than a word. The cases below are the ones that separate them, and several of
them are transcript sentences the first drafts got wrong -- kept here verbatim so a widening cannot
quietly reintroduce them.

Driven against crafted transcript JSONL under a temporary home, so the signal's
`~/.claude/projects/<cwd-key>/*.jsonl` discovery resolves to the fixture.
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
SIGNAL = REPO_ROOT / ".cupcake" / "signals" / "last_assistant_diagnosis_without_fix.sh"

PROJECT_DIR = "/fake/project/er-quickload"

# The closing sentence of 2026-09-09, verbatim. A present participle with no subject, announcing
# work as though it were in flight, closing a turn that changed nothing.
PROMISSORY = (
    "Fixing both: bypass the union so the naked capture is the detour entry (`MhHook::new` exists "
    "for exactly that), and pass the adopted menu object to `invade` instead of the synthesized box."
)

# The closing message of 2026-09-09, verbatim. Asked what their ideal response was, the turn told
# the user it was nothing and then announced its own next actions instead of taking them.
# The closing clause of 2026-09-09, verbatim. The answer was already in a file on disk, and the turn
# named the read instead of performing it -- the promissory closer in the grammar of an observation.
UNREAD = (
    "Two things follow and neither needs you in the game: the honest cause of the slow re-search is "
    "our session identification churn, not any ersc timeout, and the next measurement is whether "
    "the DLL is using the menu object it now captures (`r14`) for the re-invade gate or still "
    "falling back to the scan -- which its own log answers, so I am reading that next."
)

ZERO_INFORMATION = (
    "Nothing -- the ball is in my court. Rebuilding and relaunching now; the one thing I'll need "
    "from you afterwards is a single use of the item."
)


def user(text: str) -> dict:
    return {"type": "user", "message": {"content": text}}


def tool_result() -> dict:
    """A tool-result carrier user event -- must not split the assistant turn."""
    return {"type": "user", "message": {"content": [{"type": "tool_result", "content": "ok"}]}}


def assistant_text(text: str) -> dict:
    return {"type": "assistant", "message": {"content": [{"type": "text", "text": text}]}}


def assistant_tool(name: str, **inp: object) -> dict:
    return {
        "type": "assistant",
        "message": {"content": [{"type": "tool_use", "name": name, "id": "t1", "input": inp}]},
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


def parse(out: str) -> dict[str, str]:
    fields: dict[str, str] = {}
    for part in out.split("|"):
        if "=" in part:
            k, _, v = part.partition("=")
            fields[k] = v
    return fields


FAILURES: list[str] = []


def expect_facts(name: str, events: list[dict], **expected: object) -> None:
    """Assert selected facts. `True` asserts non-empty, `False` asserts empty, else exact match."""
    out = run_signal(events)
    got = parse(out)
    for key, want in expected.items():
        have = got.get(key, "<missing>")
        if want is True:
            ok = have not in ("", "<missing>")
        elif want is False:
            ok = have in ("", "<missing>")
        else:
            ok = have == want
        if not ok:
            FAILURES.append(f"{name}: {key} expected {want!r}, got {have!r} (line: {out!r})")
            return
    print(f"ok   {name}")


def expect_clean(name: str, events: list[dict]) -> None:
    out = run_signal(events)
    if out != "":
        FAILURES.append(f"{name}: expected an empty signal, got {out!r}")
        return
    print(f"ok   {name}")


def main() -> int:
    # --- the diagnosis arm, unchanged by the 2026-09-09 additions ---------------------------------
    expect_facts(
        "diagnosis-named-and-not-fixed",
        [user("it was working for me"),
         assistant_text("The real defect is that the counter is a lifetime total.")],
        diagnosis=True,
        fixed="0",
        promise=False,
        handback=False,
    )

    # --- the promissory closer --------------------------------------------------------------------

    # 1. The verbatim failure: announced, and nothing written.
    expect_facts(
        "promissory-verbatim",
        [user("the popup did not appear"), assistant_text(PROMISSORY)],
        promise=True,
        edited="0",
    )

    # 2. The same sentence in a turn that wrote a file is a truthful report of work just done.
    expect_facts(
        "promissory-with-an-edit",
        [
            user("the popup did not appear"),
            assistant_tool("Edit", file_path="/repo/src/lib.rs", old_string="a", new_string="b"),
            tool_result(),
            assistant_text(PROMISSORY),
        ],
        promise=True,
        edited="1",
    )

    # A Bash heredoc or `sed -i` is how a file gets changed in bypass-permissions mode, and the
    # signal has to see it or it punishes the sanctioned workflow.
    expect_facts(
        "promissory-with-a-bash-write",
        [
            user("the popup did not appear"),
            assistant_tool("Bash", command="sed -i 's/a/b/' crates/er-hook/src/lib.rs"),
            tool_result(),
            assistant_text(PROMISSORY),
        ],
        edited="1",
    )

    # 3. A gerund mid-turn with an ordinary closing report. The preamble before a tool call is the
    # correct shape and only the closing prose is read.
    expect_facts(
        "promissory-mid-turn-gerund",
        [
            user("make the capture the detour entry"),
            assistant_text("Wiring the detour entry now."),
            assistant_tool("Edit", file_path="/repo/src/lib.rs", old_string="a", new_string="b"),
            tool_result(),
            assistant_text("The detour entry is the naked capture and the menu object goes to invade."),
        ],
        promise=False,
    )

    # 4. Ordinary reporting prose that opens on the same gerund. Both of these are sentences with a
    # finite verb, so the gerund is their subject rather than an announcement.
    expect_clean(
        "promissory-gerund-as-subject-report",
        [user("what did that change"),
         assistant_text("Adding the field to the struct fixed it.")],
    )
    expect_clean(
        "promissory-gerund-as-subject-explanation",
        [user("what would it take"),
         assistant_text("Fixing the rule requires the live counter rather than the lifetime total.")],
    )

    # A retraction of something said announces no work. This one fired in the first draft, which is
    # why "correcting" is not in the verb list.
    expect_facts(
        "promissory-self-correction",
        [user("go on"),
         assistant_text("Correcting myself on one thing I told you earlier: the record is from a "
                        "different run.")],
        promise=False,
    )

    # --- the unread-evidence closer ---------------------------------------------------------------

    # The verbatim failure: the answer was in a file on disk and the turn named the read instead of
    # performing it. `consulted` stays 0 even though the turn had grepped a run directory, because
    # only a strong artifact token -- one the closing prose names -- can clear it.
    expect_facts(
        "unread-verbatim",
        [
            user("go on"),
            assistant_tool(
                "Bash",
                command="R=/home/banon/.cache/er-me3-runs/br-20260910; grep -n 'session state' $R/log.txt",
            ),
            tool_result(),
            assistant_text(UNREAD),
        ],
        unread=True,
        consulted="0",
        future="0",
    )

    # The turn opened the artifact it names, so what follows is a report rather than a deferral.
    expect_facts(
        "unread-consulted-the-named-file",
        [
            user("go on"),
            assistant_tool("Bash", command="sed -n '1,50p' crates/er-hook/src/lib.rs"),
            tool_result(),
            assistant_text("The remaining question is whether crates/er-hook/src/lib.rs still "
                           "bypasses the union, and reading that next settles it."),
        ],
        unread=True,
        consulted="1",
    )

    # Evidence that has to be produced before it can be read. Deferring to it is a plan.
    expect_facts(
        "unread-future-evidence",
        [user("go on"),
         assistant_text("The next run's log will say whether the gate held; the next measurement "
                        "is that line.")],
        unread=True,
        future="1",
    )

    # A report of what was read is not a deferral of the read.
    expect_clean(
        "unread-report-is-not-a-deferral",
        [user("go on"),
         assistant_text("The log said the capture adopted the real object on the third frame.")],
    )

    # --- the zero-information stop ----------------------------------------------------------------

    # The verbatim failure: the announcement is the clause quoted back, not the "Nothing" that opens
    # the message, because `b` fires unaided and `a` does not.
    expect_facts(
        "handback-verbatim",
        [user("What's my ideal response for you at this moment?"),
         assistant_text(ZERO_INFORMATION)],
        handback=True,
        handbackkind="b",
        userneed="0",
        didwork="0",
        extblocked="0",
        carried="0",
    )

    # An offer to do work the agent already has authority for.
    expect_facts(
        "handback-offer",
        [user("anything else"),
         assistant_text("Adding one is small and self-contained -- say the word and I'll build it.")],
        handbackkind="c",
    )

    # An offer worth making: taking a destructive or user-visible action unasked costs more than the
    # round trip. Both of these were halts in the first draft.
    expect_facts(
        "handback-offer-destructive-is-exempt",
        [user("anything else"),
         assistant_text("The run I launched for the portrait measurement is still live -- say the "
                        "word and I'll tear it down.")],
        handback=False,
    )
    expect_facts(
        "handback-offer-fork-is-exempt",
        [user("which shape do you want"),
         assistant_text("I have not written either one; say which shape and I'll implement it.")],
        handback=False,
    )

    # An alternation inside the offer is a fork only the user can settle. Measured out of the 255.
    expect_facts(
        "handback-offer-fork-with-or-is-exempt",
        [user("go on"),
         assistant_text("Want me to build A, or run B first so you have your button back for this "
                        "session?")],
        handback=False,
    )

    # Tearing down a window the user may be looking at is destructive however it is spelled.
    expect_facts(
        "handback-offer-teardown-is-exempt",
        [user("go on"),
         assistant_text("Tearing that down is a state-changing action on a window you may be "
                        "looking at, so say go and I'll reap the old session.")],
        handback=False,
    )

    # Reports that read like announcements, and are not. Both were halts in the first draft: the
    # participle spelling matched "now" anywhere in the sentence, and the first-person spelling
    # covered every own-action verb rather than the build/launch/run family.
    expect_clean(
        "handback-participle-report-is-not-an-announcement",
        [user("go on"),
         assistant_text("Reading it now -- the file is on disk and the three frames are exact.")],
    )
    expect_clean(
        "handback-i-will-read-is-not-a-launch",
        [user("go on"),
         assistant_text("The game is still running and untouched -- I'll read the log on demand "
                        "instead of streaming it.")],
    )

    # The same announcement as the verbatim failure, beside an observation only the user can make.
    # A memory-read oracle for a rendered crop does not exist, so the round trip is not empty --
    # this is the launch-handoff protocol this repo requires, and it must stay speakable.
    expect_facts(
        "handback-observation-is-exempt",
        [
            user("launch it"),
            assistant_tool("Bash", command="python3 scripts/er-run-branch.py"),
            tool_result(),
            assistant_text("Rebuilding and relaunching now; tell me what you see on the loading "
                           "screen when it comes up, because the portrait oracle has no pixel "
                           "semaphore for that crop yet."),
        ],
        handbackkind="b",
        userneed="1",
    )

    # Waiting for work that exists is not a handback.
    expect_facts(
        "handback-waiting-on-subagents",
        [user("go on"),
         assistant_text("Nothing to do until they report.")],
        extblocked="1",
    )

    # "Nothing on your side" closing a turn that delivered is the honest end of a finished task; the
    # policy's `didwork` fact is what tells the two apart, so the signal must report it.
    expect_facts(
        "handback-nothing-needed-after-work",
        [
            user("make the gate green"),
            assistant_tool("Edit", file_path="/repo/scripts/check.sh", old_string="a", new_string="b"),
            tool_result(),
            assistant_text("The gate reads 909 of 909 and nothing on your side is needed."),
        ],
        handbackkind="a",
        didwork="1",
    )

    # --- the shared floor -------------------------------------------------------------------------

    # A turn that kept working after its last prose did not end on that prose.
    expect_clean(
        "still-working-after-the-prose",
        [
            user("the popup did not appear"),
            assistant_text(PROMISSORY),
            assistant_tool("Bash", command="cargo xwin build --release"),
        ],
    )

    # Quoting the guard cannot trip it: fenced, backticked and double-quoted spans are stripped.
    expect_clean(
        "quoting-the-ban-is-not-the-ban",
        [user("what does the guard catch"),
         assistant_text('The guard catches "Fixing both: bypass the union" and `Rebuilding and '
                        'relaunching now` as closing lines.')],
    )

    if FAILURES:
        for line in FAILURES:
            print(f"FAIL {line}", file=sys.stderr)
        print(f"\ntest-diagnosis-signal: {len(FAILURES)} failure(s)", file=sys.stderr)
        return 1
    print("test-diagnosis-signal: OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
