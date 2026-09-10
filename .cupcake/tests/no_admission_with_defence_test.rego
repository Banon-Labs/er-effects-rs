# OPA unit tests for no_admission_with_defence: the Stop-event halt on a closing message that admits
# not following an instruction and then dilutes the admission in the same message.
#
# Run with:
#   opa test .cupcake/system/commands.rego \
#     .cupcake/policies/claude/no_admission_with_defence.rego \
#     .cupcake/tests/no_admission_with_defence_test.rego
#
# These cases exercise the conjunction, which is what lives in the policy. The prose classification
# -- which sentences count as an admission, and which clause counts as a dilution -- lives in
# scripts/cupcake_admission_with_defence.py and is proven against the verbatim shapes by
# scripts/test-admission-with-defence-classifier.py, because a Rego test cannot see a regex in a
# shell signal. Both halves are needed: a policy green on hand-typed facts says nothing about
# whether a real transcript ever produces them.
package cupcake.policies.claude.no_admission_with_defence_test

import rego.v1

import data.cupcake.policies.claude.no_admission_with_defence as guard

# The verbatim admission, 2026-09-10.
admitted := "I never drove an invasion myself all session - that was the standing order and I left it to you."

# The two verbatim dilutions.
excuse := "it is not firing right now because requesting one on top of your live negotiation would restart a search mid-handshake."

rebuttal := "So that run did announce - the auto-loop keeps searching once armed, whether or not anyone is at the pad."

# A facts line with every exemption clear, so each test can set exactly the one it is about.
admission_facts(clause, kind) := concat("", [
	"ADMISSIONFACTS|admission=", admitted,
	"|dilution=", clause,
	"|kind=", kind,
	"|table=0|solicited=0|blocked=0",
])

# The same line with one exemption flipped on. Substituted rather than appended: the policy builds
# its fact map with an object comprehension, so a repeated key is a conflict rather than an
# override, and appending would test an evaluation error instead of the rule.
admission_facts_with(clause, kind, field, value) := replace(
	admission_facts(clause, kind),
	concat("", [field, "=0"]),
	concat("", [field, "=", value]),
)

stop_event(sig) := {
	"hook_event_name": "Stop",
	"signals": {"last_assistant_admission_with_defence": sig},
}

rule_ids(halts) := {d.rule_id | some d in halts}

diluted := "ER-EFFECTS-NO-ADMISSION-WITH-DEFENCE"

# --- the verbatim instance -----------------------------------------------------------------------
# The message carried both dilutions, so this is the facts line the real transcript produces.
test_halt_on_the_verbatim_instance if {
	halts := guard.halt with input as stop_event(admission_facts(excuse, "justification+rebuttal"))
	diluted in rule_ids(halts)
}

# Either arm convicts on its own.
test_halt_on_the_excuse_alone if {
	halts := guard.halt with input as stop_event(admission_facts(excuse, "justification"))
	diluted in rule_ids(halts)
}

test_halt_on_the_rebuttal_alone if {
	halts := guard.halt with input as stop_event(admission_facts(rebuttal, "rebuttal"))
	diluted in rule_ids(halts)
}

# --- the shapes that must pass ---------------------------------------------------------------------
# An admission that simply stops emits no dilution, and the conjunction never completes.
test_no_halt_when_the_admission_stops if {
	count(guard.halt) == 0 with input as stop_event(admission_facts("", ""))
}

# A dilution with no admission in front of it is ordinary prose. The signal never emits this, and
# the policy must not halt on it either, or a crafted line could accuse a turn that conceded nothing.
test_no_halt_without_an_admission if {
	count(guard.halt) == 0 with input as stop_event(concat("", [
		"ADMISSIONFACTS|admission=|dilution=", excuse,
		"|kind=justification|table=0|solicited=0|blocked=0",
	]))
}

# The user asked for the facts, so the correction is the deliverable.
test_no_halt_when_the_correction_was_solicited if {
	count(guard.halt) == 0 with input as stop_event(admission_facts_with(rebuttal, "rebuttal", "solicited", "1"))
}

# A dependency the agent cannot dissolve by working harder.
test_no_halt_when_blocked_externally if {
	count(guard.halt) == 0 with input as stop_event(admission_facts_with(excuse, "justification", "blocked", "1"))
}

# --- shape of the guard itself ----------------------------------------------------------------------
# A clean turn emits nothing, and nothing must halt.
test_no_halt_on_empty_signal if {
	count(guard.halt) == 0 with input as stop_event("")
}

# Stop-only: a PreToolUse carrying the same signal must not halt.
test_no_halt_on_other_event if {
	count(guard.halt) == 0 with input as {
		"hook_event_name": "PreToolUse",
		"signals": {"last_assistant_admission_with_defence": admission_facts(excuse, "justification")},
	}
}

# Both the bare-string and the {output: ...} shapes cupcake may hand back.
test_halt_on_object_shaped_signal if {
	halts := guard.halt with input as {
		"hook_event_name": "Stop",
		"signals": {"last_assistant_admission_with_defence": {
			"output": admission_facts(rebuttal, "rebuttal"),
			"exit_code": 0,
		}},
	}
	diluted in rule_ids(halts)
}

# A degraded or crafted facts line that drops the exemption fields must halt rather than buy silence.
test_degraded_signal_fails_closed if {
	halts := guard.halt with input as stop_event("ADMISSIONFACTS|admission=I left it to you.|dilution=The reason is the tree was mid-rebuild.")
	diluted in rule_ids(halts)
}

# An unrecognised kind still produces a reason, so a degraded line cannot make the halt fall through
# to silence by way of an undefined rule body.
test_unknown_kind_still_halts_with_a_reason if {
	halts := guard.halt with input as stop_event(admission_facts(excuse, "something-else"))
	some d in halts
	d.rule_id == diluted
	count(d.reason) > 100
}

# --- what the correction has to say -----------------------------------------------------------------
# Both fragments are quoted back, so the agent can see which sentence to keep and which to cut.
test_reason_quotes_the_admission if {
	halts := guard.halt with input as stop_event(admission_facts(excuse, "justification"))
	some d in halts
	contains(d.reason, admitted)
}

test_reason_quotes_the_dilution if {
	halts := guard.halt with input as stop_event(admission_facts(excuse, "justification"))
	some d in halts
	contains(d.reason, excuse)
}

# The correction is different for each shape, because the fix is different: cut an excuse, or stop
# correcting someone who did not ask.
test_reason_names_the_excuse_for_a_justification if {
	halts := guard.halt with input as stop_event(admission_facts(excuse, "justification"))
	some d in halts
	contains(d.reason, "an excuse")
}

test_reason_names_the_uninvited_correction_for_a_rebuttal if {
	halts := guard.halt with input as stop_event(admission_facts(rebuttal, "rebuttal"))
	some d in halts
	contains(d.reason, "did not ask you to check")
}

test_reason_names_both_when_the_message_carried_both if {
	halts := guard.halt with input as stop_event(admission_facts(excuse, "justification+rebuttal"))
	some d in halts
	contains(d.reason, "an excuse and a rebuttal")
}

# The admission itself must never read as the violation, or the rule teaches agents to admit less.
# The correction has to say plainly that the admission is the part worth keeping.
test_reason_keeps_the_admission if {
	halts := guard.halt with input as stop_event(admission_facts(excuse, "justification"))
	some d in halts
	contains(d.reason, "the only part of that message worth reading")
}

# And it has to say what does not count as a reason to stop, because the verbatim excuse was a
# consequence the agent predicted for itself.
test_reason_rejects_a_self_predicted_consequence if {
	halts := guard.halt with input as stop_event(admission_facts(excuse, "justification"))
	some d in halts
	contains(d.reason, "a consequence you predict for yourself is not a blocker")
}
