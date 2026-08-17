# OPA unit tests for block_askuserquestion_reminder.
# Run with:
#   opa test .cupcake/policies/claude/block_askuserquestion_reminder.rego \
#            .cupcake/tests/block_askuserquestion_reminder_test.rego
package cupcake.policies.claude.block_askuserquestion_reminder_test

import rego.v1

import data.cupcake.policies.claude.block_askuserquestion_reminder as guard

has_advisory(ctxs) if {
	some c in ctxs
	contains(c, "AskUserQuestion")
	contains(c, "advisory only")
}

# Every UserPromptSubmit turn gets the advisory reminder. This is the live "goal-active case still gets
# a signal" coverage for the advisory path: since no reliable goal-active signal exists (see
# block_askuserquestion.rego METADATA), the reminder fires unconditionally rather than being gated.
test_advisory_present_on_every_prompt if {
	ctxs := guard.add_context with input as {"hook_event_name": "UserPromptSubmit"}
	has_advisory(ctxs)
}

test_advisory_mentions_goal_work if {
	ctxs := guard.add_context with input as {"hook_event_name": "UserPromptSubmit"}
	some c in ctxs
	contains(c, "/goal work")
}

# Reminder only applies on UserPromptSubmit; it must not fire on other events (in particular, it must
# not be mistaken for a PreToolUse effect -- PreToolUse cannot carry add_context in this build anyway).
test_no_advisory_on_non_prompt_event if {
	ctxs := guard.add_context with input as {"hook_event_name": "PreToolUse", "tool_name": "AskUserQuestion"}
	count(ctxs) == 0
}

# This policy is advisory-only: it must never emit a deny decision.
test_no_deny_verb if {
	count(guard.deny) == 0
}
