# OPA unit tests for no_future_tense_commitment: the Stop-event halt on a closing message that ends
# on work the turn could have done, and on a claim about a delegate that has not reported.
#
# Run with:
#   opa test .cupcake/system/commands.rego \
#     .cupcake/policies/claude/no_future_tense_commitment.rego \
#     .cupcake/tests/no_future_tense_commitment_test.rego
#
# These cases exercise the conjunction, which is what lives in the policy. The prose classification
# -- which sentences count as a promise at all -- lives in scripts/cupcake_future_commitment.py and
# is proven against the verbatim shapes by scripts/test-future-commitment-signal.py, because a Rego
# test cannot see a regex in a shell signal. Both halves are needed: a policy green on hand-typed
# facts says nothing about whether a real transcript ever produces them.
package cupcake.policies.claude.no_future_tense_commitment_test

import rego.v1

import data.cupcake.policies.claude.no_future_tense_commitment as guard

# A facts line with every exemption clear, so each test can set exactly the one it is about.
commit_facts(clause) := concat("", [
	"FUTUREFACTS|commit=", clause,
	"|actionclass=build|timemarker=1|acted=0|blocked=0|planasked=0|deferred=0|conditional=0",
	"|delegation=|reported=1|instructed=0",
])

# The same line with one exemption flipped on. Substituted rather than appended: the policy builds
# its fact map with an object comprehension, so a repeated key is a conflict rather than an
# override, and appending would test an evaluation error instead of the rule.
commit_facts_with(clause, field, value) := replace(
	commit_facts(clause),
	concat("", [field, "=0"]),
	concat("", [field, "=", value]),
)

delegation_facts(clause) := concat("", [
	"FUTUREFACTS|commit=|actionclass=|timemarker=0|acted=0|blocked=0|planasked=0|deferred=0",
	"|conditional=0|delegation=", clause, "|reported=0|instructed=0",
])

stop_event(sig) := {
	"hook_event_name": "Stop",
	"signals": {"last_assistant_future_commitment": sig},
}

rule_ids(halts) := {d.rule_id | some d in halts}

commitment := "ER-EFFECTS-NO-FUTURE-TENSE-COMMITMENT"

delegation := "ER-EFFECTS-NO-DELEGATION-AS-COMPLETION"

# --- one case per catchable phrasing -------------------------------------------------------------
# The exact sentence the user quoted back: a time marker, a first-person future, two concrete verbs
# and a trailing purpose clause.
test_halt_on_the_verbatim_instance if {
	halts := guard.halt with input as stop_event(commit_facts("Next run I'll build this in and re-attach Frida to confirm zero DISCARDING lines across a full invade-reject-reinvade cycle."))
	commitment in rule_ids(halts)
}

test_halt_on_next_time if {
	halts := guard.halt with input as stop_event(commit_facts("Next time I'll rebuild before launching."))
	commitment in rule_ids(halts)
}

test_halt_on_afterwards if {
	halts := guard.halt with input as stop_event(commit_facts("Afterwards I'll re-run the gate and report the number."))
	commitment in rule_ids(halts)
}

test_halt_on_then_i_will if {
	halts := guard.halt with input as stop_event(commit_facts("Then I'll commit the fix."))
	commitment in rule_ids(halts)
}

test_halt_on_once_clause if {
	halts := guard.halt with input as stop_event(commit_facts("Once the DLL is staged, I'll relaunch and watch the log."))
	commitment in rule_ids(halts)
}

test_halt_on_bare_first_person_future if {
	halts := guard.halt with input as stop_event(commit_facts("I'll build the package."))
	commitment in rule_ids(halts)
}

test_halt_on_i_will if {
	halts := guard.halt with input as stop_event(commit_facts("I will re-attach Frida."))
	commitment in rule_ids(halts)
}

test_halt_on_going_to if {
	halts := guard.halt with input as stop_event(commit_facts("I'm going to rerun the probe."))
	commitment in rule_ids(halts)
}

test_halt_on_plan_to if {
	halts := guard.halt with input as stop_event(commit_facts("I plan to push the branch."))
	commitment in rule_ids(halts)
}

test_halt_on_trailing_purpose_clause if {
	halts := guard.halt with input as stop_event(commit_facts("I'll relaunch to confirm the counter reaches zero."))
	commitment in rule_ids(halts)
}

# --- one case per exemption ----------------------------------------------------------------------
# The turn already did work of the promised class: the sentence is a report, not a promise.
test_no_halt_when_the_turn_acted if {
	count(guard.halt) == 0 with input as stop_event(commit_facts_with("I'll rebuild it now.", "acted", "1"))
}

# A dependency outside the agent's reach -- an observation only the user can make, a credential, a
# purchase, a decision that is theirs, an instruction to stop.
test_no_halt_when_blocked_externally if {
	count(guard.halt) == 0 with input as stop_event(commit_facts_with("I'll rerun it once you tell me what you saw on screen.", "blocked", "1"))
}

# The user asked what the plan is. Stating one is then the answer.
test_no_halt_when_the_plan_was_requested if {
	count(guard.halt) == 0 with input as stop_event(commit_facts_with("I'll rebuild, then relaunch.", "planasked", "1"))
}

# The work waits on something that does not exist yet, and this turn started that thing.
test_no_halt_when_deferred_behind_started_work if {
	count(guard.halt) == 0 with input as stop_event(commit_facts_with("I'll read the log once the run it is doing now exits.", "deferred", "1"))
}

# The promise hangs on a decision the user has not made, or on an event that may never happen.
# The first shape belongs to ER-EFFECTS-NO-ZERO-INFORMATION-STOP, not here; the second is not due.
test_no_halt_on_a_conditional_offer if {
	count(guard.halt) == 0 with input as stop_event(commit_facts_with("Say the word and I'll build it.", "conditional", "1"))
}

# --- the delegation arm --------------------------------------------------------------------------
test_halt_on_delegate_present_tense_claim if {
	halts := guard.halt with input as stop_event(delegation_facts("It has the verbatim sentence, the exemptions that must not trip, and the same evidence bar the others got: a measured false-positive rate, all four repo gates, and a cupcake eval returning decision:block."))
	delegation in rule_ids(halts)
}

test_halt_on_delegate_future_tense_claim if {
	halts := guard.halt with input as stop_event(delegation_facts("It will produce the policy, the signal, the tests, and a measured rate."))
	delegation in rule_ids(halts)
}

# The completion notification is in the transcript: the message relays results instead of predicting
# them.
test_no_halt_when_the_delegate_reported if {
	count(guard.halt) == 0 with input as stop_event(concat("", [
		"FUTUREFACTS|commit=|actionclass=|timemarker=0|acted=0|blocked=0|planasked=0|deferred=0",
		"|conditional=0|delegation=It has the policy, the signal, and the tests.|reported=1|instructed=0",
	]))
}

# A statement about what was asked of the agent is a fact about the caller's own tool call, and the
# policy must not halt on one. The signal will not emit this pair -- a message that describes its
# instruction and then asserts the output still convicts on the assertion, and reports instructed=0 --
# so this case pins the policy's own condition rather than a line production makes.
test_no_halt_when_the_claim_is_instruction_framed if {
	count(guard.halt) == 0 with input as stop_event(concat("", [
		"FUTUREFACTS|commit=|actionclass=|timemarker=0|acted=0|blocked=0|planasked=0|deferred=0",
		"|conditional=0|delegation=I asked it to measure the rate, run the gates, and report the number.",
		"|reported=0|instructed=1",
	]))
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
		"signals": {"last_assistant_future_commitment": commit_facts("I'll rebuild it.")},
	}
}

# Both the bare-string and the {output: ...} shapes cupcake may hand back.
test_halt_on_object_shaped_signal if {
	halts := guard.halt with input as {
		"hook_event_name": "Stop",
		"signals": {"last_assistant_future_commitment": {
			"output": commit_facts("Next run I'll rebuild it."),
			"exit_code": 0,
		}},
	}
	commitment in rule_ids(halts)
}

# A degraded or crafted facts line that drops the exemption fields must halt rather than buy silence.
test_degraded_signal_fails_closed if {
	halts := guard.halt with input as stop_event("FUTUREFACTS|commit=Next run I'll build it.")
	commitment in rule_ids(halts)
}

# The offending sentence is quoted back, so the agent knows which sentence to make true or withdraw.
test_reason_quotes_the_promise if {
	halts := guard.halt with input as stop_event(commit_facts("Next run I'll build this in."))
	some d in halts
	contains(d.reason, "Next run I'll build this in.")
}

# The halt names the class of work that was owed, which is what tells the agent which tool call is
# missing.
test_reason_names_the_action_class if {
	halts := guard.halt with input as stop_event(commit_facts("Next run I'll build this in."))
	some d in halts
	contains(d.reason, "build")
}

test_delegation_reason_quotes_the_claim if {
	halts := guard.halt with input as stop_event(delegation_facts("It has the tests, the rates, and the gates."))
	some d in halts
	contains(d.reason, "It has the tests, the rates, and the gates.")
}
