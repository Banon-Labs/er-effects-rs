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
	object.get(statement, "command_name", null) != null
	env_prefix_assignment
}

inline_env_assignment if {
	no_usable_ast
	env_prefix_assignment
}

inline_env_assignment if {
	regex.match("(^|[;&|()][[:space:]]*)export[[:space:]]+[A-Za-z_][A-Za-z0-9_]*(=|[[:space:];]|$)", scrubbed_command)
}

inline_env_assignment if {
	regex.match("(^|[;&|()][[:space:]]*)env[[:space:]]+[A-Za-z_][A-Za-z0-9_]*=", scrubbed_command)
}

# Direct environment prefixes are conventional uppercase assignments in the
# same simple command as the command they affect, e.g. `FOO=bar ./cmd`. This
# source-text confirmation avoids over-trusting coarse AST `env_setting` markers
# that can cover lowercase local shell bookkeeping before later commands.
env_prefix_assignment if {
	regex.match("(^|\\n|[;&|()][ \\t]*)[A-Z_][A-Z0-9_]*=[^ \\t\\n;&|()$]*[ \\t]+[^ \\t\\n;&|()=]+", scrubbed_command)
}

env_prefix_assignment if {
	regex.match("(^|\\n|[;&|()][ \\t]*)[A-Z_][A-Z0-9_]*=\\$\\([^\\n]*\\)[ \\t]+[^ \\t\\n;&|()=]+", scrubbed_command)
}

# Scrub the command of quoted spans and heredoc bodies so a `=` that lives inside
# a quoted argument or interpreter input is not read as a shell env assignment.
# regex.replace is undefined in Cupcake's WASM runtime, so this is built with the
# WASM-safe split/replace/concat builtins: trim at the heredoc opener, drop
# backslash-escaped quotes, then keep only the even-indexed (outside-quote)
# segments after splitting on " and then '.
env_heredoc_trimmed := split(command, "<<")[0]

env_escapes_stripped := replace(replace(env_heredoc_trimmed, `\"`, ""), `\'`, "")

env_double_parts := split(env_escapes_stripped, `"`)

env_outside_double := concat(" ", [env_double_parts[idx] |
	some idx
	env_double_parts[idx]
	idx % 2 == 0
])

env_single_parts := split(env_outside_double, "'")

scrubbed_command := concat(" ", [env_single_parts[idx] |
	some idx
	env_single_parts[idx]
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
