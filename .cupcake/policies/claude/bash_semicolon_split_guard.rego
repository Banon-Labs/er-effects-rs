# METADATA
# scope: package
# title: Bash Semicolon Split Guard
# description: Prefer named scripts over inline Bash command chains split by semicolons.
# custom:
#   severity: MEDIUM
#   id: ER-EFFECTS-BASH-SEMICOLON-SPLIT-GUARD
#   routing:
#     required_events: ["PreToolUse"]
#     required_tools: ["Bash"]
package cupcake.policies.bash_semicolon_split_guard

import rego.v1

command := input.tool_input.command

deny contains decision if {
	input.hook_event_name == "PreToolUse"
	input.tool_name == "Bash"
	semicolon_split

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

semicolon_split if {
	ast := object.get(input.tool_input, "command_ast", {})
	object.get(ast, "parse_ok", false) == true
	object.get(ast, "top_level_semicolon_count", 0) > 0
}

semicolon_split if {
	no_usable_ast
	regex.match(";", scrubbed_command)
}

# Scrub quoted spans and heredoc bodies so semicolons inside shell words or
# interpreter input are not treated as shell command separators. This mirrors
# the env/RTK guards and uses WASM-safe split/replace/concat builtins.
semicolon_heredoc_trimmed := split(command, "<<")[0]

semicolon_escapes_stripped := replace(replace(semicolon_heredoc_trimmed, `\"`, ""), `\'`, "")

semicolon_double_parts := split(semicolon_escapes_stripped, `"`)

semicolon_outside_double := concat(" ", [semicolon_double_parts[idx] |
	some idx
	semicolon_double_parts[idx]
	idx % 2 == 0
])

semicolon_single_parts := split(semicolon_outside_double, "'")

scrubbed_command := concat(" ", [semicolon_single_parts[idx] |
	some idx
	semicolon_single_parts[idx]
	idx % 2 == 0
])

no_usable_ast if {
	ast := object.get(input.tool_input, "command_ast", null)
	ast == null
}

no_usable_ast if {
	ast := object.get(input.tool_input, "command_ast", {})
	object.get(ast, "parse_ok", false) == false
}
