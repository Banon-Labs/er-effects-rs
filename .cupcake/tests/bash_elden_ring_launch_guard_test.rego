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
