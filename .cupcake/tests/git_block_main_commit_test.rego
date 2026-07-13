# OPA unit tests for git_block_main_commit.
# Run with:
#   opa test .cupcake/system/commands.rego \
#     .cupcake/policies/claude/git_block_main_commit.rego \
#     .cupcake/tests/git_block_main_commit_test.rego
package cupcake.policies.claude.git_block_main_commit_test

import rego.v1

import data.cupcake.policies.claude.git_block_main_commit as guard

bash_event(cmd, branch) := {
	"hook_event_name": "PreToolUse",
	"tool_name": "Bash",
	"tool_input": {"command": cmd, "timeout": 30000},
	"signals": {"current_branch": branch},
}

bash_event_object_signal(cmd, branch) := {
	"hook_event_name": "PreToolUse",
	"tool_name": "Bash",
	"tool_input": {"command": cmd, "timeout": 30000},
	"signals": {"current_branch": {"output": branch, "exit_code": 0}},
}

bash_event_no_branch_signal(cmd) := {
	"hook_event_name": "PreToolUse",
	"tool_name": "Bash",
	"tool_input": {"command": cmd, "timeout": 30000},
	"signals": {},
}

rule_ids(denials) := {d.rule_id | some d in denials}

test_deny_git_commit_on_main_string_signal if {
	denials := guard.deny with input as bash_event("git commit -m 'bad'", "main\n")
	"ER-EFFECTS-BLOCK-MAIN-COMMIT" in rule_ids(denials)
}

test_deny_git_commit_on_main_object_signal if {
	denials := guard.deny with input as bash_event_object_signal("git commit --allow-empty -m bad", "main\n")
	"ER-EFFECTS-BLOCK-MAIN-COMMIT" in rule_ids(denials)
}

test_deny_git_add_then_commit_on_main if {
	denials := guard.deny with input as bash_event("git add . && git commit -m bad", "main")
	"ER-EFFECTS-BLOCK-MAIN-COMMIT" in rule_ids(denials)
}

test_deny_git_commit_when_branch_signal_missing if {
	denials := guard.deny with input as bash_event_no_branch_signal("git commit -m 'bad'")
	"ER-EFFECTS-BLOCK-MAIN-COMMIT" in rule_ids(denials)
}

test_deny_git_commit_when_branch_signal_empty if {
	denials := guard.deny with input as bash_event("git commit -m 'bad'", "\n")
	"ER-EFFECTS-BLOCK-MAIN-COMMIT" in rule_ids(denials)
}

test_allow_git_commit_on_feature_branch if {
	denials := guard.deny with input as bash_event("git commit -m 'ok'", "feature/title-fadein-gfx-flash\n")
	count(denials) == 0
}

test_allow_non_commit_git_on_main if {
	denials := guard.deny with input as bash_event("git status --short && git log --oneline -3", "main\n")
	count(denials) == 0
}

test_allow_commit_word_without_git_on_main if {
	denials := guard.deny with input as bash_event("echo commit", "main\n")
	count(denials) == 0
}
