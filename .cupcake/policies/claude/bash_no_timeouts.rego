# METADATA
# scope: package
# title: Bash No Timeout/Sleep Guard
# description: Reject agent-invoked shell timeouts and sleeps; require deterministic readiness, events, or repo-approved drivers instead.
# custom:
#   severity: HIGH
#   id: ER-EFFECTS-BASH-NO-TIMEOUTS
#   routing:
#     required_events: ["PreToolUse"]
#     required_tools: ["Bash"]
package cupcake.policies.bash_no_timeouts

import rego.v1

command := object.get(input.tool_input, "command", "")

timeout_fields := {"timeout", "timeout_ms", "timeout_seconds"}
timeout_option_prefixes := {"--timeout", "--max-time", "--connect-timeout", "--read-timeout", "--write-timeout"}

tool_applies if {
	input.hook_event_name == "PreToolUse"
	input.tool_name == "Bash"
}

violation_reasons contains "Bash tool timeout parameter" if {
	tool_applies
	some field in timeout_fields
	object.get(input.tool_input, field, null) != null
}

violation_reasons contains "shell sleep command" if {
	tool_applies
	ast_command_name("sleep")
}

violation_reasons contains "shell timeout command" if {
	tool_applies
	ast_command_name("timeout")
}

violation_reasons contains "shell sleep command" if {
	tool_applies
	no_usable_ast
	uses_shell_word(command, "sleep")
}

violation_reasons contains "shell timeout command" if {
	tool_applies
	no_usable_ast
	uses_shell_word(command, "timeout")
}

violation_reasons contains "shell `read -t` timeout option" if {
	tool_applies
	read_with_timeout_option
}

violation_reasons contains sprintf("timeout-style option `%s`", [token]) if {
	tool_applies
	ast := object.get(input.tool_input, "command_ast", {})
	object.get(ast, "parse_ok", false) == true
	statements := object.get(ast, "statements", [])
	some statement in statements
	tokens := object.get(statement, "tokens", [])
	some token in tokens
	some prefix in timeout_option_prefixes
	startswith(token, prefix)
}

violation_reasons contains "timeout-style shell option" if {
	tool_applies
	no_usable_ast
	regex.match("(^|[\\s])--(timeout|max-time|connect-timeout|read-timeout|write-timeout)(=|[\\s]|$)", command)
}

deny contains decision if {
	reasons := [reason | some reason in violation_reasons]
	count(reasons) > 0

	decision := {
		"rule_id": "ER-EFFECTS-BASH-NO-TIMEOUTS",
		"severity": "HIGH",
		"reason": concat("", [
			"🧁 Cupcake denied this Bash invocation because timeouts and sleeps are not permitted in coding workflows. Command: ",
			command,
			"\n\nDetected: ",
			concat(", ", reasons),
			"\n\nWhy this policy exists: timeout- and sleep-based control hides races, strands runtime probes behind arbitrary wall-clock guesses, and converts correctness into luck.",
			"\n\nHappy path: omit Bash tool timeout fields, avoid `sleep`/`timeout`/timeout options, and use deterministic readiness instead: event files, process exit, inotify/file changes, explicit driver acknowledgements, game/task-frame state, or a repo-approved helper that returns only after an observable completion or structured failure condition.",
		]),
	}
}

ast_command_name(expected) if {
	ast := object.get(input.tool_input, "command_ast", {})
	object.get(ast, "parse_ok", false) == true
	statements := object.get(ast, "statements", [])
	some statement in statements
	object.get(statement, "command_name", "") == expected
}

read_with_timeout_option if {
	ast := object.get(input.tool_input, "command_ast", {})
	object.get(ast, "parse_ok", false) == true
	statements := object.get(ast, "statements", [])
	some statement in statements
	object.get(statement, "command_name", "") == "read"
	tokens := object.get(statement, "tokens", [])
	some token in tokens
	token == "-t"
}

read_with_timeout_option if {
	no_usable_ast
	regex.match("(^|[\\s;|&()])read[\\s][^\n;|&]*(^|[\\s])-t([\\s]|$)", command)
}

no_usable_ast if {
	ast := object.get(input.tool_input, "command_ast", null)
	ast == null
}

no_usable_ast if {
	ast := object.get(input.tool_input, "command_ast", {})
	object.get(ast, "parse_ok", false) == false
}

uses_shell_word(cmd, word) if {
	regex.match(concat("", ["(^|[\\s;|&()])", word, "([\\s;|&()]|$)"]), cmd)
}
