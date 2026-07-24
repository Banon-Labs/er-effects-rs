# METADATA
# scope: package
# title: Require explicit per-session user authorization for approved Elden Ring launches
# description: >-
#   Deny APPROVED offline Elden Ring launch commands -- me3 launch, direct eldenring.exe, renderdoccmd
#   capture, the me3_launch helper, and the repo's launcher SCRIPTS (and the scripts that delegate to
#   them) -- UNLESS the current agent session has been authorized by the user via the launch-clearance
#   keyphrase (signal er_launch_authorized == "1"). Complements bash_elden_ring_launch_guard (which
#   unconditionally blocks the forbidden Steam/EAC/start_protected_game/ersc.dll forms); this adds the
#   "even an approved launch needs explicit per-session user approval" layer that the prose directive
#   (bd er-effects-rs-pe98 / session-gate-all-launches-2026-07-23) failed to enforce.
#
#   AUTHORIZATION: the USER types "LAUNCH CLEARANCE GRANTED" in a prompt to authorize launches for the
#   rest of THIS session (revoke with "LAUNCH CLEARANCE REVOKED"); the er_launch_authorized signal scans
#   the session transcript for it. Fail-closed: absent/error signal => unauthorized => deny.
#
#   DETECTION (hardened 2026-07-24 after an adversarial workflow found 90 bypasses + 9 false positives):
#     * Shell `command` is quote-scrubbed (span scrub, so a MENTION inside quotes is not denied) then
#       matched. me3-launch is COMMAND-POSITION anchored (so `echo me3 launch` is NOT denied) AND the
#       highly-specific `launch -g eldenring` signature is matched anywhere (so a var/quoted me3 like
#       `"$ME3" launch -g eldenring` is still caught). Launcher SCRIPTS are matched two ways:
#       INTERPRETER-FED (name right after bash|sh|zsh|dash|source|.|python3|exec|command, with flags
#       allowed) which is wrapper-agnostic (timeout/nice/env/stdbuf/... before the interpreter no longer
#       evade), and DIRECT command-position (./script or scripts/script). Any path prefix is allowed
#       (absolute / ~ / nested). Conditional mushroom scripts require a real `--launch` FLAG token
#       (comment-stripped) so `--launchpad` / `--launch-later` / a path containing `--launch` do NOT
#       falsely deny.
#     * `code` / `commands` payloads (run_experiment / ctx_execute / ctx_batch_execute) are code, not
#       shell-with-mentions, so they get a coarse RAW scan (launcher basename / eldenring.exe /
#       renderdoccmd / me3+launch / me3_launch / `launch -g eldenring`). This closes the run_experiment
#       launch path (the sanctioned runtime-probe tool).
#
#   IN-ENGINE CONSTRAINTS (bd cupcake-policy-authoring-signals-rerun-no-sprintf-2026-07-23): the cupcake
#   wasm host lacks sprintf/regex.replace/regex.find_all_string_submatch_n, so detection uses ONLY
#   static (literal) regex + split/replace/concat/contains/lower/trim/regex.match/comprehensions/funcs.
#
#   DOCUMENTED RESIDUALS (deliberate-obfuscation class; NOT defended, by design -- a cooperative agent
#   does not launch ER this way and the deny message forbids routing around the gate; robustly catching
#   them needs bash_elden_ring_launch_guard's full bd/gh/git text-exemption machinery, which trades
#   complexity + false-positive risk): `bash -c '<hidden launch>'` / `eval` / `sh -c` (quoted payload is
#   scrubbed away), pipe-to-shell (`echo <launcher> | bash`, `cat script | bash`, `... | xargs bash`),
#   shell-variable indirection (`X=me3; $X launch`, `S=scripts/x; bash $S`), base64/printf-decode-to-
#   bash, backtick command substitution, and a quoted spaceless script path (`bash "scripts/x.sh"`).
# custom:
#   severity: HIGH
#   id: ER-EFFECTS-ELDEN-RING-LAUNCH-REQUIRES-SESSION-AUTH
#   routing:
#     required_events: ["PreToolUse"]
#     required_tools: []
#     required_signals: ["er_launch_authorized"]
package cupcake.policies.claude.er_launch_requires_session_auth

import rego.v1

command := object.get(input.tool_input, "command", "")

tool_name := object.get(input, "tool_name", "")

lower_tool_name := lower(tool_name)

# --- authorization signal (fail-closed): "1" == this session is user-authorized to launch ------------
# Tolerates the bare-string and {output: ...} shapes cupcake may hand back; absent/other => unauthorized.
# cupcake JSON-parses the signal's stdout, so a bare "1" arrives as the NUMBER 1 (not the string "1").
# Accept the number, the string (opa tests + a trailing-whitespace string), and the {output: ...} object
# shape. Absent / any other value => not authorized (fail-closed). (Bug fixed 2026-07-24: the gate denied
# a valid clearance because it only matched the string "1"; is_string(1) is false -> fell through.)
session_authorized if {
	input.signals.er_launch_authorized == 1
}

session_authorized if {
	input.signals.er_launch_authorized == "1"
}

session_authorized if {
	is_string(input.signals.er_launch_authorized)
	trim(input.signals.er_launch_authorized, " \t\r\n") == "1"
}

session_authorized if {
	input.signals.er_launch_authorized.output == 1
}

session_authorized if {
	input.signals.er_launch_authorized.output == "1"
}

# --- deny an approved ER launch when the session is not authorized ------------------------------------
auth_reason := "🧁 Cupcake blocked this Elden Ring launch: this agent session is NOT authorized to launch the game. Every ER launch must be gated on explicit user approval (bd er-effects-rs-pe98 / session-gate-all-launches-2026-07-23; the prose-only directive was violated on 2026-07-23). To authorize launches for THIS session, the USER types the keyphrase LAUNCH CLEARANCE GRANTED in a prompt (revoke with LAUNCH CLEARANCE REVOKED). Do NOT self-authorize, do NOT route around this by inlining me3/eldenring.exe or obfuscating the command, and do NOT ask the user to run the launch for you -- ask them to grant clearance, then launch with the loud banner."

deny contains decision if {
	input.hook_event_name == "PreToolUse"
	guarded_executable_tool
	er_launch_detected
	not session_authorized

	decision := {
		"rule_id": "ER-EFFECTS-ELDEN-RING-LAUNCH-REQUIRES-SESSION-AUTH",
		"severity": "HIGH",
		"reason": concat("", [auth_reason, "\n\nSource: ", command]),
	}
}

guarded_executable_tool if {
	tool_name == "Bash"
}

guarded_executable_tool if {
	contains(lower_tool_name, "ctx_execute")
}

guarded_executable_tool if {
	contains(lower_tool_name, "ctx_batch_execute")
}

guarded_executable_tool if {
	tool_name == "run_experiment"
}

# --- launch detection --------------------------------------------------------------------------------
# Shell `command`: quote-scrubbed then matched with command-position / interpreter-fed rules (mention-safe).
er_launch_detected if {
	launch_form(scrub(command))
}

# `code` / `commands` payloads (run_experiment / ctx_*): coarse RAW scan (code, not shell-with-mentions).
er_launch_detected if {
	some p in code_payloads
	is_string(p)
	code_launch(lower(p))
}

code_payloads contains p if {
	p := object.get(input.tool_input, "code", "")
}

code_payloads contains p if {
	some p in object.get(input.tool_input, "commands", [])
}

# --- shell-command launch forms (operate on the quote-scrubbed command) ------------------------------
# me3 real launch: the `launch -g eldenring` signature is specific enough to match ANY me3 spelling
# (bare, path, quoted var, unquoted var), which is how every genuine me3 launch appears.
launch_form(s) if {
	regex.match(`(?i)launch[[:space:]]+-g[[:space:]]+eldenring`, s)
}

# me3 launch at command position (catches me3-first forms even without -g; `echo me3 launch` is NOT here
# because me3 is not at a command boundary there).
launch_form(s) if {
	regex.match(`(?i)(^|[;|&()]|&&|\|\|)[[:space:]]*([^[:space:];|&()]*/)?me3(\.exe)?[[:space:]]+launch([[:space:];|&()]|$)`, s)
}

# me3_launch helper (from me3-launch-lib.sh) invoked at command position.
launch_form(s) if {
	regex.match(`(^|[;|&()]|&&|\|\|)[[:space:]]*me3_launch([[:space:];|&()]|$)`, s)
}

# renderdoccmd capture (wraps the me3 launch for RenderDoc).
launch_form(s) if {
	regex.match(`(?i)renderdoccmd(\.exe)?[[:space:]]+capture([[:space:];|&()]|$)`, s)
}

# Direct eldenring.exe execution at command position (any path prefix).
launch_form(s) if {
	regex.match(`(?i)(^|[;|&()]|&&|\|\|)[[:space:]]*([^[:space:];|&()]*/)?eldenring\.exe([[:space:];|&()]|$)`, s)
}

# UNCONDITIONAL launcher scripts, INTERPRETER-FED (name right after bash|sh|zsh|dash|source|.|python3|
# exec|command, flags allowed): wrapper-agnostic (timeout/nice/env/stdbuf/... before the interpreter no
# longer evade, since we anchor on the interpreter, not command position).
launch_form(s) if {
	regex.match(`(?i)(^|[[:space:];|&()])(bash|sh|zsh|dash|source|\.|python3?|exec|command)[[:space:]]+(-[^[:space:];|&()]+[[:space:]]+)*([^[:space:];|&()]*/)?(capture-er-frame\.sh|er-smoke-driver\.sh|run-golden-mount-trace\.sh|run-golden-observe\.sh|run-me3-product-smoke\.sh|run-product-continue-direct-probe\.sh|run-multi-save-load-proof\.sh|run-samechar-3x-threedll\.sh|run-samechar-3x-twodll\.sh|run-vanilla-continue-capture\.sh|run-vanilla-reload-agentdriven\.sh|run-vanilla-reload-fps\.sh|run-vanilla-trace-userdrive\.sh|launch-vanilla-offline\.sh|run-accept-byte-focus-probe\.sh|run-camera-smoke\.sh|run-control-no-ownload-probe\.sh|run-install-job-probe\.sh|run-menu-drive-probe\.sh|run-native-continue-probe\.sh|run-own-dispatch-treatment-probe\.sh|run-own-load-pump-probe\.sh|run-ownpump-verify-probe\.sh|run-pab-advance-probe\.sh|run-product-menu-load-probe\.sh|run-realload-headless\.sh|run-realload-onscreen\.sh|run-refactor-smoke-onscreen\.sh|run-refactor-smoke\.sh|run-save-trace-probe\.sh|run-savedir-fix-probe\.sh|run-watch-onscreen\.sh|run-profile-portrait-capture-probe\.sh|record-portrait-run\.sh|record-title-gfx-proof-wf\.sh|run-onscreen-capture-at\.sh|capture-er-at-marker\.sh|launch-golden-both-trace\.sh|launch-golden-confirm-trace\.sh|launch-golden-mount-trace\.sh|smoke-er-window-placement\.sh|me3_live_launch\.py|sq-repro-selfdrive-monitor\.py|y22i-windows-ab-probe\.py|y22i_bounded_probe\.py|record-er-boot-video\.py|vanilla-control-probe\.py)([[:space:];|&()]|$)`, s)
}

# UNCONDITIONAL launcher scripts, DIRECT command-position execution (./script or path/script).
launch_form(s) if {
	regex.match(`(?i)(^|[;|&()]|&&|\|\|)[[:space:]]*(\./)?([^[:space:];|&()]*/)?(capture-er-frame\.sh|er-smoke-driver\.sh|run-golden-mount-trace\.sh|run-golden-observe\.sh|run-me3-product-smoke\.sh|run-product-continue-direct-probe\.sh|run-multi-save-load-proof\.sh|run-samechar-3x-threedll\.sh|run-samechar-3x-twodll\.sh|run-vanilla-continue-capture\.sh|run-vanilla-reload-agentdriven\.sh|run-vanilla-reload-fps\.sh|run-vanilla-trace-userdrive\.sh|launch-vanilla-offline\.sh|run-accept-byte-focus-probe\.sh|run-camera-smoke\.sh|run-control-no-ownload-probe\.sh|run-install-job-probe\.sh|run-menu-drive-probe\.sh|run-native-continue-probe\.sh|run-own-dispatch-treatment-probe\.sh|run-own-load-pump-probe\.sh|run-ownpump-verify-probe\.sh|run-pab-advance-probe\.sh|run-product-menu-load-probe\.sh|run-realload-headless\.sh|run-realload-onscreen\.sh|run-refactor-smoke-onscreen\.sh|run-refactor-smoke\.sh|run-save-trace-probe\.sh|run-savedir-fix-probe\.sh|run-watch-onscreen\.sh|run-profile-portrait-capture-probe\.sh|record-portrait-run\.sh|record-title-gfx-proof-wf\.sh|run-onscreen-capture-at\.sh|capture-er-at-marker\.sh|launch-golden-both-trace\.sh|launch-golden-confirm-trace\.sh|launch-golden-mount-trace\.sh|smoke-er-window-placement\.sh|me3_live_launch\.py|sq-repro-selfdrive-monitor\.py|y22i-windows-ab-probe\.py|y22i_bounded_probe\.py|record-er-boot-video\.py|vanilla-control-probe\.py)([[:space:];|&()]|$)`, s)
}

# CONDITIONAL launcher scripts (launch ER only with a real --launch FLAG), interpreter-fed OR direct.
launch_form(s) if {
	has_launch_flag(s)
	regex.match(`(?i)(^|[[:space:];|&()])(bash|sh|zsh|dash|source|\.|python3?|exec|command)[[:space:]]+(-[^[:space:];|&()]+[[:space:]]+)*([^[:space:];|&()]*/)?(build_mushroom_blender_edit\.sh|route_a_mushroom_build_arm_sweep\.sh|install_mushroom_man\.py)([[:space:];|&()]|$)`, s)
}

launch_form(s) if {
	has_launch_flag(s)
	regex.match(`(?i)(^|[;|&()]|&&|\|\|)[[:space:]]*(\./)?([^[:space:];|&()]*/)?(build_mushroom_blender_edit\.sh|route_a_mushroom_build_arm_sweep\.sh|install_mushroom_man\.py)([[:space:];|&()]|$)`, s)
}

# `--launch` as a whole FLAG token (not a substring like --launchpad / --launch-later / foo--launch.log),
# with any trailing ` #comment` stripped so a `--launch` mentioned in a comment does not falsely gate.
has_launch_flag(s) if {
	regex.match(`(^|[[:space:]])--launch([[:space:]=]|$)`, split(s, " #")[0])
}

# --- code/commands payload launch forms (coarse RAW scan; code, not shell-with-mentions) -------------
code_launch(low) if {
	regex.match("launch[[:space:]\"',]+-g[[:space:]\"',]+eldenring", low)
}

code_launch(low) if {
	contains(low, "eldenring.exe")
}

code_launch(low) if {
	contains(low, "renderdoccmd")
}

code_launch(low) if {
	contains(low, "me3_launch")
}

code_launch(low) if {
	regex.match(`(^|[^a-z0-9_])me3([^a-z0-9_]|$)`, low)
	contains(low, "launch")
}

code_launch(low) if {
	regex.match(`(capture-er-frame\.sh|er-smoke-driver\.sh|run-golden-mount-trace\.sh|run-golden-observe\.sh|run-me3-product-smoke\.sh|run-product-continue-direct-probe\.sh|run-multi-save-load-proof\.sh|run-samechar-3x-threedll\.sh|run-samechar-3x-twodll\.sh|run-vanilla-continue-capture\.sh|run-vanilla-reload-agentdriven\.sh|run-vanilla-reload-fps\.sh|run-vanilla-trace-userdrive\.sh|launch-vanilla-offline\.sh|run-accept-byte-focus-probe\.sh|run-camera-smoke\.sh|run-control-no-ownload-probe\.sh|run-install-job-probe\.sh|run-menu-drive-probe\.sh|run-native-continue-probe\.sh|run-own-dispatch-treatment-probe\.sh|run-own-load-pump-probe\.sh|run-ownpump-verify-probe\.sh|run-pab-advance-probe\.sh|run-product-menu-load-probe\.sh|run-realload-headless\.sh|run-realload-onscreen\.sh|run-refactor-smoke-onscreen\.sh|run-refactor-smoke\.sh|run-save-trace-probe\.sh|run-savedir-fix-probe\.sh|run-watch-onscreen\.sh|run-profile-portrait-capture-probe\.sh|record-portrait-run\.sh|record-title-gfx-proof-wf\.sh|run-onscreen-capture-at\.sh|capture-er-at-marker\.sh|launch-golden-both-trace\.sh|launch-golden-confirm-trace\.sh|launch-golden-mount-trace\.sh|smoke-er-window-placement\.sh|me3_live_launch\.py|sq-repro-selfdrive-monitor\.py|y22i-windows-ab-probe\.py|y22i_bounded_probe\.py|record-er-boot-video\.py|vanilla-control-probe\.py|build_mushroom_blender_edit\.sh|route_a_mushroom_build_arm_sweep\.sh|install_mushroom_man\.py)`, low)
}

# --- quote-span scrub as a function (verbatim idiom from bash_elden_ring_launch_guard; in-engine-safe)
scrub(c) := result if {
	trimmed := split(c, "<<")[0]
	stripped := replace(replace(trimmed, `\"`, ""), `\'`, "")
	dparts := split(stripped, `"`)
	outside_d := concat(" ", [dparts[i] |
		some i
		dparts[i]
		i % 2 == 0
	])
	sparts := split(outside_d, "'")
	result := concat(" ", [sparts[i] |
		some i
		sparts[i]
		i % 2 == 0
	])
}
