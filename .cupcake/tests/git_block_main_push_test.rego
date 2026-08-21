# OPA unit tests for git_block_main_push.
# Run with:
#   opa test .cupcake/system/commands.rego \
#     .cupcake/policies/claude/git_block_main_push.rego \
#     .cupcake/tests/git_block_main_push_test.rego
package cupcake.policies.claude.git_block_main_push_test

import rego.v1

import data.cupcake.policies.claude.git_block_main_push as guard

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

test_deny_bare_git_push_on_main if {
	denials := guard.deny with input as bash_event("git push", "main\n")
	"ER-EFFECTS-BLOCK-MAIN-PUSH" in rule_ids(denials)
}

test_deny_git_push_when_branch_signal_missing if {
	denials := guard.deny with input as bash_event_no_branch_signal("git push")
	"ER-EFFECTS-BLOCK-MAIN-PUSH" in rule_ids(denials)
}

test_deny_git_push_when_branch_signal_empty if {
	denials := guard.deny with input as bash_event("git push", "\n")
	"ER-EFFECTS-BLOCK-MAIN-PUSH" in rule_ids(denials)
}

test_deny_git_push_main_from_feature_branch if {
	denials := guard.deny with input as bash_event("git push origin main", "feature/no-main-push")
	"ER-EFFECTS-BLOCK-MAIN-PUSH" in rule_ids(denials)
}

test_deny_git_c_push_main_from_feature_branch if {
	denials := guard.deny with input as bash_event("git -C /tmp/repo push origin main", "feature/no-main-push")
	"ER-EFFECTS-BLOCK-MAIN-PUSH" in rule_ids(denials)
}

test_deny_git_push_head_to_main_from_feature_branch if {
	denials := guard.deny with input as bash_event("git push origin HEAD:main", "feature/no-main-push")
	"ER-EFFECTS-BLOCK-MAIN-PUSH" in rule_ids(denials)
}

test_deny_git_push_ref_heads_main_from_feature_branch if {
	denials := guard.deny with input as bash_event("git push origin feature/no-main-push:refs/heads/main", "feature/no-main-push")
	"ER-EFFECTS-BLOCK-MAIN-PUSH" in rule_ids(denials)
}

test_deny_object_branch_signal_on_main if {
	denials := guard.deny with input as bash_event_object_signal("git push", "main\n")
	"ER-EFFECTS-BLOCK-MAIN-PUSH" in rule_ids(denials)
}

test_allow_git_push_feature_branch if {
	denials := guard.deny with input as bash_event("git push -u origin guard/no-direct-main-push", "guard/no-direct-main-push\n")
	count(denials) == 0
}

test_allow_git_push_head_to_feature_branch if {
	denials := guard.deny with input as bash_event("git push origin HEAD:refs/heads/guard/no-direct-main-push", "guard/no-direct-main-push\n")
	count(denials) == 0
}

# current_branch can describe the parent checkout rather than this session's
# worktree. The explicit non-main upstream destination must therefore override
# that stale main signal, including when followed by harmless status inspection.
test_allow_explicit_nonmain_upstream_when_branch_signal_stale_main if {
	denials := guard.deny with input as bash_event(
		"git push -u origin fix/title-accept-in-update && git status --short --branch",
		"main\n",
	)
	count(denials) == 0
}

test_allow_explicit_nonmain_upstream_when_branch_signal_missing if {
	denials := guard.deny with input as bash_event_no_branch_signal(
		"git push --set-upstream origin feature/no-direct-main-push",
	)
	count(denials) == 0
}

test_deny_explicit_main_upstream_when_branch_signal_stale_main if {
	denials := guard.deny with input as bash_event("git push -u origin main", "main\n")
	"ER-EFFECTS-BLOCK-MAIN-PUSH" in rule_ids(denials)
}

test_deny_explicit_head_to_main_upstream_when_branch_signal_stale_main if {
	denials := guard.deny with input as bash_event("git push -u origin HEAD:main", "main\n")
	"ER-EFFECTS-BLOCK-MAIN-PUSH" in rule_ids(denials)
}

test_deny_explicit_nonmain_upstream_chained_with_bare_push_when_branch_signal_stale_main if {
	denials := guard.deny with input as bash_event(
		"git push -u origin feature/no-direct-main-push && git push",
		"main\n",
	)
	"ER-EFFECTS-BLOCK-MAIN-PUSH" in rule_ids(denials)
}

test_allow_non_push_git_on_main if {
	denials := guard.deny with input as bash_event("git status --short && git log --oneline -3", "main\n")
	count(denials) == 0
}

test_allow_push_word_without_git_on_main if {
	denials := guard.deny with input as bash_event("echo push main", "main\n")
	count(denials) == 0
}

# --- Worktree-target exception ------------------------------------------------

bash_event_with_worktrees(cmd, branch, worktrees) := {
	"hook_event_name": "PreToolUse",
	"tool_name": "Bash",
	"tool_input": {"command": cmd, "timeout": 30000},
	"signals": {"current_branch": branch, "worktree_branches": worktrees},
}

worktree_fixture := concat("\n", [
	"worktree /home/banon/projects/er-effects-rs",
	"HEAD 0000000000000000000000000000000000000000",
	"branch refs/heads/main",
	"",
	"worktree /home/banon/projects/er-effects-rs/.worktrees/portrait-stats-crate",
	"HEAD 1111111111111111111111111111111111111111",
	"branch refs/heads/feature/portrait-stats-crate",
	"",
])

test_allow_git_c_push_feature_from_nonmain_worktree_main_session if {
	denials := guard.deny with input as bash_event_with_worktrees(
		"git -C /home/banon/projects/er-effects-rs/.worktrees/portrait-stats-crate push -u origin feature/portrait-stats-crate",
		"main\n", worktree_fixture,
	)
	count(denials) == 0
}

test_deny_git_c_push_main_refspec_from_nonmain_worktree if {
	denials := guard.deny with input as bash_event_with_worktrees(
		"git -C /home/banon/projects/er-effects-rs/.worktrees/portrait-stats-crate push origin HEAD:main",
		"main\n", worktree_fixture,
	)
	"ER-EFFECTS-BLOCK-MAIN-PUSH" in rule_ids(denials)
}

test_deny_git_c_push_unregistered_path_from_main_session if {
	denials := guard.deny with input as bash_event_with_worktrees(
		"git -C /tmp/not-a-worktree push -u origin feature/foo",
		"main\n", worktree_fixture,
	)
	"ER-EFFECTS-BLOCK-MAIN-PUSH" in rule_ids(denials)
}

test_deny_git_c_push_main_worktree_from_main_session if {
	denials := guard.deny with input as bash_event_with_worktrees(
		"git -C /home/banon/projects/er-effects-rs push -u origin feature/foo",
		"main\n", worktree_fixture,
	)
	"ER-EFFECTS-BLOCK-MAIN-PUSH" in rule_ids(denials)
}

test_deny_git_c_push_chained_with_bare_push_from_main_session if {
	denials := guard.deny with input as bash_event_with_worktrees(
		"git -C /home/banon/projects/er-effects-rs/.worktrees/portrait-stats-crate push -u origin feature/portrait-stats-crate && git push",
		"main\n", worktree_fixture,
	)
	"ER-EFFECTS-BLOCK-MAIN-PUSH" in rule_ids(denials)
}
