# OPA unit tests for bash_elden_ring_launch_guard.
#
# Not loaded by the cupcake engine (which scans .cupcake/policies/<harness>/
# and .cupcake/system/ only). Run with:
#   opa test .cupcake/policies/claude/bash_elden_ring_launch_guard.rego \
#            .cupcake/tests/bash_elden_ring_launch_guard_test.rego
# End-to-end engine coverage lives in scripts/test-cupcake-policies.py.
package cupcake.policies.bash_elden_ring_launch_guard_test

import rego.v1

import data.cupcake.policies.bash_elden_ring_launch_guard as guard

bash_event(cmd) := {
	"hook_event_name": "PreToolUse",
	"tool_name": "Bash",
	"tool_input": {"command": cmd, "timeout": 30000, "description": "test case"},
}

bash_event_with_description(cmd, desc) := {
	"hook_event_name": "PreToolUse",
	"tool_name": "Bash",
	"tool_input": {"command": cmd, "timeout": 30000, "description": desc},
}

rule_ids(denials) := {d.rule_id | some d in denials}

# --- (a) Pure bd text commands mentioning forbidden forms are ALLOWED -------

# The 2026-07-04 false positive: bd create with the EAC launcher named inside
# quoted -d prose that also contains a generic executable marker word (bash).
test_allow_bd_create_mentioning_eac_launcher if {
	cmd := `/home/banon/.local/bin/bd create "me3 launch path" -d "me3 Linux launch via bash scripts must not use forbidden forms (steam -applaunch / steam:// URLs / start_protected_game.exe)." -t task -p 1`
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

test_allow_bd_remember_mentioning_eac_launcher if {
	cmd := `/home/banon/.local/bin/bd remember --key k 'never launch start_protected_game.exe from bash or python wrappers'`
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

test_allow_bd_create_mentioning_steam_applaunch_appid if {
	cmd := `/home/banon/.local/bin/bd create "launch policy" -d "steam -applaunch 1245620 is a forbidden form; drive it from bash probes instead" -t task`
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

# GitHub PR body text may document the forbidden launcher while using a real
# body-file payload. The text is not an execution path.
test_allow_gh_pr_create_body_file_mentioning_eac_launcher if {
	cmd := concat("\n", [
		"tmp_body=$(mktemp)",
		"cat > \"$tmp_body\" <<'EOF'",
		"Policy note: start_protected_game.exe remains forbidden; python tests passed.",
		"EOF",
		"gh pr create --base main --head branch --title t --body-file \"$tmp_body\"",
	])
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

test_allow_bd_remember_mentioning_ersc_bundle_words if {
	cmd := `/home/banon/.local/bin/bd remember --key k 'do not bundle or stage ersc.dll into release artifacts'`
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

# ---------------------------------------------------------------------------
# 2026-08-04 false positive: NAMING the launcher as data was denied.
#
# The blocked command recorded a knowledge-base memory ABOUT the launcher --
# that /proc/<pid>/comm truncates at 15 chars, so an exact-match entry for the
# 24-char name could never match a process. The substring fallback asked only
# "payload contains the name" plus a generic marker word, and "bash" in prose
# is such a word. Naming a file is not running it: AGENTS.md forbids LAUNCHING
# the EAC launcher while explicitly permitting detection of stale
# start_protected_game.exe processes, so the rule has to stay documentable in
# the repo that enforces it.
# ---------------------------------------------------------------------------

# The exact denied command shape. Two independent defects: the bd text
# exemption's shape did not anticipate a QUOTED binary path (the leading `"`),
# and the fallback denied any payload naming the launcher next to a generic
# marker word.
test_allow_bd_remember_quoted_binary_path_naming_eac_launcher if {
	cmd := `"$HOME/.local/bin/bd" remember "er-stale-run-sentinel compared start_protected_game.exe (24 chars) verbatim against /proc/<pid>/comm, which the kernel truncates at 15 chars, so that entry could never match any process; run bash scripts/er-stale-run-sentinel.sh --selftest" --key stale-sentinel-comm-truncation-2026-08-04`
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

# Pipes disable the structural data exemption (see the deny case below), so
# this shape can only pass through the bd text exemption -- it pins the quoted
# binary path specifically.
test_allow_bd_remember_quoted_binary_path_with_pipes_in_prose if {
	cmd := `"$HOME/.local/bin/bd" remember "comm cap 15 | start_protected_game.exe is 24 | the entry never matched | reproduce with bash" --key stale-sentinel-comm-truncation-2026-08-04`
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

test_allow_bd_close_quoted_binary_path_naming_eac_launcher if {
	cmd := `"/home/banon/.local/bin/bd" close er-effects-rs-aaa --reason "sentinel now truncates start_protected_game.exe before comparing; verified from bash"`
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

# Printing the name is not running it.
test_allow_echo_prose_naming_eac_launcher if {
	cmd := `echo 'the sentinel detects start_protected_game.exe; never launch it from bash or proton wrappers'`
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

test_allow_printf_prose_naming_eac_launcher if {
	cmd := `printf '%s\n' 'stale-run sentinel comm list: eldenring.exe, me3, start_protected_game.exe (matched from bash)'`
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

# Parentheses around the name are prose punctuation, not a subshell: the
# statement is still headed by the inert command.
test_allow_echo_parenthesised_launcher_name if {
	cmd := `echo 'the EAC launcher (start_protected_game.exe) must never be started from bash'`
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

# An inline interpreter program may hold the name as its own nested string
# literal even when it never touches /proc.
test_allow_python_c_string_literal_naming_eac_launcher if {
	cmd := `python3 -c "print('sentinel comm entry: start_protected_game.exe truncated to 15 chars')"`
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

# Searching the repo's own sentinel for the comm name it detects.
test_allow_grep_launcher_name_in_sentinel_script if {
	cmd := `grep -n 'start_protected_game.exe' scripts/er-stale-run-sentinel.sh; echo 'run the sentinel with bash'`
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

# Several mentions, every one of them an argument of an inert command.
test_allow_multiple_inert_statements_naming_eac_launcher if {
	cmd := `echo 'comm cap hides start_protected_game.exe'; printf '%s\n' 'so start_protected_game.exe needs truncating in bash too'`
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

# --- ... and naming-as-data must not become a launch bypass ------------------

# An inert first statement does not launder a launch in the second: the
# statement that names the launcher there is headed by `setsid`, which is not
# an inert command. (No regex arm covers this shape -- `setsid` is not a
# recognised wrapper verb and the name is not at command position -- so this
# pins the fallback itself.)
test_deny_echo_prose_then_setsid_bare_launcher if {
	cmd := `echo 'do not run start_protected_game.exe from bash'; setsid start_protected_game.exe`
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

test_deny_echo_prose_then_nohup_quoted_launcher_path if {
	cmd := `echo 'note about start_protected_game.exe written from bash'; nohup '/opt/er/start_protected_game.exe' &`
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

# An inert head does not make a PIPE inert: `echo <name> | bash` executes it.
test_deny_echo_launcher_name_piped_into_shell if {
	cmd := `echo 'start_protected_game.exe' | bash`
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

# A command substitution inside the inert command's quoted argument is expanded
# by the shell before the inert command ever runs.
test_deny_echo_with_command_substitution_launcher if {
	cmd := `echo "$(bash -c /opt/er/start_protected_game.exe)"`
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

# A nested string literal alongside an exec mechanism is an argv element, not
# data -- the no-exec-mechanism condition is what makes shape (II) safe.
test_deny_python_c_nested_literal_with_os_system if {
	cmd := `python3 -c "import os; n = 'start_protected_game.exe'; os.system('/opt/er/' + n)"`
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

# The exemption reads the COMMAND; it cannot be claimed while a non-command
# tool field carries the launcher name.
test_deny_launcher_named_in_non_command_tool_field if {
	event := bash_event_with_description(
		`echo 'a note written from bash'`,
		"probe start_protected_game.exe",
	)
	denials := guard.deny with input as event
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

# The widened bd path match must not leak the exemption to a chained launch.
test_deny_bd_quoted_binary_path_chained_with_wine_launch if {
	cmd := `"$HOME/.local/bin/bd" remember "note" --key k; wine /opt/er/start_protected_game.exe`
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

test_deny_bd_quoted_binary_path_chained_with_bare_launcher if {
	cmd := `"$HOME/.local/bin/bd" remember "note about bash" --key k && start_protected_game.exe --offline`
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

# ---------------------------------------------------------------------------
# 2026-08-04 false positive #2: EDITING A DOCSTRING that names the launcher was
# denied.
#
# The blocked command was a `python3 - <<'PY'` heredoc that removed this
# sentence from scripts/frida-dump-module.py's module docstring:
#
#     * Offline `eldenring.exe` ONLY. Refuses `start_protected_game.exe` / EAC,
#       like the sibling `frida-nudge.py`.
#
# Root cause: the substring fallback arm. `scrubbed_command` drops the heredoc
# BODY, so no command-position regex can see anything; the fallback instead
# scans the RAW payload and denies on "the name occurs" plus
# `executable_source_marker` -- a set of generic words that includes "python",
# which the interpreter's own name always supplies. So every python heredoc
# whose body names the launcher was denied regardless of what the program did.
#
# This repo deliberately writes refusal logic and safety documentation that
# NAMES the forbidden binary, so that made its own safety text uneditable.
# ---------------------------------------------------------------------------

# The exact denied command (the docstring keeps its markdown backticks, which is
# why the exemption cannot simply ban backticks in a quoted-tag heredoc body:
# `<<'PY'` makes the body fully literal, so nothing there expands).
test_allow_python_heredoc_editing_docstring_naming_eac_launcher if {
	cmd := concat("\n", [
		"python3 - <<'PY'",
		"from pathlib import Path",
		"p = Path('scripts/frida-dump-module.py')",
		"s = p.read_text(encoding='utf-8')",
		"old = \"\"\"* Offline `eldenring.exe` ONLY. Refuses `start_protected_game.exe` / EAC, like the sibling",
		"  `frida-nudge.py`.",
		"\"\"\"",
		"assert old in s",
		"p.write_text(s.replace(old, ''), encoding='utf-8')",
		"print('docstring line removed')",
		"PY",
	])
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

# The live engine delivers the command whitespace-flattened; same verdict.
test_allow_python_heredoc_editing_docstring_flattened if {
	cmd := "python3 - <<'PY' from pathlib import Path p = Path('scripts/frida-dump-module.py') s = p.read_text(encoding='utf-8') old = \"\"\"* Offline `eldenring.exe` ONLY. Refuses `start_protected_game.exe` / EAC, like the sibling `frida-nudge.py`. \"\"\" assert old in s p.write_text(s.replace(old, ''), encoding='utf-8') PY"
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

# A worktree `cd` prefix before the same edit is still an edit.
test_allow_cd_prefixed_python_heredoc_editing_repo_file if {
	cmd := concat("\n", [
		"cd /home/banon/projects/er-effects-rs && python3 - <<'PY'",
		"from pathlib import Path",
		"p = Path('scripts/frida-dump-module.py')",
		"p.write_text(p.read_text().replace('Refuses start_protected_game.exe / EAC, ', ''))",
		"PY",
	])
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

# Adding a safety COMMENT that names the binary it refuses.
test_allow_python_heredoc_writing_refusal_comment_naming_eac_launcher if {
	cmd := concat("\n", [
		"python3 - <<'PY'",
		"from pathlib import Path",
		"p = Path('scripts/frida-dump-module.py')",
		"lines = p.read_text().splitlines()",
		"lines.insert(1, '# Refuses start_protected_game.exe / EAC targets by design.')",
		"p.write_text('\\n'.join(lines))",
		"PY",
	])
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

# READING a repo file that contains the name, from the same inline-program shape.
test_allow_python_heredoc_reading_file_containing_launcher_name if {
	cmd := concat("\n", [
		"python3 - <<'PY'",
		"from pathlib import Path",
		"for line in Path('scripts/er-stale-run-sentinel.sh').read_text().splitlines():",
		"    if 'start_protected_game.exe' in line:",
		"        print(line)",
		"PY",
	])
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

# The `python3 -c` twin of the same edit.
test_allow_python_c_rewriting_repo_file_naming_eac_launcher if {
	cmd := `python3 -c "import pathlib; p = pathlib.Path('scripts/frida-dump-module.py'); p.write_text(p.read_text().replace('Refuses start_protected_game.exe / EAC, ', ''))"`
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

# Non-interpreter editors/readers operating on a repo file that names the
# launcher (these already passed; they pin the other half of the distinction).
test_allow_sed_inplace_edit_of_file_naming_eac_launcher if {
	cmd := `sed -i 's/start_protected_game.exe/REDACTED/' scripts/er-stale-run-sentinel.sh`
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

test_allow_sed_print_matching_lines_naming_eac_launcher if {
	cmd := `sed -n '/start_protected_game.exe/p' scripts/er-stale-run-sentinel.sh`
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

# --- ... and the inline-program exemption must not become a launch bypass ----

# An edit that also launches: the exec mechanism keeps the exemption off.
test_deny_python_heredoc_file_edit_plus_subprocess_launch if {
	cmd := concat("\n", [
		"python3 - <<'PY'",
		"import subprocess",
		"from pathlib import Path",
		"Path('scripts/frida-dump-module.py').write_text('# start_protected_game.exe')",
		"subprocess.run(['wine', '/opt/er/start_protected_game.exe'])",
		"PY",
	])
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

# Dynamic dispatch reconstructs an exec call out of fragments that no literal
# marker matches (`getattr(os, 'sy' + 'stem')`). The exemption additionally
# refuses attribute/namespace lookup verbs for exactly this shape.
test_deny_python_heredoc_file_edit_with_getattr_obfuscated_exec if {
	cmd := concat("\n", [
		"python3 - <<'PY'",
		"import os",
		"from pathlib import Path",
		"Path('scripts/frida-dump-module.py').write_text('# note')",
		"launcher = 'start_protected_game.exe'",
		"getattr(os, 'sy' + 'stem')('/opt/er/' + launcher)",
		"PY",
	])
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

test_deny_python_heredoc_file_edit_with_globals_obfuscated_exec if {
	cmd := concat("\n", [
		"python3 - <<'PY'",
		"import os",
		"open('scripts/note.txt', 'w').write('start_protected_game.exe')",
		"globals()['o' + 's'].system('/opt/er/start_protected_game.exe')",
		"PY",
	])
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

# Shell riding after the heredoc terminator breaks the single-program shape.
test_deny_python_heredoc_file_edit_then_trailing_launch if {
	cmd := concat("\n", [
		"python3 - <<'PY'",
		"from pathlib import Path",
		"Path('scripts/frida-dump-module.py').write_text('# start_protected_game.exe')",
		"PY",
		"setsid '/opt/er/start_protected_game.exe'",
	])
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

# An UNQUOTED heredoc tag lets the shell expand the body, so a command
# substitution in there really runs. The exemption requires a quoted tag.
test_deny_python_heredoc_unquoted_tag_with_command_substitution if {
	cmd := concat("\n", [
		"python3 - <<PY",
		"print('editing')",
		"$( '/opt/er/start_protected_game.exe' )",
		"PY",
	])
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

# A backtick substitution in the `cd` prefix is real shell, outside the literal
# heredoc body.
test_deny_backtick_substitution_in_cd_prefix_before_python_heredoc if {
	cmd := concat("\n", [
		"cd `cat /tmp/dir` && python3 - <<'PY'",
		"from pathlib import Path",
		"Path('x.py').write_text('start_protected_game.exe')",
		"PY",
	])
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

# A chained launch after the `-c` edit breaks the single-program shape.
test_deny_python_c_file_edit_chained_with_wine_launch if {
	cmd := `python3 -c "open('x.py','w').write('start_protected_game.exe')"; wine /opt/er/start_protected_game.exe`
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

# The name reaching the SHELL unquoted is a command word, not program text.
test_deny_python_heredoc_edit_with_unquoted_launcher_outside_heredoc if {
	cmd := concat("\n", [
		"python3 - <<'PY' /opt/er/start_protected_game.exe",
		"from pathlib import Path",
		"Path('x.py').write_text('note')",
		"PY",
	])
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

# ---------------------------------------------------------------------------
# Shell riding on the heredoc REDIRECTION LINE (found 2026-08-04 while adding
# the exemption above; the `/proc` heredoc exemption had the same hole). Every
# heredoc shape checked the region before `<<` and the region after the
# terminator, but not the region between the tag and the body -- and
# `python3 - <<'PY' | bash` feeds the PROGRAM'S OUTPUT to a shell, so a path the
# program merely prints really executes.
# ---------------------------------------------------------------------------

test_deny_python_heredoc_edit_piped_into_shell if {
	cmd := concat("\n", [
		"python3 - <<'PY' | bash",
		"print(\"'/opt/er/start_protected_game.exe'\")",
		"PY",
	])
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

test_deny_proc_scan_heredoc_piped_into_shell if {
	cmd := concat("\n", [
		"python3 - <<'PY' | bash",
		"print(open('/proc/1/comm').read())",
		"print(\"'/opt/er/start_protected_game.exe'\")",
		"PY",
	])
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

# A plain output redirect on that line is not an execution path and stays
# allowed -- the refusal is scoped to chaining operators.
test_allow_python_heredoc_edit_with_plain_output_redirect if {
	cmd := concat("\n", [
		"python3 - <<'PY' > /tmp/edit.log",
		"from pathlib import Path",
		"p = Path('scripts/frida-dump-module.py')",
		"p.write_text(p.read_text().replace('Refuses start_protected_game.exe / EAC, ', ''))",
		"PY",
	])
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

# ... but a redirect followed by a pipe is still a pipe.
test_deny_python_heredoc_edit_redirect_then_piped_into_shell if {
	cmd := concat("\n", [
		"python3 - <<'PY' 2>/dev/null | bash",
		"print(\"'/opt/er/start_protected_game.exe'\")",
		"PY",
	])
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

# Process substitution on the redirection line executes its own command.
test_deny_python_heredoc_edit_with_process_substitution if {
	cmd := concat("\n", [
		"python3 - <<'PY' > >(wine /opt/er/start_protected_game.exe)",
		"print('note')",
		"PY",
	])
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

# The exemption reads the COMMAND; a non-command tool field naming the launcher
# keeps the guard on.
test_deny_python_heredoc_edit_with_launcher_in_description_field if {
	event := bash_event_with_description(
		concat("\n", [
			"python3 - <<'PY'",
			"from pathlib import Path",
			"Path('scripts/frida-dump-module.py').write_text('# note')",
			"PY",
		]),
		"probe start_protected_game.exe",
	)
	denials := guard.deny with input as event
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

# --- (b) Exact stale-process detection is ALLOWED ---------------------------

# Process detection is explicitly allowed by repo policy; only launching the
# EAC/protected executable is forbidden.
test_allow_pgrep_start_protected_detection if {
	denials := guard.deny with input as bash_event("pgrep -x start_protected_game.exe")
	count(denials) == 0
}

# Regression for the 2026-07-04 manual me3 smoke false positive: a preflight
# guard may check both the approved direct runtime and the stale protected
# launcher before refusing to mix process ownership.
test_allow_runtime_preflight_pgrep_start_protected_detection if {
	cmd := `if pgrep -x eldenring.exe >/dev/null || pgrep -x start_protected_game.exe >/dev/null; then echo 'already running'; exit 2; fi`
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

# The pgrep allowance must not hide a later real protected-launch token.
test_deny_pgrep_then_proton_start_protected_launch if {
	cmd := `pgrep -x start_protected_game.exe >/dev/null; proton run /tmp/start_protected_game.exe`
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

# Regression for the 2026-07-07 false positive in the runtime-probe preflight:
# a shell Steam status check followed by Python exact `pgrep -x <name>` process
# detection is still read-only and must not be treated as launching EAC.
test_allow_shell_steam_status_then_python_subprocess_pgrep_start_protected_detection if {
	cmd := concat("\n", [
		"pgrep -x steam >/dev/null && echo steam-running || echo steam-missing; python3 - <<'PY'",
		"import subprocess",
		"for name in ['eldenring.exe','start_protected_game.exe']:",
		"    p = subprocess.run(['pgrep','-x',name], text=True, capture_output=True)",
		"    print(name, p.returncode)",
		"PY",
	])
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

# The live cupcake command string can arrive with the pre-heredoc semicolon
# absent in the policy source text; keep that equivalent read-only shape allowed.
test_allow_shell_steam_status_then_python_subprocess_pgrep_start_protected_detection_semicolonless_source if {
	cmd := concat("\n", [
		"pgrep -x steam >/dev/null && echo steam-running || echo steam-missing python3 - <<'PY'",
		"import subprocess",
		"for name in ['eldenring.exe','start_protected_game.exe']:",
		"    p = subprocess.run(['pgrep','-x',name], text=True, capture_output=True)",
		"    print(name, p.returncode)",
		"PY",
	])
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

# Regression for the 2026-07-05 false positive in the runtime-probe preflight:
# Python may shell out to exact `pgrep -x <name>` checks for process status,
# including stale EAC launcher detection, without launching the named process.
test_allow_python_subprocess_pgrep_start_protected_detection if {
	cmd := concat("\n", [
		"python3 - <<'PY'",
		"import subprocess",
		"for name in ['steam', 'eldenring.exe', 'start_protected_game.exe']:",
		"    subprocess.run(['pgrep', '-x', name])",
		"PY",
	])
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

# Regression for the 2026-07-07 false positive: assigning the pgrep result
# for later status inspection is still read-only process detection, not a
# protected-game launch.
test_allow_python_subprocess_pgrep_assignment_start_protected_detection if {
	cmd := concat("\n", [
		"python3 - <<'PY'",
		"import subprocess, os",
		"names=['eldenring.exe','start_protected_game.exe']",
		"for name in names:",
		"    p=subprocess.run(['pgrep','-x',name], text=True, capture_output=True)",
		"    print(name, p.returncode)",
		"PY",
	])
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

# Regression for the 2026-07-14 false positive: after an approved offline/me3
# launch, runtime validation may wait briefly before a read-only Python exact
# pgrep check for stale protected-launcher processes.
test_allow_sleep_then_python_subprocess_pgrep_start_protected_detection if {
	cmd := concat("\n", [
		"sleep 5; python3 - <<'PY'",
		"import subprocess",
		"p = subprocess.run(['pgrep','-x','start_protected_game.exe'], text=True, capture_output=True)",
		"print(p.returncode)",
		"PY",
	])
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

# The sleep-prefixed Python allowance must not hide a later launch-shaped
# subprocess call.
test_deny_sleep_then_python_subprocess_pgrep_then_proton_launch if {
	cmd := concat("\n", [
		"sleep 5; python3 - <<'PY'",
		"import subprocess",
		"subprocess.run(['pgrep','-x','start_protected_game.exe'])",
		"subprocess.run(['proton', 'run', 'start_protected_game.exe'])",
		"PY",
	])
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

# The Python pgrep allowance must stay limited to the single pgrep call; a
# later launch-shaped subprocess call keeps the protected-launch guard active.
test_deny_python_subprocess_pgrep_then_proton_launch if {
	cmd := concat("\n", [
		"python3 - <<'PY'",
		"import subprocess",
		"for name in ['steam', 'eldenring.exe', 'start_protected_game.exe']:",
		"    subprocess.run(['pgrep', '-x', name])",
		"subprocess.run(['proton', 'run', 'start_protected_game.exe'])",
		"PY",
	])
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

# Capturing a launch-shaped subprocess result remains a launch form, not a
# process status check.
test_deny_python_subprocess_assignment_proton_launch if {
	cmd := concat("\n", [
		"python3 - <<'PY'",
		"import subprocess",
		"p=subprocess.run(['proton', 'run', 'start_protected_game.exe'])",
		"print(p.returncode)",
		"PY",
	])
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

# A quoted executable path in subprocess args is a launch form, not a process
# status check, even though it is inside a single-python heredoc.
test_deny_python_subprocess_direct_quoted_launcher if {
	cmd := concat("\n", [
		"python3 - <<'PY'",
		"import subprocess",
		"subprocess.run(['/opt/er/start_protected_game.exe'])",
		"PY",
	])
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

# --- (b') Read-only /proc comm scans naming the launcher are ALLOWED --------

# The 2026-07-05 false positive: a python heredoc scanning /proc/<pid>/comm
# and comparing each comm against a tuple of process names, with the EAC
# launcher named only inside quoted string literals. pgrep is banned for
# process checks (it self-matches its own command line), so this is the
# sanctioned detection form and must not be denied.
test_allow_proc_comm_scan_heredoc_naming_eac_launcher if {
	cmd := concat("\n", [
		"python3 - <<'PY'",
		"import glob",
		"names = ('steam', 'eldenring.exe', 'start_protected_game.exe')",
		"found = {n: False for n in names}",
		"for path in glob.glob('/proc/[0-9]*/comm'):",
		"    try:",
		"        comm = open(path).read().strip()",
		"    except OSError:",
		"        continue",
		"    if comm in names:",
		"        found[comm] = True",
		"for n in names:",
		"    print(n, 'up' if found[n] else 'down')",
		"PY",
	])
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

# Exact blocked command from the 2026-07-05 cupcake false positive: a worktree
# `cd` prefix before the same read-only /proc/<pid>/comm scan is still process
# detection, not a protected-game launch.
test_allow_cd_prefixed_proc_comm_scan_exact_false_positive if {
	cmd := concat("\n", [
		"cd .worktrees/steam-screenshot-boot-bg && python3 - <<'PY'",
		"import glob",
		"names={'steam','eldenring.exe','start_protected_game.exe'}",
		"found={n:[] for n in names}",
		"for path in glob.glob('/proc/[0-9]*/comm'):",
		"    try:",
		"        pid=int(path.split('/')[2]); comm=open(path).read().strip()",
		"    except (OSError,ValueError):",
		"        continue",
		"    if comm in names:",
		"        found[comm].append(pid)",
		"for n in sorted(names):",
		"    print(n, found[n])",
		"PY",
	])
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

# Exact 2026-07-13 false positive: shell pgrep checks Steam, then a Python heredoc scans
# /proc/<pid>/comm for exact game/EAC process names. Cupcake receives this shape whitespace-flattened;
# with no semicolon before `python3`, it is still read-only process detection, not a launch.
test_allow_steam_pgrep_prefixed_proc_comm_scan_flattened_false_positive if {
	cmd := `pgrep -x steam >/dev/null && echo steam-running || echo steam-missing python3 - <<'PY' import os for name in ('eldenring.exe','start_protected_game.exe'): found=[] for pid in filter(str.isdigit, os.listdir('/proc')): try: comm=open(f'/proc/{pid}/comm',encoding='utf-8',errors='replace').read().strip() except OSError: continue if comm == name: found.append(pid) print(f'{name}:', 'running '+','.join(found) if found else 'not-running') PY`
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

# Same detection intent as a `python3 -c` one-liner with the whole program
# quoted (the RTK-caveat-sanctioned inspection form).
test_allow_proc_comm_scan_python_c_naming_eac_launcher if {
	cmd := `python3 -c 'import glob; print(any(open(p).read().strip() == "start_protected_game.exe" for p in glob.glob("/proc/[0-9]*/comm")))'`
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

# Regression for er-effects-rs-9iz: exact /proc process teardown for stale
# Elden Ring/EAC launcher processes is allowed. It reads /proc, matches only
# exact comm names, and sends SIGTERM/SIGKILL to those pids; it never launches
# the named executable.
test_allow_proc_comm_scan_sigterm_sigkill_cleanup_naming_eac_launcher if {
	cmd := concat("\n", [
		"python3 - <<'PY'",
		"import glob, os, signal",
		"names = {'eldenring.exe', 'start_protected_game.exe'}",
		"for path in glob.glob('/proc/[0-9]*/comm'):",
		"    try:",
		"        pid = int(path.split('/')[2])",
		"        comm = open(path).read().strip()",
		"    except (OSError, ValueError):",
		"        continue",
		"    if comm in names:",
		"        for sig in (signal.SIGTERM, signal.SIGKILL):",
		"            try:",
		"                os.kill(pid, sig)",
		"            except OSError:",
		"                pass",
		"PY",
	])
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

# ---------------------------------------------------------------------------
# 2026-07-28 false positive: the sanctioned Steam preflight followed by the
# sanctioned /proc process check was denied. Nothing launches -- the launcher
# name is a python string literal in a membership test -- but the exemptions
# whitelist whole-command SHAPES, so the extra read-only statement dropped the
# payload into the raw substring fallback. Deny now requires the name to occur
# somewhere a shell/interpreter could actually exec it.
# ---------------------------------------------------------------------------

# The exact denied command.
test_allow_steam_helper_then_proc_comm_scan_exact_false_positive if {
	cmd := concat("\n", [
		`cd /home/banon/projects/er-effects-rs; bash -c 'source scripts/steam-running.sh && if steam_running; then echo "STEAM: running"; else echo "STEAM: NOT running"; fi'`,
		`python3 -c "`,
		"import os,glob",
		"hits=[]",
		"for p in glob.glob('/proc/[0-9]*'):",
		"    try: comm=open(os.path.join(p,'comm')).read().strip()",
		"    except Exception: continue",
		"    if comm in ('eldenring.exe','me3','start_protected_game.exe'): hits.append(comm)",
		"print('game running:', hits if hits else 'none')",
		`"`,
	])
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

# The same /proc scan on its own.
test_allow_proc_comm_scan_python_c_double_quoted_program if {
	cmd := concat("\n", [
		`python3 -c "`,
		"import os,glob",
		"hits=[]",
		"for p in glob.glob('/proc/[0-9]*'):",
		"    try: comm=open(os.path.join(p,'comm')).read().strip()",
		"    except Exception: continue",
		"    if comm in ('eldenring.exe','me3','start_protected_game.exe'): hits.append(comm)",
		"print('game running:', hits if hits else 'none')",
		`"`,
	])
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

# The live engine delivers the command whitespace-flattened; same verdict.
test_allow_steam_helper_then_proc_comm_scan_flattened if {
	cmd := `cd /home/banon/projects/er-effects-rs; bash -c 'source scripts/steam-running.sh && if steam_running; then echo "STEAM: running"; else echo "STEAM: NOT running"; fi' python3 -c " import os,glob hits=[] for p in glob.glob('/proc/[0-9]*'): try: comm=open(os.path.join(p,'comm')).read().strip() except Exception: continue if comm in ('eldenring.exe','me3','start_protected_game.exe'): hits.append(comm) print('game running:', hits if hits else 'none') "`
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

# Read-only shell work may also follow the scan.
test_allow_proc_comm_scan_then_read_only_shell_statements if {
	cmd := `python3 -c "print([p for p in __builtins__.__dict__ if 'start_protected_game.exe' in '/proc'])"; echo done`
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

# --- (b'') ... and the nested-literal exemption must not become a bypass ----

# Shell quoting does NOT make a shell word data: a bare quoted launcher name in
# command position still execs, so it stays denied even next to a /proc read.
test_deny_proc_scan_then_setsid_bare_quoted_launcher if {
	cmd := `python3 -c "print(open('/proc/1/comm').read())"; setsid 'start_protected_game.exe'`
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

test_deny_proc_scan_then_nohup_quoted_launcher_path if {
	cmd := `python3 -c "print(open('/proc/1/comm').read())"; nohup '/opt/er/start_protected_game.exe' &`
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

# A nested literal inside a program that also holds an exec mechanism is an
# argv element, not data.
test_deny_proc_scan_program_with_nested_literal_and_subprocess if {
	cmd := `python3 -c "import subprocess; print(open('/proc/1/comm').read()); subprocess.run(['start_protected_game.exe'])"`
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

# A command substitution inside the quoted program is expanded by the SHELL
# before python sees it, so a nested-looking literal there would really exec.
test_deny_proc_scan_program_with_command_substitution_launcher if {
	cmd := `python3 -c "print(open('/proc/1/comm').read()); $( 'start_protected_game.exe' )"`
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

# The exemption is for process-state readers; a payload that merely quotes the
# name without reading /proc is not covered.
test_deny_quoted_launcher_without_proc_read if {
	cmd := `python3 -c "import os; os.execvp('start_protected_game.exe', ['start_protected_game.exe'])"`
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

# --- (b') ... but the /proc mention must never become a launch bypass -------

# A /proc-scanning heredoc that ALSO launches stays denied (exec mechanism
# present in the payload keeps the exemption off).
test_deny_proc_scan_heredoc_with_subprocess_launch if {
	cmd := concat("\n", [
		"python3 - <<'PY'",
		"import subprocess",
		"print(open('/proc/1/comm').read())",
		"subprocess.run(['wine', 'start_protected_game.exe'])",
		"PY",
	])
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

# Trailing shell after the heredoc terminator keeps the exemption OFF even
# when the launcher path is quoted and the wrapper is not a listed launcher.
test_deny_proc_scan_heredoc_with_trailing_quoted_launch if {
	cmd := concat("\n", [
		"python3 - <<'PY'",
		"print(open('/proc/1/comm').read())",
		"PY",
		"setsid '/opt/er/start_protected_game.exe'",
	])
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

# A chained command after a python -c /proc reader breaks the inline shape.
test_deny_python_c_proc_read_chained_quoted_launch if {
	cmd := `python3 -c 'print(open("/proc/1/comm").read())'; env '/opt/er/start_protected_game.exe'`
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

# An unquoted (execution-position) launcher name inside an otherwise
# /proc-flavored heredoc keeps the exemption off.
test_deny_proc_scan_heredoc_unquoted_launcher_name if {
	cmd := concat("\n", [
		"python3 - <<'PY'",
		"# stale check for start_protected_game.exe via /proc/",
		"import os",
		"os.system('/opt/er/' + 'start_protected_game' + '.exe')",
		"PY",
	])
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

# ---------------------------------------------------------------------------
# 2026-08-04 false positive #3: INERT SHELL AFTER THE HEREDOC TERMINATOR.
#
# A read-only /proc walk was denied for printing a timestamp after the program:
#
#     python3 - <<'PY'
#     ... opens /proc/<pid>/{comm,cmdline,stat} and prints every process whose
#     ... comm matches ["eldenring.exe","me3","start_protected_game.exe"] ...
#     PY
#     echo "--- date ---"; date -u +%Y-%m-%dT%H:%M:%SZ
#
# It launches nothing -- the launcher name is a python list element compared
# against /proc/<pid>/comm, which is exactly the process DETECTION AGENTS.md
# permits and exactly what scripts/er-stale-run-sentinel.sh does with those same
# three names. Measured on the denied payload: `proc_scan_exec_marker`
# undefined, `heredoc_tail_chains_command` undefined, tag `PY` resolved, the
# name quoted. The SOLE failing condition was `terminator_parts[1] == ""`, so
# the two trailing inert statements dropped the payload into the substring
# fallback -- whose entire test is "the name occurs" plus a generic marker word,
# and an interpreter invocation always supplies "python".
#
# The narrowing accepts a non-empty trailer only when it is provably inert: no
# `$`/backtick/`(`/`)`/`<`/`>`/`|`, every `;`/`&`-separated statement headed by a
# word in `launcher_inert_heads`, and no launcher name in it.
# ---------------------------------------------------------------------------

# The read-only /proc walk, verbatim from the denied command. Both the full and
# the 15-char-truncated comm names are matched because the kernel truncates
# /proc/<pid>/comm at 15 characters.
proc_walk_lines := [
	"python3 - <<'PY'",
	"import os, glob",
	"COMM_MAX=15",
	"names={}",
	`for raw in ["eldenring.exe","me3","start_protected_game.exe"]:`,
	"    names[raw]=1; names[raw[:COMM_MAX]]=1",
	"for d in sorted(glob.glob('/proc/[0-9]*'), key=lambda x:int(os.path.basename(x))):",
	"    try: comm=open(os.path.join(d,'comm')).read().strip()",
	"    except OSError: continue",
	"    if comm in names:",
	"        pid=os.path.basename(d)",
	`        try: cmd=[a.decode('utf-8','replace') for a in open(os.path.join(d,'cmdline'),'rb').read().split(b'\0') if a]`,
	"        except OSError: cmd=[]",
	"        try:",
	"            s=open(f'/proc/{pid}/stat').read(); ppid=s[s.rindex(')')+2:].split()[1]",
	"        except OSError: ppid='?'",
	`        print(pid, comm, "ppid="+ppid, cmd)`,
	"PY",
]

proc_walk_with_trailer(trailer) := concat("\n", array.concat(proc_walk_lines, [trailer]))

# The exact denied command.
test_allow_proc_walk_heredoc_with_inert_trailing_statements if {
	cmd := proc_walk_with_trailer(`echo "--- date ---"; date -u +%Y-%m-%dT%H:%M:%SZ`)
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

# The live engine delivers the command whitespace-flattened; same verdict.
test_allow_proc_walk_heredoc_with_inert_trailing_statements_flattened if {
	cmd := `python3 - <<'PY' import os, glob COMM_MAX=15 names={} for raw in ["eldenring.exe","me3","start_protected_game.exe"]: names[raw]=1; names[raw[:COMM_MAX]]=1 for d in sorted(glob.glob('/proc/[0-9]*'), key=lambda x:int(os.path.basename(x))): try: comm=open(os.path.join(d,'comm')).read().strip() except OSError: continue if comm in names: pid=os.path.basename(d) try: cmd=[a.decode('utf-8','replace') for a in open(os.path.join(d,'cmdline'),'rb').read().split(b'\0') if a] except OSError: cmd=[] try: s=open(f'/proc/{pid}/stat').read(); ppid=s[s.rindex(')')+2:].split()[1] except OSError: ppid='?' print(pid, comm, "ppid="+ppid, cmd) PY echo "--- date ---"; date -u +%Y-%m-%dT%H:%M:%SZ`
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

# A single inert trailing statement.
test_allow_proc_walk_heredoc_with_single_trailing_echo if {
	cmd := proc_walk_with_trailer("echo done")
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

# `&&`-chained inert statements split the same way `;` does.
test_allow_proc_walk_heredoc_with_and_chained_inert_trailer if {
	cmd := proc_walk_with_trailer("date -u && echo scan-complete")
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

# --- ... and an inert trailer must not become a launch bypass ----------------

test_deny_proc_walk_heredoc_then_trailing_wine_launch if {
	cmd := proc_walk_with_trailer(`echo "--- date ---"; wine /opt/er/start_protected_game.exe`)
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

test_deny_proc_walk_heredoc_then_trailing_bare_launcher if {
	cmd := proc_walk_with_trailer("echo done; start_protected_game.exe --offline")
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

test_deny_proc_walk_heredoc_then_trailing_setsid_quoted_launcher if {
	cmd := proc_walk_with_trailer(`echo done; setsid '/opt/er/start_protected_game.exe'`)
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

test_deny_proc_walk_heredoc_then_trailing_env_wrapped_launcher if {
	cmd := proc_walk_with_trailer("echo done; env WINEPREFIX=/tmp/pfx /opt/er/start_protected_game.exe")
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

# A command substitution in the trailer runs before the inert head ever does.
test_deny_proc_walk_heredoc_then_trailing_command_substitution_launch if {
	cmd := proc_walk_with_trailer(`echo "$(wine /opt/er/start_protected_game.exe)"`)
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

# An inert head does not make a PIPE inert.
test_deny_proc_walk_heredoc_then_trailing_pipe_into_shell if {
	cmd := proc_walk_with_trailer(`echo '/opt/er/start_protected_game.exe' | bash`)
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

# Shells and interpreters are deliberately absent from `launcher_inert_heads`:
# a trailing script invocation can start anything, so it keeps the guard on.
test_deny_proc_walk_heredoc_then_trailing_bash_script if {
	cmd := proc_walk_with_trailer("echo done; bash scripts/er-stale-run-sentinel.sh")
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

# Deliberate conservatism: the trailer may not NAME the launcher even under an
# inert head. Pre-existing behavior -- the post-terminator deny arms
# (`launch_post_heredoc_region`) already match a bare name out there -- so the
# trailer rule stays consistent with them rather than carving an exception.
test_deny_proc_walk_heredoc_trailing_echo_naming_launcher if {
	cmd := proc_walk_with_trailer(`echo 'start_protected_game.exe not found'`)
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

# The Steam launch arms are untouched by the trailer narrowing.
test_deny_proc_walk_heredoc_then_trailing_steam_applaunch if {
	cmd := proc_walk_with_trailer("echo done; steam -applaunch 1245620")
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-ELDEN-RING-LAUNCH-GUARD" in rule_ids(denials)
}

test_deny_proc_walk_heredoc_then_trailing_xdg_open_steam_url if {
	cmd := proc_walk_with_trailer("echo done; xdg-open steam://run/1245620")
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-ELDEN-RING-LAUNCH-GUARD" in rule_ids(denials)
}

# The exemption reads the COMMAND; a non-command tool field naming the launcher
# keeps the guard on even with a fully inert trailer.
test_deny_proc_walk_heredoc_inert_trailer_with_launcher_in_description if {
	event := bash_event_with_description(
		proc_walk_with_trailer("echo done"),
		"probe start_protected_game.exe",
	)
	denials := guard.deny with input as event
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

# --- (c) Real launch/execution forms stay DENIED ----------------------------

test_deny_proton_run_launcher if {
	denials := guard.deny with input as bash_event("proton run /tmp/start_protected_game.exe")
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

test_deny_wine_run_launcher if {
	denials := guard.deny with input as bash_event("wine /opt/er/start_protected_game.exe")
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

test_deny_direct_path_launcher if {
	denials := guard.deny with input as bash_event("/tmp/start_protected_game.exe")
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

test_deny_relative_dot_slash_launcher if {
	denials := guard.deny with input as bash_event("./start_protected_game.exe")
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

test_deny_steam_rungameid_url if {
	denials := guard.deny with input as bash_event("steam steam://rungameid/1245620")
	"ER-EFFECTS-ELDEN-RING-LAUNCH-GUARD" in rule_ids(denials)
}

# Every genuine launch form stays denied after the 2026-07-28 narrowing.

test_deny_steam_run_url if {
	denials := guard.deny with input as bash_event("steam steam://run/1245620")
	"ER-EFFECTS-ELDEN-RING-LAUNCH-GUARD" in rule_ids(denials)
}

test_deny_xdg_open_steam_run_url if {
	denials := guard.deny with input as bash_event("xdg-open steam://run/1245620")
	"ER-EFFECTS-ELDEN-RING-LAUNCH-GUARD" in rule_ids(denials)
}

test_deny_xdg_open_steam_rungameid_url if {
	denials := guard.deny with input as bash_event("xdg-open steam://rungameid/1245620")
	"ER-EFFECTS-ELDEN-RING-LAUNCH-GUARD" in rule_ids(denials)
}

test_deny_steam_wrapping_launcher_binary if {
	denials := guard.deny with input as bash_event(`steam "/mnt/c/SteamLibrary/steamapps/common/ELDEN RING/Game/start_protected_game.exe"`)
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

test_deny_bare_launcher_binary_in_command_position if {
	denials := guard.deny with input as bash_event("start_protected_game.exe --offline")
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

test_deny_env_wrapped_launcher_binary if {
	denials := guard.deny with input as bash_event("env WINEPREFIX=/tmp/pfx /opt/er/start_protected_game.exe")
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

# --- (c) bash -c indirection stays DENIED ------------------------------------

test_deny_bash_c_quoted_launcher if {
	denials := guard.deny with input as bash_event(`bash -c '/opt/er/start_protected_game.exe'`)
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

test_deny_heredoc_python_launcher if {
	cmd := concat("\n", [
		"python3 - <<'PY'",
		"import subprocess",
		"subprocess.run(['proton','run','start_protected_game.exe'])",
		"PY",
	])
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

# --- bd exemption must not leak to chained/indirected commands --------------

test_deny_bd_chained_with_proton_launch if {
	cmd := `/home/banon/.local/bin/bd create "note" -d "text" && proton run /tmp/start_protected_game.exe`
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

test_deny_bd_chained_with_python_c_launch if {
	cmd := `/home/banon/.local/bin/bd create "note" -d "text"; python3 -c 'import subprocess; subprocess.run(["proton","run","start_protected_game.exe"])'`
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

# A command-substituted payload inside bd argument text keeps the exemption
# OFF (falls through to the raw-text scan with its executable marker).
test_deny_bd_with_command_substitution_marker_payload if {
	cmd := `/home/banon/.local/bin/bd create "x" -d "$(bash -c /opt/er/start_protected_game.exe)"`
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

# Other executables mentioning the launcher in quoted text are NOT exempted.
test_deny_python_c_launcher_in_quotes if {
	cmd := `python3 -c 'import subprocess; subprocess.run(["/opt/er/start_protected_game.exe"])'`
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

# --- Pre-existing behavior unaffected ----------------------------------------

test_deny_steam_applaunch if {
	denials := guard.deny with input as bash_event("steam -applaunch 1245620")
	"ER-EFFECTS-ELDEN-RING-LAUNCH-GUARD" in rule_ids(denials)
}

test_deny_ersc_copy_bundle if {
	denials := guard.deny with input as bash_event("cp -f /tmp/ersc.dll target/release/ersc.dll")
	"ER-EFFECTS-ERSC-DLL-BUNDLE-GUARD" in rule_ids(denials)
}

test_allow_quoted_forbidden_launch_note_echo if {
	denials := guard.deny with input as bash_event(`echo 'do not run steam -applaunch 1245620'`)
	count(denials) == 0
}

# The direct ersc regex must scan the quote-scrubbed command: a quoted prose
# mention shaped `... bash ... ersc.dll ...` is not a bundling command.
test_allow_bd_remember_prose_bash_before_ersc_dll if {
	cmd := `/home/banon/.local/bin/bd remember --key k 'guard words like bash appear in prose near ersc.dll mentions'`
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

test_deny_unquoted_cp_ersc_dll_still_matches_scrubbed if {
	denials := guard.deny with input as bash_event(`cp SeamlessCoop/ersc.dll target/release/ersc.dll`)
	"ER-EFFECTS-ERSC-DLL-BUNDLE-GUARD" in rule_ids(denials)
}

# --- git commit message text mentioning ersc.dll is ALLOWED ------------------

# The 2026-07-07 false positive: a commit whose quoted -m prose mentions
# ersc.dll alongside marker substrings ("stage", "tar" inside "target") was
# denied by the raw marker fallback even though the command bundles nothing.
test_allow_git_commit_message_mentioning_ersc_dll if {
	cmd := `git add -A && git commit -m "loader precedence: a resident ersc.dll wins over the env hint; never stage it into target/"`
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

# Canonical Claude Code commit form: the -m "$(cat <<'EOF' ... EOF)" heredoc
# message is commit prose piped through cat, not an executable payload.
test_allow_git_commit_heredoc_message_mentioning_ersc_dll if {
	cmd := concat("\n", [
		`git add -A && git commit -m "$(cat <<'EOF'`,
		"guard: document that a resident ersc.dll wins over the env hint",
		"",
		"The bundling rule still blocks staging ersc.dll into release artifacts.",
		"EOF",
		`)"`,
	])
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

# The OTHER canonical commit form: `git commit -F -` reads the message from
# STDIN, so the message arrives as a plain heredoc with no `$(cat ...)`
# substitution. Regression 2026-07-29: a commit DESCRIBING this guard's own
# launcher fix was denied, because the message necessarily names the launcher and
# guard prose is full of executable marker words ("shell", "bash", "python"), so
# the marker fallback fired on documentation. The `-m` and `-m "$(cat <<'TAG'`
# forms were both exempt; this one was not.
test_allow_git_commit_stdin_heredoc_message_mentioning_eac_launcher if {
	cmd := concat("\n", [
		"git add .cupcake/policies/claude/bash_elden_ring_launch_guard.rego && git commit -q -F - <<'EOF'",
		"guard: close two EAC launcher gaps",
		"",
		`A quoted path (steam "/mnt/c/SteamLibrary/.../start_protected_game.exe") was`,
		"invisible because scrubbed_command deletes quoted spans wholesale, and the",
		"bare form fell between the wrapper and path clauses. Both deny now.",
		"EOF",
	])
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

# `git -C <path>` is the form AGENTS.md mandates for worktree-scoped work, so the
# exemption has to recognise it. A hard-coded assumption about the invocation
# shape silently disabled this exemption for the documented form once before
# (2026-07-28, the `/home/banon/...` literal in bd_text_command).
test_allow_git_commit_stdin_heredoc_with_dash_c_worktree_path if {
	cmd := concat("\n", [
		"git -C /home/banon/projects/er-effects-rs add .cupcake/policies/claude/bash_elden_ring_launch_guard.rego && git -C /home/banon/projects/er-effects-rs commit -q -F - <<'EOF'",
		"guard: close two EAC launcher gaps",
		"",
		"The bare start_protected_game.exe form fell between two clauses.",
		"EOF",
	])
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

# The exemption is shape-bound, not keyword-bound: anything riding after the
# heredoc terminator turns it back off, so a real launch cannot hide behind a
# commit message.
test_deny_git_commit_stdin_heredoc_then_quoted_launcher if {
	cmd := concat("\n", [
		"git commit -q -F - <<'EOF'",
		"guard: a perfectly ordinary commit message",
		"EOF",
		`steam "/opt/er/start_protected_game.exe"`,
	])
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD" in rule_ids(denials)
}

# Real bundling stays denied: cp with an ersc.dll path operand.
test_deny_cp_seamless_ersc_dll_to_dist if {
	denials := guard.deny with input as bash_event(`cp SeamlessCoop/ersc.dll dist/`)
	"ER-EFFECTS-ERSC-DLL-BUNDLE-GUARD" in rule_ids(denials)
}

# The git-commit text exemption must not leak to a chained copy, even with the
# ersc.dll operand fully quoted (the raw marker fallback keeps matching).
test_deny_git_commit_chained_quoted_ersc_copy if {
	cmd := `git commit -m "note" && cp 'SeamlessCoop/ersc.dll' dist/`
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-ERSC-DLL-BUNDLE-GUARD" in rule_ids(denials)
}

# A command substitution that is not the canonical `"$(cat <<'TAG'` message
# shape keeps the exemption off (this one would really execute cp).
test_deny_git_commit_substitution_cp_ersc if {
	cmd := `git commit -m "$(cp SeamlessCoop/ersc.dll dist/)"`
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-ERSC-DLL-BUNDLE-GUARD" in rule_ids(denials)
}

# Trailing shell after the message-substitution terminator keeps the
# exemption off.
test_deny_git_commit_heredoc_then_quoted_ersc_copy if {
	cmd := concat("\n", [
		`git commit -m "$(cat <<'EOF'`,
		"note mentioning ersc.dll",
		"EOF",
		`)" && cp 'SeamlessCoop/ersc.dll' dist/`,
	])
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-ERSC-DLL-BUNDLE-GUARD" in rule_ids(denials)
}

# ---------------------------------------------------------------------------
# 2026-07-28 false positive: the bundling rule fired on TEXT that only mentions
# the filename. Two defects: the bd text exemption matched only a hard-coded
# `/home/banon/...` path (not the `$HOME/.local/bin/bd` form AGENTS.md
# documents), and the fallback denied on `ersc.dll` plus ANY of the substrings
# "stage"/"bundle"/"archive"/"tar"/"rar" -- which hide inside "target",
# "startup", "library", "staged". Deny now requires a real file-moving verb.
# ---------------------------------------------------------------------------

ctx_execute_event(code) := {
	"hook_event_name": "PreToolUse",
	"tool_name": "ctx_execute",
	"tool_input": {"language": "python", "code": code},
}

write_event(path, content) := {
	"hook_event_name": "PreToolUse",
	"tool_name": "Write",
	"tool_input": {"file_path": path, "content": content},
}

# --- (i) TEXT that merely mentions the DLL is ALLOWED -----------------------

# The exact denied command (abridged prose, same shape): `$HOME`-relative bd
# binary, quoted memory body naming the DLL, `target/` and `stage` prose.
test_allow_bd_remember_home_var_mentioning_ersc_dll if {
	cmd := `$HOME/.local/bin/bd remember "SAVE-DISABLE INTERCEPTION STRATEGY: swallow the SL submit and fake the status poll. Also above any path redirection so it works identically under Seamless Co-op (ersc.dll redirects paths, not the SL submit). Nothing is staged into the target/ bundle." --key save-disable-strategy-swallow-SL-submit-fake-status-poll-2026-07-28`
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

test_allow_bd_create_home_var_mentioning_ersc_dll if {
	cmd := `${HOME}/.local/bin/bd create "seamless compat" -d "ersc.dll must never be staged into a me3 profile or the target/ release bundle" -t task`
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

test_allow_bd_update_tilde_path_mentioning_ersc_dll if {
	cmd := `~/.local/bin/bd update er-effects-rs-1 --notes "profile references the game-installed ersc.dll; no archive step"`
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

# Another user's home must work too (no hard-coded /home/banon).
test_allow_bd_remember_other_user_home_mentioning_ersc_dll if {
	cmd := `/home/choza/.local/bin/bd remember --key k "ersc.dll stays a compatibility target, never staged"`
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

test_allow_bare_bd_remember_mentioning_ersc_dll if {
	cmd := `bd remember --key k "ersc.dll is referenced from the me3 profile, not bundled"`
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

test_allow_git_commit_message_mentioning_ersc_dll_seamless_compat if {
	cmd := `git commit -m "docs: record that ersc.dll redirects save paths under Seamless Co-op; nothing is staged into target/"`
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

test_allow_echo_prose_mentioning_ersc_dll if {
	cmd := `echo 'ersc.dll is a compatibility target; it is never archived into the target/ bundle'`
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

test_allow_printf_prose_mentioning_ersc_dll if {
	cmd := `printf '%s\n' 'the me3 profile references the installed ersc.dll; do not stage it'`
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

test_allow_gh_pr_create_body_file_mentioning_ersc_dll if {
	cmd := concat("\n", [
		"tmp_body=$(mktemp)",
		"cat > \"$tmp_body\" <<'EOF'",
		"Policy note: ersc.dll is never staged into the target/ release bundle.",
		"EOF",
		"gh pr create --base main --head branch --title t --body-file \"$tmp_body\"",
	])
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

# Read-only inspection of the game-installed DLL is not bundling.
test_allow_sha256sum_of_installed_ersc_dll if {
	denials := guard.deny with input as bash_event(`sha256sum 'SeamlessCoop/ersc.dll'`)
	count(denials) == 0
}

# Arrow notation in a doc heredoc is prose, not a redirect into the DLL.
test_allow_heredoc_doc_arrow_notation_mentioning_ersc_dll if {
	cmd := concat("\n", [
		"cat > docs/seamless-compat.md <<'EOF'",
		"load order: me3 profile -> ersc.dll -> er_effects_rs.dll",
		"EOF",
	])
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

# Writing documentation that names the DLL is not an executable payload at all.
test_allow_write_markdown_doc_mentioning_ersc_dll if {
	event := write_event("docs/seamless-compat.md", "The me3 profile references the game-installed ersc.dll; it is never bundled.")
	denials := guard.deny with input as event
	count(denials) == 0
}

# --- (ii) Real bundling stays DENIED ----------------------------------------

# Staging into a me3 profile directory.
test_deny_cp_ersc_dll_into_me3_profile if {
	cmd := `cp -f "/mnt/c/SteamLibrary/steamapps/common/ELDEN RING/Game/SeamlessCoop/ersc.dll" /home/banon/Elden/profile/ersc.dll`
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-ERSC-DLL-BUNDLE-GUARD" in rule_ids(denials)
}

test_deny_install_ersc_dll_into_me3_profile if {
	cmd := `install -Dm755 SeamlessCoop/ersc.dll target/me3-profile/ersc.dll`
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-ERSC-DLL-BUNDLE-GUARD" in rule_ids(denials)
}

# Quoted operands survive the quote scrub via the statement-head verb check.
test_deny_mv_quoted_ersc_dll_into_target_bundle if {
	cmd := `mv 'SeamlessCoop/ersc.dll' target/release-bundle/`
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-ERSC-DLL-BUNDLE-GUARD" in rule_ids(denials)
}

test_deny_zip_quoted_ersc_dll_into_release_artifact if {
	cmd := `zip -j target/release/er-net-effects.zip 'SeamlessCoop/ersc.dll'`
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-ERSC-DLL-BUNDLE-GUARD" in rule_ids(denials)
}

test_deny_tar_ersc_dll_into_release_artifact if {
	cmd := `tar -czf target/er-net-effects-release.tgz me3/profile SeamlessCoop/ersc.dll`
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-ERSC-DLL-BUNDLE-GUARD" in rule_ids(denials)
}

# A redirect whose write TARGET is the DLL is staging, even with a read-only
# command word in front.
test_deny_redirect_write_target_ersc_dll if {
	cmd := `cat vendor/seamless-coop-v1.9.9/SeamlessCoop/ersc.dll > target/release/ersc.dll`
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-ERSC-DLL-BUNDLE-GUARD" in rule_ids(denials)
}

# A staging script handed the DLL path.
test_deny_staging_script_with_ersc_dll_argument if {
	cmd := `bash scripts/stage-net-effects-release.sh --dll SeamlessCoop/ersc.dll`
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-ERSC-DLL-BUNDLE-GUARD" in rule_ids(denials)
}

# argv-list copy inside an interpreter payload.
test_deny_subprocess_argv_cp_ersc_dll if {
	cmd := `python3 -c 'import subprocess; subprocess.run(["cp", "SeamlessCoop/ersc.dll", "target/release/ersc.dll"])'`
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-ERSC-DLL-BUNDLE-GUARD" in rule_ids(denials)
}

# Multi-line library copy call in a non-command tool field.
test_deny_ctx_execute_multiline_shutil_copy_ersc_dll if {
	code := concat("\n", [
		"import shutil",
		"shutil.copy2(",
		"    'SeamlessCoop/ersc.dll',",
		"    'target/release/ersc.dll',",
		")",
	])
	denials := guard.deny with input as ctx_execute_event(code)
	"ER-EFFECTS-ERSC-DLL-BUNDLE-GUARD" in rule_ids(denials)
}

# The widened bd path match must not leak the exemption to a chained copy.
test_deny_bd_home_var_chained_ersc_copy if {
	cmd := `$HOME/.local/bin/bd remember --key k "note" && cp 'SeamlessCoop/ersc.dll' target/release/`
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-ERSC-DLL-BUNDLE-GUARD" in rule_ids(denials)
}

# ... nor to a command substitution that really copies.
test_deny_bd_home_var_substitution_ersc_copy if {
	cmd := `$HOME/.local/bin/bd remember --key k "$(cp SeamlessCoop/ersc.dll target/release/)"`
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-ERSC-DLL-BUNDLE-GUARD" in rule_ids(denials)
}

# --- user game-install RESTORE rename is ALLOWED (bd er-effects-rs-gkqa) -----
#
# Re-validated 2026-07-30 via opa eval before fixing: this exact command was
# denied by the file-moving arms even though the destination is the same
# game-install path the DLL came from -- only the repo's `.er-effects-staged`
# suffix is removed, which is the opposite of bundling.

test_allow_mv_restore_staged_ersc_dll_same_gameinstall_path if {
	cmd := `mv -f '/mnt/c/SteamLibrary/steamapps/common/ELDEN RING/Game/SeamlessCoop/ersc.dll.er-effects-staged' '/mnt/c/SteamLibrary/steamapps/common/ELDEN RING/Game/SeamlessCoop/ersc.dll'`
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

test_allow_mv_restore_staged_ersc_dll_double_quoted if {
	cmd := `mv -f "/mnt/c/SteamLibrary/steamapps/common/ELDEN RING/Game/SeamlessCoop/ersc.dll.er-effects-staged" "/mnt/c/SteamLibrary/steamapps/common/ELDEN RING/Game/SeamlessCoop/ersc.dll"`
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

test_allow_mv_restore_staged_ersc_dll_no_flags if {
	cmd := `mv '/mnt/c/SteamLibrary/steamapps/common/ELDEN RING/Game/SeamlessCoop/ersc.dll.er-effects-staged' '/mnt/c/SteamLibrary/steamapps/common/ELDEN RING/Game/SeamlessCoop/ersc.dll'`
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

# Destination scoping: the SAME staged source moved anywhere else stays denied.
test_deny_mv_staged_ersc_dll_into_target_bundle if {
	cmd := `mv -f '/mnt/c/SteamLibrary/steamapps/common/ELDEN RING/Game/SeamlessCoop/ersc.dll.er-effects-staged' 'target/release-bundle/ersc.dll'`
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-ERSC-DLL-BUNDLE-GUARD" in rule_ids(denials)
}

test_deny_mv_staged_ersc_dll_into_me3_profile_dir if {
	cmd := `mv -f '/mnt/c/SteamLibrary/steamapps/common/ELDEN RING/Game/SeamlessCoop/ersc.dll.er-effects-staged' '/home/banon/Elden/profile/ersc.dll'`
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-ERSC-DLL-BUNDLE-GUARD" in rule_ids(denials)
}

# A restore rename with a second command riding along stays denied.
test_deny_mv_restore_then_chained_ersc_copy if {
	cmd := `mv -f '/mnt/d/er/Game/SeamlessCoop/ersc.dll.er-effects-staged' '/mnt/d/er/Game/SeamlessCoop/ersc.dll' && cp '/mnt/d/er/Game/SeamlessCoop/ersc.dll' dist/`
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-ERSC-DLL-BUNDLE-GUARD" in rule_ids(denials)
}

# The STAGING direction (plain name -> .er-effects-staged) is not the restore
# shape and keeps denying; only the suffix-stripping restore is exempt.
test_deny_mv_ersc_dll_to_staged_name if {
	cmd := `mv -f '/mnt/d/er/Game/SeamlessCoop/ersc.dll' '/mnt/d/er/Game/SeamlessCoop/ersc.dll.er-effects-staged'`
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-ERSC-DLL-BUNDLE-GUARD" in rule_ids(denials)
}

# --- read-only interpreter scans of the INSTALLED DLL are ALLOWED ------------
#
# Re-validated 2026-07-30 via opa eval before fixing (bd er-effects-rs-gkqa):
# arm (a) counted python/bash as bundling verbs, so a read-only scan passing
# the game-install DLL path as an unquoted operand was denied.

test_allow_python_scan_gameinstall_ersc_dll_unquoted_operand if {
	cmd := `python3 scripts/pe_export_dump.py /mnt/c/SteamLibrary/steamapps/common/ELDEN\ RING/Game/SeamlessCoop/ersc.dll`
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

test_allow_python_scan_gameinstall_ersc_dll_no_space_path if {
	cmd := `python3 scripts/pe_export_dump.py /mnt/d/er-game/Game/SeamlessCoop/ersc.dll`
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

test_allow_bash_script_scan_gameinstall_ersc_dll if {
	cmd := `bash scripts/inspect-dll.sh /mnt/c/SteamLibrary/steamapps/common/ELDEN\ RING/Game/SeamlessCoop/ersc.dll`
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

# The read-only staged SIBLING may be scanned alongside the installed DLL.
test_allow_python_scan_gameinstall_ersc_dll_and_staged_sibling if {
	cmd := `python3 scripts/cmp_dll.py /mnt/d/er/Game/SeamlessCoop/ersc.dll /mnt/d/er/Game/SeamlessCoop/ersc.dll.er-effects-staged`
	denials := guard.deny with input as bash_event(cmd)
	count(denials) == 0
}

# A relative/repo-tree operand is not a game-install scan target: the staging
# script deny above (`bash scripts/stage-net-effects-release.sh --dll
# SeamlessCoop/ersc.dll`) keeps its contract, and so does this python twin.
test_deny_python_script_with_repo_relative_ersc_dll_operand if {
	cmd := `python3 scripts/pe_export_dump.py SeamlessCoop/ersc.dll`
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-ERSC-DLL-BUNDLE-GUARD" in rule_ids(denials)
}

# A visible repo-tree destination alongside the game-install source is a
# bundling shape, not a scan.
test_deny_python_script_gameinstall_source_with_target_destination if {
	cmd := `python3 scripts/stage_helper.py /mnt/c/SteamLibrary/steamapps/common/ELDEN\ RING/Game/SeamlessCoop/ersc.dll target/release/`
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-ERSC-DLL-BUNDLE-GUARD" in rule_ids(denials)
}

# A file-moving verb smuggled as an interpreter argument keeps the guard on.
test_deny_python_helper_with_cp_token_and_gameinstall_ersc_dll if {
	cmd := `python3 scripts/run_tool.py cp /mnt/d/er/Game/SeamlessCoop/ersc.dll /mnt/d/elsewhere/`
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-ERSC-DLL-BUNDLE-GUARD" in rule_ids(denials)
}

# A chained real copy after the scan stays denied via the sibling mover arm.
test_deny_python_scan_then_chained_cp_ersc_dll if {
	cmd := `python3 scripts/pe_export_dump.py /mnt/d/er/Game/SeamlessCoop/ersc.dll && cp /mnt/d/er/Game/SeamlessCoop/ersc.dll dist/`
	denials := guard.deny with input as bash_event(cmd)
	"ER-EFFECTS-ERSC-DLL-BUNDLE-GUARD" in rule_ids(denials)
}
