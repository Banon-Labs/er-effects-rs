# METADATA
# scope: package
# title: Block Local Commits on Main
# authors: ["er-effects-rs agents"]
# custom:
#   severity: CRITICAL
#   id: ER-EFFECTS-BLOCK-MAIN-COMMIT
#   routing:
#     required_events: ["PreToolUse"]
#     required_tools: ["Bash"]
#     required_signals: ["current_branch"]
package cupcake.policies.claude.git_block_main_commit

import rego.v1

# Never allow local commits while the active branch is main. Agents must create a
# feature/tooling branch from the intended base first, then commit there. If the
# branch signal is missing, fail closed: a missing signal caused a live main
# commit to slip through this guard on 2026-07-13.
deny contains decision if {
	input.hook_event_name == "PreToolUse"
	input.tool_name == "Bash"
	is_git_commit(lower(input.tool_input.command))
	blocked_branch_context

	decision := {
		"rule_id": "ER-EFFECTS-BLOCK-MAIN-COMMIT",
		"reason": "Do not commit unless the guard can confirm the active branch is not main. Create/switch to a feature or tooling branch based on the intended base, and ensure the current_branch signal is available.",
		"severity": "CRITICAL",
	}
}

blocked_branch_context if {
	current_branch == "main"
}

blocked_branch_context if {
	current_branch == ""
}

# Match a real git commit invocation instead of any command text containing both
# words. This avoids false positives from shell comments, printf labels, and
# variable names such as archive_commit while still blocking direct git commit
# calls, including common global-option forms such as `git -C <repo> commit`.
git_commit_command_pattern := `(^|[;&|(\n])\s*(command\s+)?git([ \t]+((-c|--git-dir|--work-tree|--namespace|--config-env)(=|[ \t]+)("[^"\n]*"|'[^'\n]*'|[^ \t;&|()\n]+)|--(bare|no-pager|paginate|literal-pathspecs|no-replace-objects|exec-path)(=("[^"\n]*"|'[^'\n]*'|[^ \t;&|()\n]+))?))*[ \t]+commit([ \t;&|)\n]|$)`

is_git_commit(cmd) if {
	regex.match(git_commit_command_pattern, cmd)
}

current_branch := branch if {
	branch := trim(input.signals.current_branch, " \t\r\n")
} else := branch if {
	branch := trim(input.signals.current_branch.output, " \t\r\n")
} else := "" if {
	true
}
