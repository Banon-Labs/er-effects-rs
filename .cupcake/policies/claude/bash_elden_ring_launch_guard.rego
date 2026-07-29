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

# The launcher in COMMAND POSITION with no path at all: `start_protected_game.exe
# --offline`.  The path-prefixed clause above requires a `./`, `/` or `dir/`
# prefix, and the wrapper clause requires an executor verb, so a bare invocation
# resolved through PATH or a Wine working directory slipped between them.
# Anchored to command position, so prose (`echo start_protected_game.exe ...`)
# still does not match.
start_protected_launch_detected if {
	regex.match(`(?i)(^|[;|&()][[:space:]]*)start_protected_game\.exe([[:space:];|&()]|$)`, scrubbed_command)
}

# An executor verb in COMMAND POSITION whose operand is a QUOTED path to the
# launcher: `steam "/mnt/c/.../start_protected_game.exe"`.
#
# `scrubbed_command` deliberately deletes quoted spans so that prose like
# `echo 'do not run steam -applaunch 1245620'` is not blocked -- but that also
# deleted the launcher path out of a real quoted invocation, which is why this
# form went undetected.  Matching a quote-DELETED variant recovers it, and the
# command-position anchor is what keeps the prose exemption intact: in
# `echo "wine start_protected_game.exe"` the command is `echo`, not `wine`.
start_protected_launch_detected if {
	regex.match(
		`(?i)(^|[;|&()][[:space:]]*)(steam|wine|wine64|proton|env|bash|sh|python|python3)[[:space:]][^;|&()]*start_protected_game\.exe`,
		launch_quotes_deleted,
	)
	not start_protected_process_detection_only

	# Deleting quotes exposes issue-tracker PROSE to a command-position match:
	# `bd create -d "... forbidden forms (steam -applaunch / start_protected_game.exe)"`
	# puts a literal `(` before `steam`, which reads as a subshell opener. bd only
	# records text, so reuse the same narrow exemption the substring fallbacks use.
	not bd_text_command
}

# Quotes removed rather than quoted SPANS removed, so a quoted operand survives
# for the command-position checks above.  Heredoc payload is already trimmed off
# upstream, so an interpreter heredoc that merely scans for the launcher name is
# unaffected.
launch_quotes_deleted := replace(replace(launch_escapes_stripped, `"`, ""), "'", "")

# ---------------------------------------------------------------------------
# Shell AFTER a heredoc terminator.
#
# `launch_heredoc_trimmed` keeps only `split(command, "<<")[0]`, which drops the
# payload -- and everything past the terminator with it.  So a real launch chained
# after a heredoc was invisible to EVERY check built on the scrub chain:
#
#     git commit -q -F - <<'EOF'
#     ... message ...
#     EOF
#     steam "/opt/er/start_protected_game.exe"
#
# The payload must stay excluded (it is data), but post-terminator text is genuine
# shell and has to be scanned.  `protected_paths.rego` draws exactly this
# distinction with `command_operand_region`; this is the launch-guard half.
# ---------------------------------------------------------------------------
launch_post_heredoc_region := region if {
	count(proc_scan_heredoc_parts) == 2
	parts := split(proc_scan_norm_command, concat("", [" ", proc_scan_heredoc_tag]))
	count(parts) == 2
	region := replace(replace(parts[1], `"`, ""), "'", "")
}

# Executor verb plus the launcher, after the terminator.
start_protected_launch_detected if {
	regex.match(
		`(?i)(^|[[:space:];|&()])(steam|wine|wine64|proton|env|bash|sh|python|python3)[[:space:]][^;|&()]*start_protected_game\.exe`,
		launch_post_heredoc_region,
	)
}

# The launcher itself, bare or path-prefixed, after the terminator.
start_protected_launch_detected if {
	regex.match(
		`(?i)(^|[[:space:];|&()])(\./|/|[[:alnum:]_.-]+/)*start_protected_game\.exe([[:space:];|&()]|$)`,
		launch_post_heredoc_region,
	)
}

# ---------------------------------------------------------------------------
# Seamless Co-op DLL bundling detection.
#
# The repo rule is about MOVING/PACKAGING the file (copy, move, archive, stage
# into a me3 profile / target bundle / release artifact) -- NOT about naming it.
# Documentation, issue text, commit messages and knowledge-base memories that
# merely MENTION `ersc.dll` while describing Seamless compatibility are exactly
# what AGENTS.md asks agents to record, so every arm below requires a real
# file-moving/packaging VERB acting on the path:
#
#   (a) a copy/archive/interpreter command word at command position with the
#       path as an operand, in the quote-scrubbed command;
#   (b) a redirect whose write TARGET is the file;
#   (c) a library copy/archive CALL (shutil.copy2, tarfile, ...) -- code-shaped
#       tokens that do not occur in English prose; or
#   (d) a copy/archive verb heading the very statement that names the path,
#       which catches quoted operands (`cp 'SeamlessCoop/ersc.dll' dist/`) that
#       the quote scrub removes from (a).
#
# False positive fixed 2026-07-28: `$HOME/.local/bin/bd remember "... works
# identically under Seamless Co-op (ersc.dll redirects paths ...)" --key ...`
# was denied. Two independent defects: the bd text exemption below matched only
# a hard-coded `/home/banon/...` path (never the `$HOME/...` form AGENTS.md
# actually documents), and the old fallback denied on `ersc.dll` appearing
# anywhere together with ANY of the substrings "stage"/"bundle"/"archive"/
# "tar"/"rar"/"zip" -- which match inside ordinary words like "target",
# "startup", "library" and "staged". That made every doc/memory/commit/PR body
# about Seamless compatibility unwritable.
# ---------------------------------------------------------------------------

# (a) copy/archive/interpreter command word + unquoted path operand.
ersc_bundle_detected if {
	regex.match(`(?i)(^|[[:space:];|&()])(cp|copy|mv|move|install|rsync|scp|ln|dd|tee|zip|unzip|tar|7z|7za|rar|unrar|cpio|gzip|xz|xcopy|robocopy|python|python3|bash|sh)[^;|&()]*ersc\.dll([[:space:];|&()]|$)`, scrubbed_command)
}

# (b) redirect whose write target is the DLL (`cat ... > profile/ersc.dll`).
# `[^-=]` in front keeps prose arrows (`SeamlessCoop -> ersc.dll`) out.
ersc_bundle_detected if {
	regex.match(`(^|[^-=])>{1,2}[[:space:]]*['"]?[^[:space:];|&()'"]*ersc\.dll`, marker_scan_text)
}

# (c) library copy/archive calls. These tokens are code, not prose, so they are
# scanned across the whole payload (covers multi-line ctx_execute code fields).
ersc_bundle_detected if {
	contains(marker_scan_text, "ersc.dll")
	some marker in {
		"shutil.copy", "shutil.move", "copyfile", "copy2", "copytree",
		"write_bytes", "zipfile", "tarfile", "os.rename", "os.replace",
		"make_archive",
	}
	contains(marker_scan_text, marker)
}

# (d) copy/archive verb heading the statement that names the DLL.
ersc_bundle_detected if {
	some tokens in ersc_naming_statement_tokens
	tokens[0] in ersc_bundle_verbs
}

ersc_bundle_detected if {
	some tokens in ersc_naming_statement_tokens
	tokens[0] in ersc_command_wrappers
	tokens[1] in ersc_bundle_verbs
}

ersc_bundle_detected if {
	some tokens in ersc_naming_statement_tokens
	tokens[0] in ersc_command_wrappers
	tokens[1] in ersc_command_wrappers
	tokens[2] in ersc_bundle_verbs
}

ersc_bundle_verbs := {
	"cp", "copy", "mv", "move", "install", "rsync", "scp", "ln", "dd", "tee",
	"zip", "unzip", "tar", "7z", "7za", "rar", "unrar", "cpio", "gzip", "xz",
	"xcopy", "robocopy",
}

# Words that may precede the real verb without changing what the statement does.
ersc_command_wrappers := {
	"sudo", "env", "nohup", "setsid", "time", "xargs", "command",
	"subprocess.run", "subprocess.call", "subprocess.check_call",
	"subprocess.check_output", "subprocess.popen", "os.system",
}

# Statement separators normalized to `;` so the statement that NAMES the DLL can
# be isolated. `$(` counts as a separator: a command substitution starts a fresh
# command context (`git commit -m "$(cp SeamlessCoop/ersc.dll dist/)"`).
ersc_scan_separated := replace(replace(replace(replace(replace(replace(marker_scan_text, "$(", ";"), "\n", ";"), "\r", ";"), "|", ";"), "&", ";"), "`", ";")

ersc_scan_pieces := split(ersc_scan_separated, "ersc.dll")

# One entry per occurrence: the tokens of the statement naming the DLL. Quote,
# bracket and comma punctuation becomes whitespace so an argv-list operand
# (`subprocess.run(["cp", "SeamlessCoop/ersc.dll", ...])`) tokenizes like a
# shell command line.
ersc_naming_statement_tokens contains tokens if {
	some idx, piece in ersc_scan_pieces
	idx < count(ersc_scan_pieces) - 1
	statements := split(piece, ";")
	statement := statements[count(statements) - 1]
	cleaned := replace(replace(replace(replace(replace(replace(statement, "\t", " "), `"`, " "), "'", " "), "(", " "), "[", " "), ",", " ")
	tokens := [token |
		some token in split(cleaned, " ")
		token != ""
	]
}

executable_source_marker if {
	some marker in {"subprocess", "popen", "exec", "system(", "os.system", "spawn", "command::new", "process::command", "shell", "bash", "python", "ctx_execute", "run_experiment"}
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
#     text-recording subcommand. The path is matched CURRENT-USER-AGNOSTICALLY
#     (`$HOME`/`${HOME}`/`~`/`/home/<user>`/`/root`/`/Users/<user>`, or a bare
#     `bd`): AGENTS.md documents the invocation as `$HOME/.local/bin/bd`, and a
#     hard-coded `/home/banon/...` literal silently disabled this exemption for
#     the documented form and for every other user's home (2026-07-28);
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
	regex.match(`^[[:space:]]*((\$HOME|\$\{HOME\}|~|/home/[[:alnum:]._-]+|/root|/Users/[[:alnum:]._-]+)/\.local/bin/)?bd[[:space:]]+(create|update|comment|comments|remember|close)([[:space:]]|$)`, command)
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

# git commit messages are text payloads, not executable payloads. A commit
# whose quoted -m body (or `-m "$(cat <<'TAG' ...)"` heredoc message) merely
# MENTIONS ersc.dll or a forbidden launcher is documentation, not bundling.
# False positive fixed 2026-07-07: `git add -A && git commit -m "... a
# resident ersc.dll wins over an env hint ..."` was denied because the raw
# marker fallback scans quoted prose, and bundle_source_marker substrings
# ("stage", "bundle", even "tar" inside "target"/"start" and "rar" inside
# "library") routinely appear in commit prose. Keep direct shell-command
# matching on scrubbed_command; only the marker-based raw fallbacks scan the
# quote-scrubbed command instead.
#
# The exemption is deliberately narrow and fail-closed. Both forms require a
# Bash tool command consisting ONLY of git add/commit invocations:
#   * plain form: no heredoc, no `$(`, no backtick anywhere; the quote-
#     scrubbed command must be `git add|commit ...` segments joined by `&&`
#     with no separators, redirects, or unquoted newlines; and no unquoted
#     token may be a copy/archive/interpreter/launcher word (so a smuggled
#     `cp`/`wine` that survives engine whitespace collapsing still keeps the
#     exemption off);
#   * heredoc form: the canonical `git commit ... -m "$(cat <<'TAG'` message
#     substitution -- exactly one `$(`, immediately a `cat` reading a single
#     quoted-tag heredoc, and the whitespace-normalized command must end at
#     the terminator followed by exactly `)"` (nothing rides after the
#     message text, which cat only prints and git only records).
# Anything chained or indirected falls through to the raw-text scan, and the
# direct scrubbed_command regex rules above are unaffected either way.
git_commit_text_command if {
	tool_name == "Bash"
	not contains(command, "$(")
	not contains(command, "`")
	not contains(command, "<<")
	regex.match(`^[[:space:]]*git[[:space:]]+(add|commit)[^;|&()<>\n\r]*(&&[[:space:]]*git[[:space:]]+(add|commit)[^;|&()<>\n\r]*)*$`, scrubbed_command)
	not git_commit_smuggled_word
}

git_commit_text_command if {
	tool_name == "Bash"
	not contains(command, "`")
	count(split(command, "$(")) == 2
	count(proc_scan_heredoc_parts) == 2
	regex.match(`^(git add [^;|&()<>]*&& )?git commit [^;|&()<>]*"\$\(cat $`, proc_scan_heredoc_parts[0])
	terminator_parts := split(proc_scan_norm_command, concat("", [" ", proc_scan_heredoc_tag]))
	count(terminator_parts) == 2
	terminator_parts[1] == ` )"`
}

# Third form: `git commit -F -` reads the message from STDIN, so the message is a
# plain heredoc with no `$(cat ...)` substitution at all.
#
# False positive fixed 2026-07-29: a commit DESCRIBING this guard's own launcher
# fix was denied. Such a message necessarily names the launcher, and guard prose
# is dense with executable marker words ("shell", "bash", "python"), so the
# marker fallback fired on pure documentation. The `-m` and `-m "$(cat <<'TAG'`
# forms were already exempt; this one was simply never written.
#
# Same fail-closed shape as the sibling clauses: exactly one heredoc, the region
# before it must be nothing but git add/commit ending in `-F -`, the terminator
# must be the LAST thing in the normalized command (so no trailing shell rides
# along), no command substitution or backtick anywhere, and no dangerous unquoted
# token. `git` only reads stdin here -- it never executes the message.
git_commit_text_command if {
	tool_name == "Bash"
	not contains(command, "$(")
	not contains(command, "`")
	count(proc_scan_heredoc_parts) == 2

	# `git -C <path>` is accepted because AGENTS.md mandates it for worktree-scoped
	# work; requiring a bare `git add`/`git commit` would disable this exemption for
	# the documented invocation, the same way a hard-coded home-directory literal
	# disabled bd_text_command for the documented `$HOME/.local/bin/bd` form.
	regex.match(`^(git( -C [^[:space:];|&()<>]+)? add [^;|&()<>]*&& )?git( -C [^[:space:];|&()<>]+)? commit [^;|&()<>]*-F - $`, proc_scan_heredoc_parts[0])
	terminator_parts := split(proc_scan_norm_command, concat("", [" ", proc_scan_heredoc_tag]))
	count(terminator_parts) == 2
	terminator_parts[1] == ""
	not git_commit_smuggled_word
}

# Unquoted command words that could copy/stage/launch if a second command is
# smuggled into the git-only shape (e.g. a newline-chained payload after the
# engine collapses whitespace). Scanned as whole tokens of the quote-scrubbed
# command, so commit-message prose (quoted, already scrubbed) never matches.
git_commit_smuggled_word if {
	dangerous := {
		"cp", "mv", "install", "rsync", "zip", "unzip", "tar", "7z", "rar",
		"python", "python3", "bash", "sh", "dash", "zsh", "fish", "env",
		"xargs", "eval", "exec", "nohup", "setsid", "dd", "ln", "curl",
		"wget", "wine", "wine64", "proton", "steam", "xdg-open", "gio", "gh",
	}
	normalized := replace(replace(replace(scrubbed_command, "\t", " "), "\r", " "), "\n", " ")
	some token in split(lower(normalized), " ")
	token in dangerous
}

marker_scan_text := lower(scrubbed_command) if {
	git_commit_text_command
}

marker_scan_text := lower_source_text if {
	not bd_text_command
	not gh_text_body_command
	not git_commit_text_command
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

# /proc process-detection/teardown python payloads may NAME the protected
# launcher inside quoted string literals. The sanctioned no-pgrep process
# check (pgrep self-matches its own command line) scans /proc/<pid>/comm from
# a python heredoc or `python3 -c` one-liner and compares each comm against a
# tuple of names; it may also send SIGTERM/SIGKILL to exact matching stale
# process ids. That detection/cleanup payload does not launch anything, but
# the raw marker fallback used to deny it because it contains both a generic
# executable marker ("python") and the process name (false positive 2026-07-05).
#
# The exemption is deliberately narrow and fail-closed. It requires ALL of:
#   * Bash tool, and the whole shell command is a single python invocation
#     reading inline code, optionally preceded only by `cd <dir> &&`: either
#     `python3 - <<'TAG'` with a quoted heredoc tag, nothing else on the
#     first line, and nothing after the terminator line; or `python3 -c`
#     whose quote-scrubbed remainder is empty (the entire program is quoted,
#     nothing chained);
#   * the payload mentions `/proc/` (it is a process-state reader/teardown
#     loop);
#   * no `$(` and no backtick anywhere (no command substitution rides along);
#   * every `start_protected_game.exe` occurrence sits inside a quoted string
#     literal, never in shell/python execution position, and the name does
#     not appear in non-command tool fields; and
#   * the payload contains no process-launch mechanism (subprocess,
#     os.system, exec*/spawn/eval, `sh -c`, wine/proton/steam-launch tokens).
# Any failed condition falls through to the raw marker fallback, and the
# direct scrubbed_command regex rules are unaffected either way.
start_protected_process_detection_only if {
	proc_scan_detection_or_teardown_command
}

start_protected_process_detection_only if {
	pgrep_subprocess_detection_command
}

start_protected_process_detection_only if {
	launcher_named_only_as_nested_string_literal
}

proc_scan_detection_or_teardown_command if {
	tool_name == "Bash"
	proc_scan_python_shape
	contains(lower(command), "/proc/")
	not contains(command, "$(")
	not contains(command, "`")
	not contains(detection_unquoted_command, "start_protected_game.exe")
	not proc_scan_exec_marker
	not contains(lower(other_text), "start_protected_game.exe")
}

# The live cupcake engine collapses whitespace in the command before policy
# evaluation (heredoc newlines arrive as single spaces), while `opa test`
# sees the raw multiline text. Normalize explicitly so the shape checks
# behave identically in both environments. NOTE: the engine evaluates
# policies as `opa build -t wasm` modules and its host does NOT provide
# `regex.replace`, `regex.find_all_string_submatch_n`, or `sprintf` (calls
# silently evaluate to undefined), so everything below sticks to builtins the
# rest of this policy already proves work in-engine: split, replace, concat,
# lower, contains, count, regex.match, and comprehensions.
proc_scan_norm_command := concat(" ", [word |
	some word in split(replace(replace(replace(command, "\t", " "), "\r", " "), "\n", " "), " ")
	word != ""
])

proc_scan_heredoc_parts := split(proc_scan_norm_command, "<<")

# Heredoc form: the command starts with `python3 - <<'TAG'` (or "TAG") and the
# tag word appears exactly once more, as the final token. The quoted-tag
# requirement keeps the heredoc body fully literal (no $-expansion), and the
# single-terminal-occurrence requirement rejects any trailing shell after the
# terminator (e.g. `PY\nsetsid '/opt/er/start_protected_game.exe'`) as well
# as a second stray terminator line.
proc_scan_heredoc_tag := tag if {
	quote_parts := split(proc_scan_heredoc_parts[1], "'")
	count(quote_parts) >= 3
	regex.match(`^-? ?$`, quote_parts[0])
	tag := quote_parts[1]
	regex.match(`^[A-Za-z_][A-Za-z0-9_]*$`, tag)
}

proc_scan_heredoc_tag := tag if {
	quote_parts := split(proc_scan_heredoc_parts[1], `"`)
	count(quote_parts) >= 3
	regex.match(`^-? ?$`, quote_parts[0])
	tag := quote_parts[1]
	regex.match(`^[A-Za-z_][A-Za-z0-9_]*$`, tag)
}

proc_scan_python_shape if {
	count(proc_scan_heredoc_parts) == 2
	regex.match(`^(cd [^;|&()<>]+ && )?(/usr/bin/)?python3? - ?$`, proc_scan_heredoc_parts[0])
	terminator_parts := split(proc_scan_norm_command, concat("", [" ", proc_scan_heredoc_tag]))
	count(terminator_parts) == 2
	terminator_parts[1] == ""
}

# Runtime preflight may check Steam with shell pgrep before a Python heredoc scans exact
# `/proc/<pid>/comm` names. Keep that composite read-only shape allowed without opening a generic
# chained-command bypass.
proc_scan_python_shape if {
	count(proc_scan_heredoc_parts) == 2
	regex.match(`^(/usr/bin/)?pgrep -x steam >/dev/null && echo steam-running \|\| echo steam-missing;? (/usr/bin/)?python3? - ?$`, proc_scan_heredoc_parts[0])
	terminator_parts := split(proc_scan_norm_command, concat("", [" ", proc_scan_heredoc_tag]))
	count(terminator_parts) == 2
	terminator_parts[1] == ""
}

# Inline form: `python3 -c '<program>'` with nothing outside the quotes; the
# quote-scrubbed command must reduce to exactly the python invocation, so any
# chained `; wrapper '/path/start_protected_game.exe'` breaks the shape.
proc_scan_python_shape if {
	not contains(command, "<<")
	regex.match(`^[[:space:]]*(cd[[:space:]]+[^;|&()<>]+[[:space:]]+&&[[:space:]]+)?(?:/usr/bin/)?python3?[[:space:]]+-c[[:space:]]`, command)
	regex.match(`^[[:space:]]*(cd[[:space:]]+[^;|&()<>]+[[:space:]]+&&[[:space:]]+)?(?:/usr/bin/)?python3?[[:space:]]+-c[[:space:]]*$`, scrubbed_command)
}

# Quote-scrub of the FULL raw command (no heredoc trimming), for asserting
# that the launcher name only ever appears inside quoted string literals.
detection_escapes_stripped := replace(replace(command, `\"`, ""), `\'`, "")

detection_double_parts := split(detection_escapes_stripped, `"`)

detection_outside_double := concat(" ", [detection_double_parts[idx] |
	some idx
	detection_double_parts[idx]
	idx % 2 == 0
])

detection_single_parts := split(detection_outside_double, "'")

detection_unquoted_command := lower(concat(" ", [detection_single_parts[idx] |
	some idx
	detection_single_parts[idx]
	idx % 2 == 0
]))

# Process-execution mechanisms. Without one of these, a pure-python /proc
# reader has no way to launch the named executable; with one, the exemption
# stays off and the raw marker fallback denies as before.
proc_scan_exec_markers := {
	"subprocess", "os.system", "system(", "popen", "spawn",
	"exec(", "execv", "execl", "eval(", "__import__", "importlib",
	"ctypes", "pexpect", "shell=", "startfile", "runpy",
	"multiprocessing", "sh -c", "wine", "proton",
	"steam -applaunch", "steam://", "xdg-open",
}

# Scanned against the RAW lowercased command, so quoting cannot hide them.
proc_scan_exec_marker if {
	some marker in proc_scan_exec_markers
	contains(lower(proc_scan_norm_command), marker)
}

# ---------------------------------------------------------------------------
# Structural "named only as data" exemption for read-only process detection.
#
# False positive fixed 2026-07-28. The denied command was the sanctioned Steam
# preflight followed by the sanctioned /proc process check:
#
#   cd <repo>; bash -c 'source scripts/steam-running.sh && if steam_running; ...'
#   python3 -c "
#   import os,glob
#   ...
#   if comm in ('eldenring.exe','me3','start_protected_game.exe'): hits.append(comm)
#   print('game running:', hits if hits else 'none')
#   "
#
# Nothing is launched: the launcher name is a python string literal in a
# membership test, and AGENTS.md explicitly allows "process detection of stale
# start_protected_game.exe". It was denied because the exemptions above
# whitelist whole-command SHAPES -- `[cd X &&] python3 -c '...'` plus three
# fixed heredoc prefixes -- so ANY additional statement, even a read-only one,
# drops the payload into the raw substring fallback, which denies on the bare
# name plus a generic marker word ("python", "bash", ...). That made the
# sanctioned detection idiom unwritable in a preflight.
#
# This arm asks a structural question instead of matching whole-command shapes:
# WHERE does the name occur? A launch needs the name to reach an exec as a
# SHELL WORD (`./start_protected_game.exe`, `wine /opt/er/...`, `setsid
# 'start_protected_game.exe'` -- shell quoting does not make a word data) or as
# an argv element of an exec call inside a program. So the exemption requires
# every occurrence to be a NESTED string literal: quote-wrapped at one level
# AND inside a shell-quoted region at another (`'name'` inside "..." or "name"
# inside '...'), which is only reachable as an inline interpreter program's own
# literal, never as a shell word. Splitting the command on a quote character
# puts quoted content at ODD indices, so the nested forms can be counted
# exactly and compared against the total occurrence count: if even one
# occurrence sits anywhere else, the counts differ and the fallback denies.
#
# Fail-closed companions, each closing a specific escape:
#   * no `$(` / backtick anywhere -- a command substitution INSIDE the quoted
#     program is expanded by the shell before the interpreter sees it, so
#     `python3 -c "... $( 'start_protected_game.exe' ) ..."` would otherwise
#     execute a nested-looking literal;
#   * the quoted text must mention `/proc` -- this exemption is for the
#     sanctioned process-state reader, not for arbitrary payloads that happen
#     to quote the name;
#   * no exec mechanism inside the QUOTED text (subprocess, os.system, exec*,
#     wine/proton/steam://, ...) -- scoped to the quoted regions so an
#     unrelated read-only `bash -c` statement elsewhere in the preflight no
#     longer poisons the whole payload, while any exec mechanism in the program
#     that actually names the launcher keeps the exemption off; and
#   * the name must not appear in non-command tool fields.
# Heredoc bodies have no enclosing shell quotes, so they never satisfy this arm
# and keep relying on the explicit heredoc shapes above.
# ---------------------------------------------------------------------------

launcher_named_only_as_nested_string_literal if {
	tool_name == "Bash"
	not contains(command, "$(")
	not contains(command, "`")
	not contains(lower(other_text), "start_protected_game.exe")
	launcher_mention_total > 0
	launcher_mention_nested == launcher_mention_total
	contains(mention_quoted_text, "/proc")
	not mention_quoted_exec_marker
}

# Whitespace-normalized, lowercased command with escaped quotes removed, so the
# quote-index parity below cannot be desynced by `\"` / `\'`.
mention_scan_text := lower(replace(replace(proc_scan_norm_command, `\"`, ""), `\'`, ""))

# Content of shell double-quoted regions (odd indices of a split on `"`).
# Joined with a newline so two separate regions cannot form a match across the
# seam between them.
mention_double_quoted_text := concat("\n", [segment |
	some idx, segment in split(mention_scan_text, `"`)
	idx % 2 == 1
])

# Content of shell single-quoted regions (odd indices of a split on `'`).
mention_single_quoted_text := concat("\n", [segment |
	some idx, segment in split(mention_scan_text, "'")
	idx % 2 == 1
])

mention_quoted_text := concat("\n", [mention_double_quoted_text, mention_single_quoted_text])

launcher_mention_total := count(split(mention_scan_text, "start_protected_game.exe")) - 1

# `'name'` inside a double-quoted region, or `"name"` inside a single-quoted
# region. A path-prefixed operand (`'/opt/er/start_protected_game.exe'`) is not
# one of these forms and therefore never counts as a nested literal.
launcher_mention_nested := nested_in_double + nested_in_single if {
	nested_in_double := count(split(mention_double_quoted_text, `'start_protected_game.exe'`)) - 1
	nested_in_single := count(split(mention_single_quoted_text, `"start_protected_game.exe"`)) - 1
}

mention_quoted_exec_marker if {
	some marker in proc_scan_exec_markers
	contains(mention_quoted_text, marker)
}

# Read-only process checks that shell out to exact `pgrep -x` from Python are
# allowed. This covers the repo runtime preflight form that checks Steam, the
# approved direct game process, and stale `start_protected_game.exe` presence
# before an offline/direct probe. It is narrower than the /proc reader above:
# exactly one direct subprocess.run call must be the pgrep call, and any other
# process-execution or launch-shaped marker keeps the exemption off.
pgrep_subprocess_detection_command if {
	tool_name == "Bash"
	pgrep_subprocess_python_shape
	not contains(command, "$(")
	not contains(command, "`")
	not contains(lower(other_text), "start_protected_game.exe")
	not contains(detection_unquoted_command, "start_protected_game.exe")
	count(split(lower(proc_scan_norm_command), "start_protected_game.exe")) == 2
	count(split(lower(proc_scan_norm_command), "subprocess.run")) == 2
	regex.match(`(?i)subprocess\.run[[:space:]]*\([[:space:]]*\[[^\]]*['"]pgrep['"][^\]]*['"]-x['"][^\]]*\]`, proc_scan_norm_command)
	not pgrep_subprocess_forbidden_marker
}

pgrep_subprocess_python_shape if {
	proc_scan_python_shape
}

# Runtime preflight often checks Steam with shell pgrep before a Python heredoc
# checks exact game/EAC process names. Keep that composite read-only shape
# allowed without opening a generic chained-command bypass.
pgrep_subprocess_python_shape if {
	count(proc_scan_heredoc_parts) == 2
	regex.match(`^(/usr/bin/)?pgrep -x steam >/dev/null && echo steam-running \|\| echo steam-missing;? (/usr/bin/)?python3? - ?$`, proc_scan_heredoc_parts[0])
	terminator_parts := split(proc_scan_norm_command, concat("", [" ", proc_scan_heredoc_tag]))
	count(terminator_parts) == 2
	terminator_parts[1] == ""
}

# Runtime validation may sleep briefly after an approved direct/offline launch
# before checking for stale protected-launcher processes. Allow only that
# sleep-then-single-python-heredoc shape; the exact pgrep subprocess and
# no-launch checks above still gate the payload.
pgrep_subprocess_python_shape if {
	count(proc_scan_heredoc_parts) == 2
	regex.match(`^(/usr/bin/)?sleep [0-9]+(\.[0-9]+)?; (/usr/bin/)?python3? - ?$`, proc_scan_heredoc_parts[0])
	terminator_parts := split(proc_scan_norm_command, concat("", [" ", proc_scan_heredoc_tag]))
	count(terminator_parts) == 2
	terminator_parts[1] == ""
}

pgrep_subprocess_forbidden_marker if {
	some marker in {
		"os.system", "system(", "popen", "spawn", "exec(", "execv",
		"execl", "eval(", "__import__", "importlib", "ctypes", "pexpect",
		"shell=", "shell =", "startfile", "runpy", "multiprocessing",
		" sh -c", "bash -c", "wine", "proton", "steam -applaunch",
		"steam://", "xdg-open", "/start_protected_game.exe",
	}
	contains(lower(proc_scan_norm_command), marker)
}

