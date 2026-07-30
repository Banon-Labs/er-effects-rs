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
#     session. There is NO escape hatch for anything that could EXECUTE pgrep:
#     the only sanctioned pgrep lives INSIDE the committed helper
#     `scripts/steam-running.sh` (a file on disk, not an agent Bash command, so
#     it is never intercepted here). Steam checks must go through that helper;
#     any other process check must use tasklist.exe / a WSL-aware path. The one
#     narrow non-executing exemption: a single, non-chained bd issue-tracker
#     command whose pgrep token sits entirely inside quoted text (bd only
#     records text; see bd er-effects-rs-uxyz).
#     See bd steam-detection-wsl-false-negative-2026-07-18.
#   routing:
#     required_events: ["PreToolUse"]
#     required_tools: ["Bash"]
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
# past the guard inside quotes -- with ONE narrow non-executing exception, the
# bd issue-tracker text exemption below. The only sanctioned pgrep lives inside
# scripts/steam-running.sh (a file on disk, not an agent Bash command).
pgrep_token_pattern := "(^|[[:space:];|&('\"`])/?([[:alnum:]_.-]+/)*pgrep($|[^[:alnum:]_])"

manual_pgrep_detected if {
	regex.match(pgrep_token_pattern, command)
	not pgrep_quoted_text_mention_only
}

# ---------------------------------------------------------------------------
# bd issue-tracker text exemption (mirrors bash_elden_ring_launch_guard's
# bd_text_command).
#
# False positive fixed 2026-07-30 (bd er-effects-rs-uxyz): a `bd close` whose
# quoted --reason described launch-guard allow-tests was denied because the
# reason text mentioned the tool by name, e.g.
#
#   $HOME/.local/bin/bd close er-effects-rs-aaa --reason "launch-guard
#   allow-test keeps pgrep -x start_protected_game.exe detection green"
#
# Nothing executed pgrep -- the token appeared only inside quoted
# issue-tracker text. bd only records text, so for a single, non-chained Bash
# invocation of the real bd binary whose pgrep token sits entirely inside
# quoted text, the mention is documentation, not a process check.
#
# The exemption is deliberately narrow and fail-closed:
#   * Bash tool only, command starting with the bd binary (current-user-
#     agnostic path or bare `bd`) and a text-recording subcommand;
#   * the quote-scrubbed command must contain no separators, subshells,
#     redirects, or backticks (so no second command rides along);
#   * the raw command must contain no `$(` or backtick anywhere (command
#     substitution inside double quotes executes even though the quote scrub
#     removes it); and
#   * the pgrep token must NOT appear in the quote-scrubbed command -- an
#     unquoted pgrep anywhere keeps the guard on.
# A chained `bash -c '... bd close ... && bd close ...'` batch (the exact
# shape denied on 2026-07-29) does NOT get the exemption -- it is not a single
# bd invocation -- and stays denied by design; run bd invocations one at a
# time. Anything that could execute pgrep still has no escape hatch.
# ---------------------------------------------------------------------------

pgrep_quoted_text_mention_only if {
	bd_text_command
	not regex.match(pgrep_token_pattern, pgrep_unquoted_command)
}

bd_text_command if {
	input.tool_name == "Bash"
	regex.match(`^[[:space:]]*((\$HOME|\$\{HOME\}|~|/home/[[:alnum:]._-]+|/root|/Users/[[:alnum:]._-]+)/\.local/bin/)?bd[[:space:]]+(create|update|comment|comments|remember|close)([[:space:]]|$)`, command)
	not regex.match(`[;|&()<>\x60\n\r]`, pgrep_unquoted_command)
	not contains(command, "$(")
	not contains(command, "`")
}

# Quote scrub (same shape as the launch guard's scrubbed_command): strip
# escaped quotes, then keep only the text OUTSIDE double- and single-quoted
# spans. Only the bd text exemption reads this; detection itself still scans
# the raw command.
pgrep_escapes_stripped := replace(replace(command, `\"`, ""), `\'`, "")

pgrep_double_parts := split(pgrep_escapes_stripped, `"`)

pgrep_outside_double := concat(" ", [pgrep_double_parts[idx] |
	some idx
	pgrep_double_parts[idx]
	idx % 2 == 0
])

pgrep_single_parts := split(pgrep_outside_double, "'")

pgrep_unquoted_command := concat(" ", [pgrep_single_parts[idx] |
	some idx
	pgrep_single_parts[idx]
	idx % 2 == 0
])

block_reason := "🧁 Cupcake blocked a manual pgrep. On this WSL2 + native-Windows-Steam box manual pgrep is blocked because it FALSE-NEGATIVES: Steam runs as the Windows process steam.exe (and the game/EAC processes are Windows processes too), visible only via tasklist.exe, so `pgrep -x steam` reports 'down' while Steam is UP. That false negative once blocked an entire overnight runtime session. For a Steam check run `bash scripts/steam-running.sh` (the committed WSL-aware helper). For any OTHER process use tasklist.exe or a WSL-aware check, never raw pgrep. This guard has NO escape hatch for anything that could execute pgrep: the only sanctioned pgrep lives INSIDE scripts/steam-running.sh itself. (Narrow text exemption: a single non-chained bd issue-tracker command may MENTION the token inside quoted text; chained batches do not qualify -- run bd invocations one at a time.) See bd steam-detection-wsl-false-negative-2026-07-18."

deny contains decision if {
	input.hook_event_name == "PreToolUse"
	input.tool_name == "Bash"
	manual_pgrep_detected

	decision := {
		"rule_id": "ER-EFFECTS-BLOCK-MANUAL-PGREP",
		"severity": "HIGH",
		"reason": concat("", [block_reason, "\n\nSource: ", command]),
	}
}
