# OPA unit tests for no_narrated_action: the Stop-event halt on a closing message that ends by
# narrating the action instead of by carrying its outcome.
#
# Run with:
#   opa test .cupcake/system/commands.rego \
#     .cupcake/policies/claude/no_narrated_action.rego \
#     .cupcake/tests/no_narrated_action_test.rego
#
# These cases exercise the conjunction, which is what lives in the policy. The prose classification
# -- which sentences count as a narration at all -- lives in scripts/cupcake_narrated_action.py and
# is proven against the verbatim shapes by scripts/test-narrated-action-classifier.py, because a
# Rego test cannot see a regex in a shell signal. Both halves are needed: a policy green on
# hand-typed facts says nothing about whether a real transcript ever produces them.
package cupcake.policies.claude.no_narrated_action_test

import rego.v1

import data.cupcake.policies.claude.no_narrated_action as guard

# A facts line with every exemption clear, so each test can set exactly the one it is about.
narration_facts(clause) := concat("", [
	"NARRATIONFACTS|narration=", clause,
	"|actionclass=launch|shape=fragment|reported=0|banner=0|blocked=0",
])

# The same line with one exemption flipped on. Substituted rather than appended: the policy builds
# its fact map with an object comprehension, so a repeated key is a conflict rather than an
# override, and appending would test an evaluation error instead of the rule.
narration_facts_with(clause, field, value) := replace(
	narration_facts(clause),
	concat("", [field, "=0"]),
	concat("", [field, "=", value]),
)

stop_event(sig) := {
	"hook_event_name": "Stop",
	"signals": {"last_assistant_narrated_action": sig},
}

rule_ids(halts) := {d.rule_id | some d in halts}

narrated := "ER-EFFECTS-NO-NARRATED-ACTION"

# --- the five verbatim instances -----------------------------------------------------------------
test_halt_on_the_rerun_instance if {
	halts := guard.halt with input as stop_event(narration_facts("Re-running it now without the cap."))
	narrated in rule_ids(halts)
}

test_halt_on_the_rebuild_instance if {
	halts := guard.halt with input as stop_event(narration_facts("Rebuilding and relaunching now."))
	narrated in rule_ids(halts)
}

test_halt_on_the_bringing_up_instance if {
	halts := guard.halt with input as stop_event(narration_facts("Bringing it up to read the pointer chain live rather than guessing another offset:"))
	narrated in rule_ids(halts)
}

test_halt_on_the_diagnosable_instance if {
	halts := guard.halt with input as stop_event(narration_facts("Making the empty read diagnosable - logging each of the three hops so the next invasion says which one returned zero instead of just that one did:"))
	narrated in rule_ids(halts)
}

test_halt_on_the_dispatch_instance if {
	halts := guard.halt with input as stop_event(narration_facts("Dispatching a subagent to enumerate the menu builder's rows properly, and unblocking you now by narrowing the guard:"))
	narrated in rule_ids(halts)
}

# --- one case per exemption ----------------------------------------------------------------------
# The narration carries something measured, so the sentence reports what happened instead of
# announcing what has not.
test_no_halt_when_the_outcome_is_reported if {
	count(guard.halt) == 0 with input as stop_event(narration_facts_with("Re-running it now - exit 0, 26 rows.", "reported", "1"))
}

# The loud launch banner AGENTS.md mandates immediately before a game launch. It is a required form,
# and a rule that made it unspeakable would take a safety announcement away from the user.
test_no_halt_on_the_mandated_launch_banner if {
	count(guard.halt) == 0 with input as stop_event(narration_facts_with("Relaunching Stink Bean now.", "banner", "1"))
}

# A dependency the agent cannot dissolve by working harder.
test_no_halt_when_blocked_externally if {
	count(guard.halt) == 0 with input as stop_event(narration_facts_with("Bringing it up once you tell me what you saw on screen.", "blocked", "1"))
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
		"signals": {"last_assistant_narrated_action": narration_facts("Re-running it now without the cap.")},
	}
}

# Both the bare-string and the {output: ...} shapes cupcake may hand back.
test_halt_on_object_shaped_signal if {
	halts := guard.halt with input as {
		"hook_event_name": "Stop",
		"signals": {"last_assistant_narrated_action": {
			"output": narration_facts("Measuring it now."),
			"exit_code": 0,
		}},
	}
	narrated in rule_ids(halts)
}

# A degraded or crafted facts line that drops the exemption fields must halt rather than buy silence.
test_degraded_signal_fails_closed if {
	halts := guard.halt with input as stop_event("NARRATIONFACTS|narration=Dispatching on it.")
	narrated in rule_ids(halts)
}

# The offending sentence is quoted back, so the agent knows which sentence to make true or withdraw.
test_reason_quotes_the_narration if {
	halts := guard.halt with input as stop_event(narration_facts("Re-running it now without the cap."))
	some d in halts
	contains(d.reason, "Re-running it now without the cap.")
}

# The halt names the class of work that was owed, which is what tells the agent which tool call is
# missing.
test_reason_names_the_action_class if {
	halts := guard.halt with input as stop_event(narration_facts("Re-running it now without the cap."))
	some d in halts
	contains(d.reason, "launch")
}

# The correction has to say the thing this rule is not about, or an agent reading it will stop
# writing the one-line preambles that make a tool-heavy turn readable.
test_reason_exempts_mid_turn_narration if {
	halts := guard.halt with input as stop_event(narration_facts("Dispatching on it."))
	some d in halts
	contains(d.reason, "Mid-turn narration between tool calls is fine")
}
