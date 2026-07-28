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
