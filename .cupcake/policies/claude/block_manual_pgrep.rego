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
#     any other process check must use tasklist.exe / a WSL-aware path. The
#     narrow non-executing exemptions cover TEXT positions only: a single,
#     non-chained bd issue-tracker command whose pgrep token sits entirely
#     inside quoted text (bd only records text; see bd er-effects-rs-uxyz), and
#     a single git commit whose token sits in the recorded MESSAGE (2026-08-12).
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

pgrep_quoted_text_mention_only if {
	git_commit_text_command
	not regex.match(pgrep_token_pattern, pgrep_commit_executable_region)
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

# ---------------------------------------------------------------------------
# git commit MESSAGE text exemption (mirrors bash_elden_ring_launch_guard's
# git_commit_text_command, which exists for the same class of false positive).
#
# False positive fixed 2026-08-12: a commit whose message PROSE described
# removing a raw process-name probe call from a shell script was denied --
#
#   git commit -F - <<'EOF'
#   preflight: stop probing the process table by name
#
#   The preflight called pgrep -x steam directly, which false-negatives on
#   this WSL2 + Windows-Steam box. It now sources scripts/steam-running.sh.
#   EOF
#
# Nothing executed the probe: git reads the message from stdin and records it.
# The agent worked around the denial by writing the message to a file and
# passing `git commit -F <path>`, which is an escape hatch around a guard --
# exactly the thing this repo treats as a defect rather than routine friction.
# A guard whose own removal cannot be described in the commit that removes it
# is unwritable in the repo that enforces it.
#
# Three message-carrying forms qualify, each already proven fail-closed in the
# launch guard, and each additionally requires that the token does NOT appear
# in the command's EXECUTABLE region (pgrep_commit_executable_region below):
#   * plain form: no heredoc, no `$(`, no backtick anywhere; the quote-scrubbed
#     command must be `git [-C <path>] add|commit ...` segments joined by `&&`
#     with no separators, subshells, redirects or unquoted newlines;
#   * `-m "$(cat <<'TAG' ... TAG )"` message substitution: exactly one `$(`,
#     immediately a `cat` reading a single quoted-tag heredoc, and the
#     whitespace-normalized command must end at the terminator followed by
#     exactly `)"`, so `cat` is the only command inside the substitution and
#     nothing rides after the message text;
#   * `-F -` heredoc: exactly one heredoc with a QUOTED tag (an unquoted tag
#     leaves the body subject to expansion, so its contents are not inert), the
#     region before it must be nothing but git add/commit ending in `-F -`, and
#     the terminator must be the LAST thing in the normalized command so no
#     trailing shell rides along.
# Anything chained, piped, substituted, wrapped in `bash -c`, or carrying a
# second heredoc fails the shape and stays denied. Nothing that could EXECUTE
# the probe gets an escape hatch: the exemption can only silence a mention that
# sits in the message git records.
# ---------------------------------------------------------------------------

git_commit_text_command if {
	input.tool_name == "Bash"
	not contains(command, "$(")
	not contains(command, "`")
	not contains(command, "<<")
	regex.match(git_commit_only_pattern, pgrep_unquoted_command)
}

git_commit_text_command if {
	input.tool_name == "Bash"
	not contains(command, "`")
	count(split(command, "$(")) == 2
	count(pgrep_heredoc_parts) == 2
	regex.match(`^(git( -C [^[:space:];|&()<>]+)? add [^;|&()<>]*&& )?git( -C [^[:space:];|&()<>]+)? commit [^;|&()<>]*"\$\(cat $`, pgrep_heredoc_parts[0])
	terminator_parts := split(pgrep_norm_command, concat("", [" ", pgrep_heredoc_tag]))
	count(terminator_parts) == 2
	terminator_parts[1] == ` )"`
}

git_commit_text_command if {
	input.tool_name == "Bash"
	not contains(command, "$(")
	not contains(command, "`")
	count(pgrep_heredoc_parts) == 2

	# `git -C <path>` is accepted because AGENTS.md mandates it for worktree-scoped
	# work; requiring a bare `git commit` would disable this exemption for the
	# documented invocation, the same way a hard-coded home-directory literal once
	# disabled bd_text_command for the documented `$HOME/.local/bin/bd` form.
	regex.match(`^(git( -C [^[:space:];|&()<>]+)? add [^;|&()<>]*&& )?git( -C [^[:space:];|&()<>]+)? commit [^;|&()<>]*-F - $`, pgrep_heredoc_parts[0])
	terminator_parts := split(pgrep_norm_command, concat("", [" ", pgrep_heredoc_tag]))
	count(terminator_parts) == 2
	terminator_parts[1] == ""
}

git_commit_only_pattern := `^[[:space:]]*git([[:space:]]+-C[[:space:]]+[^[:space:];|&()<>]+)?[[:space:]]+(add|commit)[^;|&()<>\n\r]*(&&[[:space:]]*git([[:space:]]+-C[[:space:]]+[^[:space:];|&()<>]+)?[[:space:]]+(add|commit)[^;|&()<>\n\r]*)*$`

# The part of a git commit command the shell can EXECUTE, i.e. everything that
# is not the recorded message. For the heredoc forms that is the text before
# `<<` (the shape checks above already pin what follows the terminator: nothing
# at all, or exactly the ` )"` that closes the message substitution); for the
# plain form it is the whole quote-scrubbed command. A token here is a real
# process probe, not prose, and keeps the guard on.
#
# The default is a fail-closed sentinel: it CONTAINS the token, so if the
# region is ever undefined the match succeeds, `not` fails, and no exemption is
# granted. (Rego's `not <undefined>` is true, so a bare undefined region would
# otherwise hand out the exemption.)
default pgrep_commit_executable_region := " pgrep "

pgrep_commit_executable_region := pgrep_heredoc_parts[0] if {
	count(pgrep_heredoc_parts) == 2
}

pgrep_commit_executable_region := pgrep_unquoted_command if {
	count(pgrep_heredoc_parts) == 1
}

# Whitespace-normalized command. The live cupcake engine collapses whitespace
# before policy evaluation (heredoc newlines arrive as single spaces) while
# `opa test` sees the raw multiline text, so normalize explicitly and the shape
# checks behave identically in both. The engine evaluates policies as
# `opa build -t wasm` modules whose host does NOT provide `regex.replace`,
# `regex.find_all_string_submatch_n` or `sprintf`, so this sticks to builtins
# the rest of this policy already proves work in-engine.
pgrep_norm_command := concat(" ", [word |
	some word in split(replace(replace(replace(command, "\t", " "), "\r", " "), "\n", " "), " ")
	word != ""
])

pgrep_heredoc_parts := split(pgrep_norm_command, "<<")

# Heredoc tag, required to be QUOTED so the body is fully literal (no
# $-expansion). Single- and double-quoted tags are both accepted; the
# `^-? ?$` anchor on the text between `<<` and the first quote is what keeps a
# stray quote later in the command from being mistaken for a tag delimiter.
pgrep_heredoc_tag := tag if {
	quote_parts := split(pgrep_heredoc_parts[1], "'")
	count(quote_parts) >= 3
	regex.match(`^-? ?$`, quote_parts[0])
	tag := quote_parts[1]
	regex.match(`^[A-Za-z_][A-Za-z0-9_]*$`, tag)
}

pgrep_heredoc_tag := tag if {
	quote_parts := split(pgrep_heredoc_parts[1], `"`)
	count(quote_parts) >= 3
	regex.match(`^-? ?$`, quote_parts[0])
	tag := quote_parts[1]
	regex.match(`^[A-Za-z_][A-Za-z0-9_]*$`, tag)
}

block_reason := "🧁 Cupcake blocked a manual pgrep. On this WSL2 + native-Windows-Steam box manual pgrep is blocked because it FALSE-NEGATIVES: Steam runs as the Windows process steam.exe (and the game/EAC processes are Windows processes too), visible only via tasklist.exe, so `pgrep -x steam` reports 'down' while Steam is UP. That false negative once blocked an entire overnight runtime session. For a Steam check run `bash scripts/steam-running.sh` (the committed WSL-aware helper). For any OTHER process use tasklist.exe or a WSL-aware check, never raw pgrep. This guard has NO escape hatch for anything that could execute pgrep: the only sanctioned pgrep lives INSIDE scripts/steam-running.sh itself. (Narrow text exemptions: a single non-chained bd issue-tracker command may MENTION the token inside quoted text, and a single git commit may mention it in the recorded MESSAGE -- `-m \"...\"`, `-m \"$(cat <<'TAG' ... TAG )\"`, or `-F -` with a quoted-tag heredoc. Chained batches, `bash -c` wrappers, command substitution and anything after the heredoc terminator do not qualify -- run those invocations one at a time.) See bd steam-detection-wsl-false-negative-2026-07-18."

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
