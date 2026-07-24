# METADATA
# scope: package
# title: Block Manual pgrep in Agent Bash Commands (WSL false-negative guard)
# authors: ["er-effects-rs agents"]
# custom:
#   severity: HIGH
#   id: ER-EFFECTS-BLOCK-MANUAL-PGREP
#   description: >-
#     Hard block on agent-invoked `pgrep` inside Bash tool commands. On this WSL2 +
#     native-Windows-Steam box, `pgrep -x steam` (and pgrep for the game/EAC
#     processes, which are also Windows processes) is a FALSE NEGATIVE: those
#     processes are only visible via tasklist.exe, so pgrep reports "down" while
#     they are up. That false negative once blocked an entire overnight runtime
#     session. There is NO escape hatch: the only sanctioned pgrep lives INSIDE the
#     committed helper `scripts/steam-running.sh` (a file on disk, not an agent
#     Bash command, so it is never intercepted here). Steam checks must go through
#     that helper; any other process check must use tasklist.exe / a WSL-aware path.
#     See bd steam-detection-wsl-false-negative-2026-07-18.
#   routing:
#     required_events: ["PreToolUse"]
#     required_tools: ["Bash", "bash"]
package cupcake.policies.claude.block_manual_pgrep

import rego.v1

command := object.get(input.tool_input, "command", "")

# A `pgrep` command token: at command start or after a shell separator
# (whitespace, `;`, `|`/`||`, `&`/`&&`, `(`/`$(`, a backtick, or a quote),
# optionally preceded by an absolute/relative path prefix (`/usr/bin/pgrep`,
# `./pgrep`), and terminated by a non-identifier char so
# `mypgrep`/`pgreptool`/`mypgreptool` never match (the token there is preceded
# or followed by an identifier char, not a delimiter). Quotes ARE delimiters so
# there is NO escape hatch: `bash -c 'pgrep ...'`, `sh -c "pgrep ..."`, and even a
# quoted subprocess arg like `subprocess.run(['pgrep', ...])` are all caught --
# a python subprocess pgrep is still raw Linux pgrep, not a WSL-aware check. The
# whole raw command is scanned (no quote scrubbing) so nothing can be smuggled
# past the guard inside quotes. The only sanctioned pgrep lives inside
# scripts/steam-running.sh (a file on disk, not an agent Bash command).
pgrep_token_pattern := "(^|[[:space:];|&('\"`])/?([[:alnum:]_.-]+/)*pgrep($|[^[:alnum:]_])"

manual_pgrep_detected if {
	regex.match(pgrep_token_pattern, command)
}

block_reason := "🧁 Cupcake blocked a manual pgrep. On this WSL2 + native-Windows-Steam box manual pgrep is blocked because it FALSE-NEGATIVES: Steam runs as the Windows process steam.exe (and the game/EAC processes are Windows processes too), visible only via tasklist.exe, so `pgrep -x steam` reports 'down' while Steam is UP. That false negative once blocked an entire overnight runtime session. For a Steam check run `bash scripts/steam-running.sh` (the committed WSL-aware helper). For any OTHER process use tasklist.exe or a WSL-aware check, never raw pgrep. This guard has NO escape hatch: the only sanctioned pgrep lives INSIDE scripts/steam-running.sh itself. See bd steam-detection-wsl-false-negative-2026-07-18."

deny contains decision if {
	input.hook_event_name == "PreToolUse"
	lower(input.tool_name) == "bash"
	manual_pgrep_detected

	decision := {
		"rule_id": "ER-EFFECTS-BLOCK-MANUAL-PGREP",
		"severity": "HIGH",
		"reason": concat("", [block_reason, "\n\nSource: ", command]),
	}
}
