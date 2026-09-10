#!/usr/bin/env bash
# Cupcake signal: last_assistant_future_commitment
#
# Scans the most recently completed assistant turn and emits one facts line when that turn ended on
# work that had not happened:
#
#   FUTUREFACTS|commit=..|actionclass=..|timemarker=..|acted=..|blocked=..|planasked=..|deferred=..
#            |conditional=..|delegation=..|reported=..|instructed=..
#
# A clean turn emits nothing (fail-open, like every neighbouring signal).
#
# Two shapes, one transcript walk
# -------------------------------
# `commit` is a first-person future-tense promise closing the turn. Verbatim, 2026-09-09:
#
#     Next run I'll build this in and re-attach Frida to confirm zero `DISCARDING` lines across a
#     full invade-reject-reinvade cycle.
#
# The build command and the launch script were both available in that turn, so the sentence bought a
# round trip carrying no information. The user's reply was "'Next run I'll' SOUND FAMILIAR?", which
# is a repeat offence, not a first one.
#
# `delegation` is the same defect with the subject swapped from the agent to a delegate. Verbatim,
# from the turn that dispatched the agent which wrote this file:
#
#     It has the verbatim sentence, the exemptions that must not trip (a real blocker, the user
#     owning the observation, work already started in-turn), and the same evidence bar the others
#     got: measured false-positive rate over real transcripts, all four repo gates, and a
#     `cupcake eval` returning `decision:block` on that exact line rather than a passing unit test.
#
# The `Agent` tool's own result says the caller knows nothing about a delegate's results until the
# completion notification arrives. Listing what a just-dispatched agent contains converts a dispatch
# into a claim of delivery, and it reads to the user as work already banked.
#
# Why the three neighbouring Stop guards do not catch the first shape. This was measured, not
# assumed: the real turn was replayed through each signal on 2026-09-09 and all three stayed silent.
#   * `last_assistant_unexecuted_promise` finds the promise and then exempts it. Its `HANDOFF_RE`
#     reads "next session/turn/time" as the ball being handed to the user, and its live-background
#     arm exempts any promise sitting near a running job -- the game was up, so the promise to build
#     and re-attach was covered by a session it did not depend on. A deadline disarmed the guard
#     that exists to refuse deadlines.
#   * `last_assistant_described_next_step` needs an impersonal prescription ("the next step is");
#     this sentence commits the agent by name.
#   * `last_assistant_diagnosis_without_fix` did produce a handback fact for it (`handbackkind=b`)
#     and exempted on the same `carried=1`.
# So the fact that convicts here is narrower and is not a copy of any of theirs: whether the turn
# performed a tool call of the class the promise named.
#
# The exemptions, each a shape that is allowed to end on future work
#   acted     -- the turn already did work of the promised class. An `Edit` does not keep a promise
#               to rebuild and a build does not keep a promise to relaunch, so the class has to
#               match; every branch of that test is deliberately generous.
#   blocked   -- a dependency outside the agent's reach: an observation only the user can make, a
#               credential, a login, a purchase, a decision that is theirs, or an explicit
#               instruction to stop. Deliberately not the broad "blocked on the user" fact the
#               siblings fold in, because "I'll do it next run" is not a dependency on anyone.
#   planasked -- the user asked what the plan is. Then stating one is the answer. Narrow on purpose:
#               a bare question mark cannot exempt, or nothing in this repo would ever halt.
#   deferred  -- the work waits on something that does not exist yet, and this turn started that
#               thing. Both halves are required; a promise deferred behind a run somebody else
#               started is the exact hole that let the instance above through.
#   conditional -- the promise hangs on something that has not happened: a decision the user has not
#               made ("say the word and I'll build it"), or an event that may never occur ("I'll
#               relaunch them that way if either trips over the other"). The first belongs to
#               `ER-EFFECTS-NO-ZERO-INFORMATION-STOP`, which convicts an offer to do authorised work
#               as a handback; the second is a contingency that is not yet due. Charging either here
#               would be the wrong charge for the right sentence.
#   reported  -- the delegate's completion notification is in the transcript, so the message relays
#               real results instead of predicting them.
#   instructed -- the delegate sentence describes what was asked of the agent rather than what it
#               produced. That is a fact about the caller's own tool call, which the caller does
#               know.
#
# Fenced code, backtick spans and double-quoted spans are stripped before matching, so quoting these
# sentences -- this file, the policy, a report about the guard -- cannot trip it. Only the last two
# sentences of the closing prose are read for the promise, which is what keeps ordinary work
# untouched: a mid-turn "I'll check the offsets" followed by the check is the correct shape and is
# never seen. The shared half (transcript discovery, turn bucketing) comes from
# `scripts/cupcake_turn_scan.py` and the classification from `scripts/cupcake_future_commitment.py`,
# so the guards cannot drift into disagreeing about the same turn.
set -uo pipefail
CUPCAKE_SIGNAL_REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]:-$0}")/../.." && pwd)"
export CUPCAKE_SIGNAL_REPO_ROOT
python3 - <<'PY' 2>/dev/null || true
import os, sys

sys.path.insert(0, os.path.join(os.environ.get("CUPCAKE_SIGNAL_REPO_ROOT", "."), "scripts"))
try:
    import cupcake_turn_scan as scan
    import cupcake_future_commitment as fc
except Exception:
    sys.exit(0)  # fail open: a missing helper must never wedge a session

path = scan.latest_transcript()
if not path:
    sys.exit(0)
events = scan.load_events(path)
turns = scan.split_turns(events)
turn = scan.last_text_turn(turns)
if turn is None:
    sys.exit(0)

# A turn that kept going past its last prose did not end on that prose.
if turn.last_text_index < 0 or turn.tool_after(turn.last_text_index):
    sys.exit(0)

runs = turn.text_runs
if not runs:
    sys.exit(0)
closing = runs[-1]
tool_blocks = [block for kind, block in turn.blocks if kind == "tool"]

promise = fc.promised_action(closing)
commit, action_class, time_marker, conditional = promise if promise else ("", "", False, False)

acted = bool(commit) and fc.action_taken(action_class, tool_blocks)
blocked = bool(fc.EXTERNAL_BLOCKER_RE.search(fc.strip_quoted(closing)))
try:
    live_background = bool(scan.live_background_work(events))
except Exception:
    live_background = False
# The promise sentence, not the whole message: see `deferred_to_started_work`.
deferred = bool(commit) and fc.deferred_to_started_work(commit, tool_blocks, live_background)

# `split_turns` keeps blocks, not the prompt that opened the turn, so the prompt is recovered the
# way the neighbouring signals recover it: the last real user prompt in the transcript.
def prompt_text(ev):
    content = ev.get("message", {}).get("content")
    if isinstance(content, str):
        return content
    parts = []
    for block in content or []:
        if isinstance(block, dict) and block.get("type") == "text":
            parts.append(block.get("text") or "")
        elif isinstance(block, str):
            parts.append(block)
    return "\n".join(parts)


prompt = ""
try:
    for ev in events:
        if scan.is_real_user_prompt(ev):
            prompt = prompt_text(ev)
except Exception:
    prompt = ""
planasked = fc.plan_requested(prompt)

delegation = ""
reported = True
instructed = False
if fc.unreported_dispatch(tool_blocks, events):
    delegation = fc.delegation_claim(closing)
    reported = False
    # Only when no claim survived: a message that describes its instruction and then asserts the
    # output still convicts on the assertion. See `delegation_instruction_framed`.
    instructed = not delegation and fc.delegation_instruction_framed(closing)

if not commit and not delegation:
    sys.exit(0)

print(
    "FUTUREFACTS|commit={}|actionclass={}|timemarker={}|acted={}|blocked={}|planasked={}"
    "|deferred={}|conditional={}|delegation={}|reported={}|instructed={}".format(
        commit,
        action_class,
        int(time_marker),
        int(acted),
        int(blocked),
        int(planasked),
        int(deferred),
        int(conditional),
        delegation,
        int(reported),
        int(instructed),
    )
)
PY
