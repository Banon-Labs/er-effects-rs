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
