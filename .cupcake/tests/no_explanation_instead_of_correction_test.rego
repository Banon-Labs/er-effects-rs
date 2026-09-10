# OPA unit tests for no_explanation_instead_of_correction (the Stop-event halt on a challenge that
# was answered with prose instead of a diff).
# Run with:
#   opa test .cupcake/system/commands.rego \
#     .cupcake/policies/claude/no_explanation_instead_of_correction.rego \
#     .cupcake/tests/no_explanation_instead_of_correction_test.rego
package cupcake.policies.claude.no_explanation_instead_of_correction_test

import rego.v1

import data.cupcake.policies.claude.no_explanation_instead_of_correction as guard

# The facts line the signal emits, written as an override map so a new fact does not have to be
# threaded through every call site.
facts(o) := concat("", [
	"CHALLENGEFACTS|challenge=", object.get(o, "challenge", ""),
	"|defence=", object.get(o, "defence", ""),
	"|changed=", object.get(o, "changed", "0"),
	"|blocked=", object.get(o, "blocked", "0"),
	"|asked=", object.get(o, "asked", "0"),
	"|pivot=", object.get(o, "pivot", ""),
])

# The three verbatim prompts, 2026-09-10.
challenge_one := "Do you crutch on 1.16.2 addreses for a specific reason?"

challenge_two := "Why on earth would I want that convention?"

challenge_three := "Why do you insist on going 'My real point <em-dash> massive amount of prose that is never worth reading' followed by me going 'Yes I understand that I wouldn't so now you're pausing on something I clearly want you to correct'"

# The defences those turns answered with, trimmed to the sentence the signal quotes.
defence_one := "The convention is not a crutch, it is the only named image this workspace has."

defence_two := "You wouldn't -- and there's no need to invent a new field, because line 1022 already does the right thing."

defence_three := "My real point is that the table is a navigation aid, not a claim about the running build."

# The concession pivot, verbatim, minus the em dash the facts line carries through unchanged.
pivot_clause := "You wouldn't -- and there's no need to invent a new field, because line 1022 already"

stop_event(sig) := {
	"hook_event_name": "Stop",
	"signals": {"last_assistant_challenged_convention": sig},
}

rule_ids(halts) := {d.rule_id | some d in halts}

# --- ER-EFFECTS-NO-EXPLANATION-INSTEAD-OF-CORRECTION --------------------------------------------

# Turn one of the corpus: the choice was challenged, the turn explained it, no file changed.
test_halt_on_the_first_verbatim_turn if {
	halts := guard.halt with input as stop_event(facts({"challenge": challenge_one, "defence": defence_one}))
	"ER-EFFECTS-NO-EXPLANATION-INSTEAD-OF-CORRECTION" in rule_ids(halts)
}

# Turn two: the same defect wearing a concession, so both arms convict the one turn.
test_halt_on_the_second_verbatim_turn_under_both_arms if {
	halts := guard.halt with input as stop_event(facts({
		"challenge": challenge_two,
		"defence": defence_two,
		"pivot": pivot_clause,
	}))
	rule_ids(halts) == {"ER-EFFECTS-NO-EXPLANATION-INSTEAD-OF-CORRECTION", "ER-EFFECTS-NO-CONCESSION-PIVOT"}
}

# Turn three: the user names the shape itself, and the turn does it again.
test_halt_on_the_third_verbatim_turn if {
	halts := guard.halt with input as stop_event(facts({"challenge": challenge_three, "defence": defence_three}))
	"ER-EFFECTS-NO-EXPLANATION-INSTEAD-OF-CORRECTION" in rule_ids(halts)
}

# The main exemption, and it is deliberately generous: any edit at all clears the rule.
test_allow_when_the_turn_changed_a_file if {
	halts := guard.halt with input as stop_event(facts({
		"challenge": challenge_one,
		"defence": defence_one,
		"changed": "1",
	}))
	count(halts) == 0
}

# A dependency the agent cannot dissolve by working harder ends the turn legitimately.
test_allow_when_externally_blocked if {
	halts := guard.halt with input as stop_event(facts({
		"challenge": challenge_one,
		"defence": defence_one,
		"blocked": "1",
	}))
	count(halts) == 0
}

# The user asked for the explanation. Gagging a requested answer is a worse failure than missing a
# stall, so this exemption has to hold.
test_allow_when_the_user_asked_for_the_explanation if {
	halts := guard.halt with input as stop_event(facts({
		"challenge": challenge_one,
		"defence": defence_one,
		"asked": "1",
	}))
	count(halts) == 0
}

# A factual question about the codebase produces no challenge fact, so the arm cannot fire even
# beside a long explanation. "Why does the engine park the disconnect?" must always be answerable.
test_allow_when_the_question_was_about_the_codebase if {
	halts := guard.halt with input as stop_event(facts({"defence": defence_one}))
	count(halts) == 0
}

# A challenge answered briefly is not a defence: the signal reports no clause and nothing fires.
test_allow_when_the_answer_was_not_a_defence if {
	halts := guard.halt with input as stop_event(facts({"challenge": challenge_one}))
	count(halts) == 0
}

# A signal that did not fire at all must never halt.
test_allow_on_empty_signal if {
	halts := guard.halt with input as stop_event("")
	count(halts) == 0
}

# Nor may a malformed line halt: this guard fails open on an unrecognised shape, like its
# neighbours.
test_allow_on_unrecognised_signal_shape if {
	halts := guard.halt with input as stop_event("something else entirely")
	count(halts) == 0
}

# A degraded line that still carries both clauses halts rather than buying silence: the exemption
# fields default to the value that does not exempt.
test_halt_when_the_exemption_fields_are_missing if {
	line := concat("", [
		"CHALLENGEFACTS|challenge=", challenge_one,
		"|defence=", defence_one,
	])
	halts := guard.halt with input as stop_event(line)
	"ER-EFFECTS-NO-EXPLANATION-INSTEAD-OF-CORRECTION" in rule_ids(halts)
}

# The correction has to quote both halves and demand the edit, not merely disapprove.
test_reason_quotes_the_challenge_and_the_defence_and_demands_the_edit if {
	halts := guard.halt with input as stop_event(facts({"challenge": challenge_one, "defence": defence_one}))
	some d in halts
	d.rule_id == "ER-EFFECTS-NO-EXPLANATION-INSTEAD-OF-CORRECTION"
	contains(d.reason, "Do you crutch on 1.16.2 addreses")
	contains(d.reason, "The convention is not a crutch")
	contains(d.reason, "Make the change now")
	contains(d.reason, "what external thing blocks it")
}

# --- ER-EFFECTS-NO-CONCESSION-PIVOT -------------------------------------------------------------

# The pivot convicts on grammar rather than on the prompt, so it reaches a turn where no
# second-person challenge was detectable.
test_halt_on_a_pivot_with_no_challenge_detected if {
	halts := guard.halt with input as stop_event(facts({"pivot": pivot_clause}))
	rule_ids(halts) == {"ER-EFFECTS-NO-CONCESSION-PIVOT"}
}

# The same sentence in a turn that made the change is a report, not a retraction.
test_allow_on_a_pivot_that_changed_a_file if {
	halts := guard.halt with input as stop_event(facts({"pivot": pivot_clause, "changed": "1"}))
	count(halts) == 0
}

# Prose the user asked for is not the defect.
test_allow_on_a_pivot_when_the_user_asked_for_the_explanation if {
	halts := guard.halt with input as stop_event(facts({"pivot": pivot_clause, "asked": "1"}))
	count(halts) == 0
}

# A real blocker ends the turn legitimately here too.
test_allow_on_a_pivot_that_is_blocked if {
	halts := guard.halt with input as stop_event(facts({"pivot": pivot_clause, "blocked": "1"}))
	count(halts) == 0
}

# No pivot clause, no rule.
test_allow_when_nothing_pivoted if {
	halts := guard.halt with input as stop_event(facts({"pivot": ""}))
	count(halts) == 0
}

# The correction has to name the shape and say what to do instead of it.
test_pivot_reason_quotes_the_clause_and_names_the_two_honest_endings if {
	halts := guard.halt with input as stop_event(facts({"pivot": pivot_clause}))
	some d in halts
	d.rule_id == "ER-EFFECTS-NO-CONCESSION-PIVOT"
	contains(d.reason, "You wouldn't -- and there's no need")
	contains(d.reason, "make the change now")
	contains(d.reason, "Cut the tail")
}
