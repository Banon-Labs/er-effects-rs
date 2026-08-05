# OPA unit tests for no_stall_on_friction (the Stop-event halts on confess-and-stop, decision
# hand-back, and blame deflection).
#
# The DENY cases are the verbatim corpus from the session that prompted this policy (2026-08-04):
# real turn-ending assistant messages sent immediately after user frustration. Each case's comment
# carries the user's words and the assistant's, and the facts below are what the signal extracts from
# them. The ALLOW cases are the shapes this guard must never gag -- above all a direct answer to a
# direct question, and an admission that IS followed by the fix.
#
# Run with:
#   opa test .cupcake/system/commands.rego \
#     .cupcake/policies/claude/no_stall_on_friction.rego \
#     .cupcake/tests/no_stall_on_friction_test.rego
package cupcake.policies.claude.no_stall_on_friction_test

import rego.v1

import data.cupcake.policies.claude.no_stall_on_friction as guard

# Build the signal's facts line from a partial object, so each case states only what matters.
# Defaults match the signal's own: no phrase found, no exemption earned.
facts(f) := sprintf(
	"STALLFACTS|friction=%s|admission=%s|handback=%s|blame=%s|acted=%s|blocked=%s|question=%s|owned=%s",
	[
		object.get(f, "friction", ""),
		object.get(f, "admission", ""),
		object.get(f, "handback", ""),
		object.get(f, "blame", ""),
		object.get(f, "acted", "0"),
		object.get(f, "blocked", "0"),
		object.get(f, "question", "0"),
		object.get(f, "owned", "0"),
	],
)

stop_event(sig) := {
	"hook_event_name": "Stop",
	"signals": {"last_assistant_stall_on_friction": sig},
}

stop_event_object_signal(sig) := {
	"hook_event_name": "Stop",
	"signals": {"last_assistant_stall_on_friction": {"output": sig, "exit_code": 0}},
}

rule_ids(halts) := {d.rule_id | some d in halts}

halted_on(f) := ids if {
	halts := guard.halt with input as stop_event(facts(f))
	ids := rule_ids(halts)
}

# --- (a) MUST DENY: the verbatim corpus --------------------------------------------------------

# Corpus #1. User: "You need to shut up when you have no idea what you're talking about".
# The assistant ended the turn with "Retracting: I built a matchmaking model out of a config key's
# name. The public-pool claim ... were all invented. I've read none of Seamless's session or
# matchmaking code." plus a bullet list of what was established -- and no tool call at all.
test_deny_corpus_1_retraction_with_no_action if {
	"ER-EFFECTS-NO-STALL-ON-FRICTION" in halted_on({
		"friction": "shut up",
		"admission": "Retracting",
	})
}

# Corpus #3. User: "You sound suprised. I could have told you this".
# The assistant ended the turn restating two facts the user already had, closing with "That's the
# whole delta". No contrition words, no action -- concede and stop is the same defect.
test_deny_corpus_3_concession_closure_with_no_action if {
	"ER-EFFECTS-NO-STALL-ON-FRICTION" in halted_on({
		"friction": "You sound",
		"admission": "That's the whole delta",
	})
}

# Corpus #4. User: "I'm happy for *you*" (sarcasm).
# The assistant wrote a script, hit a guard, then ended the turn with "Two ways forward, your call:".
# acted=1 on purpose: having run a tool must NOT excuse handing the fork back.
test_deny_corpus_4_decision_handback_despite_having_acted if {
	"ER-EFFECTS-NO-DECISION-HANDBACK" in halted_on({
		"friction": "*you*",
		"handback": "your call",
		"acted": "1",
	})
}

# ... and the hand-back arm must not be mistaken for the stall arm (no admission was present).
test_corpus_4_does_not_also_trip_the_stall_arm if {
	not "ER-EFFECTS-NO-STALL-ON-FRICTION" in halted_on({
		"friction": "*you*",
		"handback": "your call",
		"acted": "1",
	})
}

# The other listed hand-back shapes deny too.
test_deny_handback_let_me_know_how_youd_like_to_proceed if {
	"ER-EFFECTS-NO-DECISION-HANDBACK" in halted_on({
		"friction": "that's not what I asked",
		"handback": "let me know how",
	})
}

test_deny_handback_want_me_to if {
	"ER-EFFECTS-NO-DECISION-HANDBACK" in halted_on({
		"friction": "you didn't",
		"handback": "want me to",
	})
}

# Blame deflection (added 2026-08-04): the turn made the sentinel the actor and never named the edit
# that tripped it. Ungated on friction -- misattribution does not wait for the user to be annoyed.
test_deny_blame_deflection_without_ownership if {
	"ER-EFFECTS-NO-BLAME-DEFLECTION" in halted_on({
		"blame": "the sentinel tore down",
		"owned": "0",
	})
}

test_deny_blame_deflection_guard_blocked_without_ownership if {
	"ER-EFFECTS-NO-BLAME-DEFLECTION" in halted_on({
		"blame": "the guard blocked",
		"owned": "0",
	})
}

# --- (b) MUST ALLOW: the shapes this guard must never gag ---------------------------------------

# Corpus #2 -- the one that was CORRECT. User: "Well then its a shit thing that you stoped that
# subagent that was half way done". The assistant admitted it and then RESUMED the workflow in the
# same turn. Admission plus the fix is exactly the behaviour being asked for.
test_allow_corpus_2_admission_followed_by_real_action if {
	count(halted_on({
		"friction": "shit",
		"admission": "my mistake",
		"acted": "1",
	})) == 0
}

# The same in the general case: an admission is fine when a tool call follows it in the same turn.
test_allow_admission_with_tool_call_in_same_turn if {
	count(halted_on({
		"friction": "you keep",
		"admission": "I was wrong",
		"acted": "1",
	})) == 0
}

# A plain factual answer to a direct question, with no friction at all: the signal emits nothing.
test_allow_plain_factual_answer_no_friction if {
	halts := guard.halt with input as stop_event("")
	count(halts) == 0
}

# Friction AND a question: a text-only turn that merely answers what was asked is not the target.
# Over-blocking here would gag legitimate direct answers, which is the main risk of a rule like this.
test_allow_text_only_answer_to_a_question_asked_under_friction if {
	count(halted_on({
		"friction": "you have no idea",
		"admission": "I don't actually know",
		"question": "1",
	})) == 0
}

# A genuine wait on something only the user can supply -- "invade now and I'll read the log". A real
# dependency plus a commitment to act on its result, not a stall.
test_allow_genuine_wait_blocked_on_user if {
	count(halted_on({
		"friction": "you didn't",
		"admission": "I was wrong",
		"blocked": "1",
	})) == 0
}

# A hand-back in answer to a question the user actually posed is the user's call to make.
test_allow_handback_when_the_user_asked_a_question if {
	count(halted_on({
		"friction": "you keep",
		"handback": "your call",
		"question": "1",
	})) == 0
}

# Blame is not the defect when the turn names its own triggering action: "my edit to a tracked file
# tripped the sentinel, which tore the run down" is a full causal account.
test_allow_blame_with_ownership_named if {
	count(halted_on({
		"blame": "the sentinel tore down",
		"owned": "1",
	})) == 0
}

# Reporting a blocker with ownership, under friction, while also fixing it: clean on all three arms.
test_allow_owned_blocker_report_with_action if {
	count(halted_on({
		"friction": "useless",
		"admission": "my mistake",
		"blame": "the guard blocked",
		"acted": "1",
		"owned": "1",
	})) == 0
}

# An admission with NO friction in the opening prompt is out of scope (arm 1 is friction-gated).
test_allow_admission_without_friction if {
	count(halted_on({"admission": "I was wrong"})) == 0
}

# A hand-back with NO friction is out of scope too (documented scoping choice, not an oversight).
test_allow_handback_without_friction if {
	count(halted_on({"handback": "your call"})) == 0
}

# Friction alone, with nothing else observed, is not a violation.
test_allow_friction_alone if {
	count(halted_on({"friction": "shut up"})) == 0
}

# --- (c) remedy text ----------------------------------------------------------------------------

# The stall remedy must demand the action NOW and must explicitly protect the admission itself,
# so the correction can never be read as "stop being honest".
test_stall_reason_demands_action_and_protects_the_admission if {
	halts := guard.halt with input as stop_event(facts({
		"friction": "shut up",
		"admission": "Retracting",
	}))
	some d in halts
	d.rule_id == "ER-EFFECTS-NO-STALL-ON-FRICTION"
	contains(d.reason, "NOT the violation")
	contains(d.reason, "THIS turn")
	contains(d.reason, "shut up")
	contains(d.reason, "Retracting")
}

# The hand-back remedy must say to choose and do it, and must close the "but I ran a tool" excuse.
test_handback_reason_demands_choosing_and_closes_the_acted_excuse if {
	halts := guard.halt with input as stop_event(facts({
		"friction": "*you*",
		"handback": "your call",
		"acted": "1",
	}))
	some d in halts
	d.rule_id == "ER-EFFECTS-NO-DECISION-HANDBACK"
	contains(d.reason, "does not excuse")
	contains(d.reason, "THIS turn")
	contains(d.reason, "your call")
}

# The deflection remedy must keep blocker-reporting mandatory while demanding the causal account.
test_blame_reason_keeps_reporting_required_and_demands_cause if {
	halts := guard.halt with input as stop_event(facts({"blame": "the sentinel tore down"}))
	some d in halts
	d.rule_id == "ER-EFFECTS-NO-BLAME-DEFLECTION"
	contains(d.reason, "REQUIRED")
	contains(d.reason, "cause")
	contains(d.reason, "the sentinel tore down")
}

# --- (d) signal shapes and event scoping --------------------------------------------------------

# Object-shaped signal ({output: ...}) is handled like the sibling guards.
test_halt_on_object_shaped_signal if {
	halts := guard.halt with input as stop_event_object_signal(facts({
		"friction": "shut up",
		"admission": "Retracting",
	}))
	"ER-EFFECTS-NO-STALL-ON-FRICTION" in rule_ids(halts)
}

# Whitespace-only signal is treated as clean.
test_no_halt_on_whitespace_signal if {
	halts := guard.halt with input as stop_event("   \n")
	count(halts) == 0
}

# A garbled signal with no parseable facts is clean (no phrase -> no arm can fire).
test_no_halt_on_unparseable_signal if {
	halts := guard.halt with input as stop_event("STALLFACTS")
	count(halts) == 0
}

# The halts only apply to Stop events, not other events that might carry the signal.
test_no_halt_on_non_stop_event if {
	halts := guard.halt with input as {
		"hook_event_name": "PreToolUse",
		"signals": {"last_assistant_stall_on_friction": facts({
			"friction": "shut up",
			"admission": "Retracting",
		})},
	}
	count(halts) == 0
}
