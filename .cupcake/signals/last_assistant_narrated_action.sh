#!/usr/bin/env bash
# Cupcake signal: last_assistant_narrated_action
#
# Scans the most recently completed assistant turn and emits one facts line when that turn ended by
# narrating the action it was taking instead of by having taken it:
#
#   NARRATIONFACTS|narration=..|actionclass=..|shape=..|reported=..|banner=..|blocked=..
#
# A clean turn emits nothing (fail-open, like every neighbouring signal).
#
# The failure this exists to refuse
# ---------------------------------
# Five turn-final sentences from one session, 2026-09-10, each with the action either already
# dispatched or dispatchable in the same turn:
#
#   Re-running it now without the cap.
#   Rebuilding and relaunching now.
#   Bringing it up to read the pointer chain live rather than guessing another offset:
#   Making the empty read diagnosable - logging each of the three hops so the next invasion says
#     which one returned zero instead of just that one did:
#   Dispatching a subagent to enumerate the menu builder's rows properly, and unblocking you now by
#     narrowing the guard:
#
# The user's words when they called it out:
#
#   "'Re-running it now without the cap' me: *points to the rego policy you `MUST` update to
#    disallow this prose without a stop hook*"
#
# They had named the general form twice already in the same session, and `AGENTS.md` names it in
# the stop-too-early list: "Announcing your own next action instead of taking it".
#
# Why the neighbouring Stop guards do not catch it
#   * `last_assistant_future_commitment` needs a first-person future opener ("next run I'll ...",
#     "I'm going to ..."). A bare participial clause commits nobody and names no future.
#   * `last_assistant_diagnosis_without_fix` carries the promissory closer, but its gerunds are the
#     code-change family (fixing, wiring, patching, ...) and its zero-information arm reaches only
#     the build/launch/run family end-anchored on "now". Four of the five sentences above use a verb
#     neither list holds, and the fifth is the zero-information arm's own verbatim instance wearing
#     a semicolon clause, which this signal deliberately leaves to it.
#   * `last_assistant_described_next_step` needs an impersonal prescription ("the next step is").
#     The present participle points at now, not next.
#
# The exemptions
#   reported -- the narration carries, or is followed by, something measured: a number with a unit,
#               an exit code, a hash, an address, a path, a file name, or the fenced block a
#               command's output lands in. Then the sentence reports rather than announces.
#   banner   -- the loud launch or teardown banner `AGENTS.md` mandates immediately before a game
#               launch. It is a required form, and a rule that made it unspeakable would take a
#               safety announcement away from the user to save a round trip.
#   blocked  -- a dependency the agent cannot dissolve by working harder: an observation only the
#               user can make, a credential, sudo, a login, a purchase, a decision that is theirs,
#               a guard that refused the write, an instruction to stop.
#
# Two structural narrowings keep it off ordinary work, and neither is a judgement about intent. A
# turn that kept going past its last prose exits below, so a mid-turn one-line preamble between two
# tool calls is never read -- the same line `wall_of_text.rego` draws in its own correction text.
# And the narration has to be the final sentence of the closing prose, which is what stops it from
# charging a sentence the zero-information arm already owns.
#
# Fenced code is folded to one placeholder, and backtick spans and double-quoted spans are stripped
# before matching, so quoting these sentences -- this file, the policy, a report about the guard --
# cannot trip it. The shared half (transcript discovery, turn bucketing) comes from
# `scripts/cupcake_turn_scan.py` and the classification from `scripts/cupcake_narrated_action.py`,
# so the guards cannot drift into disagreeing about the same turn.
set -uo pipefail
CUPCAKE_SIGNAL_REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]:-$0}")/../.." && pwd)"
export CUPCAKE_SIGNAL_REPO_ROOT
python3 - <<'PY' 2>/dev/null || true
import os, sys

sys.path.insert(0, os.path.join(os.environ.get("CUPCAKE_SIGNAL_REPO_ROOT", "."), "scripts"))
try:
    import cupcake_turn_scan as scan
    import cupcake_narrated_action as na
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

# A turn that kept going past its last prose did not end on that prose, so its narration sat between
# two tool calls and is exactly the shape this rule must never touch.
if turn.last_text_index < 0 or turn.tool_after(turn.last_text_index):
    sys.exit(0)

runs = turn.text_runs
if not runs:
    sys.exit(0)
closing = runs[-1]

found = na.narrated_action(closing)
if not found:
    sys.exit(0)
narration, action_class, shape = found

print(
    "NARRATIONFACTS|narration={}|actionclass={}|shape={}|reported={}|banner={}|blocked={}".format(
        narration,
        action_class,
        shape,
        int(na.reported_outcome(closing)),
        int(na.launch_banner(closing)),
        int(na.externally_blocked(closing)),
    )
)
PY
