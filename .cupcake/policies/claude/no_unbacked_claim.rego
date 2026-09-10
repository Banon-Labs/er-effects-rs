# METADATA
# scope: package
# title: Ban ending a turn on a claim that an artifact exists when nothing was written
# authors: ["er-quickload agents"]
# custom:
#   severity: HIGH
#   id: ER-EFFECTS-NO-UNBACKED-CLAIM
#   description: >-
#     User, 2026-08-23: "You didn't. You recorded a beads memory. That ABSOLUTELY IS NOT EVEN
#     REMOTELY CLOSE to a conformance GATE."
#
#     The turn had opened "Build a conformance gate, because 'read the reference first' is a note and
#     notes are advisory", described a reference-implementations.toml and a check wired into
#     check.sh, and shipped ONE `bd remember` call. No gate. No file. A memory costs a single tool
#     call and FEELS like delivery, which is precisely why it substitutes for it.
#
#     SIBLING of ER-EFFECTS-NO-UNEXECUTED-PROMISE, covering the opposite tense. That guard catches
#     "I'll build it" ending in nothing; this one catches "I built it" when nothing was built.
#
#     THE VIOLATION IS A CONJUNCTION OF THREE FACTS, all computed in the signal:
#       1. the FINAL prose block carries a first-person COMPLETION claim (built/added/created/
#          wrote/wired/landed/shipped/implemented/patched/updated/removed);
#       2. its object is a REPO ARTIFACT -- a path-like token or gate/check/hook/policy/test/guard;
#       3. NOTHING IN THE TURN WROTE A FILE: no Edit/Write/NotebookEdit, and no Bash call carrying a
#          write construct. `bd remember` is explicitly not a write -- it is the substitution.
#
#     BIASED HARD TOWARD NOT FIRING: only the closing block is scanned, quoted/backticked/fenced
#     spans are stripped, and an honest confession of absence ("no gate exists", "I have not built
#     it") SUPPRESSES the hit -- admitting the thing does not exist is the behaviour being asked
#     for and must never be punished. The pronoun must also OPEN its clause: "the gate I wrote" and
#     "the function I hooked" refer back to an artifact, they do not assert one into existence, and
#     over 2,370 real turns that relative-clause shape was nine of the signal's ten hits.
#
#     INERT UNTIL 2026-09-09, and green the whole time. The signal read `turn.events` off a
#     cupcake_turn_scan.Turn, which has no such attribute; the AttributeError went to /dev/null and
#     the empty signal read as a clean turn. The tests below never caught it because they hand the
#     policy a signal string directly. The end-to-end proof lives in
#     scripts/test-cupcake-stop-guards.py (unbacked_claim.jsonl halts, backed_claim.jsonl does not).
#
#     KNOWN GAP, stated so its silence is never mistaken for proof: a bare IMPERATIVE that reads as
#     delivered ("Build a conformance gate.") has no first-person verb and passes. Matching bare
#     imperatives would fire on every legitimate recommendation and turn the guard into noise.
#   routing:
#     required_events: ["Stop"]
#     required_signals: ["last_assistant_unbacked_claim"]
package cupcake.policies.claude.no_unbacked_claim

import rego.v1

halt contains decision if {
	input.hook_event_name == "Stop"
	some p in [phrase]
	decision := {
		"rule_id": "ER-EFFECTS-NO-UNBACKED-CLAIM",
		"reason": reason_for(p),
		"severity": "HIGH",
	}
}

reason_for(p) := msg if {
	msg := concat("", ["You are ending the turn claiming an artifact exists that you did not create: '", p, "'. No Edit, no Write, and no Bash call in this turn wrote a file -- a `bd remember` is not a build, it is the substitution this guard exists to catch. Pick one, now: (a) ACTUALLY CREATE IT in this turn, then say so; or (b) REWRITE the sentence to state plainly that it does NOT exist, what you did instead, and what it would take -- an honest 'no gate exists, I have not built it' passes this guard by design. Describing a thing in the same breath as claiming it is the failure; the user cannot see your tool calls and will read the claim as delivery."])
}

phrase := p if {
	startswith(raw, "UNBACKED:")
	p := trim(trim_prefix(raw, "UNBACKED:"), " \t\r\n")
} else := p if {
	raw != ""
	p := raw
}

raw := trim(matched_phrase, " \t\r\n")

matched_phrase := p if {
	p := input.signals.last_assistant_unbacked_claim
	is_string(p)
} else := p if {
	p := input.signals.last_assistant_unbacked_claim.output
} else := ""
