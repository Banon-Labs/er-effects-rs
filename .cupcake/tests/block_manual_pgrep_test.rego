# OPA unit tests for block_manual_pgrep.
#
# Not loaded by the cupcake engine (which scans .cupcake/policies/<harness>/
# and .cupcake/system/ only). Run with:
#   opa test .cupcake/policies/claude/block_manual_pgrep.rego \
#            .cupcake/tests/block_manual_pgrep_test.rego
# End-to-end engine coverage lives in scripts/test-cupcake-policies.py.
package cupcake.policies.claude.block_manual_pgrep_test

import rego.v1

import data.cupcake.policies.claude.block_manual_pgrep as guard

bash_event(cmd) := {
	"hook_event_name": "PreToolUse",
	"tool_name": "Bash",
	"tool_input": {"command": cmd, "timeout": 30000, "description": "test case"},
}

rule_ids(denials) := {d.rule_id | some d in denials}

denied(cmd) if {
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-BLOCK-MANUAL-PGREP" in rule_ids(denials)
}

# --- (a) Bare / shell-token pgrep is DENIED, no escape hatch -----------------

# The canonical false-negative form that blocked the overnight session.
test_deny_bare_pgrep_steam if {
	denied("pgrep -x steam")
}

test_deny_bare_pgrep_steamwebhelper if {
	denied("pgrep steamwebhelper")
}

# Piped into pgrep.
test_deny_piped_pgrep if {
	denied("foo | pgrep bar")
}

# && / || / ; separated.
test_deny_and_chained_pgrep if {
	denied("echo hi && pgrep -x steam")
}

test_deny_semicolon_chained_pgrep if {
	denied("echo hi; pgrep -x steam")
}

# $( ... ) command substitution.
test_deny_command_substitution_pgrep if {
	denied("count=$(pgrep -c steam)")
}

# Backtick command substitution.
test_deny_backtick_pgrep if {
	denied("count=`pgrep -c steam`")
}

# Absolute-path invocation must not bypass the guard.
test_deny_usr_bin_pgrep if {
	denied("/usr/bin/pgrep -x steam")
}

# Relative-path invocation must not bypass the guard.
test_deny_dot_slash_pgrep if {
	denied("./pgrep -x steam")
}

# No quote scrubbing: `bash -c 'pgrep ...'` cannot smuggle pgrep past the guard.
test_deny_bash_c_quoted_pgrep if {
	denied(`bash -c 'pgrep -x steam >/dev/null && echo up'`)
}

test_deny_sh_c_quoted_pgrep if {
	denied(`sh -c "pgrep -x steam"`)
}

# The exact WSL false-negative preflight shape (game/EAC process detection) is
# ALSO blocked now: on this box those are Windows processes, so pgrep is a false
# negative for them too. Detection must go through a WSL-aware check.
test_deny_runtime_preflight_pgrep_game_processes if {
	denied("if pgrep -x eldenring.exe >/dev/null || pgrep -x start_protected_game.exe >/dev/null; then echo running; fi")
}

test_deny_pgrep_start_protected_detection if {
	denied("pgrep -x start_protected_game.exe")
}

# --- (b) Negatives: things that must NOT be denied ---------------------------

# The sanctioned WSL-aware helper (its internal pgrep lives inside the script
# file, not in this agent Bash command, so it is naturally not intercepted).
test_allow_steam_running_helper if {
	not denied("bash scripts/steam-running.sh")
}

test_allow_steam_running_helper_direct if {
	not denied("scripts/steam-running.sh")
}

# A benign command with no pgrep token at all.
test_allow_benign_git_status if {
	not denied("git status")
}

# Word-boundary: a filename/word that merely CONTAINS "pgrep" is not a pgrep
# command and must not be denied.
test_allow_mypgrep_word if {
	not denied("mypgrep --help")
}

test_allow_mypgreptool_word if {
	not denied("./mypgreptool run")
}

test_allow_pgreptool_prefix_word if {
	not denied("pgreptool --version")
}

test_allow_mypgrep_in_path if {
	not denied("bash /home/choza/bin/mypgrep")
}

# Quotes ARE delimiters, so a quoted subprocess arg
# (`subprocess.run(['pgrep', ...])`) is ALSO caught. A python subprocess pgrep
# is still raw Linux pgrep, not a WSL-aware check, so there is no escape hatch.
test_deny_python_subprocess_pgrep_quoted_arg if {
	cmd := concat("\n", [
		"python3 - <<'PY'",
		"import subprocess",
		"for name in ['eldenring.exe','start_protected_game.exe']:",
		"    p = subprocess.run(['pgrep','-x',name], text=True, capture_output=True)",
		"    print(name, p.returncode)",
		"PY",
	])
	denied(cmd)
}

# --- (c) bd issue-tracker text mentions (bd er-effects-rs-uxyz) --------------
#
# False positive re-validated 2026-07-30 via opa eval before fixing: a bd
# close whose quoted --reason described launch-guard allow-tests was denied
# because the reason text mentioned the tool token. bd only records text; a
# single, non-chained bd invocation whose pgrep token sits entirely inside
# quoted text is exempt.

# The recorded false-positive family: a quoted --reason mentioning
# `pgrep -x start_protected_game.exe` while describing launch-guard tests.
test_allow_bd_close_reason_mentioning_pgrep if {
	not denied(`$HOME/.local/bin/bd close er-effects-rs-aaa --reason "launch-guard allow-test keeps pgrep -x start_protected_game.exe detection green"`)
}

test_allow_bd_remember_mentioning_pgrep if {
	not denied(`$HOME/.local/bin/bd remember --key steam-check "never use raw pgrep -x steam on this box; it false-negatives"`)
}

test_allow_bd_update_other_home_mentioning_pgrep if {
	not denied(`/home/choza/.local/bin/bd update er-effects-rs-1 --notes 'the guard denies bare pgrep everywhere'`)
}

test_allow_bare_bd_create_mentioning_pgrep if {
	not denied(`bd create "guard note" -d "manual pgrep is a WSL false negative" -t chore`)
}

# The ORIGINAL denied shape (2026-07-29) -- a chained `bash -c` batch of bd
# closes -- stays denied BY DESIGN: the exemption covers a single non-chained
# bd invocation only. Run bd invocations one at a time.
test_deny_chained_bash_c_bd_close_batch_mentioning_pgrep if {
	denied(`bash -c '"$HOME/.local/bin/bd" close er-effects-rs-aaa --reason "keeps pgrep -x start_protected_game.exe detection green" && "$HOME/.local/bin/bd" close er-effects-rs-bbb --reason "second"'`)
}

# Chained real pgrep after a bd text command must still deny.
test_deny_bd_close_then_chained_pgrep if {
	denied(`$HOME/.local/bin/bd close er-effects-rs-aaa --reason "done" && pgrep -x steam`)
}

test_deny_bd_close_semicolon_then_pgrep if {
	denied(`$HOME/.local/bin/bd close er-effects-rs-aaa --reason "done"; pgrep -x steam`)
}

# Command substitution inside the quoted reason EXECUTES; must still deny.
test_deny_bd_close_reason_command_substitution_pgrep if {
	denied(`$HOME/.local/bin/bd close er-effects-rs-aaa --reason "$(pgrep -x steam)"`)
}

test_deny_bd_close_reason_backtick_pgrep if {
	denied("$HOME/.local/bin/bd close er-effects-rs-aaa --reason \"`pgrep -x steam`\"")
}

# An UNQUOTED pgrep token in a bd command keeps the guard on.
test_deny_bd_close_unquoted_pgrep_token if {
	denied(`$HOME/.local/bin/bd close er-effects-rs-aaa --reason pgrep`)
}

# A non-bd command quoting pgrep is unchanged: still denied, no quote scrub.
test_deny_echo_quoted_pgrep_still_denied if {
	denied(`echo "pgrep -x steam"`)
}

# Non-Bash tools are out of scope for this Bash-command guard.
test_allow_non_bash_tool if {
	denials := guard.deny with input as {
		"hook_event_name": "PreToolUse",
		"tool_name": "Write",
		"tool_input": {"file_path": "/tmp/x", "content": "pgrep -x steam"},
	}
	count(denials) == 0
}

# Non-PreToolUse events are out of scope.
test_allow_non_pretooluse_event if {
	denials := guard.deny with input as {
		"hook_event_name": "PostToolUse",
		"tool_name": "Bash",
		"tool_input": {"command": "pgrep -x steam"},
	}
	count(denials) == 0
}
