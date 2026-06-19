# METADATA
# scope: package
# title: Bash Bounded Timeout Guard
# description: Require agent-invoked Bash commands to carry a tool-level timeout of 120 seconds or less; reject sleeps that hide readiness bugs.
# custom:
#   severity: HIGH
#   id: ER-EFFECTS-BASH-BOUNDED-TIMEOUT
#   routing:
#     required_events: ["PreToolUse"]
#     required_tools: ["Bash"]
package cupcake.policies.bash_no_timeouts

import rego.v1

command := object.get(input.tool_input, "command", "")

timeout_fields := {"timeout", "timeout_ms", "timeout_seconds"}

tool_applies if {
	input.hook_event_name == "PreToolUse"
	input.tool_name == "Bash"
}

has_tool_timeout if {
	some field in timeout_fields
	object.get(input.tool_input, field, null) != null
}

valid_seconds_string(value) if {
	is_string(value)
	regex.match("^([1-9]|[1-9][0-9]|1[01][0-9]|120)$", value)
}

valid_milliseconds_string(value) if {
	is_string(value)
	regex.match("^([1-9][0-9]{0,4}|1[01][0-9]{4}|120000)$", value)
}

valid_tool_timeout if {
	value := object.get(input.tool_input, "timeout", null)
	is_number(value)
	not value <= 0
	value <= 120000
}

valid_tool_timeout if {
	value := object.get(input.tool_input, "timeout", null)
	valid_milliseconds_string(value)
}

valid_tool_timeout if {
	value := object.get(input.tool_input, "timeout_seconds", null)
	is_number(value)
	not value <= 0
	value <= 120
}

valid_tool_timeout if {
	value := object.get(input.tool_input, "timeout_seconds", null)
	valid_seconds_string(value)
}

valid_tool_timeout if {
	value := object.get(input.tool_input, "timeout_ms", null)
	is_number(value)
	not value <= 0
	value <= 120000
}

valid_tool_timeout if {
	value := object.get(input.tool_input, "timeout_ms", null)
	valid_milliseconds_string(value)
}

violation_reasons contains "missing Bash tool timeout parameter" if {
	tool_applies
	not has_tool_timeout
}

violation_reasons contains "Bash tool timeout parameter must be greater than 0 and no more than 120 seconds (timeout/timeout_ms <= 120000ms, timeout_seconds <= 120)" if {
	tool_applies
	has_tool_timeout
	not valid_tool_timeout
}

violation_reasons contains "shell sleep command" if {
	tool_applies
	ast_command_name("sleep")
}

violation_reasons contains "shell sleep command" if {
	tool_applies
	no_usable_ast
	uses_shell_word(command, "sleep")
}

deny contains decision if {
	reasons := [reason | some reason in violation_reasons]
	count(reasons) > 0

	decision := {
		"rule_id": "ER-EFFECTS-BASH-BOUNDED-TIMEOUT",
		"severity": "HIGH",
		"reason": concat("", [
			"🧁 Cupcake denied this Bash invocation because agent-run shell commands must be hard-bounded. Command: ",
			command,
			"\n\nDetected: ",
			concat(", ", reasons),
			"\n\nWhy this policy exists: unbounded shell commands can strand shared tools, game processes, or remote analysis jobs. Every agent-invoked Bash command must include a tool-level timeout of 120 seconds or less; runtime helpers should still prefer observable completion and structured failures inside that hard cap.",
			"\n\nHappy path: set the Bash tool timeout to 120 seconds or less. Cupcake receives Bash tool `timeout`/`timeout_ms` in milliseconds and `timeout_seconds` in seconds. Keep long-running workflows split into bounded steps, and avoid `sleep`; use event files, process exit, inotify/file changes, explicit driver acknowledgements, game/task-frame state, or a repo-approved helper that returns before the hard cap.",
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
