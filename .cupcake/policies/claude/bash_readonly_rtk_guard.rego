# METADATA
# scope: package
# title: Prefer RTK and Ripgrep for Read-Only Shell Inspection
# description: Block native grep/find/ls/git inspection forms and guide agents to rtk wrappers; rtk grep uses ripgrep.
# custom:
#   severity: MEDIUM
#   id: ER-EFFECTS-RTK-READONLY-GUARD
#   routing:
#     required_events: ["PreToolUse"]
#     required_tools: ["Bash"]
package cupcake.policies.bash_readonly_rtk_guard

import rego.v1

command := input.tool_input.command

native_search_or_listing_tools := {"grep", "egrep", "fgrep", "zgrep", "rg", "find", "ls"}
readonly_git_subcommands := {"status", "diff", "log", "show", "branch"}

deny contains decision if {
	input.hook_event_name == "PreToolUse"
	input.tool_name == "Bash"
	native_search_or_listing_detected
	not uses_shell_word(command, "rtk")

	decision := {
		"rule_id": "ER-EFFECTS-RTK-READONLY-GUARD",
		"severity": "MEDIUM",
		"reason": concat("", [
			"🧁 Cupcake paused this read-only inspection command so it goes through the repo's RTK path. Command: ",
			command,
			"\n\nWhy this policy exists: RTK is the workspace-standard wrapper for token-efficient, consistent inspection, and `rtk grep` uses ripgrep instead of legacy grep.",
			"\n\nHappy path: use `rtk grep ...` for search, `rtk find ...` for discovery, or `rtk ls ...` for listing. If you truly need raw shell semantics, make that exception explicit and keep the command bounded.",
		]),
	}
}

deny contains decision if {
	input.hook_event_name == "PreToolUse"
	input.tool_name == "Bash"
	readonly_git_inspection_detected
	not uses_shell_word(command, "rtk")

	decision := {
		"rule_id": "ER-EFFECTS-RTK-GIT-INSPECTION-GUARD",
		"severity": "MEDIUM",
		"reason": concat("", [
			"🧁 Cupcake paused this git inspection command so it goes through RTK. Command: ",
			command,
			"\n\nWhy this policy exists: this workspace standardizes read-only git inspection through `rtk git ...` so agents do not accidentally drift into mutating or noisy shell habits.",
			"\n\nHappy path: use the matching `rtk git ...` inspection command. Keep mutating git operations separate and subject to the repo's normal guardrails.",
		]),
	}
}

native_search_or_listing_detected if {
	ast := object.get(input.tool_input, "command_ast", {})
	object.get(ast, "parse_ok", false) == true
	statements := object.get(ast, "statements", [])
	some statement in statements
	command_name := object.get(statement, "command_name", "")
	native_search_or_listing_tools[command_name]
}

native_search_or_listing_detected if {
	ast := object.get(input.tool_input, "command_ast", null)
	ast == null
	some tool in native_search_or_listing_tools
	uses_shell_word(command, tool)
}

readonly_git_inspection_detected if {
	ast := object.get(input.tool_input, "command_ast", {})
	object.get(ast, "parse_ok", false) == true
	statements := object.get(ast, "statements", [])
	some statement in statements
	object.get(statement, "command_name", "") == "git"
	tokens := object.get(statement, "tokens", [])
	some index, token in tokens
	index > 0
	readonly_git_subcommands[token]
}

readonly_git_inspection_detected if {
	ast := object.get(input.tool_input, "command_ast", null)
	ast == null
	uses_shell_word(command, "git")
	some subcommand in readonly_git_subcommands
	regex.match(concat("", ["(^|[[:space:];|&()])git[[:space:]]+", subcommand, "([[:space:];|&()]|$)"]), command)
}

uses_shell_word(cmd, word) if {
	regex.match(concat("", ["(^|[[:space:];|&()])", word, "([[:space:];|&()]|$)"]), cmd)
}
