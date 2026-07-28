# METADATA
# scope: package
# title: Block Direct Pushes to Main
# authors: ["er-effects-rs agents"]
# custom:
#   severity: CRITICAL
#   id: ER-EFFECTS-BLOCK-MAIN-PUSH
#   routing:
#     required_events: ["PreToolUse"]
#     required_tools: ["Bash"]
#     required_signals: ["current_branch"]
package cupcake.policies.claude.git_block_main_push

import rego.v1

# Agents must not push directly to main. Work lands on feature/tooling branches;
# main is updated only through the user's preferred review/merge path. If the
# branch signal is missing, fail closed for push commands because bare `git push`
# inherits the current branch/refspec from Git config.
deny contains decision if {
	input.hook_event_name == "PreToolUse"
	input.tool_name == "Bash"
	is_git_push(lower(input.tool_input.command))
	blocked_push_context(lower(input.tool_input.command))

	decision := {
		"rule_id": "ER-EFFECTS-BLOCK-MAIN-PUSH",
		"reason": "Do not push directly to main. Push a feature/tooling branch and let main update through the review/merge path.",
		"severity": "CRITICAL",
	}
}

blocked_push_context(cmd) if {
	current_branch == "main"
}

blocked_push_context(cmd) if {
	current_branch == ""
}

blocked_push_context(cmd) if {
	push_targets_main(cmd)
}

# Match a real git push invocation, including common global-option forms such
# as `git -C <repo> push`.
git_push_command_pattern := `(^|[;&|(
])\s*(command\s+)?git([ \t]+((-c|--git-dir|--work-tree|--namespace|--config-env)(=|[ \t]+)("[^"\n]*"|'[^'\n]*'|[^ \t;&|()\n]+)|--(bare|no-pager|paginate|literal-pathspecs|no-replace-objects|exec-path)(=("[^"\n]*"|'[^'\n]*'|[^ \t;&|()\n]+))?))*[ \t]+push([ \t;&|)\n]|$)`

is_git_push(cmd) if {
	regex.match(git_push_command_pattern, cmd)
}

# Main-target forms we block even from a feature branch:
#   git push origin main
#   git push origin HEAD:main
#   git push origin feature:refs/heads/main
#   git -C repo push origin refs/heads/main
push_targets_main(cmd) if {
	regex.match(git_push_main_target_pattern, cmd)
}

git_push_main_target_pattern := `(?m)(^|[;&|]\s*|\n)\s*(command\s+)?git(\s+((-c|--git-dir|--work-tree|--namespace|--config-env)(=|\s+)("[^"\n]*"|'[^'\n]*'|[^\s;&|()]+)|--(bare|no-pager|paginate|literal-pathspecs|no-replace-objects|exec-path)(=("[^"\n]*"|'[^'\n]*'|[^\s;&|()]+))?))*\s+push(\s+[^;&|\n]*)?(\s|:)(refs/heads/)?main(\s|$|[;&|\n])`

current_branch := branch if {
	branch := trim(input.signals.current_branch, " \t\r\n")
} else := branch if {
	branch := trim(input.signals.current_branch.output, " \t\r\n")
} else := "" if {
	true
}
