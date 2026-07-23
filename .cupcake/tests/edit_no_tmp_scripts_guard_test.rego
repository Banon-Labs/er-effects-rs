# OPA unit tests for edit_no_tmp_scripts_guard.
#
# Run with:
#   opa test .cupcake/policies/claude/edit_no_tmp_scripts_guard.rego \
#            .cupcake/tests/edit_no_tmp_scripts_guard_test.rego
package cupcake.policies.claude.edit_no_tmp_scripts_guard_test

import rego.v1

import data.cupcake.policies.claude.edit_no_tmp_scripts_guard as guard

write_event(path) := {
	"hook_event_name": "PreToolUse",
	"tool_name": "write",
	"tool_input": {"path": path, "content": "test"},
}

claude_write_event(path) := {
	"hook_event_name": "PreToolUse",
	"tool_name": "Write",
	"tool_input": {"file_path": path, "content": "test"},
}

bash_event(command) := {
	"hook_event_name": "PreToolUse",
	"tool_name": "Bash",
	"tool_input": {"command": command, "timeout": 30000, "description": "test case"},
}

rule_ids(denials) := {d.rule_id | some d in denials}

denied(input_event) if {
	denials := guard.deny with input as input_event
	"ER-EFFECTS-NO-TMP-SCRIPTS-GUARD" in rule_ids(denials)
}

test_deny_pi_write_tmp_ps1_path_key if {
	denied(write_event("/tmp/check-er-net-effects.ps1"))
}

test_deny_claude_write_tmp_python_file_path_key if {
	denied(claude_write_event("/tmp/ghidra_probe.py"))
}

test_deny_bash_cat_tmp_script if {
	denied(bash_event("cat > /tmp/check-er-net-effects.ps1 <<'EOF'\nWrite-Output hi\nEOF"))
}

test_deny_bash_python_tmp_script if {
	denied(bash_event("python3 - <<'PY'\nopen('/tmp/probe.py','w').write('print(1)')\nPY"))
}

test_allow_tmp_json_artifact if {
	not denied(write_event("/tmp/check-er-net-effects.json"))
}

test_allow_repo_script if {
	not denied(write_event("/home/choza/projects/er-effects-rs/scripts/check-er-net-effects.ps1"))
}

test_allow_bash_read_tmp_script_path_without_authoring_verb if {
	not denied(bash_event("powershell.exe -File /tmp/existing-user-script.ps1"))
}

test_allow_non_pretooluse_event if {
	denials := guard.deny with input as {
		"hook_event_name": "PostToolUse",
		"tool_name": "write",
		"tool_input": {"path": "/tmp/x.ps1"},
	}
	count(denials) == 0
}
