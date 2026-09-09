# METADATA
# scope: package
# title: Refuse input injection that names no target window
# authors: ["er-quickload agents"]
# custom:
#   severity: HIGH
#   id: ER-EFFECTS-BLOCK-UNTARGETED-INPUT
#   description: >-
#     Refuse wtype / ydotool, and xdotool input verbs without --window, from
#     agent Bash commands. These deliver to whatever window currently holds
#     focus, so on 2026-09-09 two F3 presses meant for Elden Ring landed in the
#     application the user was actually playing. Injection that names its
#     target window is allowed; read-only xdotool queries are untouched.
#   routing:
#     required_events: ["PreToolUse"]
#     required_tools: ["Bash"]
package cupcake.policies.claude.block_compositor_input_injection

import rego.v1

# The defect is UNTARGETED input, not input.
#
# Measured 2026-09-09 with `scripts/frida/keystate-probe.js` while the game polled
# `GetAsyncKeyState` ~46 times a second and `GetForegroundWindow` was never null: `wtype -k F3`,
# `xdotool key F3` and `ydotool key 61:1 61:0` produced zero down events in Elden Ring. Two of
# them were delivered to an unrelated application instead, because a focus-addressed press goes
# wherever focus is -- and Elden Ring was mapped, fullscreen and unfocused on another workspace.
#
# "Check focus first, then press" is not the fix and must not be offered as one: `hyprctl
# activewindow` reported `steam_app_1245620` in the same minute `xdotool getwindowfocus` reported
# a different application, and focus can move between the check and the press regardless.
#
# So the rule is that the press must NAME what it is aimed at. Three forms do:
#   * `xdotool key --window <id> ...`, which addresses one X window;
#   * a Frida `Interceptor` on `user32!GetAsyncKeyState` returning 0x8001 for the vkey, which
#     addresses the process and drives the DLL's real poll, edge latch and handler;
#   * the input harness's `inputmgr+0x90+eventId` keystate write, for a native game binding.
#
# `wtype` and `ydotool` have no target parameter at all -- wtype speaks the Wayland
# virtual-keyboard protocol and ydotool writes a uinput device, and both land on the focused
# surface by construction -- so there is no targeted form of them to allow.
#
# See bd never-inject-keys-at-the-compositor-it-hits-the-focused-app-2026-09-09.

command := object.get(input.tool_input, "command", "")

# `wtype` and `ydotool` exist only to synthesise focus-addressed input, so the bare tool name is
# the token. `xdotool` also answers read-only questions, so only its input-generating verbs are
# named -- `xdotool search`, `getactivewindow` and `getwindowclassname` are how a window is
# identified in the first place and stay allowed.
#
# The token must sit at command start or after a shell separator, and quotes count as separators,
# so a `bash -c '...'` wrapper is caught. An optional path prefix covers `/usr/bin/<tool>`.
#
# A hyphen or dot AFTER the token ends it too, so a script whose file name merely begins with the
# tool's name is not an invocation of it -- the same reasoning as the leading identifier char.
untargeted_tool_pattern := `(^|[[:space:];|&('"\x60])/?([[:alnum:]_.-]+/)*(wtype|ydotool)($|[^[:alnum:]_.-])`

xdotool_input_pattern := `(^|[[:space:];|&('"\x60])/?([[:alnum:]_.-]+/)*xdotool[[:space:]]+(key|keydown|keyup|type|click|mousedown|mouseup|mousemove|mousemove_relative)($|[^[:alnum:]_.-])`

injection_detected if {
	regex.match(untargeted_tool_pattern, command)
	not text_mention_only
}

injection_detected if {
	regex.match(xdotool_input_pattern, command)
	not regex.match(`--window([[:space:]]|=)`, command)
	not text_mention_only
}

# --- text exemptions ---------------------------------------------------------
#
# The same shape the pgrep guard uses, and needed for the same reason: the memory recording this
# lesson names all three tools, and a raw scan would refuse the sentence that documents the rule.
# Fail-closed -- a single, non-chained text-recording command whose token appears only inside
# quotes.

text_mention_only if {
	bd_text_command
	not regex.match(untargeted_tool_pattern, unquoted_command)
	not regex.match(xdotool_input_pattern, unquoted_command)
}

text_mention_only if {
	git_commit_text_command
	not regex.match(untargeted_tool_pattern, unquoted_command)
	not regex.match(xdotool_input_pattern, unquoted_command)
}

bd_text_command if {
	regex.match(`^[[:space:]]*((\$HOME|\$\{HOME\}|~|/home/[[:alnum:]._-]+|/root|/Users/[[:alnum:]._-]+)/\.local/bin/)?bd[[:space:]]+(create|update|comment|comments|remember|close)([[:space:]]|$)`, command)
	single_command
}

git_commit_text_command if {
	regex.match(`^[[:space:]]*(command[[:space:]]+)?git[[:space:]]+commit([[:space:]]|$)`, command)
	single_command
}

# No second command may ride along, and no substitution may execute from inside the quotes the
# exemption is trusting.
single_command if {
	not regex.match(`[;|&()<>\x60\n\r]`, unquoted_command)
	not contains(command, "$(")
	not regex.match(`\x60`, command)
}

# Keep only the text OUTSIDE quoted spans: escaped quotes first, then double, then single.
escapes_stripped := replace(replace(command, `\"`, ""), `\'`, "")

double_parts := split(escapes_stripped, `"`)

outside_double := concat(" ", [double_parts[idx] |
	some idx
	double_parts[idx]
	idx % 2 == 0
])

single_parts := split(outside_double, "'")

unquoted_command := concat(" ", [single_parts[idx] |
	some idx
	single_parts[idx]
	idx % 2 == 0
])

block_reason := concat("", [
	"This press names no target window, so it goes wherever focus is. On 2026-09-09 that put two ",
	"F3 presses into an unrelated application while Elden Ring sat unfocused, and a probe measured ",
	"ZERO of them reaching the game. Checking focus first does not fix it: hyprctl and the X ",
	"server disagreed about focus in the same minute, and focus can move between the check and ",
	"the press. Name the target instead: `xdotool key --window <id> ...` for an X window, a Frida ",
	"Interceptor on user32!GetAsyncKeyState returning 0x8001 for the vkey (see ",
	"scripts/frida/f3-toggle-drive.js) for an OS-level hotkey, or the input harness's ",
	"inputmgr+0x90 keystate write for a native game binding. wtype and ydotool have no target ",
	"parameter at all. Read-only xdotool queries are not blocked.",
])

deny contains decision if {
	input.hook_event_name == "PreToolUse"
	input.tool_name == "Bash"
	injection_detected

	decision := {
		"rule_id": "ER-EFFECTS-BLOCK-UNTARGETED-INPUT",
		"severity": "HIGH",
		"reason": concat("", [block_reason, "\n\nSource: ", command]),
	}
}
