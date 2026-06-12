# METADATA
# scope: package
# title: Bash Invocation 30 Second Limit
# description: Keep agent-invoked shell commands bounded so long-running/game/GUI jobs do not strand the session.
# custom:
#   severity: HIGH
#   id: ER-EFFECTS-BASH-30S-LIMIT
#   routing:
#     required_events: ["PreToolUse"]
#     required_tools: ["Bash"]
package cupcake.policies.bash_invocation_30s_limit

import rego.v1

hard_max_ms := 30000

command := input.tool_input.command

# Claude Code Bash timeouts are milliseconds. If the agent asks for a longer
# timeout, fail closed with a friendly correction instead of silently allowing a
# long-running foreground process.
block contains decision if {
	input.hook_event_name == "PreToolUse"
	input.tool_name == "Bash"
	requested := input.tool_input.timeout
	is_number(requested)
	requested > hard_max_ms

	decision := {
		"rule_id": "ER-EFFECTS-BASH-30S-LIMIT",
		"severity": "HIGH",
		"reason": concat("", [
			"🧁 Cupcake paused this command because its requested timeout is over this repo's 30000ms hard limit. Command: ",
			command,
			"\n\nWhy this policy exists: this project frequently launches Proton/Elden Ring/Ghidra/UI automation, and unbounded foreground commands can strand the agent session or hide the useful artifact trail.",
			"\n\nHappy path: rerun the work as a bounded <=30s check, or start the long-running process in the background with logs/artifacts and poll it using short <=30s follow-up commands.",
		]),
	}
}

# Missing explicit timeouts are also rejected so every shell invocation has a
# deterministic upper bound. This makes the happy path obvious instead of
# relying on harness defaults that vary between agent surfaces.
block contains decision if {
	input.hook_event_name == "PreToolUse"
	input.tool_name == "Bash"
	not input.tool_input.timeout

	decision := {
		"rule_id": "ER-EFFECTS-BASH-30S-LIMIT-MISSING",
		"severity": "HIGH",
		"reason": concat("", [
			"🧁 Cupcake needs this command to declare an explicit <=30000ms timeout before it runs. Command: ",
			command,
			"\n\nWhy this policy exists: deterministic short shell invocations keep game, Ghidra, and smoke-test workflows recoverable and prevent accidental long foreground runs.",
			"\n\nHappy path: rerun with timeout <=30000ms, or background the long-running process with output redirected to an artifact/log file and inspect it with short follow-up commands.",
		]),
	}
}
