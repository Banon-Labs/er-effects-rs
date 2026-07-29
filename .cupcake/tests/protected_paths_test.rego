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
	"builtin_config": {"protected_paths": {"message": "This path is read-only and cannot be modified", "paths": ["/System/", "/etc/", "~/.ssh/"]}},
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

# A second destructive statement that names an absolute path keeps the root
# over-approximation usable, so the parent rule still denies.
test_deny_extra_absolute_delete_even_with_mktemp_cleanup if {
	cmd := concat("\n", [
		"tmp=$(mktemp)",
		"/home/banon/.local/bin/bd comment er-effects-rs-22h --file \"$tmp\" --json",
		"rm -f \"$tmp\"",
		"rm -rf /var/tmp/stale",
	])
	denials := protected.halt with input as bash_event(cmd, ["/"])
	"BUILTIN-PROTECTED-PATHS-PARENT" in rule_ids(denials)
}

# Contract change 2026-07-29: a destructive statement whose operands are all
# relative no longer inherits the preprocessor's root over-approximation. The
# old policy denied this because ANY destructive verb plus a reported `/` was
# enough; that is the same reasoning that denied a plain `git commit -F -`
# heredoc, and root is never the real target here (real preprocessing reports
# `<cwd>/relative-file`, not `/`).
test_allow_relative_delete_with_overapproximated_root if {
	cmd := concat("\n", [
		"tmp=$(mktemp)",
		"/home/banon/.local/bin/bd comment er-effects-rs-22h --file \"$tmp\" --json",
		"rm -f \"$tmp\"",
		"rm -f relative-file",
	])
	denials := protected.halt with input as bash_event(cmd, ["/"])
	count(denials) == 0
}

# --------------------------------------------------------------------------
# Heredoc payload is DATA, never a path operand.
#
# Regression for the 2026-07-29 false positive: `git add <paths> && git commit
# -q -F - <<'EOF' ...` was denied twice ("(/etc/ would be affected by operation
# on /)", "(/System/ ...)") because the preprocessor tokenized the commit
# message and reported `/` (from a bare slash between two code spans) as an
# affected parent directory, while the prose verb "install" satisfied the
# destructive-command check. Nothing in the command mutates anything but the
# index and the commit object.
# --------------------------------------------------------------------------

commit_message_heredoc_command := concat("\n", [
	"git add crates/er-effects-rs/src/lib.rs docs/save/save-machinery-1162.md && git commit -q -F - <<'EOF' && git log --oneline -4 && git status --porcelain",
	"save-flow: install the save-write suppression, making Save Game the only writer",
	"",
	"Both DLLs would detour `0x140e6fb50` / `0x140e6e430`, corrupting each other's",
	"trampolines, so driving System->Quit twice is the only supported path.",
	"Doc comments in scripts/check-save-disable-warnings.py were de-scoped 3/4.",
	"",
	"Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>",
	"EOF",
])

test_allow_git_commit_message_heredoc_with_prose_verb_and_bare_slash if {
	affected := [
		"/home/banon/projects/er-effects-rs/crates/er-effects-rs/src/lib.rs",
		"/home/banon/projects/er-effects-rs",
		"/",
		"/home/banon/projects/er-effects-rs/3/4",
	]
	denials := protected.halt with input as bash_event(commit_message_heredoc_command, affected)
	count(denials) == 0
}

# The command region AFTER a heredoc terminator is still shell: a destructive
# statement there must keep denying.
test_deny_root_delete_after_heredoc_payload if {
	cmd := concat("\n", [
		"git commit -q -F - <<'EOF'",
		"message text with a bare / in it",
		"EOF",
		"rm -rf /",
	])
	denials := protected.halt with input as bash_event(cmd, ["/"])
	"BUILTIN-PROTECTED-PATHS-PARENT" in rule_ids(denials)
}

# An unterminated heredoc must not leave payload lines readable as operands.
test_allow_unterminated_heredoc_payload if {
	cmd := concat("\n", [
		"git commit -q -F - <<'EOF'",
		"install the thing at / for good",
	])
	denials := protected.halt with input as bash_event(cmd, ["/"])
	count(denials) == 0
}

# A real destructive verb outside the heredoc, with only relative operands, is
# still not evidence of a root operation.
test_allow_relative_copy_with_overapproximated_root if {
	denials := protected.halt with input as bash_event("cp target/a.txt target/b.txt", ["/"])
	count(denials) == 0
}

# --------------------------------------------------------------------------
# Genuinely destructive operations stay blocked.
# --------------------------------------------------------------------------

test_deny_root_recursive_delete if {
	denials := protected.halt with input as bash_event("rm -rf /", ["/"])
	"BUILTIN-PROTECTED-PATHS-PARENT" in rule_ids(denials)
}

test_deny_root_glob_delete if {
	denials := protected.halt with input as bash_event("rm -rf /*", ["/"])
	"BUILTIN-PROTECTED-PATHS-PARENT" in rule_ids(denials)
}

test_deny_root_recursive_chmod if {
	denials := protected.halt with input as bash_event("chmod -R 777 /", ["/"])
	"BUILTIN-PROTECTED-PATHS-PARENT" in rule_ids(denials)
}

test_deny_etc_directory_delete if {
	denials := protected.halt with input as bash_event("rm -rf /etc", ["/etc"])
	"BUILTIN-PROTECTED-PATHS-PARENT" in rule_ids(denials)
}

test_deny_system_directory_recursive_chown if {
	denials := protected.halt with input as bash_event("chown -R root /System", ["/System"])
	"BUILTIN-PROTECTED-PATHS-PARENT" in rule_ids(denials)
}

# Real preprocessing reports the operand itself (`/etc/my.conf`), not `/etc`,
# for this shape, so the parent rule is silent and the protected-path REFERENCE
# rule owns the denial. Asserted here so the split stays visible.
test_deny_move_into_etc_by_reference_rule if {
	affected := ["/home/banon/projects/er-effects-rs/my.conf", "/etc/my.conf"]
	denials := protected.halt with input as bash_event("mv ./my.conf /etc/my.conf", affected)
	"BUILTIN-PROTECTED-PATHS" in rule_ids(denials)
}

test_deny_write_into_system_directory_by_reference_rule if {
	denials := protected.halt with input as bash_event("cp ./libfoo.dylib /System/Library/libfoo.dylib", [])
	"BUILTIN-PROTECTED-PATHS" in rule_ids(denials)
}

# Heredoc payload is excluded from OPERAND analysis only. An interpreter
# heredoc that writes a protected path is still a real write, so the reference
# rule must keep scanning body text.
test_deny_interpreter_heredoc_writing_etc if {
	cmd := concat("\n", [
		"python3 - <<'PY'",
		"open('/etc/passwd', 'w').write('x')",
		"PY",
	])
	denials := protected.halt with input as bash_event(cmd, ["/"])
	count(denials) > 0
}

# --------------------------------------------------------------------------
# Inline-script rule: repo-relative paths are not absolute protected paths.
#
# Regression for a same-family false positive hit while investigating the one
# above: reading the policy tree with `python3 -c` was denied with "(inline
# script mentions '/System/')" because `.cupcake/system/` contains "/system/"
# after case folding. The reference rule already required a path boundary; the
# inline-script rule used a bare substring test.
# --------------------------------------------------------------------------

test_allow_inline_script_reading_repo_relative_cupcake_system if {
	cmd := "python3 -c \"import glob; print(glob.glob('.cupcake/system/commands.rego'))\""
	denials := protected.halt with input as bash_event(cmd, [])
	count(denials) == 0
}

test_deny_inline_script_writing_absolute_etc if {
	cmd := "python3 -c \"open('/etc/passwd', 'w').write('x')\""
	denials := protected.halt with input as bash_event(cmd, [])
	"BUILTIN-PROTECTED-PATHS-SCRIPT" in rule_ids(denials)
}

test_deny_inline_script_writing_home_ssh if {
	cmd := "python3 -c \"open('~/.ssh/authorized_keys', 'a').write('k')\""
	denials := protected.halt with input as bash_event(cmd, [])
	"BUILTIN-PROTECTED-PATHS-SCRIPT" in rule_ids(denials)
}
