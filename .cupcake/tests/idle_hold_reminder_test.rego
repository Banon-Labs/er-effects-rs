# OPA unit tests for idle_hold_reminder (the UserPromptSubmit standing reminder + interlock backstop
# that catches an interrupted-turn idle slip the Stop halt could not).
# Run with:
#   opa test .cupcake/system/commands.rego \
#     .cupcake/policies/claude/idle_hold_reminder.rego \
#     .cupcake/tests/idle_hold_reminder_test.rego
package cupcake.policies.claude.idle_hold_reminder_test

import rego.v1

import data.cupcake.policies.claude.idle_hold_reminder as guard

ups_event(sig) := {
	"hook_event_name": "UserPromptSubmit",
	"signals": {"last_assistant_idle_hold": sig},
}

has_interlock(ctxs) if {
	some c in ctxs
	startswith(c, "INTERLOCK TRIPPED:")
}

has_standing_reminder(ctxs) if {
	some c in ctxs
	startswith(c, "NEVER IDLE-HOLD:")
}

has_verbose_interlock(ctxs) if {
	some c in ctxs
	startswith(c, "INTERLOCK TRIPPED:")
	contains(c, "long message")
}

# The standing reminder is injected on every UserPromptSubmit, even with a clean prior turn.
test_standing_reminder_always_present if {
	ctxs := guard.add_context with input as ups_event("")
	has_standing_reminder(ctxs)
}

# Clean prior turn -> no interlock directive (only the standing reminder).
test_no_interlock_on_clean_prior_turn if {
	ctxs := guard.add_context with input as ups_event("")
	not has_interlock(ctxs)
}

# A tagged prior-turn idle slip trips the interlock backstop on the next prompt.
test_interlock_on_idlehold_tagged_signal if {
	ctxs := guard.add_context with input as ups_event("IDLEHOLD:standing by")
	has_interlock(ctxs)
}

# A bare/untagged non-empty signal trips the interlock too.
test_interlock_on_bare_string_signal if {
	ctxs := guard.add_context with input as ups_event("I'm holding")
	has_interlock(ctxs)
}

# Object-shaped signal ({output: ...}) trips the interlock too.
test_interlock_on_object_signal if {
	ctxs := guard.add_context with input as {
		"hook_event_name": "UserPromptSubmit",
		"signals": {"last_assistant_idle_hold": {"output": "IDLEHOLD:holding for", "exit_code": 0}},
	}
	has_interlock(ctxs)
}

# A VERBOSEPAUSE tag trips the distinct verbose-pause interlock backstop.
test_verbose_interlock_on_verbosepause_signal if {
	ctxs := guard.add_context with input as ups_event("VERBOSEPAUSE:612")
	has_verbose_interlock(ctxs)
}

# The standing reminder still rides alongside a VERBOSEPAUSE interlock.
test_standing_reminder_with_verbosepause if {
	ctxs := guard.add_context with input as ups_event("VERBOSEPAUSE:612")
	has_standing_reminder(ctxs)
}

# A VERBOSEPAUSE tag must NOT trip the idle-hold interlock text (no phrase fallback).
test_verbosepause_not_idlehold_interlock if {
	ctxs := guard.add_context with input as ups_event("VERBOSEPAUSE:612")
	every c in ctxs {
		not contains(c, "unjustified hold/idle")
	}
}

# The standing reminder now carries the terse-when-blocked tightening.
test_standing_reminder_mentions_terse_when_blocked if {
	ctxs := guard.add_context with input as ups_event("")
	some c in ctxs
	startswith(c, "NEVER IDLE-HOLD:")
	contains(c, "SHORT")
}

# Whitespace-only signal is treated as clean -> no interlock.
test_no_interlock_on_whitespace_signal if {
	ctxs := guard.add_context with input as ups_event("   \n")
	not has_interlock(ctxs)
}

# The interlock only applies to UserPromptSubmit, not other events carrying the signal.
test_no_interlock_on_non_ups_event if {
	ctxs := guard.add_context with input as {
		"hook_event_name": "Stop",
		"signals": {"last_assistant_idle_hold": "IDLEHOLD:standing by"},
	}
	count(ctxs) == 0
}
