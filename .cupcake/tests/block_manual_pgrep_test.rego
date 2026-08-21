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

# --- (d) git commit message text mentions -----------------------------------
#
# False positive 2026-08-12: `git commit -F - <<'EOF' ... EOF` was denied
# because the MESSAGE PROSE described removing a raw process-name probe call
# from a shell script. Nothing executed the probe -- the token appeared only as
# descriptive English in the message body, which git reads from stdin and
# records. The agent worked around it by writing the message to a file, i.e. an
# escape hatch around a guard, which is what this exemption removes the need
# for. Same shape as bash_elden_ring_launch_guard's git_commit_text_command.

# The recorded false-positive shape: a `-F -` heredoc commit message whose
# prose names the probe it removed.
test_allow_git_commit_dash_f_heredoc_message_mentioning_pgrep if {
	not denied(concat("\n", [
		"git commit -F - <<'EOF'",
		"preflight: stop probing the process table by name",
		"",
		"The preflight called pgrep -x steam directly, which false-negatives on",
		"this WSL2 + Windows-Steam box. It now sources scripts/steam-running.sh",
		"and calls steam_running instead.",
		"EOF",
	]))
}

# The same message via `git add -A && git commit -F -`.
test_allow_git_add_then_commit_heredoc_message_mentioning_pgrep if {
	not denied(concat("\n", [
		"git add -A && git commit -F - <<'EOF'",
		"guard: drop the raw pgrep -x steam call from the launch preflight",
		"EOF",
	]))
}

# `git -C <worktree>` is the invocation AGENTS.md mandates for worktree-scoped
# work, so it must not disable the exemption.
test_allow_git_c_worktree_commit_heredoc_message_mentioning_pgrep if {
	not denied(concat("\n", [
		"git -C /home/banon/projects/er-effects-rs/.worktrees/guard commit -F - <<'EOF'",
		"scripts: replace pgrep -x steam with scripts/steam-running.sh",
		"EOF",
	]))
}

# A double-quoted heredoc tag is equally literal.
test_allow_git_commit_heredoc_double_quoted_tag_mentioning_pgrep if {
	not denied(concat("\n", [
		`git commit -F - <<"MSG"`,
		"scripts: replace pgrep -x steam with the WSL-aware helper",
		"MSG",
	]))
}

# The plain `-m` form: the token sits inside the quoted message.
test_allow_git_commit_dash_m_message_mentioning_pgrep if {
	not denied(`git commit -m "preflight: drop the raw pgrep -x steam probe in favour of scripts/steam-running.sh"`)
}

test_allow_git_add_then_commit_dash_m_message_mentioning_pgrep if {
	not denied(`git add -A && git commit -m "guard: document why pgrep -x steam is a WSL false negative"`)
}

# The canonical `-m "$(cat <<'TAG' ...)"` message-substitution form.
test_allow_git_commit_cat_heredoc_substitution_message_mentioning_pgrep if {
	not denied(concat("\n", [
		`git commit -m "$(cat <<'EOF'`,
		"guard: stop calling pgrep -x steam in the runtime preflight",
		"",
		"scripts/steam-running.sh is the sanctioned WSL-aware check.",
		"EOF",
		`)"`,
	]))
}

# --- (d2) ... and the carve-out must not become an escape hatch --------------
#
# Every case below LOOKS like a commit-message command but reaches a real,
# executing process-name probe. All must stay denied.

# Shell chained after the heredoc terminator (the shape the launch guard's
# post-terminator scan exists for).
test_deny_git_commit_heredoc_then_newline_chained_pgrep if {
	denied(concat("\n", [
		"git commit -F - <<'EOF'",
		"guard: drop the raw pgrep -x steam probe",
		"EOF",
		"pgrep -x steam",
	]))
}

test_deny_git_commit_heredoc_then_semicolon_chained_pgrep if {
	denied(concat("\n", [
		"git commit -F - <<'EOF'",
		"guard: drop the raw process probe",
		"EOF",
		"; pgrep -x steam",
	]))
}

test_deny_git_commit_heredoc_piped_into_shell if {
	denied(concat("\n", [
		"git commit -F - <<'EOF'",
		"guard: drop the raw process probe",
		"EOF",
		"| xargs pgrep",
	]))
}

# An unquoted heredoc tag leaves the body subject to expansion, so its contents
# are not inert text.
test_deny_git_commit_heredoc_unquoted_tag if {
	denied(concat("\n", [
		"git commit -F - <<EOF",
		"guard: drop the raw pgrep -x steam probe",
		"EOF",
	]))
}

# A second heredoc carries a second command.
test_deny_git_commit_heredoc_then_second_heredoc_probe if {
	denied(concat("\n", [
		"git commit -F - <<'EOF'",
		"guard: drop the raw process probe",
		"EOF",
		"python3 - <<'PY'",
		"import subprocess; subprocess.run(['pgrep','-x','steam'])",
		"PY",
	]))
}

# The commit is not the command: `bash -c` executes the whole string.
test_deny_bash_c_wrapped_git_commit_heredoc if {
	denied(concat("\n", [
		`bash -c "git commit -F - <<'EOF'`,
		"guard: drop the raw pgrep -x steam probe",
		"EOF",
		`"`,
	]))
}

# Command substitution in the message EXECUTES.
test_deny_git_commit_message_command_substitution_pgrep if {
	denied(`git commit -m "$(pgrep -x steam)"`)
}

test_deny_git_commit_message_backtick_pgrep if {
	denied("git commit -m \"`pgrep -x steam`\"")
}

# ... including inside the `$(cat <<'TAG' ...)` message-substitution shape.
test_deny_git_commit_cat_substitution_with_trailing_probe if {
	denied(concat("\n", [
		`git commit -m "$(cat <<'EOF'`,
		"guard: drop the raw process probe",
		"EOF",
		`; pgrep -x steam)"`,
	]))
}

# && / ; chained probe after a plain -m commit.
test_deny_git_commit_dash_m_then_and_chained_pgrep if {
	denied(`git commit -m "guard: drop the raw process probe" && pgrep -x steam`)
}

test_deny_git_commit_dash_m_then_semicolon_chained_pgrep if {
	denied(`git commit -m "guard: drop the raw process probe"; pgrep -x steam`)
}

# An UNQUOTED token in a git commit command keeps the guard on.
test_deny_git_commit_unquoted_pgrep_token if {
	denied("git commit -am pgrep")
}

# A non-commit git subcommand gets no exemption.
test_deny_git_status_then_chained_pgrep if {
	denied("git status --short && pgrep -x steam")
}

# The `git -C <path>` surface cannot smuggle a separator: the accepted path is
# separator-free, so a quoted path carrying `; <probe>` fails the shape.
test_deny_git_c_path_with_embedded_separator_probe if {
	denied(concat("\n", [
		`git -C "/tmp/x; pgrep -x steam" commit -F - <<'EOF'`,
		"guard: drop the raw process probe",
		"EOF",
	]))
}

# An `add` step chained with a non-git command is not the git-only shape.
test_deny_git_add_then_probe_then_commit_heredoc if {
	denied(concat("\n", [
		"git add -A && pgrep -x steam && git commit -F - <<'EOF'",
		"guard: drop the raw process probe",
		"EOF",
	]))
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
