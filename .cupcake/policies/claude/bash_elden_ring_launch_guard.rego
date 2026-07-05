# METADATA
# scope: package
# title: Block Steam/EAC Elden Ring Launches and Seamless DLL Bundling
# description: Prevent agent-run executable tool payloads from launching Elden Ring through Steam/EAC or bundling Seamless Co-op's ersc.dll.
# custom:
#   severity: HIGH
#   id: ER-EFFECTS-ELDEN-RING-LAUNCH-GUARD
#   routing:
#     required_events: ["PreToolUse"]
#     required_tools: []
package cupcake.policies.bash_elden_ring_launch_guard

import rego.v1

command := object.get(input.tool_input, "command", "")
code := object.get(input.tool_input, "code", "")
tool_name := object.get(input, "tool_name", "")
lower_tool_name := lower(tool_name)

# Include non-command tool fields so context-mode/run_experiment/batch payloads
# are policy-covered too. `sprintf` keeps arrays/objects searchable enough for
# commands embedded in ctx_batch_execute-style lists.
other_text := concat("\n", [sprintf("%v", [value]) |
	some key, value in input.tool_input
	key != "command"
	key != "code"
])

source_text := concat("\n", [command, code, other_text])
lower_source_text := lower(source_text)

steam_launch_reason := "🧁 Cupcake blocked this Elden Ring launch command. Do not launch AppID 1245620 through Steam from agent workflows (`steam -applaunch 1245620`, `steam://run/1245620`, or `steam://rungameid/1245620`). Use only the repo-approved direct offline eldenring.exe/runtime-probe path when a deliberately authorized runtime probe is required."

start_protected_reason := "🧁 Cupcake blocked this Elden Ring EAC launcher command. Do not run `start_protected_game.exe` directly or via Proton/Wine/Steam from agent workflows. Runtime work must avoid the EAC launcher and use only approved offline/direct eldenring.exe probe paths."

ersc_bundle_reason := "🧁 Cupcake blocked this Seamless Co-op DLL bundling command. Do not copy, move, archive, stage into release artifacts, or package `ersc.dll`; Seamless Co-op is a compatibility target, not a file this repo bundles."

deny contains decision if {
	input.hook_event_name == "PreToolUse"
	guarded_executable_tool
	steam_app_launch_detected

	decision := {
		"rule_id": "ER-EFFECTS-ELDEN-RING-LAUNCH-GUARD",
		"severity": "HIGH",
		"reason": concat("", [steam_launch_reason, "\n\nSource: ", source_text]),
	}
}

deny contains decision if {
	input.hook_event_name == "PreToolUse"
	guarded_executable_tool
	start_protected_launch_detected

	decision := {
		"rule_id": "ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD",
		"severity": "HIGH",
		"reason": concat("", [start_protected_reason, "\n\nSource: ", source_text]),
	}
}

deny contains decision if {
	input.hook_event_name == "PreToolUse"
	guarded_executable_tool
	ersc_bundle_detected

	decision := {
		"rule_id": "ER-EFFECTS-ERSC-DLL-BUNDLE-GUARD",
		"severity": "HIGH",
		"reason": concat("", [ersc_bundle_reason, "\n\nSource: ", source_text]),
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

guarded_executable_tool if {
	tool_name == "tracebreakpoint"
}

steam_app_launch_detected if {
	regex.match(`(?i)(^|[[:space:];|&()])steam[[:space:]]+([^;|&()]*[[:space:]]+)*-applaunch[[:space:]]+1245620([[:space:];|&()]|$)`, scrubbed_command)
}

steam_app_launch_detected if {
	regex.match(`(?i)(^|[[:space:];|&()])(steam|xdg-open|gio|kioclient5|kioclient)[^;|&()]*steam://(run|rungameid)/1245620([/?#[:space:];|&()]|$)`, scrubbed_command)
}

steam_app_launch_detected if {
	executable_source_marker
	contains(marker_scan_text, "steam")
	contains(marker_scan_text, "-applaunch")
	contains(marker_scan_text, "1245620")
}

steam_app_launch_detected if {
	executable_source_marker
	contains(marker_scan_text, "steam://run/1245620")
}

steam_app_launch_detected if {
	executable_source_marker
	contains(marker_scan_text, "steam://rungameid/1245620")
}

start_protected_launch_detected if {
	regex.match(`(?i)(^|[[:space:];|&()])(steam|wine|wine64|proton|env|bash|sh|python|python3)[^;|&()]*start_protected_game\.exe([[:space:];|&()]|$)`, scrubbed_command)
}

start_protected_launch_detected if {
	regex.match(`(?i)(^|[;|&()][[:space:]]*)(\./|/|[[:alnum:]_.-]+/)[^[:space:];|&()]*start_protected_game\.exe([[:space:];|&()]|$)`, scrubbed_command)
}

start_protected_launch_detected if {
	executable_source_marker
	contains(marker_scan_text, "start_protected_game.exe")
	not start_protected_process_detection_only
}

ersc_bundle_detected if {
	regex.match(`(?i)(^|[[:space:];|&()])(cp|mv|install|rsync|zip|tar|7z|rar|python|python3|bash|sh)[^;|&()]*ersc\.dll([[:space:];|&()]|$)`, scrubbed_command)
}

ersc_bundle_detected if {
	contains(marker_scan_text, "ersc.dll")
	bundle_source_marker
}

executable_source_marker if {
	some marker in {"subprocess", "popen", "exec", "system(", "os.system", "spawn", "command::new", "process::command", "shell", "bash", "python", "ctx_execute", "run_experiment"}
	contains(lower_source_text, marker)
}

bundle_source_marker if {
	some marker in {"cp ", "mv ", "install ", "rsync", "zip", "tar", "7z", "rar", "shutil.copy", "copy2", "copyfile", "write_bytes", "zipfile", "tarfile", "archive", "bundle", "stage"}
	contains(lower_source_text, marker)
}

# Scrub quoted spans and heredoc bodies for direct shell command matching so a
# note like `echo 'do not run steam -applaunch 1245620'` is not blocked. Raw
# source_text is still scanned when an executable-code marker is present, which
# catches Python/Rust/shell subprocess wrappers and heredoc bodies that build the
# forbidden launch indirectly.
launch_heredoc_trimmed := split(command, "<<")[0]

launch_escapes_stripped := replace(replace(launch_heredoc_trimmed, `\"`, ""), `\'`, "")

launch_double_parts := split(launch_escapes_stripped, `"`)

launch_outside_double := concat(" ", [launch_double_parts[idx] |
	some idx
	launch_double_parts[idx]
	idx % 2 == 0
])

launch_single_parts := split(launch_outside_double, "'")

scrubbed_command := concat(" ", [launch_single_parts[idx] |
	some idx
	launch_single_parts[idx]
	idx % 2 == 0
])

# ---------------------------------------------------------------------------
# bd issue-tracker text exemption for the marker-based substring fallbacks.
#
# False positive fixed 2026-07-04: `/home/banon/.local/bin/bd create ... -d
# "... do not use start_protected_game.exe ..."` was denied because the
# raw-substring fallbacks scan the whole tool payload, and generic executable
# markers ("bash", "python", "shell", ...) routinely appear in issue prose.
# bd only records text -- it never executes its arguments -- so for a single,
# non-chained Bash invocation of the real bd binary the quoted argument text
# is documentation, not an executable payload.
#
# The exemption is deliberately narrow:
#   * Bash tool only, command starting with the real bd binary path and a
#     text-recording subcommand;
#   * the quote-scrubbed command must contain no separators, subshells,
#     redirects, or backticks (so no second command rides along); and
#   * the raw command must contain no `$(` or backtick anywhere (command
#     substitution inside double quotes executes even though the quote scrub
#     removes it from scrubbed_command).
# Anything chained or indirected falls through to the raw-text scan, and the
# direct scrubbed_command regex rules above are unaffected either way.
# ---------------------------------------------------------------------------

bd_text_command if {
	tool_name == "Bash"
	regex.match(`^[[:space:]]*/home/banon/\.local/bin/bd[[:space:]]+(create|update|comment|comments|remember|close)([[:space:]]|$)`, command)
	not regex.match(`[;|&()<>\x60\n\r]`, scrubbed_command)
	not contains(command, "$(")
	not contains(command, "`")
}

# Text scanned by the marker-based substring fallbacks. For a pure bd text
# command the quote-scrubbed command is scanned instead of the raw payload, so
# a forbidden-form *mention* inside a quoted issue description cannot deny,
# while any unquoted or chained launch token still can.
marker_scan_text := lower(scrubbed_command) if {
	bd_text_command
}

# GitHub issue/PR bodies are text payloads, not executable payloads. Keep direct
# shell-command matching on scrubbed_command, but do not let marker-based raw
# fallback checks deny a `gh ... --body-file` command merely because the PR body
# mentions a forbidden executable by name while documenting policy behavior.
gh_text_body_command if {
	tool_name == "Bash"
	regex.match(`(?is)(^|[[:space:];|&()])gh[[:space:]]+(pr[[:space:]]+(create|comment)|issue[[:space:]]+comment)([[:space:]]|.|\n)*--body-file([[:space:]]|=)`, command)
}

marker_scan_text := lower(scrubbed_command) if {
	gh_text_body_command
}

marker_scan_text := lower_source_text if {
	not bd_text_command
	not gh_text_body_command
}

# Allow exact process-detection checks for the stale protected launcher while
# keeping every launch/execution form blocked. Project policy permits detecting
# stale `start_protected_game.exe` processes before an approved direct/offline
# run; the raw marker fallback used to deny any Bash payload that merely
# contained both a generic executable marker and that process name.
#
# This exemption is intentionally narrow: the full marker scan must contain
# exactly one `start_protected_game.exe` occurrence, and that occurrence must be
# an exact `pgrep -x start_protected_game.exe` token sequence. If a command both
# checks with pgrep and later launches the protected executable, the occurrence
# count is greater than one and the fallback still denies.
start_protected_process_detection_only if {
	count(split(marker_scan_text, "start_protected_game.exe")) == 2
	regex.match(`(?i)(^|[[:space:];|&()])(/usr/bin/)?pgrep[[:space:]]+-x[[:space:]]+(--[[:space:]]+)?start_protected_game\.exe([[:space:];|&()]|$)`, marker_scan_text)
}
