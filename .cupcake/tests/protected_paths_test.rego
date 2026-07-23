# OPA unit tests for the builtin protected-path policy.
# Run with:
#   opa test .cupcake/system/commands.rego \
#            .cupcake/policies/claude/builtins/protected_paths.rego \
#            .cupcake/tests/protected_paths_test.rego
package cupcake.policies.builtins.protected_paths_test

import rego.v1

import data.cupcake.policies.builtins.protected_paths as protected

bash_event(cmd, affected_dirs) := {
	"hook_event_name": "PreToolUse",
	"tool_name": "Bash",
	"tool_input": {"command": cmd, "timeout": 30000, "description": "test case"},
	"affected_parent_directories": affected_dirs,
	"builtin_config": {"protected_paths": {"message": "This path is read-only and cannot be modified", "paths": ["/System/", "/etc/"]}},
}

rule_ids(denials) := {d.rule_id | some d in denials}

test_allow_mktemp_bd_comment_file_when_preprocessor_overapproximates_root if {
	cmd := concat("\n", [
		"tmp=$(mktemp)",
		"cat > \"$tmp\" <<'EOF'",
		"bd issue comment body",
		"EOF",
		"/home/banon/.local/bin/bd comment er-effects-rs-22h --file \"$tmp\" --json",
		"rm -f \"$tmp\"",
	])
	denials := protected.halt with input as bash_event(cmd, ["/"])
	count(denials) == 0
}

test_deny_literal_root_delete_even_with_mktemp_cleanup if {
	cmd := concat("\n", [
		"tmp=$(mktemp)",
		"cat > \"$tmp\" <<'EOF'",
		"bd issue comment body",
		"EOF",
		"/home/banon/.local/bin/bd comment er-effects-rs-22h --file \"$tmp\" --json",
		"rm -f \"$tmp\"",
		"rm -rf /",
	])
	denials := protected.halt with input as bash_event(cmd, ["/"])
	"BUILTIN-PROTECTED-PATHS-PARENT" in rule_ids(denials)
}

test_deny_literal_etc_delete_even_with_mktemp_cleanup if {
	cmd := concat("\n", [
		"tmp=$(mktemp)",
		"/home/banon/.local/bin/bd comment er-effects-rs-22h --file \"$tmp\" --json",
		"rm -f \"$tmp\" /etc/passwd",
	])
	denials := protected.halt with input as bash_event(cmd, ["/"])
	"BUILTIN-PROTECTED-PATHS-PARENT" in rule_ids(denials)
}
