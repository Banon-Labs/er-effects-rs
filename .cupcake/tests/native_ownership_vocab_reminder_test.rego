# OPA unit tests for native_ownership_vocab_reminder.
# Run with:
#   opa test .cupcake/system/commands.rego \
#     .cupcake/policies/claude/native_ownership_vocab_reminder.rego \
#     .cupcake/tests/native_ownership_vocab_reminder_test.rego
package cupcake.policies.claude.native_ownership_vocab_reminder_test

import rego.v1

import data.cupcake.policies.claude.native_ownership_vocab_reminder as guard

ups_event(sig) := {
	"hook_event_name": "UserPromptSubmit",
	"signals": {"last_assistant_native_ownership_vocab": sig},
}

has_caution(ctxs) if {
	some c in ctxs
	contains(c, "NATIVE OWNERSHIP VOCABULARY CAUTION")
}

# Clean prior turn -> no advisory context.
test_no_caution_on_clean_signal if {
	ctxs := guard.add_context with input as ups_event("")
	not has_caution(ctxs)
}

# Tagged signal -> advisory reminder is injected.
test_caution_on_tagged_signal if {
	ctxs := guard.add_context with input as ups_event("NATIVEVOCAB:pulse,pump")
	has_caution(ctxs)
	some c in ctxs
	contains(c, "pulse,pump")
	contains(c, "advisory, non-blocking")
	contains(c, "You are obligated to read")
}

# Object-shaped signal ({output: ...}) is accepted, matching Cupcake signal conventions.
test_caution_on_object_signal if {
	ctxs := guard.add_context with input as {
		"hook_event_name": "UserPromptSubmit",
		"signals": {"last_assistant_native_ownership_vocab": {"output": "NATIVEVOCAB:direct-field-adjustment", "exit_code": 0}},
	}
	has_caution(ctxs)
}

# Whitespace-only signal is clean.
test_no_caution_on_whitespace_signal if {
	ctxs := guard.add_context with input as ups_event("  \n")
	not has_caution(ctxs)
}

# Reminder only applies on UserPromptSubmit. It must not halt/block Stop or tool events.
test_no_caution_on_non_prompt_event if {
	ctxs := guard.add_context with input as {
		"hook_event_name": "Stop",
		"signals": {"last_assistant_native_ownership_vocab": "NATIVEVOCAB:pump"},
	}
	count(ctxs) == 0
}

# This policy is advisory-only: it must never emit halt decisions.
test_no_halt_verb if {
	count(guard.halt) == 0
}
