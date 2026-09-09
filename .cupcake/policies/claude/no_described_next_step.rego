# METADATA
# scope: package
# title: Ban ending a turn on a next step that was described instead of started
# authors: ["er-quickload agents"]
# custom:
#   severity: HIGH
#   id: ER-EFFECTS-NO-DESCRIBED-NEXT-STEP
#   description: >-
#     User directive 2026-09-08, in their words: "I feel like I probably should have a rego policy
#     that catches this prose and encourages you to do the thing you talked about."
#
#     THE INSTANCE. After seven runs had proven that the writable data of ersc.dll does not hold the
#     live Seamless session pointer, a turn closed with: "The next approach follows from the
#     disassembly rather than from hope: the session identifies itself by holding an active state at
#     +0x150 with a std::mutex at +0x100 during a join, and that signature does not require the
#     pointer to live in Seamless's data at all -- the address-space walk I deleted an hour ago is
#     exactly the right tool pointed at the right object this time. I'd rather say that plainly than
#     dress up a seventh variation on searching the one place we now know it isn't." It built none of
#     it. The turn had the mechanism, the target and the tool, and spent them on a paragraph.
#
#     WHY PROSE COULD NOT FIX THIS. AGENTS.md has banned it since July -- "Before asking the user to
#     proceed, ask yourself 'do I already know how to proceed?' If yes, do NOT respond -- just
#     proceed. Reserve user-facing turns for genuine forks, blockers, or destructive/irreversible
#     steps" -- and the turn above was written anyway. The user's standing rule is that binding
#     behaviour needs executable enforcement, never a note or a memory. This is the executable half.
#
#     WHY THE SIBLING GUARD DOES NOT ALREADY COVER IT. no_unexecuted_promise catches a FIRST-PERSON
#     commitment that nothing keeps: its OPENER_RE requires "I'll", "I'm going to", "let me". The
#     failing sentence commits nobody -- "the next approach follows", "the walk is the right tool".
#     Describing the step impersonally is the more comfortable way to leave it undone, because no one
#     was ever said to be doing it. That is the gap this closes; it is not a second copy.
#
#     THE VIOLATION IS A CONJUNCTION OF FIVE FACTS, computed in the signal:
#       1. the turn names a concrete next action in a forward-looking prescription ("the next step is
#          reading X", "the right approach would be to walk the address space", "what comes next is
#          Y"), and the thing named carries a work word;
#       2. no substantive tool call follows the sentence that named it -- described, then not begun.
#          Saying it and then running it in the same turn is the correct shape and always passes;
#       3. the turn does not hand the ball to the user -- no question, no fork, no "want me to", no
#          dependency on a user action;
#       4. the turn does not state a genuine blocker -- sudo, a live game run, approval for a
#          destructive step, a guard refusal, a tool that is absent, a step waiting on a merge;
#       5. no live background work is carrying it.
#     Any one of the five missing means the turn is fine, and the signal stays silent.
#
#     BIASED HARD TOWARD NOT FIRING, AND THE BIAS WAS MEASURED. A false positive blocks a legitimate
#     turn -- a genuine fork, a real blocker, a destructive step awaiting approval -- which costs more
#     than a miss. Replayed over 2,142 real turns from 70 session transcripts by
#     scripts/audit-described-next-step-false-positives.py, it halts 9 of them (0.4%), and each
#     quoted sentence is a next step that was described and then abandoned. Two whole pattern
#     families were deleted during that replay because they fired on turns doing the work: "what is
#     left is ..." reads as definition, and "... is the right move" reads as justification for a move
#     being made now.
#
#     The signal emits ONE facts line --
#     NEXTSTEPFACTS|nextstep=<clause>|acted=0|1|blocked=0|1|handoff=0|1|carried=0|1 -- so the
#     OBSERVATION lives in the shell and the RULE lives here, where it is unit-testable. Empty signal
#     -> no halt.
#
#     KNOWN GAP: an interrupted turn fires no Stop event, so a described-and-abandoned step the user
#     cuts short is not caught. The sibling pairs (no_authority_agreement + _reminder, idle_hold +
#     _reminder) close that with a UserPromptSubmit interlock reading the same signal; add one the
#     same way if the gap bites.
#   routing:
#     required_events: ["Stop"]
#     required_signals: ["last_assistant_described_next_step"]
package cupcake.policies.claude.no_described_next_step

import rego.v1

# Enforcement: block turn-end when the turn named a next step it could have begun, and began none of
# it.
halt contains decision if {
	input.hook_event_name == "Stop"
	described_not_started
	decision := {
		"rule_id": "ER-EFFECTS-NO-DESCRIBED-NEXT-STEP",
		"reason": reason,
		"severity": "HIGH",
	}
}

# The conjunction. `acted`, `handoff`, `blocked` and `carried` are the four ways out, and each is a
# turn shape that is allowed to end on a described next step: one that started it, one that is
# waiting on the user, one that hit something real, one whose work is already running.
described_not_started if {
	nextstep != ""
	acted == "0"
	handoff == "0"
	blocked == "0"
	carried == "0"
}

reason := msg if {
	msg := concat("", ["You ended the turn by describing a next step instead of starting it: '", nextstep, "'. You named the mechanism, the target and the tool, and then stopped -- so the work does not exist, and the user has to notice and ask for the thing you just told them was the right thing to do. Nothing was blocking you: no tool call began it, no background task is carrying it, you asked the user nothing and named no blocker. Do it NOW, in THIS turn: make the tool call that begins the step you just described. If it is genuinely too large for one turn, START it (a backgrounded command or a subagent) rather than narrating it. Stop instead only when the next move is destructive/irreversible, is a real fork the user must decide, or needs something only they can supply -- and then say in ONE line exactly what that is, rather than describing the step you are not taking."])
}

# --- signal parsing ------------------------------------------------------------------------------
# NEXTSTEPFACTS|nextstep=<clause>|acted=0|blocked=0|handoff=0|carried=0
# The leading tag carries no "=" so it drops out of the fact map on its own. A field the signal omits
# falls back to a default that does NOT exempt: a degraded or crafted signal fails closed and halts,
# matching how the sibling guards treat an untagged non-empty value.
fact[k] := v if {
	some kv in split(raw, "|")
	n := indexof(kv, "=")
	n > 0
	k := trim(substring(kv, 0, n), " \t\r\n")
	v := trim(substring(kv, n + 1, -1), " \t\r\n")
}

nextstep := object.get(fact, "nextstep", "")

acted := object.get(fact, "acted", "0")

handoff := object.get(fact, "handoff", "0")

blocked := object.get(fact, "blocked", "0")

carried := object.get(fact, "carried", "0")

raw := trim(matched_facts, " \t\r\n")

# Signal value tolerates both the bare-string and {output: ...} shapes cupcake may hand back.
matched_facts := p if {
	p := input.signals.last_assistant_described_next_step
	is_string(p)
} else := p if {
	p := input.signals.last_assistant_described_next_step.output
} else := ""
