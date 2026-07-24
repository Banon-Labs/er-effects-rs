# OPA unit tests for er_launch_requires_session_auth (the per-session ER-launch authorization gate).
#
# Not loaded by the cupcake engine (which scans .cupcake/policies/<harness>/ and .cupcake/system/ only).
# Run with:
#   opa test .cupcake/policies/claude/er_launch_requires_session_auth.rego \
#            .cupcake/tests/er_launch_requires_session_auth_test.rego
# The er_launch_authorized SIGNAL itself is covered separately by scripts/test-er-launch-auth-signal.py
# (this file controls input.signals.er_launch_authorized directly, the way the live engine hands it in).
# End-to-end engine coverage lives in scripts/test-cupcake-policies.py.
package cupcake.policies.claude.er_launch_requires_session_auth_test

import rego.v1

import data.cupcake.policies.claude.er_launch_requires_session_auth as guard

RULE := "ER-EFFECTS-ELDEN-RING-LAUNCH-REQUIRES-SESSION-AUTH"

# Event with an explicit authorization signal value.
ev(cmd, auth) := {
	"hook_event_name": "PreToolUse",
	"tool_name": "Bash",
	"tool_input": {"command": cmd},
	"signals": {"er_launch_authorized": auth},
}

# Event with NO signals key at all (fail-closed path).
ev_nosignal(cmd) := {
	"hook_event_name": "PreToolUse",
	"tool_name": "Bash",
	"tool_input": {"command": cmd},
}

rule_ids(denials) := {d.rule_id | some d in denials}

denied(event) if {
	denials := guard.deny with input as event
	RULE in rule_ids(denials)
}

# === (a) UNAUTHORIZED launches are DENIED ===========================================================

test_deny_unauth_me3_launch if {
	denied(ev("me3.exe launch -g eldenring --online false -p x.me3", "0"))
}

test_deny_unauth_me3_launch_argv_form if {
	denied(ev("/some/path/me3 launch -g eldenring -n dll.dll", "0"))
}

# Regression (2026-07-24): the armament-icons smoke launcher was ABSENT from the launch_form allowlist,
# so the agent launched ER through `bash scripts/run-armament-icons-smoke.sh` with no clearance. Gated now.
test_deny_unauth_run_armament_icons_smoke if {
	denied(ev("bash scripts/run-armament-icons-smoke.sh", "0"))
}

test_deny_unauth_run_armament_icons_smoke_wrapped if {
	denied(ev("LLDRAW=1 MODE=equip bash scripts/run-armament-icons-smoke.sh", "0"))
}

test_deny_unauth_armament_watch_direct if {
	denied(ev("python3 scripts/armament-icons-watch.py --max-seconds 300", "0"))
}

test_allow_auth_run_armament_icons_smoke if {
	not denied(ev("bash scripts/run-armament-icons-smoke.sh", "1"))
}

# The exact command the agent ran without asking (the failure that motivated this policy).
test_deny_unauth_the_failure_script if {
	denied(ev("bash scripts/run-vanilla-reload-agentdriven.sh", "0"))
}

test_deny_unauth_delegator_hub if {
	denied(ev("bash scripts/run-product-continue-direct-probe.sh", "0"))
}

test_deny_unauth_delegator_camera_smoke if {
	denied(ev("bash scripts/run-camera-smoke.sh", "0"))
}

test_deny_unauth_launcher_dot_slash if {
	denied(ev("./scripts/run-samechar-3x-threedll.sh", "0"))
}

test_deny_unauth_launcher_bare_scripts_prefix if {
	denied(ev("scripts/run-me3-product-smoke.sh &", "0"))
}

test_deny_unauth_python_launcher if {
	denied(ev("python3 scripts/me3_live_launch.py --dll er_effects_rs.dll", "0"))
}

test_deny_unauth_eldenring_exe_exec if {
	denied(ev("./eldenring.exe", "0"))
}

test_deny_unauth_renderdoccmd_capture if {
	denied(ev("renderdoccmd.exe capture --opt-hook-children", "0"))
}

test_deny_unauth_chained_after_and if {
	denied(ev("cargo build --release && bash scripts/run-vanilla-reload-agentdriven.sh", "0"))
}

test_deny_conditional_with_launch_flag if {
	denied(ev("bash scripts/build_mushroom_blender_edit.sh --launch", "0"))
}

# === (b) fail-closed: absent / non-"1" signal treats a launch as unauthorized =======================

test_deny_launch_when_signal_absent if {
	denied(ev_nosignal("bash scripts/run-vanilla-reload-agentdriven.sh"))
}

test_deny_launch_when_signal_empty if {
	denied(ev("bash scripts/run-vanilla-reload-agentdriven.sh", ""))
}

test_deny_launch_when_signal_garbage if {
	denied(ev("bash scripts/run-vanilla-reload-agentdriven.sh", "yes-please"))
}

# === (c) AUTHORIZED launches are ALLOWED ============================================================

test_allow_authorized_me3_launch if {
	not denied(ev("me3.exe launch -g eldenring --online false -p x.me3", "1"))
}

test_allow_authorized_the_failure_script if {
	not denied(ev("bash scripts/run-vanilla-reload-agentdriven.sh", "1"))
}

test_allow_authorized_signal_trimmed if {
	not denied(ev("bash scripts/run-vanilla-reload-agentdriven.sh", "1\n"))
}

test_allow_authorized_signal_output_shape if {
	not denied(ev("bash scripts/run-vanilla-reload-agentdriven.sh", {"output": "1"}))
}

# === (d) NON-launch commands are never denied (regardless of signal) =================================

test_allow_non_launch_git_status if {
	not denied(ev("git status", "0"))
}

test_allow_non_launch_cargo_build if {
	not denied(ev("cargo xwin build --release --target x86_64-pc-windows-msvc", "0"))
}

# A launcher-name as an ARGUMENT to another command (git add / shellcheck / ruff) is NOT an execution.
test_allow_git_add_launcher_name if {
	not denied(ev("git add scripts/run-vanilla-reload-agentdriven.sh", "0"))
}

test_allow_shellcheck_launcher_name if {
	not denied(ev("shellcheck scripts/run-camera-smoke.sh", "0"))
}

test_allow_ruff_python_launcher_name if {
	not denied(ev("ruff check scripts/me3_live_launch.py", "0"))
}

# A launcher name MENTIONED inside quotes (bd/commit prose) is scrubbed, so it must not deny.
test_allow_quoted_launcher_mention if {
	not denied(ev("/home/choza/.local/bin/bd remember \"run-camera-smoke.sh launches ER\"", "0"))
}

# The conditional mushroom scripts do offline build/print work WITHOUT --launch -> must not deny.
test_allow_conditional_without_launch_flag if {
	not denied(ev("bash scripts/build_mushroom_blender_edit.sh", "0"))
}

# A substring-similar-but-not-a-launcher script must not deny.
test_allow_non_launcher_lookalike_script if {
	not denied(ev("bash scripts/run-save-oracle-check.sh", "0"))
}

# === (e) out-of-scope events / tools ================================================================

test_allow_non_pretooluse_event if {
	not denied({
		"hook_event_name": "PostToolUse",
		"tool_name": "Bash",
		"tool_input": {"command": "bash scripts/run-vanilla-reload-agentdriven.sh"},
		"signals": {"er_launch_authorized": "0"},
	})
}

# A launch-shaped string in a NON-guarded tool (Write) is not an execution -> not denied.
test_allow_write_tool_with_launch_text if {
	not denied({
		"hook_event_name": "PreToolUse",
		"tool_name": "Write",
		"tool_input": {"file_path": "/tmp/x.sh", "content": "bash scripts/run-vanilla-reload-agentdriven.sh"},
		"signals": {"er_launch_authorized": "0"},
	})
}

# === (f) HARDENING regression (2026-07-24 adversarial pass): FALSE POSITIVES now allowed =============

# me3 launch is command-position anchored, so a mention as an argument is NOT a launch.
test_allow_echo_me3_launch_mention if {
	not denied(ev("echo me3 launch", "0"))
}

test_allow_echo_me3_launch_failed_mention if {
	not denied(ev("echo me3 launch failed", "0"))
}

# Quote-join reconstruction of a benign single token (run me3"X"launch -> me3Xlaunch) must not deny.
test_allow_join_reconstructed_me3_token if {
	not denied(ev("run me3\"X\"launch", "0"))
}

# --launch is a FLAG token, not a substring: these must NOT trip the conditional-script gate.
test_allow_launchpad_not_launch_flag if {
	not denied(ev("bash scripts/build_mushroom_blender_edit.sh --launchpad", "0"))
}

test_allow_launch_later_not_launch_flag if {
	not denied(ev("bash scripts/build_mushroom_blender_edit.sh --launch-later", "0"))
}

test_allow_launch_substring_in_path if {
	not denied(ev("bash scripts/install_mushroom_man.py --out /tmp/foo--launch.log", "0"))
}

test_allow_launch_in_trailing_comment if {
	not denied(ev("bash scripts/build_mushroom_blender_edit.sh --dry-run # add --launch later", "0"))
}

test_allow_conditional_without_launch_flag_hardened if {
	not denied(ev("bash scripts/build_mushroom_blender_edit.sh", "0"))
}

# === (g) HARDENING: bypasses now CLOSED =============================================================

test_deny_wrapper_timeout if {
	denied(ev("timeout 300 bash scripts/run-camera-smoke.sh", "0"))
}

test_deny_wrapper_nice if {
	denied(ev("nice bash scripts/run-camera-smoke.sh", "0"))
}

test_deny_wrapper_env if {
	denied(ev("env FOO=1 bash scripts/run-camera-smoke.sh", "0"))
}

test_deny_wrapper_stdbuf if {
	denied(ev("stdbuf -oL bash scripts/run-camera-smoke.sh", "0"))
}

test_deny_absolute_path_launcher if {
	denied(ev("bash /home/choza/projects/er-effects-rs/scripts/run-vanilla-reload-agentdriven.sh", "0"))
}

test_deny_tilde_path_launcher if {
	denied(ev("bash ~/projects/er-effects-rs/scripts/run-vanilla-reload-agentdriven.sh", "0"))
}

test_deny_nested_path_launcher if {
	denied(ev("bash scripts/../scripts/run-vanilla-reload-agentdriven.sh", "0"))
}

test_deny_python_launcher_hardened if {
	denied(ev("python3 scripts/me3_live_launch.py", "0"))
}

test_deny_me3_launch_g_quoted_var if {
	denied(ev("\"$ME3\" launch -g eldenring --online false -p x.me3", "0"))
}

test_deny_me3_launch_g_unquoted_var if {
	denied(ev("$ME3 launch -g eldenring", "0"))
}

test_deny_me3_launch_func if {
	denied(ev("me3_launch /tmp/p.toml", "0"))
}

test_deny_source_lib_then_me3_launch_func if {
	denied(ev("source scripts/me3-launch-lib.sh && me3_launch /tmp/p.toml", "0"))
}

test_deny_conditional_with_launch_flag_hardened if {
	denied(ev("bash scripts/build_mushroom_blender_edit.sh --launch", "0"))
}

# === (i) GENERAL future-launcher pattern (fragility reducer, 2026-07-24) =============================
# Proves a FUTURE ER launcher (not in the explicit allowlist) is gated by naming convention, while the
# offline armament tools and non-launch helpers stay UN-gated.

# (a) hypothetical future launchers by naming convention are GATED.
test_deny_general_future_armament_smoke if {
	denied(ev("bash scripts/run-armament-foo-smoke.sh", "0"))
}

test_deny_general_future_capture_armament_py if {
	denied(ev("bash scripts/capture-armament-bar.py", "0"))
}

test_deny_general_future_launch_armament if {
	denied(ev("bash scripts/launch-armament-baz.sh", "0"))
}

test_deny_general_future_record_armament if {
	denied(ev("python3 scripts/record-armament-run.py", "0"))
}

test_deny_general_future_armament_watch if {
	denied(ev("python3 scripts/armament-tiles-watch.py", "0"))
}

test_deny_general_future_generic_smoke if {
	denied(ev("bash scripts/run-someother-smoke.sh", "0"))
}

test_deny_general_direct_position_smoke if {
	denied(ev("./scripts/run-armament-foo-smoke.sh", "0"))
}

test_deny_general_wrapped_future_launcher if {
	denied(ev("timeout 300 bash scripts/run-armament-foo-smoke.sh", "0"))
}

test_deny_general_absolute_path_future_launcher if {
	denied(ev("bash /home/choza/projects/er-effects-rs/scripts/run-armament-foo-smoke.sh", "0"))
}

test_deny_general_future_launcher_run_experiment_code if {
	denied(ev_code("bash scripts/capture-armament-bar.py", "0", "run_experiment"))
}

# (b) NON-launch helpers must stay ALLOWED even though the general clause exists.
test_allow_general_steam_running if {
	not denied(ev("bash scripts/steam-running.sh", "0"))
}

test_allow_general_check_sh if {
	not denied(ev("bash scripts/check.sh", "0"))
}

# The OFFLINE armament pixel-diff oracle contains "armament" but is NOT a launcher (no launcher verb /
# not a watcher) -- the tightened armament arm must NOT gate it.
test_allow_general_armament_pixel_oracle if {
	not denied(ev("python3 scripts/armament-icons-pixel-oracle.py verdict --baseline v.png --candidate c.png", "0"))
}

# A frida instrument library that contains "armament" is not launcher-shaped -> must not gate.
test_allow_general_armament_frida_lib if {
	not denied(ev("python3 scripts/frida/armament_probe.js", "0"))
}

# A -smoke.PY summarizer (not .sh) is offline post-processing -> the -smoke arm only matches .sh.
test_allow_general_smoke_py_summarizer if {
	not denied(ev("python3 scripts/summarize-er-window-placement-smoke.py out/", "0"))
}

# A launcher name as an ARGUMENT to a non-executing tool (shellcheck/git) is not an execution.
test_allow_general_shellcheck_future_launcher if {
	not denied(ev("shellcheck scripts/run-armament-foo-smoke.sh", "0"))
}

test_allow_general_git_add_future_launcher if {
	not denied(ev("git add scripts/run-armament-foo-smoke.sh", "0"))
}

# The general clause is only a NET: an authorized session still launches a future launcher fine.
test_allow_general_authorized_future_launcher if {
	not denied(ev("bash scripts/run-armament-foo-smoke.sh", "1"))
}

# === (h) HARDENING: run_experiment / ctx code+commands payloads =====================================

ev_code(code, auth, tname) := {
	"hook_event_name": "PreToolUse",
	"tool_name": tname,
	"tool_input": {"code": code},
	"signals": {"er_launch_authorized": auth},
}

test_deny_run_experiment_code_script if {
	denied(ev_code("bash scripts/run-vanilla-reload-fps.sh", "0", "run_experiment"))
}

test_deny_run_experiment_code_me3 if {
	denied(ev_code("me3 launch -g eldenring", "0", "run_experiment"))
}

test_deny_ctx_execute_code_me3 if {
	denied(ev_code("me3 launch -g eldenring", "0", "ctx_execute"))
}

test_deny_run_experiment_code_eldenring_exe if {
	denied(ev_code("./eldenring.exe", "0", "run_experiment"))
}

test_allow_run_experiment_benign_code if {
	not denied(ev_code("print('hello world')", "0", "run_experiment"))
}

test_deny_ctx_batch_commands_array if {
	denied({
		"hook_event_name": "PreToolUse",
		"tool_name": "ctx_batch_execute",
		"tool_input": {"commands": ["git status", "bash scripts/run-camera-smoke.sh"]},
		"signals": {"er_launch_authorized": "0"},
	})
}

# Authorized still allows a wrapper / path / run_experiment launch.
test_allow_authorized_wrapper if {
	not denied(ev("timeout 300 bash scripts/run-vanilla-reload-agentdriven.sh", "1"))
}

test_allow_authorized_run_experiment_code if {
	not denied(ev_code("me3 launch -g eldenring", "1", "run_experiment"))
}
