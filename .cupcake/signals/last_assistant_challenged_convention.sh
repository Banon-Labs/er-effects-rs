#!/usr/bin/env bash
# Cupcake signal: last_assistant_challenged_convention
#
# Scans the most recently completed assistant turn and emits one facts line when that turn answered
# a challenge to one of its own choices with prose instead of a diff:
#
#   CHALLENGEFACTS|challenge=..|defence=..|changed=..|blocked=..|asked=..|pivot=..
#
# A clean turn emits nothing (fail-open, like every neighbouring signal).
#
# The failure this exists to refuse
# ---------------------------------
# Reported by the user 2026-09-10, after three consecutive turns of it:
#
#   user:      Do you crutch on 1.16.2 addreses for a specific reason?
#   assistant: a table of why the convention exists, closing "I'll ask the agent for the
#              return-value contract rather than the 1.16.2 address". No file changed.
#   user:      Why on earth would I want that convention?
#   assistant: "You wouldn't -- and there's no need to invent a new field, because line 1022
#              already does the right thing ...", then 160 words of rationale. No file changed.
#   user:      Right, but it exists. Why do you insist on going 'My real point <em-dash> massive
#              amount of prose that is never worth reading' followed by me going 'Yes I understand
#              that I wouldn't so now you're pausing on something I clearly want you to correct'
#
# The third message is the user's own diagnosis and it is the specification: a question about a
# convention the assistant chose is a correction, and the assistant kept spending turns justifying
# rather than fixing. Every explanation was accurate. None was wanted.
#
# Why the neighbouring Stop guards do not catch it
#   * `last_assistant_stall_on_friction` needs an admission or a hand-back in the reply, and it
#     exempts any turn whose opening prompt asked a question. All three prompts here are questions,
#     so its `question` fact exempts every one of them.
#   * `last_assistant_diagnosis_without_fix` needs the reply to name a defect. A defence of a
#     convention names no defect; it says the convention is fine.
#   * `last_assistant_future_commitment` needs a first-person future promise. Turn one closed on
#     one ("I'll ask the agent for ...") but the promise was a read, and the turn had read
#     already, so its class exemption cleared it.
# The fact that convicts here is the conjunction of a second-person challenge in the prompt and a
# turn that produced justification where it owed a diff.
#
# The exemptions
#   changed -- the turn wrote something, or dispatched an agent to. Deliberately generous: any edit
#              at all clears the rule, because a false "nothing changed" accuses a turn that did the
#              work, while a false "something changed" is a quiet non-event.
#   blocked -- a dependency the agent cannot dissolve by working harder (a credential, sudo, a live
#              game, a guard that refused the write, a tool that is not installed). Narrow on
#              purpose: "I need X before I can do Y" was the stall in the real transcript, so a bare
#              statement of need is not a blocker here.
#   asked   -- the user explicitly asked for an explanation. Then explaining is the deliverable and
#              this guard must stay out of it. Deliberately not a bare question mark: every prompt
#              in the corpus above ends in one.
#
# Fenced code, backtick spans and double-quoted spans are stripped before matching, so quoting these
# sentences -- this file, the policy, a report about the guard -- cannot trip it. Single-quoted
# spans are left alone, because the third prompt carries its challenge around two of them. The
# shared half (transcript discovery, turn bucketing) comes from `scripts/cupcake_turn_scan.py` and
# the classification from `scripts/cupcake_challenged_convention.py`, so the guards cannot drift
# into disagreeing about the same turn.
set -uo pipefail
CUPCAKE_SIGNAL_REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]:-$0}")/../.." && pwd)"
export CUPCAKE_SIGNAL_REPO_ROOT
python3 - <<'PY' 2>/dev/null || true
import os, sys

sys.path.insert(0, os.path.join(os.environ.get("CUPCAKE_SIGNAL_REPO_ROOT", "."), "scripts"))
try:
    import cupcake_turn_scan as scan
    import cupcake_challenged_convention as cc
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

challenge = cc.challenged_choice(prompt)
asked = cc.explanation_requested(prompt)
changed = cc.changed_a_file(tool_blocks)
blocked = cc.externally_blocked(closing)
defence = cc.defensive_prose(closing) if challenge else ""
pivot = cc.concession_pivot(closing)

if not defence and not pivot:
    sys.exit(0)

print(
    "CHALLENGEFACTS|challenge={}|defence={}|changed={}|blocked={}|asked={}|pivot={}".format(
        challenge,
        defence,
        int(changed),
        int(blocked),
        int(asked),
        pivot,
    )
)
PY
