# OPA unit tests for block_askuserquestion.
#
# Not loaded by the cupcake engine (which scans .cupcake/policies/<harness>/
# and .cupcake/system/ only). Run with:
#   opa test .cupcake/policies/claude/block_askuserquestion.rego \
#            .cupcake/tests/block_askuserquestion_test.rego
# End-to-end engine coverage lives in scripts/test-cupcake-policies.py.
package cupcake.policies.claude.block_askuserquestion_test

import rego.v1

import data.cupcake.policies.claude.block_askuserquestion as guard

ask_event(questions) := {
	"hook_event_name": "PreToolUse",
	"tool_name": "AskUserQuestion",
	"tool_input": {"questions": questions},
}

rule_ids(denials) := {d.rule_id | some d in denials}

denied(event) if {
	denials := guard.deny with input as event
	"ER-EFFECTS-BLOCK-ASKUSERQUESTION" in rule_ids(denials)
}

# --- (a) AskUserQuestion is NEVER hard-denied by this file any more -----------------------------
# 2026-08-15: the prior unconditional deny fired outside /goal work (user-reported false positive:
# "That cupcake policy is not triggered correctly"), and re-investigation found no reliable goal-active
# signal to gate a conditional deny on (see METADATA). The advisory now lives entirely in the companion
# block_askuserquestion_reminder.rego (UserPromptSubmit/add_context) -- see that file's tests.

# A typical multiple-choice questionnaire now proceeds (not denied).
test_allow_askuserquestion_basic if {
	not denied(ask_event([{
		"question": "Which direction?",
		"header": "Direction",
		"options": [{"label": "A"}, {"label": "B"}],
	}]))
}

# Empty/degenerate payloads also proceed.
test_allow_askuserquestion_empty_questions if {
	not denied(ask_event([]))
}

test_allow_askuserquestion_no_tool_input if {
	not denied({"hook_event_name": "PreToolUse", "tool_name": "AskUserQuestion"})
}

# A signal shaped like a future goal-active marker must not resurrect a deny: this file does not read
# any such signal today (no reliable one exists -- see METADATA), so it must stay inert even if a caller
# happens to pass one.
test_allow_askuserquestion_even_with_goal_active_like_signal if {
	not denied({
		"hook_event_name": "PreToolUse",
		"tool_name": "AskUserQuestion",
		"tool_input": {"questions": []},
		"signals": {"goal_active": "1"},
	})
}

# This file is advisory-elsewhere-only now: it must never emit any deny decision, period.
test_no_deny_verb_ever if {
	count(guard.deny) == 0
}

# --- (b) Negatives: things that must NOT be denied (unchanged scope) -----------------------------

# Other tools are out of scope for this questionnaire guard.
test_allow_bash_tool if {
	not denied({
		"hook_event_name": "PreToolUse",
		"tool_name": "Bash",
		"tool_input": {"command": "git status"},
	})
}

test_allow_write_tool if {
	not denied({
		"hook_event_name": "PreToolUse",
		"tool_name": "Write",
		"tool_input": {"file_path": "/tmp/x", "content": "hi"},
	})
}

# A tool whose name merely CONTAINS the string must not be denied (exact match).
test_allow_similar_tool_name if {
	not denied({
		"hook_event_name": "PreToolUse",
		"tool_name": "AskUserQuestionHelper",
		"tool_input": {},
	})
}

# Non-PreToolUse events are out of scope.
test_allow_non_pretooluse_event if {
	not denied({
		"hook_event_name": "PostToolUse",
		"tool_name": "AskUserQuestion",
		"tool_input": {"questions": []},
	})
}
