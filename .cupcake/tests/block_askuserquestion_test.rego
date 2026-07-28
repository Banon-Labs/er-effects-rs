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

# --- (a) AskUserQuestion is DENIED unconditionally ---------------------------

# A typical multiple-choice questionnaire is blocked.
test_deny_askuserquestion_basic if {
	denied(ask_event([{
		"question": "Which direction?",
		"header": "Direction",
		"options": [{"label": "A"}, {"label": "B"}],
	}]))
}

# Empty/degenerate payloads still deny (the tool itself is the trigger).
test_deny_askuserquestion_empty_questions if {
	denied(ask_event([]))
}

test_deny_askuserquestion_no_tool_input if {
	denied({"hook_event_name": "PreToolUse", "tool_name": "AskUserQuestion"})
}

# --- (b) Negatives: things that must NOT be denied ---------------------------

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
