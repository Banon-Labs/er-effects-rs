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

import data.cupcake.system.commands

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

is_git_commit(cmd) if {
	commands.has_verb(cmd, "git")
	commands.has_verb(cmd, "commit")
}

current_branch := branch if {
	branch := trim(input.signals.current_branch, " \t\r\n")
} else := branch if {
	branch := trim(input.signals.current_branch.output, " \t\r\n")
} else := "" if {
	true
}
