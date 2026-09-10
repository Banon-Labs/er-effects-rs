# OPA unit tests for no_diagnosis_without_fix (the Stop-event halt on a diagnosis nobody fixed).
# Run with:
#   opa test .cupcake/system/commands.rego \
#     .cupcake/policies/claude/no_diagnosis_without_fix.rego \
#     .cupcake/tests/no_diagnosis_without_fix_test.rego
package cupcake.policies.claude.no_diagnosis_without_fix_test

import rego.v1

import data.cupcake.policies.claude.no_diagnosis_without_fix as guard

# The four-field line the signal emitted before the promissory closer was added. Kept as-is so the
# diagnosis tests below still exercise a legacy-shaped signal, which must never fire the new arm.
facts(diagnosis, fixed, asked, blocked) := concat("", [
	"DIAGFACTS|diagnosis=", diagnosis,
	"|fixed=", fixed,
	"|asked=", asked,
	"|blocked=", blocked,
])

full_facts(diagnosis, fixed, asked, blocked, promise, edited) := concat("", [
	"DIAGFACTS|diagnosis=", diagnosis,
	"|fixed=", fixed,
	"|asked=", asked,
	"|blocked=", blocked,
	"|promise=", promise,
	"|edited=", edited,
])

# The closing sentence that prompted the rule, verbatim from the transcript.
closer := "Fixing both: bypass the union so the naked capture is the detour entry, and pass the adopted menu object to invade instead of the synthesized box."

closer_facts(promise, edited) := full_facts("", "0", "0", "0", promise, edited)

# The full line the signal emits once the zero-information-stop fields are included. Written as an
# override map so a new fact does not have to be threaded through every call site.
handback_line(o) := concat("", [
	"DIAGFACTS|diagnosis=|fixed=0|asked=0|blocked=0|promise=|edited=0",
	"|handback=", object.get(o, "handback", ""),
	"|handbackkind=", object.get(o, "kind", "b"),
	"|userneed=", object.get(o, "userneed", "0"),
	"|didwork=", object.get(o, "didwork", "0"),
	"|extblocked=", object.get(o, "extblocked", "0"),
	"|carried=", object.get(o, "carried", "0"),
])

# The closing message that prompted the rule, verbatim, minus the em dash the facts line collapses.
zero_info_closer := "Nothing -- the ball is in my court. Rebuilding and relaunching now; the one thing I'll need from you afterwards is a single use of the item."

stop_event(sig) := {
	"hook_event_name": "Stop",
	"signals": {"last_assistant_diagnosis_without_fix": sig},
}

rule_ids(halts) := {d.rule_id | some d in halts}

# The verbatim shape that prompted the rule: a real defect named, no file changed.
test_halt_when_a_diagnosis_changed_no_file if {
	halts := guard.halt with input as stop_event(facts("The real defect is the banner phrase.", "0", "0", "0"))
	"ER-EFFECTS-NO-DIAGNOSIS-WITHOUT-FIX" in rule_ids(halts)
}

# The same sentence followed by an edit is a fix being reported, which is the wanted behaviour.
test_allow_when_the_turn_edited_a_file if {
	halts := guard.halt with input as stop_event(facts("The real defect is the banner phrase.", "1", "0", "0"))
	count(halts) == 0
}

# Answering a question is the deliverable. This exemption is broad on purpose and must stay so:
# gagging an explanation the user asked for is a worse failure than missing a stall.
test_allow_when_the_user_asked if {
	halts := guard.halt with input as stop_event(facts("The bug is in the restart path.", "0", "1", "0"))
	count(halts) == 0
}

# A diagnosis that cannot be acted on yet is not a stall.
test_allow_when_genuinely_blocked if {
	halts := guard.halt with input as stop_event(facts("The cause is the stale prologue.", "0", "0", "1"))
	count(halts) == 0
}

# No diagnosis, no rule. An ordinary turn must pass untouched.
test_allow_when_no_diagnosis_was_made if {
	halts := guard.halt with input as stop_event(facts("", "0", "0", "0"))
	count(halts) == 0
}

# A signal that did not fire at all (empty output) must never halt.
test_allow_on_empty_signal if {
	halts := guard.halt with input as stop_event("")
	count(halts) == 0
}

# Nor may a malformed line halt: this guard fails open, like its neighbours.
test_allow_on_unrecognised_signal_shape if {
	halts := guard.halt with input as stop_event("something else entirely")
	count(halts) == 0
}

# The correction has to say what to do, not merely that something was wrong.
test_reason_demands_the_edit_and_offers_the_blocked_escape if {
	halts := guard.halt with input as stop_event(facts("The fix is a latch.", "0", "0", "0"))
	some d in halts
	contains(d.reason, "Make the edit now")
	contains(d.reason, "what blocks it")
}

# The offending clause is quoted back, so the correction names the sentence it is about.
test_reason_quotes_the_clause if {
	halts := guard.halt with input as stop_event(facts("The bug is in the restart path.", "0", "0", "0"))
	some d in halts
	contains(d.reason, "The bug is in the restart path.")
}

# --- ER-EFFECTS-NO-PROMISSORY-CLOSER -----------------------------------------------------------

# (a) The verbatim shape: a fix announced in the present participle, and no file written.
test_halt_on_a_promissory_closer_that_wrote_nothing if {
	halts := guard.halt with input as stop_event(closer_facts(closer, "0"))
	"ER-EFFECTS-NO-PROMISSORY-CLOSER" in rule_ids(halts)
}

# (b) The same sentence in a turn that edited a file is a truthful report of work just done.
test_allow_when_the_promissory_turn_edited_a_file if {
	halts := guard.halt with input as stop_event(closer_facts(closer, "1"))
	count(halts) == 0
}

# (c) A gerund mid-turn is ordinary narration: the signal reads only the closing prose, so it
# reports no promise and the arm cannot fire.
test_allow_on_a_mid_turn_gerund_with_an_ordinary_closing if {
	halts := guard.halt with input as stop_event(closer_facts("", "0"))
	count(halts) == 0
}

# (d) An ordinary closing report -- no defect named, no work announced -- must pass untouched.
test_allow_on_an_ordinary_closing_report if {
	halts := guard.halt with input as stop_event(full_facts("", "0", "0", "0", "", "0"))
	count(halts) == 0
}

# The other closing shapes the signal recognises, each as its own case so a regression names itself.
test_halt_on_now_fixing if {
	halts := guard.halt with input as stop_event(closer_facts("Now fixing the union bypass", "0"))
	"ER-EFFECTS-NO-PROMISSORY-CLOSER" in rule_ids(halts)
}

test_halt_on_implementing_next if {
	halts := guard.halt with input as stop_event(closer_facts("Implementing the latch next.", "0"))
	"ER-EFFECTS-NO-PROMISSORY-CLOSER" in rule_ids(halts)
}

test_halt_on_next_i if {
	halts := guard.halt with input as stop_event(closer_facts("Next I bypass the union and pass the adopted object.", "0"))
	"ER-EFFECTS-NO-PROMISSORY-CLOSER" in rule_ids(halts)
}

# A real dependency is a real dependency for both arms.
test_allow_when_the_promissory_turn_is_blocked if {
	halts := guard.halt with input as stop_event(full_facts("", "0", "0", "1", closer, "0"))
	count(halts) == 0
}

# `asked` exempts the diagnosis arm and must not exempt this one: the measured instance answered a
# question correctly and closed on "Fixing that now." with no edit.
test_halt_on_a_promissory_closer_even_when_the_user_asked if {
	halts := guard.halt with input as stop_event(full_facts("", "0", "1", "0", "Fixing that now.", "0"))
	"ER-EFFECTS-NO-PROMISSORY-CLOSER" in rule_ids(halts)
}

# A line missing the `edited` field beside a non-empty promise is degraded, not old: it halts.
test_halt_when_the_edited_field_is_missing if {
	line := concat("", ["DIAGFACTS|diagnosis=|fixed=0|asked=0|blocked=0|promise=", closer])
	halts := guard.halt with input as stop_event(line)
	"ER-EFFECTS-NO-PROMISSORY-CLOSER" in rule_ids(halts)
}

# The four-field line a pre-2026-09-09 signal emits carries no promise, so the new arm stays silent
# rather than firing on every turn a stale signal reports.
test_allow_on_a_legacy_four_field_line if {
	halts := guard.halt with input as stop_event(facts("", "0", "0", "0"))
	count(halts) == 0
}

# The correction has to quote the sentence and demand the edit, not merely disapprove.
test_promissory_reason_quotes_the_clause_and_demands_the_edit if {
	halts := guard.halt with input as stop_event(closer_facts(closer, "0"))
	some d in halts
	d.rule_id == "ER-EFFECTS-NO-PROMISSORY-CLOSER"
	contains(d.reason, "Fixing both: bypass the union")
	contains(d.reason, "Make the edit now")
	contains(d.reason, "what blocks it")
}

# One turn can carry both defects; each must be reported under its own id so they stay separately
# tunable.
test_both_arms_can_fire_on_one_turn if {
	halts := guard.halt with input as stop_event(full_facts("The real defect is the banner phrase.", "0", "0", "0", closer, "0"))
	rule_ids(halts) == {"ER-EFFECTS-NO-DIAGNOSIS-WITHOUT-FIX", "ER-EFFECTS-NO-PROMISSORY-CLOSER"}
}

# --- ER-EFFECTS-NO-PROMISSORY-CLOSER, the unread-evidence shape --------------------------------

# The closing line the signal emits for the unread-evidence shape, as an override map.
unread_line(o) := concat("", [
	"DIAGFACTS|diagnosis=|fixed=0|asked=0|blocked=", object.get(o, "blocked", "0"),
	"|promise=|edited=0|handback=|handbackkind=|userneed=", object.get(o, "userneed", "0"),
	"|didwork=1|extblocked=0|carried=0",
	"|unread=", object.get(o, "unread", ""),
	"|consulted=", object.get(o, "consulted", "0"),
	"|future=", object.get(o, "future", "0"),
])

# The verbatim instance, trimmed to the deferring clause.
unread_closer := "The next measurement is whether the DLL is using the menu object it now captures for the re-invade gate or still falling back to the scan -- which its own log answers, so I am reading that next."

# The evidence was on disk and the turn named it instead of reading it.
test_halt_on_an_unread_answer_that_was_already_on_disk if {
	halts := guard.halt with input as stop_event(unread_line({"unread": unread_closer}))
	"ER-EFFECTS-NO-PROMISSORY-CLOSER" in rule_ids(halts)
}

# The other spellings of the same deferral.
test_halt_on_the_remaining_question_is if {
	halts := guard.halt with input as stop_event(unread_line({"unread": "The remaining question is whether the scan still runs, and that will tell us which path fired."}))
	"ER-EFFECTS-NO-PROMISSORY-CLOSER" in rule_ids(halts)
}

test_halt_on_reading_that_next if {
	halts := guard.halt with input as stop_event(unread_line({"unread": "The log already says which branch took the fallback, so reading that next settles it."}))
	"ER-EFFECTS-NO-PROMISSORY-CLOSER" in rule_ids(halts)
}

# The turn opened the artifact it named: that is a read, and what follows is a report.
test_allow_when_the_turn_consulted_the_artifact if {
	halts := guard.halt with input as stop_event(unread_line({"unread": unread_closer, "consulted": "1"}))
	count(halts) == 0
}

# Evidence that does not exist yet cannot be read now. Deferring to the next run is a plan.
test_allow_when_the_evidence_does_not_exist_yet if {
	halts := guard.halt with input as stop_event(unread_line({"unread": "The next run's log will say whether the gate held.", "future": "1"}))
	count(halts) == 0
}

# An observation only the user can make is information the agent cannot fetch.
test_allow_when_the_named_step_needs_the_user if {
	halts := guard.halt with input as stop_event(unread_line({"unread": unread_closer, "userneed": "1"}))
	count(halts) == 0
}

# A genuine blocker exempts this shape as it does the others.
test_allow_when_the_unread_read_is_blocked if {
	halts := guard.halt with input as stop_event(unread_line({"unread": unread_closer, "blocked": "1"}))
	count(halts) == 0
}

# No deferral, no rule.
test_allow_when_nothing_was_deferred if {
	halts := guard.halt with input as stop_event(unread_line({"unread": ""}))
	count(halts) == 0
}

# The correction has to send the agent to the file, not merely disapprove.
test_unread_reason_quotes_the_clause_and_demands_the_read if {
	halts := guard.halt with input as stop_event(unread_line({"unread": unread_closer}))
	some d in halts
	d.rule_id == "ER-EFFECTS-NO-PROMISSORY-CLOSER"
	contains(d.reason, "which its own log answers")
	contains(d.reason, "Read it now, in this turn")
	contains(d.reason, "does not exist yet")
}

# --- ER-EFFECTS-NO-ZERO-INFORMATION-STOP -------------------------------------------------------

# The verbatim instance: the agent's own next actions announced as a closing line, and the only
# thing asked of the user is an in-game input the agent drives itself.
test_halt_on_the_verbatim_zero_information_stop if {
	halts := guard.halt with input as stop_event(handback_line({"handback": zero_info_closer, "kind": "b"}))
	"ER-EFFECTS-NO-ZERO-INFORMATION-STOP" in rule_ids(halts)
}

# Shape a on its own -- the user is told they need do nothing -- convicts only a turn that did no
# work. This is the pure zero-information stop.
test_halt_when_told_nothing_is_needed_and_nothing_was_done if {
	halts := guard.halt with input as stop_event(handback_line({"handback": "Nothing -- the ball is in my court.", "kind": "a"}))
	"ER-EFFECTS-NO-ZERO-INFORMATION-STOP" in rule_ids(halts)
}

# The same sentence closing a turn that delivered is the honest end of a finished task.
test_allow_when_nothing_is_needed_because_the_work_is_done if {
	halts := guard.halt with input as stop_event(handback_line({"handback": "Nothing on your side -- the gate is green.", "kind": "a", "didwork": "1"}))
	count(halts) == 0
}

# "Nothing." answering a question is the deliverable. Only the `a` shape reads `asked`, and only
# because the same word opening an unprompted closer is the defect.
test_allow_when_nothing_is_the_answer_to_a_question if {
	line := "DIAGFACTS|diagnosis=|fixed=0|asked=1|blocked=0|promise=|edited=0|handback=Nothing.|handbackkind=a|userneed=0|didwork=0|extblocked=0|carried=0|unread=|consulted=0|future=0"
	halts := guard.halt with input as stop_event(line)
	count(halts) == 0
}

# Shape b fires even when the turn did other work: it announced an action and did not take it.
test_halt_on_an_announced_own_next_action_despite_other_work if {
	halts := guard.halt with input as stop_event(handback_line({"handback": "Rebuilding and relaunching now.", "kind": "b", "didwork": "1"}))
	"ER-EFFECTS-NO-ZERO-INFORMATION-STOP" in rule_ids(halts)
}

# Shape c: an offer to do work the agent is already authorised to do.
test_halt_on_an_offer_to_do_authorised_work if {
	halts := guard.halt with input as stop_event(handback_line({"handback": "Say the word and I'll wire the caller check.", "kind": "c", "didwork": "1"}))
	"ER-EFFECTS-NO-ZERO-INFORMATION-STOP" in rule_ids(halts)
}

test_halt_on_want_me_to if {
	halts := guard.halt with input as stop_event(handback_line({"handback": "Want me to run the same probe against 1.17.1?", "kind": "c", "didwork": "1"}))
	"ER-EFFECTS-NO-ZERO-INFORMATION-STOP" in rule_ids(halts)
}

# An observation with no memory-read oracle is information only the user has, so the round trip is
# not empty and the turn is allowed to end on them.
test_allow_when_the_turn_asks_for_an_observation if {
	halts := guard.halt with input as stop_event(handback_line({"handback": "The game is up on PID 4711 -- tell me what you see on the loading screen.", "kind": "b", "userneed": "1", "didwork": "1"}))
	count(halts) == 0
}

# A subjective or external-only decision is the same exemption.
test_allow_when_the_decision_is_the_users if {
	halts := guard.halt with input as stop_event(handback_line({"handback": "Which do you prefer, the 30 m compass or the 3D distance?", "kind": "c", "userneed": "1"}))
	count(halts) == 0
}

# A genuine external blocker ends the turn legitimately.
test_allow_when_genuinely_blocked_from_acting if {
	halts := guard.halt with input as stop_event(handback_line({"handback": "I'll rerun it once you have logged in; the probe needs credentials I cannot supply.", "kind": "b", "didwork": "1", "extblocked": "1"}))
	count(halts) == 0
}

# No handback sentence, no rule.
test_allow_when_nothing_was_handed_back if {
	halts := guard.halt with input as stop_event(handback_line({"handback": "", "didwork": "1"}))
	count(halts) == 0
}

# A degraded line that still carries a clause halts rather than buying silence.
test_halt_when_the_handbackkind_field_is_missing if {
	line := "DIAGFACTS|diagnosis=|fixed=0|asked=0|blocked=0|promise=|edited=0|handback=Rebuilding and relaunching now."
	halts := guard.halt with input as stop_event(line)
	"ER-EFFECTS-NO-ZERO-INFORMATION-STOP" in rule_ids(halts)
}

# The six-field line the signal emitted between the promissory closer and this rule carries no
# handback, so this arm stays silent on it.
test_allow_on_a_six_field_line if {
	halts := guard.halt with input as stop_event(full_facts("", "0", "0", "0", "", "0"))
	count(halts) == 0
}

# The correction has to quote the sentence, demand the action, and name the legitimate endings.
test_handback_reason_quotes_the_clause_and_names_the_legitimate_endings if {
	halts := guard.halt with input as stop_event(handback_line({"handback": zero_info_closer, "kind": "b"}))
	some d in halts
	d.rule_id == "ER-EFFECTS-NO-ZERO-INFORMATION-STOP"
	contains(d.reason, "Rebuilding and relaunching now")
	contains(d.reason, "Take the action now")
	contains(d.reason, "tell me what you saw")
	contains(d.reason, "you drive every in-game input yourself")
}

# An offer to do something destructive or visible on the user's own desktop is an offer worth
# making: taking it unasked costs more than the round trip.
test_allow_on_an_offer_to_do_something_destructive if {
	halts := guard.halt with input as stop_event(handback_line({"handback": "The run I launched for the portrait measurement is still live -- say the word and I'll tear it down.", "kind": "", "didwork": "1"}))
	count(halts) == 0
}

# Work already in flight is not work handed back.
test_allow_while_a_subagent_is_still_running if {
	halts := guard.halt with input as stop_event(handback_line({"handback": "Nothing to do until they report.", "kind": "a", "carried": "1"}))
	count(halts) == 0
}

# The broad `blocked` fact -- which folds in "blocked on the user" -- must not exempt this arm, or
# naming the user as the dependency buys silence for work the agent owns.
test_halt_when_only_the_broad_blocked_fact_is_set if {
	line := "DIAGFACTS|diagnosis=|fixed=0|asked=0|blocked=1|promise=|edited=0|handback=I'll rebuild and relaunch it.|handbackkind=b|userneed=0|didwork=1|extblocked=0|carried=0"
	halts := guard.halt with input as stop_event(line)
	"ER-EFFECTS-NO-ZERO-INFORMATION-STOP" in rule_ids(halts)
}
