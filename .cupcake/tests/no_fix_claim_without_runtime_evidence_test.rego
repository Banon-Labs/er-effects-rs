# OPA unit tests for no_fix_claim_without_runtime_evidence: the Stop-event halt on a closing message
# that calls a change to game-loaded code a fix with no run behind it.
#
# Run with:
#   opa test .cupcake/system/commands.rego \
#     .cupcake/policies/claude/no_fix_claim_without_runtime_evidence.rego \
#     .cupcake/tests/no_fix_claim_without_runtime_evidence_test.rego
#
# These cases exercise the conjunction, which is what lives in the policy. The prose classification
# -- which sentences count as a fix claim at all, and which paths count as game-loaded code -- lives
# in scripts/cupcake_fix_claim.py and is proven by scripts/test-fix-claim-classifier.py, because a
# Rego test cannot see a regex in a shell signal. Both halves are needed: a policy green on
# hand-typed facts says nothing about whether a real transcript ever produces them, and that is the
# exact way every Stop guard in this repo stayed inert for 36 days.
package cupcake.policies.claude.no_fix_claim_without_runtime_evidence_test

import rego.v1

import data.cupcake.policies.claude.no_fix_claim_without_runtime_evidence as guard

# A facts line with the culpable half set and every exemption clear, so each test can flip exactly
# the one field it is about.
fix_facts(clause) := concat("", [
	"FIXFACTS|claim=", clause,
	"|changed=1|evidence=0|hedged=0|blocked=0|hostobject=0",
])

# The same line with one field substituted. Substituted rather than appended: the policy builds its
# fact map with an object comprehension, so a repeated key is a conflict rather than an override,
# and appending would test an evaluation error instead of the rule.
fix_facts_with(clause, field, value) := replace(
	fix_facts(clause),
	concat("", [field, "=0"]),
	concat("", [field, "=", value]),
)

stop_event(sig) := {
	"hook_event_name": "Stop",
	"signals": {"last_assistant_fix_claim_without_runtime_evidence": sig},
}

rule_ids(halts) := {d.rule_id | some d in halts}

fix_claim := "ER-EFFECTS-NO-FIX-CLAIM-WITHOUT-RUNTIME-EVIDENCE"

# --- the verbatim shape -------------------------------------------------------------------------
# The phrase the user quoted, in the sentence that closed the turn which then failed live.
test_halt_on_the_real_fix if {
	halts := guard.halt with input as stop_event(fix_facts("That is the real fix."))
	fix_claim in rule_ids(halts)
}

test_halt_on_this_is_the_fix if {
	halts := guard.halt with input as stop_event(fix_facts("This is the fix: the row was cloned before the menu existed."))
	fix_claim in rule_ids(halts)
}

test_halt_on_this_fixes_it if {
	halts := guard.halt with input as stop_event(fix_facts("This fixes the crash on the second load."))
	fix_claim in rule_ids(halts)
}

test_halt_on_the_fix_works if {
	halts := guard.halt with input as stop_event(fix_facts("The fix works."))
	fix_claim in rule_ids(halts)
}

test_halt_on_a_bare_past_tense_report if {
	halts := guard.halt with input as stop_event(fix_facts("Fixed the ordering in the row cloner."))
	fix_claim in rule_ids(halts)
}

# --- one case per exemption ----------------------------------------------------------------------
# A run artifact was opened after the edit. This is the whole point of the rule: reading what the
# run wrote is what the word costs.
test_no_halt_when_a_run_artifact_was_read if {
	count(guard.halt) == 0 with input as stop_event(fix_facts_with(
		"The fix works: er-quit-rows-debug.log shows the row on both loads.",
		"evidence",
		"1",
	))
}

# An honest hedge is the behaviour being asked for and must never be punished.
test_no_halt_when_hedged if {
	count(guard.halt) == 0 with input as stop_event(fix_facts_with(
		"That is the fix, unverified -- nothing has run yet.",
		"hedged",
		"1",
	))
}

# A dependency the agent cannot dissolve by working harder.
test_no_halt_when_blocked_externally if {
	count(guard.halt) == 0 with input as stop_event(fix_facts_with(
		"That is the real fix, which I have left for you to call.",
		"blocked",
		"1",
	))
}

# The sentence says a gate was fixed and names nothing inside the game. A green gate is the proof of
# host work, and demanding a game launch for a clippy lint is how a guard earns a reputation for
# being wrong.
test_no_halt_when_the_object_is_host_machinery if {
	count(guard.halt) == 0 with input as stop_event(fix_facts_with(
		"The integration gate came back red and I have fixed all four failures.",
		"hostobject",
		"1",
	))
}

# The turn edited nothing that can reach a DLL: scripts, docs and policies are outside the rule
# because a run cannot show any of them working or failing.
test_no_halt_when_no_game_loaded_code_changed if {
	count(guard.halt) == 0 with input as stop_event(replace(
		fix_facts("That is the fix for the audit script."),
		"changed=1",
		"changed=0",
	))
}

# --- shape of the guard itself --------------------------------------------------------------------
# A clean turn emits nothing, and nothing must halt.
test_no_halt_on_empty_signal if {
	count(guard.halt) == 0 with input as stop_event("")
}

# Stop-only: a PreToolUse carrying the same signal must not halt.
test_no_halt_on_other_event if {
	count(guard.halt) == 0 with input as {
		"hook_event_name": "PreToolUse",
		"signals": {"last_assistant_fix_claim_without_runtime_evidence": fix_facts("That is the real fix.")},
	}
}

# Both the bare-string and the {output: ...} shapes cupcake may hand back.
test_halt_on_object_shaped_signal if {
	halts := guard.halt with input as {
		"hook_event_name": "Stop",
		"signals": {"last_assistant_fix_claim_without_runtime_evidence": {
			"output": fix_facts("That is the real fix."),
			"exit_code": 0,
		}},
	}
	fix_claim in rule_ids(halts)
}

# The asymmetric defaults, which are where this rule parts company with its neighbours.
#
# A line that drops `changed` stays silent: that is the fact which makes the turn culpable, and a
# rule of this shape costs far more when it is wrong than when it is quiet.
test_missing_changed_field_stays_silent if {
	count(guard.halt) == 0 with input as stop_event("FIXFACTS|claim=That is the real fix.")
}

# A line that drops an exemption field still halts, so a crafted or degraded signal cannot buy
# silence by omission.
test_missing_exemption_fields_fail_closed if {
	halts := guard.halt with input as stop_event("FIXFACTS|claim=That is the real fix.|changed=1")
	fix_claim in rule_ids(halts)
}

# The offending sentence is quoted back, so the agent knows which sentence to make true or withdraw.
test_reason_quotes_the_claim if {
	halts := guard.halt with input as stop_event(fix_facts("That is the real fix."))
	some d in halts
	contains(d.reason, "That is the real fix.")
}

# The correction has to name what would settle it, or the agent is told only that it is wrong.
test_reason_names_the_evidence_that_would_settle_it if {
	halts := guard.halt with input as stop_event(fix_facts("That is the real fix."))
	some d in halts
	contains(d.reason, "er-me3-runs")
}

# And it has to say that withdrawing the word is a pass, or the rule teaches agents to claim less
# carefully rather than to hedge honestly.
test_reason_offers_the_hedge_as_a_pass if {
	halts := guard.halt with input as stop_event(fix_facts("That is the real fix."))
	some d in halts
	contains(d.reason, "An honest hedge passes this guard by design")
}
