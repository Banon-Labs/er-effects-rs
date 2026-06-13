# METADATA
# scope: package
# title: Bash Inline Environment Assignment Guard
# description: Prefer named env files over inline Bash environment assignments.
# custom:
#   severity: MEDIUM
#   id: ER-EFFECTS-BASH-ENV-FILE-GUARD
#   routing:
#     required_events: ["PreToolUse"]
#     required_tools: ["Bash"]
package cupcake.policies.bash_env_file_guard

import rego.v1

command := input.tool_input.command

deny contains decision if {
	input.hook_event_name == "PreToolUse"
	input.tool_name == "Bash"
	inline_env_assignment

	decision := {
		"rule_id": "ER-EFFECTS-BASH-ENV-FILE-GUARD",
		"severity": "MEDIUM",
		"reason": concat("", [
			"🧁 Cupcake paused this Bash inline because it appears to set environment variables directly. Command: ",
			command,
			"\n\nPut your envs in ./.envs/named-env.env instead and load it.",
		]),
	}
}

inline_env_assignment if {
	ast := object.get(input.tool_input, "command_ast", {})
	object.get(ast, "parse_ok", false) == true
	statements := object.get(ast, "statements", [])
	some statement in statements
	object.get(statement, "env_setting", false) == true
}

inline_env_assignment if {
	no_usable_ast
	regex.match("(^|[;&|()][[:space:]]*)[A-Za-z_][A-Za-z0-9_]*=", command)
}

inline_env_assignment if {
	no_usable_ast
	regex.match("(^|[;&|()][[:space:]]*)export[[:space:]]+[A-Za-z_][A-Za-z0-9_]*(=|[[:space:];]|$)", command)
}

inline_env_assignment if {
	no_usable_ast
	regex.match("(^|[;&|()][[:space:]]*)env[[:space:]]+[A-Za-z_][A-Za-z0-9_]*=", command)
}

no_usable_ast if {
	ast := object.get(input.tool_input, "command_ast", null)
	ast == null
}

no_usable_ast if {
	ast := object.get(input.tool_input, "command_ast", {})
	object.get(ast, "parse_ok", false) == false
}
