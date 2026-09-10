# METADATA
# scope: package
# title: Ban ending a turn on "proven" without the run that observed it
# authors: ["er-quickload agents"]
# custom:
#   severity: HIGH
#   id: ER-EFFECTS-NO-PROOF-WITHOUT-OBSERVATION
#   description: >-
#     User directive 2026-09-09, recorded as bd `proven-means-observed-in-the-real-game-not-a-boot-
#     log-line-2026-09-09`. A turn opened "Both addresses now translate instead of refusing" off one
#     boot line, `ADDRESS TRANSLATED (EQUIP_PARAM_GOODS_GET_ENTRY_RVA): 0x140d39df0 -> 0x140d3b5b0`.
#     An address resolving says a constant maps to a function; it does not say a single frame of the
#     feature ran, and the animation write and the popup skip had produced no line at all. The
#     strongest word the evidence would bear was reached for, and it was the only sentence the user
#     read.
#
#     "proven" names an outcome someone could have watched happen in the game. A build, a launch, a
#     log line, an address translation, a hook install, a green gate and a passing test are
#     preconditions. A RAM oracle reading a value is "measured", and it names the oracle.
#
#     The signal emits PROOFFACTS with two facts. The conjunction lives here so it is unit-testable
#     against the verbatim corpus instead of hiding in shell regexes:
#       claim    -- the closing prose used a proof word outside a quotation, a path, a slug, a table
#                   cell or a disclaimer ("not proven", "unproven", "yet to be proven").
#       observed -- the same turn cited a real-game observation: a `br-YYYYMMDD-HHMMSS-xxxx` run id,
#                   an `er-me3-runs` artifact path, a named `oracle_` field, a screenshot or image
#                   artifact, a pixel diff, or prose putting an observation verb next to the game or
#                   the screen.
#     Halts when the word was used and nothing observed it.
#
#     Why a new rule rather than widening a neighbour: ER-EFFECTS-NO-UNBACKED-CLAIM asks whether a
#     repo artifact the turn claims to have built exists, which a proof claim about the game never
#     touches; ER-EFFECTS-NO-FALSE-CI-GREEN is about a gate's own result. This one turns on the
#     single fact neither can see: whether the word rests on something anyone watched happen.
#   routing:
#     required_events: ["Stop"]
#     required_signals: ["last_assistant_proof_without_observation"]
package cupcake.policies.claude.no_proof_without_observation

import rego.v1

halt contains decision if {
	input.hook_event_name == "Stop"
	some clause in [offending]
	decision := {
		"rule_id": "ER-EFFECTS-NO-PROOF-WITHOUT-OBSERVATION",
		"reason": reason_for(clause),
		"severity": "HIGH",
	}
}

reason_for(clause) := msg if {
	msg := concat("", [
		"You ended the turn calling something proven ('",
		clause,
		"') and cited nothing that observed it in the game. A build, a launch, a log line, an address translation, a hook install, a green gate and a passing test are preconditions, not proof. Do one now: cite the run that observed the outcome -- a br-YYYYMMDD-HHMMSS-xxxx id, an er-me3-runs artifact path, a named oracle_ field, a screenshot, or what was seen on screen -- or drop the word and say what you actually have: measured in-process, the harness ran, or unproven.",
	])
}

facts := line if {
	line := raw_signal
	startswith(line, "PROOFFACTS|")
}

# Tolerates both shapes cupcake may hand back: a bare string, or {output: ...}.
raw_signal := s if {
	s := input.signals.last_assistant_proof_without_observation
	is_string(s)
} else := s if {
	s := input.signals.last_assistant_proof_without_observation.output
	is_string(s)
} else := ""

field(name) := value if {
	some part in split(facts, "|")
	startswith(part, concat("", [name, "="]))
	value := substring(part, count(name) + 1, -1)
}

offending := clause if {
	clause := field("claim")
	clause != ""
	field("observed") == "0"
}
