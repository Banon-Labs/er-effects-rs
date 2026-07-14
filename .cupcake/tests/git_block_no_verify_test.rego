# OPA unit tests for the builtin git no-verify policy.
# Run with:
#   opa test .cupcake/system/commands.rego \
#            .cupcake/policies/claude/builtins/git_block_no_verify.rego \
#            .cupcake/tests/git_block_no_verify_test.rego
package cupcake.policies.builtins.git_block_no_verify_test

import rego.v1

import data.cupcake.policies.builtins.git_block_no_verify as guard

bash_event(cmd) := {
	"hook_event_name": "PreToolUse",
	"tool_name": "Bash",
	"tool_input": {"command": cmd, "timeout": 30000},
}

rule_ids(denials) := {d.rule_id | some d in denials}

test_deny_git_commit_no_verify_long if {
	denials := guard.deny with input as bash_event("git commit --no-verify -m bad")
	"BUILTIN-GIT-BLOCK-NO-VERIFY" in rule_ids(denials)
}

test_deny_git_commit_no_verify_short if {
	denials := guard.deny with input as bash_event("git commit -nm bad")
	"BUILTIN-GIT-BLOCK-NO-VERIFY" in rule_ids(denials)
}

test_deny_git_dash_c_commit_no_verify_short if {
	denials := guard.deny with input as bash_event("git -C /tmp/repo commit -n -m bad")
	"BUILTIN-GIT-BLOCK-NO-VERIFY" in rule_ids(denials)
}

test_allow_git_commit_help_piped_to_sed_n if {
	denials := guard.deny with input as bash_event("git commit -h 2>&1 | sed -n '1,80p'")
	count(denials) == 0
}

test_allow_git_commit_then_later_sed_n if {
	denials := guard.deny with input as bash_event("git commit -m ok && sed -n '1,80p' file")
	count(denials) == 0
}
