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

deny contains decision if {
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

# Fallback path: Claude Code does not supply command_ast, so the two AST rules
# above never fire at runtime and this is the only active detector. A semicolon
# is a real command separator only when it sits OUTSIDE quoted arguments
# (git commit -m "a; b", python3 -c "import x; y", bd remember 'a; b' must be
# allowed). Regex builtins are NOT available in Cupcake's WASM runtime (they
# evaluate to undefined and silently disable the rule), so quoted spans are
# stripped with split(): splitting on a quote char yields alternating
# outside/inside segments, and only even-indexed segments live outside the quote.
semicolon_split_detected if {
	not input.tool_input.command_ast
	some part in outside_double_quotes
	contains(strip_single_quoted(part), ";")
}

# Heredoc bodies (python3 - <<'PY' ... PY, cat <<EOF ... EOF) are interpreter
# input, not shell, so their semicolons are never command separators. Scan only
# the text BEFORE the first heredoc opener; a real "cmd1; cmd2 <<EOF" still has
# its separator in that prefix. split() returns the whole command unchanged when
# there is no "<<".
heredoc_trimmed := split(command, "<<")[0]

# Backslash-escaped quotes (\" and \') are literal characters, not span
# boundaries, but split() cannot see the escaping and would miscount them. Drop
# them first so the even/odd segment alternation stays aligned. replace() (unlike
# regex) is available in Cupcake's WASM runtime.
escapes_stripped := replace(replace(heredoc_trimmed, `\"`, ""), `\'`, "")

# Segments of the command that fall outside double-quoted spans.
double_quote_parts := split(escapes_stripped, `"`)

outside_double_quotes := [double_quote_parts[idx] |
	some idx
	double_quote_parts[idx]
	idx % 2 == 0
]

# Remove single-quoted spans from a segment, keeping only the outside text.
strip_single_quoted(segment) := concat(" ", outside) if {
	single_quote_parts := split(segment, "'")
	outside := [single_quote_parts[idx] |
		some idx
		single_quote_parts[idx]
		idx % 2 == 0
	]
}
