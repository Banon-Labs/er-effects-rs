# OPA unit tests for no_proof_without_observation (the Stop-event halt on an unobserved proof claim).
# Run with:
#   opa test .cupcake/system/commands.rego \
#     .cupcake/policies/claude/no_proof_without_observation.rego \
#     .cupcake/tests/no_proof_without_observation_test.rego
package cupcake.policies.claude.no_proof_without_observation_test

import rego.v1

import data.cupcake.policies.claude.no_proof_without_observation as guard

facts(claim, observed) := concat("", [
	"PROOFFACTS|claim=", claim,
	"|observed=", observed,
])

stop_event(sig) := {
	"hook_event_name": "Stop",
	"signals": {"last_assistant_proof_without_observation": sig},
}

rule_ids(halts) := {d.rule_id | some d in halts}

# (b) The verbatim shape that prompted the rule: the word used off a boot line nobody watched.
test_halt_when_proven_rests_on_nothing_observed if {
	halts := guard.halt with input as stop_event(facts("Both addresses are now proven.", "0"))
	"ER-EFFECTS-NO-PROOF-WITHOUT-OBSERVATION" in rule_ids(halts)
}

# (a) The same sentence beside a run id that watched the outcome is the wanted behaviour. The signal
# sets observed=1 for a br-YYYYMMDD-HHMMSS-xxxx id, an er-me3-runs path, a named oracle_ field, a
# screenshot, a pixel diff, or an observation verb next to the game or the screen.
test_allow_when_a_run_observed_it if {
	halts := guard.halt with input as stop_event(facts("The short animation is proven by run br-20260909-232105-c040.", "1"))
	count(halts) == 0
}

# (c) A proof word that survives only inside a quotation is not a claim. The signal strips fenced
# code, backtick spans and double-quoted spans before it looks, so quoting the user (or this policy)
# leaves no claim behind and the facts line carries an empty claim -- the same shape as (d).
test_allow_when_the_word_was_only_quoted if {
	halts := guard.halt with input as stop_event(facts("", "0"))
	count(halts) == 0
}

# (d) No proof word at all. An ordinary turn must pass untouched. The signal emits nothing in this
# case; the empty-claim line is asserted here so both routes to "no claim" are covered.
test_allow_when_no_proof_word_was_used if {
	halts := guard.halt with input as stop_event("")
	count(halts) == 0
}

# The object shape cupcake may hand back instead of a bare string must halt identically.
test_halt_on_object_shaped_signal if {
	halts := guard.halt with input as {
		"hook_event_name": "Stop",
		"signals": {"last_assistant_proof_without_observation": {
			"output": facts("The popup skip is proven.", "0"),
			"exit_code": 0,
		}},
	}
	"ER-EFFECTS-NO-PROOF-WITHOUT-OBSERVATION" in rule_ids(halts)
}

# The guard is Stop-only: a PreToolUse carrying the same signal must not halt.
test_allow_on_other_event if {
	halts := guard.halt with input as {
		"hook_event_name": "PreToolUse",
		"signals": {"last_assistant_proof_without_observation": facts("It is proven.", "0")},
	}
	count(halts) == 0
}

# A malformed line must not halt: this guard fails open, like its neighbours.
test_allow_on_unrecognised_signal_shape if {
	halts := guard.halt with input as stop_event("something else entirely")
	count(halts) == 0
}

# The correction has to offer both exits, not merely say the word was wrong.
test_reason_offers_the_run_id_and_the_retraction if {
	halts := guard.halt with input as stop_event(facts("The mechanism is proven.", "0"))
	some d in halts
	contains(d.reason, "br-YYYYMMDD-HHMMSS-xxxx")
	contains(d.reason, "unproven")
}

# The offending clause is quoted back, so the correction names the sentence it is about.
test_reason_quotes_the_clause if {
	halts := guard.halt with input as stop_event(facts("The short animation is proven.", "0"))
	some d in halts
	contains(d.reason, "The short animation is proven.")
}
