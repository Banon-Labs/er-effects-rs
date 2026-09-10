# OPA unit tests for no_deferred_evidence_read (the Stop-event halt on evidence that already exists
# and was not read). Run with:
#   opa test .cupcake/policies/claude/no_deferred_evidence_read.rego \
#     .cupcake/tests/no_deferred_evidence_read_test.rego
#
# Every `deferral` string below is the clause the real signal actually emitted for that phrasing,
# copied out of a run of `.cupcake/signals/last_assistant_deferred_evidence_read.sh` over a fixture
# carrying it, so the quotes here are measurements rather than guesses. The phrase detection itself
# is proven in scripts/test-deferred-evidence-read-signal.py, which runs the shell; these tests own
# the conjunction that turns a facts line into a verdict.
package cupcake.policies.claude.no_deferred_evidence_read_test

import rego.v1

import data.cupcake.policies.claude.no_deferred_evidence_read as guard

facts(deferral, consulted, future, userneed, blocked, carried) := concat("", [
	"DEFERFACTS|deferral=", deferral,
	"|consulted=", consulted,
	"|future=", future,
	"|userneed=", userneed,
	"|blocked=", blocked,
	"|carried=", carried,
])

# The shape the guard exists for: a deferral, and every exemption absent.
unread(deferral) := facts(deferral, "0", "0", "0", "0", "0")

stop_event(sig) := {
	"hook_event_name": "Stop",
	"signals": {"last_assistant_deferred_evidence_read": sig},
}

# The sibling arm saw nothing, which is the state this rule is allowed to act in.
stop_event_with_sibling(sig, sibling) := {
	"hook_event_name": "Stop",
	"signals": {
		"last_assistant_deferred_evidence_read": sig,
		"last_assistant_diagnosis_without_fix": sibling,
	},
}

rule_ids(halts) := {d.rule_id | some d in halts}

halts_on(sig) if {
	halts := guard.halt with input as stop_event(sig)
	"ER-EFFECTS-NO-DEFERRED-EVIDENCE-READ" in rule_ids(halts)
}

# --- one test per catchable phrasing --------------------------------------------------------------

# The verbatim sentence that prompted the rule, minus the first-person tail that
# ER-EFFECTS-NO-UNEXECUTED-PROMISE already convicts.
test_halt_on_the_next_measurement_is if {
	halts_on(unread("...te or still falling back to the scan -- which its own log answers. ..."))
}

# "the next step is ..." pointed at a file that exists.
test_halt_on_the_next_step_is if {
	halts_on(unread("The next step is to read er-quickload-autoload-debug.log and see which branch ran."))
}

# "what remains is to ...", with an adjective between the determiner and the artifact noun.
test_halt_on_what_remains_is_to if {
	halts_on(unread("What remains is to read the newest er-invasion-warp log and see which branch the gate took."))
}

# "the remaining question is ..." beside the artifact that answers it.
test_halt_on_the_remaining_question_is if {
	halts_on(unread("The remaining question is which branch the gate took, and its own log records that."))
}

# "which its own log answers".
test_halt_on_which_its_own_log_answers if {
	halts_on(unread("The gate either adopted the object or fell back to the scan -- which its own log answers."))
}

# "the log will say", the modal spelling.
test_halt_on_the_log_will_say if {
	halts_on(unread("The gate either adopted the captured object or fell back to the scan, and er-quickload-autoload-debug.log will say which."))
}

# "that will tell us", including the spelling where a named artifact is the subject.
test_halt_on_that_will_tell_us if {
	halts_on(unread("the run artifact under er-me3-runs will tell us which one ran."))
}

# "its own log records that".
test_halt_on_its_own_log_records_that if {
	halts_on(unread("The scan ran on both attempts, and the trace already holds that."))
}

# "so I am reading that next".
test_halt_on_so_i_am_reading_that_next if {
	halts_on(unread("The adopted pointer is either used or ignored, and I am reading target/er-me3-runs/latest.log next."))
}

# "so that is what I check next".
test_halt_on_so_that_is_what_i_check_next if {
	halts_on(unread("The disassembly at 0x140d39df0 settles whether the branch is taken, so that is what I read next."))
}

# "reading that now", with a noun between the pronoun and the adverb.
test_halt_on_reading_that_now if {
	halts_on(unread("reading that log now."))
}

# --- one test per exemption -----------------------------------------------------------------------

# The turn opened the evidence and is reporting what it says. This is the wanted shape and must never
# be punished the same as skipping the read.
test_allow_when_the_turn_consulted_it if {
	sig := facts("The remaining question is which branch the gate took, and er-quickload-autoload-debug.log records that: the adopted pointer was null.", "1", "0", "0", "0", "0")
	halts := guard.halt with input as stop_event(sig)
	count(halts) == 0
}

# The evidence does not exist yet: a run has to write it first. Deferring to future evidence is
# legitimate and common in this repo.
test_allow_when_the_evidence_does_not_exist_yet if {
	sig := facts("the next run's log will say whether the adopted object was used.", "0", "1", "0", "0", "0")
	halts := guard.halt with input as stop_event(sig)
	count(halts) == 0
}

# The next step genuinely needs the user: an in-game observation with no memory-read oracle.
test_allow_when_the_read_needs_the_user if {
	sig := facts("...tion is which banner rendered, and only the screen capture holds that -- tell me what you saw ...", "0", "0", "1", "0", "0")
	halts := guard.halt with input as stop_event(sig)
	count(halts) == 0
}

# A real blocker was named. Evidence that cannot be opened was not skipped.
test_allow_when_a_blocker_was_stated if {
	sig := facts("The trace would settle it, but the dump is not readable without sudo.", "0", "0", "0", "1", "0")
	halts := guard.halt with input as stop_event(sig)
	count(halts) == 0
}

# Live background work is carrying the read.
test_allow_when_background_work_carries_it if {
	sig := facts("The subagent is walking the same log and its findings land in the next few minutes.", "0", "0", "0", "0", "1")
	halts := guard.halt with input as stop_event(sig)
	count(halts) == 0
}

# --- yielding to the sibling arm --------------------------------------------------------------------

# ER-EFFECTS-NO-PROMISSORY-CLOSER owns the deferrals its own signal recognises. A sentence it saw must
# collect one halt, not two, so a non-empty `unread` there silences this rule outright.
test_allow_when_the_sibling_arm_saw_the_same_sentence if {
	sibling := "DIAGFACTS|diagnosis=|promise=|unread=which its own log answers|consulted=0|future=0|userneed=0|blocked=0"
	halts := guard.halt with input as stop_event_with_sibling(unread("which its own log answers."), sibling)
	count(halts) == 0
}

# The sibling signal ran and recognised nothing, which is the state this rule covers.
test_halt_when_the_sibling_arm_saw_nothing if {
	sibling := "DIAGFACTS|diagnosis=|promise=|unread=|consulted=0|future=0|userneed=0|blocked=0"
	halts := guard.halt with input as stop_event_with_sibling(unread("reading that log now."), sibling)
	"ER-EFFECTS-NO-DEFERRED-EVIDENCE-READ" in rule_ids(halts)
}

# --- signal shape and routing -----------------------------------------------------------------------

# The object shape cupcake may hand back instead of a bare string must halt identically.
test_halt_on_object_shaped_signal if {
	halts := guard.halt with input as {
		"hook_event_name": "Stop",
		"signals": {"last_assistant_deferred_evidence_read": {
			"output": unread("reading that log now."),
			"exit_code": 0,
		}},
	}
	"ER-EFFECTS-NO-DEFERRED-EVIDENCE-READ" in rule_ids(halts)
}

# No deferral at all: an ordinary turn passes untouched. The signal emits nothing in this case.
test_allow_when_the_turn_deferred_to_nothing if {
	halts := guard.halt with input as stop_event("")
	count(halts) == 0
}

# A malformed line must not halt: this guard fails open on a signal it cannot parse, like its
# neighbours.
test_allow_on_unrecognised_signal_shape if {
	halts := guard.halt with input as stop_event("something else entirely")
	count(halts) == 0
}

# A line that carries a deferral but drops an exemption field halts rather than buying silence: a
# degraded or crafted signal must fail closed.
test_halt_when_an_exemption_field_is_missing if {
	halts := guard.halt with input as stop_event("DEFERFACTS|deferral=reading that log now.")
	"ER-EFFECTS-NO-DEFERRED-EVIDENCE-READ" in rule_ids(halts)
}

# The guard is Stop-only: a PreToolUse carrying the same signal must not halt.
test_allow_on_other_event if {
	halts := guard.halt with input as {
		"hook_event_name": "PreToolUse",
		"signals": {"last_assistant_deferred_evidence_read": unread("reading that log now.")},
	}
	count(halts) == 0
}

# The correction has to name the exits, not merely say the sentence was wrong: a guard that scolds
# without saying what passes teaches nothing.
test_reason_names_the_exits if {
	halts := guard.halt with input as stop_event(unread("reading that log now."))
	some d in halts
	contains(d.reason, "Read it now, in this turn")
	contains(d.reason, "does not exist yet")
	contains(d.reason, "only the user has")
}
