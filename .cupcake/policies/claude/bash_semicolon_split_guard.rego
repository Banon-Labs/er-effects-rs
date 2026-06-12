# METADATA
# scope: package
# title: Bash Semicolon Split Guard
# description: Prevent dense Bash one-liners that split independent commands with semicolons.
# custom:
#   severity: MEDIUM
#   id: ER-EFFECTS-BASH-SEMICOLON-SPLIT-GUARD
#   routing:
#     required_events: ["PreToolUse"]
#     required_tools: ["Bash"]
package cupcake.policies.bash_semicolon_split_guard

import rego.v1

command := input.tool_input.command

block contains decision if {
	input.hook_event_name == "PreToolUse"
	input.tool_name == "Bash"
	semicolon_split_detected

	decision := {
		"rule_id": "ER-EFFECTS-BASH-SEMICOLON-SPLIT-GUARD",
		"severity": "MEDIUM",
		"reason": concat("", [
			"🧁 Cupcake paused this Bash inline because it appears to split commands with semicolons. Command: ",
			command,
			"\n\nPrefer splitting up each command split by ; into its own file, eg ./scripts/named-file.sh, and call it in series instead, or if you think it would be faster, make a parent script for multiple scripts to be called instead in the proper sequence.",
		]),
	}
}

semicolon_split_detected if {
	ast := object.get(input.tool_input, "command_ast", {})
	object.get(ast, "top_level_semicolon_count", 0) > 0
}

semicolon_split_detected if {
	ast := object.get(input.tool_input, "command_ast", {})
	separators := object.get(ast, "separators", [])
	some separator in separators
	separator.value == ";"
	object.get(separator, "syntactic_control", false) == false
}

semicolon_split_detected if {
	not input.tool_input.command_ast
	contains(command, ";")
}
