# OPA unit tests for idle_hold (the Stop-event halt on an unjustified idle/hold announcement).
# Run with:
#   opa test .cupcake/system/commands.rego \
#     .cupcake/policies/claude/idle_hold.rego \
#     .cupcake/tests/idle_hold_test.rego
package cupcake.policies.claude.idle_hold_test

import rego.v1

import data.cupcake.policies.claude.idle_hold as guard

stop_event(sig) := {
	"hook_event_name": "Stop",
	"signals": {"last_assistant_idle_hold": sig},
}

stop_event_object_signal(sig) := {
	"hook_event_name": "Stop",
	"signals": {"last_assistant_idle_hold": {"output": sig, "exit_code": 0}},
}

rule_ids(halts) := {d.rule_id | some d in halts}

# A tagged idle-hold hit halts the turn-end.
test_halt_on_idlehold_tagged_signal if {
	halts := guard.halt with input as stop_event("IDLEHOLD:standing by")
	"ER-EFFECTS-NO-IDLE-HOLD" in rule_ids(halts)
}

# Bare/untagged non-empty value (backward compat with crafted signal shapes) still halts.
test_halt_on_bare_string_signal if {
	halts := guard.halt with input as stop_event("I'm holding")
	"ER-EFFECTS-NO-IDLE-HOLD" in rule_ids(halts)
}

# Object-shaped signal ({output: ...}) is handled too.
test_halt_on_object_signal if {
	halts := guard.halt with input as stop_event_object_signal("IDLEHOLD:holding for")
	"ER-EFFECTS-NO-IDLE-HOLD" in rule_ids(halts)
}

# The halt reason nudges toward non-overlapping work and the justification escape hatch.
test_reason_mentions_nonoverlapping_and_justification if {
	halts := guard.halt with input as stop_event("IDLEHOLD:standing by")
	some d in halts
	contains(d.reason, "NON-OVERLAPPING")
	contains(d.reason, "would normally have done")
}

# No idle hold -> no halt (empty signal).
test_no_halt_on_clean_turn if {
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
		"signals": {"last_assistant_idle_hold": "IDLEHOLD:standing by"},
	}
	count(halts) == 0
}
