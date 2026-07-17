# OPA unit tests for no_authority_agreement (the Stop-event halt on banned authority-coded agreement).
# Run with:
#   opa test .cupcake/system/commands.rego \
#     .cupcake/policies/claude/no_authority_agreement.rego \
#     .cupcake/tests/no_authority_agreement_test.rego
package cupcake.policies.claude.no_authority_agreement_test

import rego.v1

import data.cupcake.policies.claude.no_authority_agreement as guard

stop_event(sig) := {
	"hook_event_name": "Stop",
	"signals": {"last_assistant_authority_agreement": sig},
}

stop_event_object_signal(sig) := {
	"hook_event_name": "Stop",
	"signals": {"last_assistant_authority_agreement": {"output": sig, "exit_code": 0}},
}

rule_ids(halts) := {d.rule_id | some d in halts}

# A banned phrase in the last message halts the turn-end (string-shaped signal).
test_halt_on_banned_phrase_string_signal if {
	halts := guard.halt with input as stop_event("You're right")
	"ER-EFFECTS-NO-AUTHORITY-AGREEMENT" in rule_ids(halts)
}

# Object-shaped signal ({output: ...}) is handled too.
test_halt_on_banned_phrase_object_signal if {
	halts := guard.halt with input as stop_event_object_signal("Exactly,")
	"ER-EFFECTS-NO-AUTHORITY-AGREEMENT" in rule_ids(halts)
}

# No banned phrase -> no halt (empty signal).
test_no_halt_on_clean_message if {
	halts := guard.halt with input as stop_event("")
	count(halts) == 0
}

# Whitespace-only signal is treated as clean.
test_no_halt_on_whitespace_signal if {
	halts := guard.halt with input as stop_event("   \n")
	count(halts) == 0
}

# The halt only applies to Stop events, not other events that might carry the signal.
test_no_halt_on_non_stop_event if {
	halts := guard.halt with input as {
		"hook_event_name": "PreToolUse",
		"signals": {"last_assistant_authority_agreement": "You're right"},
	}
	count(halts) == 0
}
