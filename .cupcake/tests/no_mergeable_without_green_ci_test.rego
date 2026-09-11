package cupcake.policies.claude.no_mergeable_without_green_ci_test

import data.cupcake.policies.claude.no_mergeable_without_green_ci as policy

# The instance that prompted the rule: the word written while `check` was in_progress.
test_pending_ci_with_the_claim_halts if {
	count(policy.halt) == 1 with input as stop_input("MERGEABLECLAIM:1:PENDING")
}

test_failing_ci_with_the_claim_halts if {
	count(policy.halt) == 1 with input as stop_input("MERGEABLECLAIM:1:FAIL")
}

# An unmeasured verdict is not a passing one.
test_unknown_ci_with_the_claim_halts if {
	count(policy.halt) == 1 with input as stop_input("MERGEABLECLAIM:1:UNKNOWN")
}

test_no_pr_with_the_claim_halts if {
	count(policy.halt) == 1 with input as stop_input("MERGEABLECLAIM:1:NOPR")
}

# The one honest use of the word.
test_passing_ci_with_the_claim_is_allowed if {
	count(policy.halt) == 0 with input as stop_input("MERGEABLECLAIM:1:PASS")
}

# A turn that never made the claim is untouched, whatever CI says.
test_no_claim_is_untouched_on_red_ci if {
	count(policy.halt) == 0 with input as stop_input("MERGEABLECLAIM:0:FAIL")
}

test_no_claim_is_untouched_on_pending_ci if {
	count(policy.halt) == 0 with input as stop_input("MERGEABLECLAIM:0:PENDING")
}

# No-fabrication: an absent, untagged or truncated signal asserts nothing rather than inventing a
# verdict, so a broken signal can never halt a turn on a claim that was not made.
test_absent_signal_asserts_nothing if {
	count(policy.halt) == 0 with input as {"hook_event_name": "Stop", "signals": {}}
}

test_untagged_signal_asserts_nothing if {
	count(policy.halt) == 0 with input as stop_input("nonsense")
}

test_truncated_signal_asserts_nothing if {
	count(policy.halt) == 0 with input as stop_input("MERGEABLECLAIM:1")
}

# Other events are not this policy's business.
test_other_events_are_untouched if {
	count(policy.halt) == 0 with input as {
		"hook_event_name": "PreToolUse",
		"signals": {"last_assistant_mergeable_claim": "MERGEABLECLAIM:1:FAIL"},
	}
}

# The reason is the user's line and nothing else -- an appended explanation would be the prose the
# directive exists to stop.
test_the_reason_is_exactly_the_one_line if {
	some decision in policy.halt with input as stop_input("MERGEABLECLAIM:1:PENDING")
	decision.reason == "You're a fucking moron."
}

stop_input(signal) := {
	"hook_event_name": "Stop",
	"signals": {"last_assistant_mergeable_claim": signal},
}
