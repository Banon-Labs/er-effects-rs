#!/usr/bin/env bash
# This suite ACCUMULATES failures. It does not stop at the first one.
#
# It used to run under `set -e`, and the cost of that was measured twice on 2026-08-31: it went red
# at line 46 on `test-input-harness-static.py` -- whose subject, crates/er-input-harness/src/pad_inject.rs,
# was mid-edit by another agent -- and the ~130 gate invocations after it produced no verdict at all.
# Not pass, not fail, nothing. Earlier the same day the abort was at check-crate-extraction-roadmap.py.
#
# That is the same defect every gate in this file exists to refuse: A gate that never executed is
# indistinguishable from a gate that passed. Agents reported "check.sh is red at X" while holding no
# information whatever about the checks behind X, and several then called their own change green on
# the strength of running a handful by hand. `set -e` also silently made a check's position its
# authority: the identical check is load-bearing at line 46 and decorative at line 900, because
# anything ahead of it going red erases it. In a tree six agents edit concurrently an early red is
# the normal case, so most of this suite was unobserved for most of a day.
#
# So: every step runs, every failure is collected, and the summary at the end gives every step one
# of four states -- `passed`, `FAILED`, `INCONCLUSIVE` (killed before it reached a verdict) and, the
# load-bearing one, `NOT RUN`, which must never be silent. Exit is non-zero unless every step ran
# and passed; `NOT RUN` and `INCONCLUSIVE` are not passes.
#
# Adding a gate is still one line. Nothing has to be registered anywhere: put the invocation on its
# own line, starting in column 1 with `python3` / `bash` / `cargo` / `shellcheck` / `rustfmt` /
# `cupcake` / `opa` / `command -v`, exactly as every line below does. The summary discovers the
# step list by reading this file back at exit, so an appended gate is classified automatically and
# a `\`-continued multi-line invocation counts once, at its first line. Comments between steps are
# free. The only way to write a step the summary cannot see is to indent it or to hide it inside a
# function or subshell -- so don't.
#
# Fail-fast is still available, as an explicit, justified exception -- never as the default for all
# ~190 steps. Use it only where a later check consumes this step's output, so continuing would
# produce verdicts that are not about anything. The live example is the `command -v cupcake` guard
# below: `|| { echo ...; exit 127; }`. Write that shape deliberately, with the reason next to it;
# the exit trap then reports everything after it as `NOT RUN` instead of letting it vanish.
set -uo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)

# Yield to whoever is using this computer, before any gate runs. The agent harness runs its
# shells at nice -4, which is inherited by everything below and puts a 190-step suite at higher
# priority than the desktop -- measured 2026-09-06, load average 34 on 16 cores and an
# unresponsive machine. See scripts/lib/cpu-courtesy.sh for why this lives here and not in a
# wrapper the caller has to remember.
# shellcheck source=lib/cpu-courtesy.sh
# shellcheck disable=SC1091  # sourced at run time; shellcheck -x is not how this suite is linted.
. "$repo_root/scripts/lib/cpu-courtesy.sh"
cpu_courtesy check.sh


# --- who may run this, and how many at once -------------------------------------------------
# Both REFUSALS below are measurements, not policy preferences. On 2026-09-02 three subagents
# dispatched to do reverse engineering each ran ~72 minutes. The research was not the cost: the
# Ghidra MCP accounted for 51 calls totalling 59 seconds across one of them. What consumed the
# time was this file, run repeatedly, concurrently, by every one of them.
#
# 1. Agent WORKTREES. Each subagent works in its own .claude/worktrees/agent-<id> checkout with
#    its own target/, so nothing shares a build cache and each run is a cold compile of the 24
#    cargo gates below. Four overlapped on this 16-core box: load average 54.26, and a run that
#    costs minutes on a quiet tree was still unfinished at 19 minutes. Then, because that far
#    outruns the harness Bash timeout, each agent sleep-polled its own run -- 18x `sleep 118`
#    = 2003s for one, 60x `timeout 28 sleep 27` = 1330s for another. A poll costs a whole model
#    turn, not merely its sleep; one agent spent 33 of its 45 tool-minutes asleep.
#
#    A subagent does not need this file. It needs `cargo test -p <its crate>` plus the specific
#    scripts/<gate>.py its edit touches -- seconds, and an answer to the question it actually
#    has. The whole-workspace verdict is the orchestrator's, once, at integration.
#
# 2. CONCURRENCY, whoever the caller is. Two runs of this suite do not merely take twice as
#    long, they degrade each other's answer: the summary classifies steps as `INCONCLUSIVE`
#    (killed before a verdict) and `NOT RUN`, and a contended box manufactures both. A green from
#    a quiet tree is the only green that means anything, so a second concurrent run is refused
#    rather than queued -- queueing would just re-create the polling that caused this.
#
# Both refusals happen before the summary trap installs, so a refusal prints one line rather
# than a 234-row table of steps that never ran. Deliberate override: ER_CHECK_FORCE=1.
if [[ "${ER_CHECK_FORCE:-}" != "1" && "$repo_root" == */.claude/worktrees/agent-* ]]; then
	echo "check.sh: REFUSED -- this is an agent worktree ($repo_root)." >&2
	echo "  Validate your own change instead: cargo test -p <crate>, plus the specific" >&2
	echo "  scripts/<gate>.py your edit touches. Naming the crates is your work; the build" >&2
	echo "  system's answer to an unscoped request is always 'all of it'." >&2
	echo "  Measured 2026-09-02: three subagents ran this file repeatedly and spent 2003s and" >&2
	echo "  1330s merely sleep-polling their own runs. See bd" >&2
	echo "  subagent-full-check-sh-sleep-poll-is-the-hour-long-tax-2026-09-02." >&2
	echo "  Deliberate override: ER_CHECK_FORCE=1 bash scripts/check.sh" >&2
	exit 2
fi

# Machine-wide, not $repo_root-relative: every agent worktree is a separate checkout, so a lock
# under the repo would be per-worktree and would therefore never see the contention it exists to
# prevent. XDG_RUNTIME_DIR is per-user and tmpfs-backed; /tmp is the fallback. If flock is absent
# the gate runs anyway -- a missing tool must not make the suite unrunnable.
_check_lock="${XDG_RUNTIME_DIR:-/tmp}/er-mods-rs-check-sh.lock"
if [[ "${ER_CHECK_FORCE:-}" != "1" && "${ER_CHECK_LOCK_HELD:-}" != "1" ]] && command -v flock >/dev/null 2>&1; then
	exec 9>"$_check_lock" || true
	if ! flock -n 9; then
		_holder=$(cat "$_check_lock" 2>/dev/null || true)
		echo "check.sh: REFUSED -- another run already holds $_check_lock (pid ${_holder:-unknown})." >&2
		echo "  Concurrent runs do not just take longer, they corrupt each other's verdict:" >&2
		echo "  contention produces INCONCLUSIVE and NOT RUN steps, which are not passes." >&2
		echo "  Wait for that run and read ITS result, or override with ER_CHECK_FORCE=1." >&2
		exit 2
	fi
	echo "$$" >&9
	# ...and a step of this suite may re-enter this preamble. test-check-sh-accumulates.py lifts
	# it verbatim and drives it over synthetic suites -- deliberately, because testing a copy
	# would prove nothing about the file that runs. Those children are not a second run competing
	# for the box; they are this run's own steps, and the lock they find held is their parent's.
	# Without this marker every one of them exits 2 here and the gate reports thirteen failures
	# that have nothing to do with accumulation, which is exactly what CI showed on 2026-09-02.
	# Exported, so it reaches a grandchild through python/bash alike.
	export ER_CHECK_LOCK_HELD=1
fi

# --- failure accumulation ------------------------------------------------------------------
# The ERR trap fires on exactly the commands `set -e` would have exited on -- and, verified on
# this bash, not on the left side of `||`/`&&` -- so the `command -v opa >/dev/null && opa test ...`
# guards below keep behaving as they always did when opa is absent.
#
# Every step is classified by its own source line, not by a running count. A count cannot answer
# "which checks have no verdict", which is the only question that matters after an early exit; and
# the count was wrong anyway -- bash fires the DEBUG trap twice for a command that goes on to fire
# the ERR trap (measured on this bash), so every failing step was counted as two steps run. Both
# problems disappear when the DEBUG trap records a set of line numbers and the summary diffs that
# set against the step lines it reads back out of this file.
#
# The step list is snapshot now, at startup, and never re-read. The summary used to grep this file
# back when it finished; that is fine until somebody edits check.sh while a run is in flight, which
# is ordinary in a tree several agents share and happened during this very rewrite. The recorded
# line numbers then point into a file that no longer exists, every row shifts, and the table becomes
# confident nonsense -- the exact failure class this suite exists to refuse, produced by the
# instrument reporting it.
_check_step_pattern='^[[:space:]]*(python3|bash|cargo|shellcheck|rustfmt|cupcake|opa|command -v)[[:space:]]'
mapfile -t _check_step_rows < <(grep -nE "$_check_step_pattern" "${BASH_SOURCE[0]}" 2>/dev/null || true)
_check_failed_lines=()
_check_failed_cmds=()
_check_inconclusive_lines=()
_check_inconclusive_cmds=()
_check_skipped_lines=()
_check_skipped_cmds=()
# The step'S own text, keyed by its line. The ERR trap reads `$BASH_COMMAND`, which since the
# shims below is the INNERMOST command -- `command python3 "$@"` -- so the failure list read
# `command python3 "$@"` three times instead of naming three different gates. The line numbers
# were right and the table was right; only the human-readable list was useless. Each shim records
# what it was actually asked to run here, and _check_note_failure prefers it.
declare -A _check_step_cmd=()

# --- Which steps cannot run on this machine ------------------------------------------------
# A fifth state, and it exists because of one measurement. Run this suite in a tree that has no
# `eldenring-deobf*.bin` -- which is every tree except a developer's, since both images are
# gitignored, game-derived and ~100 MB -- and fourteen gates printed one line saying their input
# was absent and exited 0, having asserted nothing whatever. Several say so in their own output:
# `SKIPPED (NOT A PASS) ... no field offset was checked at all`. The summary above still wrote
# `passed` beside every one of them. That is the same defect as `set -e` erasing 130 steps,
# arriving by a different road: a gate whose input was absent is indistinguishable from a gate
# that agreed with the tree.
#
# So a step whose input is missing is now skipped, out loud, with the missing input named -- and
# skipped is not a pass. `scripts/ci-gate-portability.py` owns the decision. It classifies every
# step against a measured ledger (docs/ci-gate-portability.tsv), keyed by the step's command text
# rather than its line number so the ledger survives edits to this file, and `--check` (wired in
# below) refuses a step that has no ledger row -- which is what stops the classification drifting
# the way .github/workflows/check.yml's hand-written step list did.
#
# If the map cannot be computed, nothing is skipped. Failing open runs every gate and lets the
# missing input produce a red, which is loud; failing closed would skip the whole suite silently,
# which is the disease. The warning below says which happened.
# `command python3` on purpose: the shim further down would look this line up in a skip map
# that this very line is what builds. It also tells shellcheck (SC2218) that reaching the
# real binary before the function exists is deliberate rather than an ordering mistake.
_check_skip_src=$(command python3 "$repo_root/scripts/ci-gate-portability.py" --skip-lines 2>/dev/null)
_check_skip_rc=$?
declare -A _check_skip_reason=()
if [[ $_check_skip_rc -ne 0 ]]; then
	printf '>>> check.sh: ci-gate-portability.py --skip-lines failed (exit %s).\n' "$_check_skip_rc" >&2
	printf '>>> No step will be skipped; a gate with a missing input will go RED, not quiet.\n' >&2
elif [[ -n $_check_skip_src ]]; then
	while IFS=$'\t' read -r _skip_line _skip_reason; do
		[[ -n ${_skip_line:-} ]] && _check_skip_reason[$_skip_line]=$_skip_reason
	done <<<"$_check_skip_src"
	printf '>>> check.sh: %s step(s) will be SKIPPED here for missing inputs (see the summary).\n' \
		"${#_check_skip_reason[@]}" >&2
fi

# --- Which cargo work this diff can actually invalidate ------------------------------------
# A second source of skips, with the same honesty contract and the same machinery. The first
# asks "can this gate run HERE" (a missing game image); this one asks "can this diff have
# broken it". The user directive is blunt: neither CI nor a push should run the whole suite
# when the change does not require it -- a shell-script-only commit was compiling the entire
# workspace for ~14 minutes to prove nothing about itself.
#
# It is derived, never a second list. scripts/er-change-scope.py reads the step list back out
# of this file (through ci-gate-portability's parser, which reads this file's own
# `_check_step_pattern`) and reasons about each step's own `-p` packages against the reverse
# dependency closure of the diff -- the same walk scripts/er-dll-closure.py performs. So a
# cargo step added below is classified the moment it is written, with nothing to remember to
# update. That is the property the pre-push hook's deleted 13-gate list did not have, and one
# violation reached origin because of it (check-fnv1a-owner.py, PR #398).
#
# What it will never SKIP: anything that is not cargo-shaped. Every repo-wide invariant, every
# address-map and prologue gate, the portability ledger, shellcheck, the policy suites and
# `cargo fmt` run on every diff regardless -- they read the whole tree, so a scoped run cannot
# reason about them, and getting that boundary wrong is how a violation escapes.
#
# A skipped step is still a skipped STEP: it lands in the same skipped bucket, prints the same
# "this step did NOT run; that is not a pass" banner, and its reason begins `NOT SELECTED FOR`
# this diff. It is never counted as passed. And selection fails open -- a git failure, an
# unresolvable base, or a diff that touches anything outside a single crate selects everything.
_check_scope_src=$(command python3 "$repo_root/scripts/er-change-scope.py" --check-sh-skips 2>/dev/null)
_check_scope_rc=$?
if [[ $_check_scope_rc -ne 0 ]]; then
	printf '>>> check.sh: er-change-scope.py --check-sh-skips failed (exit %s).\n' "$_check_scope_rc" >&2
	printf '>>> EVERY cargo step will RUN. Selection fails open; it never narrows on a guess.\n' >&2
elif [[ -n $_check_scope_src ]]; then
	_check_scope_n=0
	while IFS=$'\t' read -r _scope_line _scope_reason; do
		[[ -z ${_scope_line:-} ]] && continue
		# A missing input outranks a missing reason to RUN: if the portability ledger already
		# skipped this line, keep its reason. Both are honest; the first is the more fundamental.
		[[ -n ${_check_skip_reason[$_scope_line]:-} ]] && continue
		_check_skip_reason[$_scope_line]=$_scope_reason
		_check_scope_n=$((_check_scope_n + 1))
	done <<<"$_check_scope_src"
	printf '>>> check.sh: %s cargo step(s) NOT SELECTED for this diff (see the summary; NOT passes).\n' \
		"$_check_scope_n" >&2
fi
declare -A _check_ran_at=()
_check_ran=0
_check_reached_end=0

_check_note_failure() {
	local status="$1" line="$2" cmd="${_check_step_cmd[$2]:-$3}"
	# Three outcomes, not two. 124 is GNU `timeout` reporting its deadline; >=128 is death by
	# signal N-128 (137 SIGKILL, 143 SIGTERM -- how an agent harness reclaims a long-running
	# process). A step that was killed produced no verdict: calling it failed would let a check
	# that never finished look sensitive to the tree, and calling it passed is the defect this
	# whole file exists to refuse. So it is `INCONCLUSIVE`, and it is not a pass.
	#
	# This is not hypothetical here, and this whole suite is far past any 30s per-command cap
	# regardless, so run it in the background, never inside a capped foreground shell.
	#
	# Measured 2026-08-31, since which steps those are had drifted. check-no-timeouts.py was 28-31s
	# and is now ~2s: 97% of it was `rglob`-ing 1,117,583 filesystem entries to find the 1112 files
	# `git ls-files` lists in 0.011s. check-oracle-writers.py was ~22s of genuine work over
	# crates/**/*.rs, not a whole-tree scan, and was left alone; on 2026-08-31 it was transposed
	# to one pass per file instead of one pass per (name, file) -- 2,645 names x ~900 files --
	# and now runs in under a second with a byte-identical offender set, off the cap entirely.
	#
	# The one still over the cap is test-cupcake-policies.py. It was 34.5s, of which ~13s was it
	# shelling out to test-cupcake-delivered-shape.py that lines 421/422 below already run; that
	# duplication is gone. What is left does not compress: 176 `cupcake eval` spawns, ~237
	# CPU-seconds, one event per process (there is no batch mode). Wall clock is that floor divided
	# by whatever cores the rest of this box leaves free -- three runs of the identical code came
	# back 20.4s, 23.4s and 35.0s -- so it is over the cap whenever the machine is busy, and that
	# is not something the script can fix. It now says so on its own stdout before it starts work,
	# so an agent killed at 30s has already been told why rather than inferring a hang.
	if [[ $status -eq 124 || $status -ge 128 ]]; then
		_check_inconclusive_lines+=("$line")
		_check_inconclusive_cmds+=("$cmd")
		printf '\n>>> check.sh INCONCLUSIVE at line %s (exit %s, killed): %s\n>>> no verdict from this step; NOT a pass\n\n' \
			"$line" "$status" "$cmd" >&2
		return 0
	fi
	_check_failed_lines+=("$line")
	_check_failed_cmds+=("$cmd")
	printf '\n>>> check.sh FAILED at line %s: %s\n>>> continuing; the summary at the end is the verdict\n\n' \
		"$line" "$cmd" >&2
}
trap '_check_note_failure "$?" "$LINENO" "$BASH_COMMAND"' ERR

# Record which steps executed, so the summary can name the ones that did not.
# A DEBUG trap is not inherited by shell functions unless `set -T` is on, so this only ever sees
# top-level commands; the subshell test keeps command substitutions out, and the case filter keeps
# it to step-shaped lines. (A `${#FUNCNAME[@]} -eq 1` guard was tried and is WRONG: bash reports
# depth 2 for a function invoked from a DEBUG trap here, so it counted zero steps while reporting
# every one of them as `NOT RUN` -- a summary that lied in the safe direction, but still lied.)
# `$LINENO` here is the line the command starts on, verified against a `\`-continued multi-line
# `cargo test`, which is what lets the summary pair a run with the source line it came from.
_check_count_step() {
	if [[ ${BASH_SUBSHELL} -eq 0 ]]; then
		case "$2" in
		python3\ * | bash\ * | cargo\ * | shellcheck\ * | rustfmt\ * | opa\ * | cupcake\ * | command\ *)
			_check_ran_at[$1]=1
			;;
		esac
	fi
	return 0
}
trap '_check_count_step "$LINENO" "$BASH_COMMAND"' DEBUG

# --- The skip itself -----------------------------------------------------------------------
# Every step in this file begins with `python3`, `bash`, `cargo`, `shellcheck`, `rustfmt`,
# `cupcake` or `opa`, in column 1. Shadowing those names with shell functions is what lets a step
# be skipped without touching any of the 226 step lines, without a second list of gate names,
# and without a per-gate opt-in that a new gate would forget. `command <name>` reaches the real
# program; the functions are not exported, so a gate that itself shells out to python3 is
# unaffected.
#
# `${BASH_LINENO[0]}` inside the shim is the line in this file that called it -- the same key the
# DEBUG trap records and the summary reads back -- which is why the skip can un-record a step the
# DEBUG trap has already counted as run. (DEBUG fires before the command, so without the unset a
# skipped step would appear in `steps run`.)
_check_dep_skip() {
	local line=$1
	shift
	local reason=${_check_skip_reason[$line]:-}
	[[ -z $reason ]] && return 1
	unset "_check_ran_at[$line]"
	_check_skipped_lines+=("$line")
	_check_skipped_cmds+=("$*")
	printf '\n>>> check.sh SKIPPED at line %s: %s\n>>> %s\n>>> this step did NOT run; that is not a pass\n\n' \
		"$line" "$reason" "$*" >&2
	return 0
}

# Tool-absent is the same bug, and it was live in this file until 2026-08-31 in the shape
# `command -v opa >/dev/null 2>&1 && opa test ...`. With opa absent the left side is false, no ERR
# trap fires (verified: ERR does not fire on the left of `&&`), the DEBUG trap still records the
# line, and the summary prints `passed` for a policy suite that never ran. Measured on this bash:
# ERR fired 0 times, DEBUG recorded 7 lines. Eleven steps carried that shape. They are now plain
# `opa test ...` invocations, and this is what makes a missing opa visible instead of green.
_check_tool_skip() {
	local tool=$1 line=$2
	# `if` deliberately, not `command -v ... && return 1`: a line starting with `command -v`
	# matches this file's own _check_step_pattern and would be counted as a 225th step that
	# the DEBUG trap can never record (it lives inside a function), i.e. a permanent `NOT RUN`.
	if command -v "$tool" >/dev/null 2>&1; then return 1; fi
	shift 2
	unset "_check_ran_at[$line]"
	_check_skipped_lines+=("$line")
	_check_skipped_cmds+=("$*")
	printf '\n>>> check.sh SKIPPED at line %s: %s is not installed here\n>>> %s\n>>> this step did NOT run; that is not a pass\n\n' \
		"$line" "$tool" "$*" >&2
	return 0
}

# python3 and bash interpret a repo gate, so they are the two that can hit a missing input.
python3() {
	_check_step_cmd[${BASH_LINENO[0]}]="python3 $*"
	_check_dep_skip "${BASH_LINENO[0]}" python3 "$@" && return 0
	command python3 "$@"
}
bash() {
	_check_step_cmd[${BASH_LINENO[0]}]="bash $*"
	_check_dep_skip "${BASH_LINENO[0]}" bash "$@" && return 0
	command bash "$@"
}
# The rest can only hit a missing tool. `cupcake` deliberately has no shim: the fail-fast guard
# further down owns that case, because later steps consume its output and would produce verdicts
# that are not about anything.
# `cargo` is the one tool that can also hit a missing reason to run. `_check_dep_skip` comes
# first because "this diff cannot have broken it" is a cheaper truth than "the tool is absent",
# and both land in the same skipped bucket either way.
cargo() {
	_check_step_cmd[${BASH_LINENO[0]}]="cargo $*"
	_check_dep_skip "${BASH_LINENO[0]}" cargo "$@" && return 0
	_check_tool_skip cargo "${BASH_LINENO[0]}" cargo "$@" && return 0
	command cargo "$@"
}
opa() {
	_check_step_cmd[${BASH_LINENO[0]}]="opa $*"
	_check_tool_skip opa "${BASH_LINENO[0]}" opa "$@" && return 0
	command opa "$@"
}
rustfmt() {
	_check_step_cmd[${BASH_LINENO[0]}]="rustfmt $*"
	_check_tool_skip rustfmt "${BASH_LINENO[0]}" rustfmt "$@" && return 0
	command rustfmt "$@"
}
shellcheck() {
	_check_step_cmd[${BASH_LINENO[0]}]="shellcheck $*"
	_check_tool_skip shellcheck "${BASH_LINENO[0]}" shellcheck "$@" && return 0
	command shellcheck "$@"
}

_check_summary() {
	local rc=$?
	trap - ERR DEBUG EXIT
	local failed=${#_check_failed_lines[@]}
	local inconclusive=${#_check_inconclusive_lines[@]}
	local skipped=${#_check_skipped_lines[@]}
	local total=0 not_run=0 passed=0 i line text state
	declare -A failed_at=() inconclusive_at=() skipped_at=()
	for ((i = 0; i < failed; i++)); do failed_at[${_check_failed_lines[i]}]=1; done
	for ((i = 0; i < inconclusive; i++)); do inconclusive_at[${_check_inconclusive_lines[i]}]=1; done
	for ((i = 0; i < skipped; i++)); do skipped_at[${_check_skipped_lines[i]}]=1; done
	_check_ran=${#_check_ran_at[@]}

	# The per-gate table. Read the step lines back out of this file and give every one of them a
	# state. A gate that never ran is the one thing a summary must never omit -- omission is what
	# made "check.sh is green" and "check.sh got as far as line 46" indistinguishable.
	local table="" row
	for row in ${_check_step_rows[@]+"${_check_step_rows[@]}"}; do
		line=${row%%:*}
		text=${row#*:}
		[[ -z $line ]] && continue
		total=$((total + 1))
		if [[ -n ${failed_at[$line]:-} ]]; then
			state="FAILED      "
		elif [[ -n ${inconclusive_at[$line]:-} ]]; then
			state="INCONCLUSIVE"
		elif [[ -n ${skipped_at[$line]:-} ]]; then
			state="SKIPPED     "
		elif [[ -n ${_check_ran_at[$line]:-} ]]; then
			state="passed      "
			passed=$((passed + 1))
		else
			state="NOT RUN     "
			not_run=$((not_run + 1))
		fi
		table+=$(printf '  %s  line %-5s %.96s' "$state" "$line" "${text# }")
		table+=$'\n'
	done

	echo
	echo "======================================================================"
	echo "== check.sh summary                                                 =="
	echo "======================================================================"
	printf 'steps run     : %s of %s\n' "$_check_ran" "$total"
	printf 'passed        : %s\n' "$passed"
	printf 'FAILED        : %s\n' "$failed"
	printf 'INCONCLUSIVE  : %s\n' "$inconclusive"
	printf 'SKIPPED       : %s   (input absent here, or NOT SELECTED for this diff -- NOT passes)\n' "$skipped"
	printf 'NOT RUN       : %s\n' "$not_run"

	if [[ $failed -gt 0 ]]; then
		echo
		echo "failing steps:"
		for ((i = 0; i < failed; i++)); do
			printf '  line %-5s %s\n' "${_check_failed_lines[i]}" "${_check_failed_cmds[i]}"
		done
	fi
	if [[ $inconclusive -gt 0 ]]; then
		echo
		echo "INCONCLUSIVE steps (killed before reaching a verdict -- NOT passes):"
		for ((i = 0; i < inconclusive; i++)); do
			printf '  line %-5s %s\n' "${_check_inconclusive_lines[i]}" "${_check_inconclusive_cmds[i]}"
		done
	fi

	if [[ $skipped -gt 0 ]]; then
		echo
		echo "SKIPPED steps (input absent, or not selected for this diff -- NOT passes, NOT run):"
		for ((i = 0; i < skipped; i++)); do
			printf '  line %-5s %s\n' "${_check_skipped_lines[i]}" "${_check_skipped_cmds[i]}"
			printf '        %s\n' "${_check_skip_reason[${_check_skipped_lines[i]}]:-tool not installed}"
		done
	fi

	echo
	echo "per-step state (passed / FAILED / INCONCLUSIVE / SKIPPED / NOT RUN):"
	printf '%s' "$table"

	# Did this suite damage the checkout it was checking? Snapshotted by the preflight below the
	# preamble marker. Counted as a failure before the rc decision, because a suite that disarms
	# the hooks or bares the repo has done more harm than any finding it could have reported.
	#
	# `declare -F` because scripts/test-check-sh-accumulates.py lifts this function out of this
	# file and runs it over synthetic suites that never source the guard -- an unconditional call
	# there fails every one of its cases on a missing function. It cannot silently skip in a real
	# run: the preflight asserts the function exists immediately after sourcing it, and refuses.
	if declare -F gate_config_report >/dev/null; then
		gate_config_report || failed=$((failed + 1))
	fi

	if [[ $_check_reached_end -ne 1 ]]; then
		echo
		echo "!! THE SUITE DID NOT REACH THE END. The $not_run step(s) marked NOT RUN above have"
		echo "!! NO VERDICT -- do not read that as a pass for any of them."
		echo "!! Something exited early: an explicit fail-fast guard (see the header), a missing"
		echo "!! required command, or an interrupt."
		[[ $rc -eq 0 ]] && rc=1
	elif [[ $failed -gt 0 || $inconclusive -gt 0 || $not_run -gt 0 ]]; then
		rc=1
	elif [[ $skipped -gt 0 ]]; then
		# A green with holes in it. Exit 0, because a runner that cannot hold a 100 MB gitignored
		# game image must not be permanently red for not holding it -- but never the words "all
		# steps ran", which would be false, and never without naming what did not run.
		rc=0
		echo
		echo "every step that COULD run here ran and passed."
		echo "$skipped step(s) above were SKIPPED -- their input is absent on this machine, or"
		echo "this diff cannot have invalidated them. Each one names which."
		echo "They have NO verdict. Do not read this run as covering them."
	else
		rc=0
		echo
		echo "all steps ran; none failed"
	fi

	# A green here is not the strongest statement available, and a reader who does not know that
	# will over-read it. This file proves each gate runs and agrees with the tree; it does not
	# prove any gate would notice the defect it exists to catch. That question is answered by
	# planting the real defect in the real tree and requiring the real gate to name it:
	echo
	echo "the stronger check this file cannot run: scripts/prove-gate-positive-controls.py"
	echo "  (66 positive controls over 31 gates; --list / --only <gate> / --fast). NOT wired in"
	echo "  here on purpose -- it mutates tracked files while it runs, which is unsafe in a tree"
	echo "  other agents are editing. Run it by hand on a quiet tree."
	exit "$rc"
}
trap _check_summary EXIT
# -------------------------------------------------------------------------------------------

# --- preflight, inherited from scripts/ci-local-check.sh, deleted 2026-09-03 -----------------
#
# Below the accumulation preamble on purpose: scripts/test-check-sh-accumulates.py lifts
# everything above the marker line and drives it over synthetic suites in a temp directory,
# where neither vendor/minhook nor the guard file exists. Preflight that refuses in that
# fixture would fail all sixteen of its cases for reasons unrelated to accumulation.
# That file was the pre-push hook's own 13-gate list while this file held 153, so a gate added
# here became push-invisible the moment it was written and CI was the first place it ever ran.
# One violation reached origin that way (check-fnv1a-owner.py, PR #398). The hook now runs this
# file, which makes the two sets the same set by construction rather than by anyone maintaining a
# list. Two things it carried had to survive the deletion, and they are both preambles, not gates.

# 1. MINHOOK, because a `git worktree` checkout starts without it. `vendor/` is gitignored, so an
#    agent sandbox under `.claude/worktrees/` or a `.worktrees/` lab has no MinHook while the main
#    checkout it was made from already does -- and this file is now what a push runs, so the first
#    thing such a worktree would otherwise learn is that its push is refused. The shared git dir
#    names that main checkout exactly, so link its vendor tree in rather than re-cloning ~1MB of C.
#    Fail closed and unchanged when the link cannot be made: a missing MinHook breaks the
#    cross-compile gates below, and a skipped cross-compile is not a pass.
if [[ ! -f "$repo_root/vendor/minhook/src/buffer.c" ]]; then
	_check_main_checkout=$(git -C "$repo_root" rev-parse --path-format=absolute --git-common-dir 2>/dev/null || true)
	_check_main_checkout=${_check_main_checkout%/.git}
	if [[ -n "$_check_main_checkout" ]] && [[ "$_check_main_checkout" != "$repo_root" ]] &&
		[[ -f "$_check_main_checkout/vendor/minhook/src/buffer.c" ]]; then
		mkdir -p "$repo_root/vendor"
		ln -sfn "$_check_main_checkout/vendor/minhook" "$repo_root/vendor/minhook"
		echo "[check.sh] git worktree: linked vendor/minhook -> $_check_main_checkout/vendor/minhook" >&2
	fi
fi
if [[ ! -f "$repo_root/vendor/minhook/src/buffer.c" ]]; then
	cat >&2 <<'EOF'
check.sh: REFUSED -- missing vendor/minhook/src/buffer.c

CI checks it out with:
  git clone --depth 1 --branch v1.3.4 https://github.com/TsudaKageyu/minhook.git vendor/minhook

A git worktree normally self-heals here by linking the main checkout's vendor directory; reaching
this message from one means that checkout has no MinHook either, so clone it there first.
EOF
	exit 2
fi

# 2. The gate must not damage the thing it gates. The logic lives in its own file because
#    scripts/test-check-config-guard.sh drives it against fixture repositories and must drive the
#    real text rather than a copy that can drift; the reasoning and the 2026-08-31 measurement are
#    in that file's header. The verdict is applied by _check_summary, below.
# shellcheck disable=SC1091  # sourced at run time; shellcheck -x is not how this suite is linted.
source "$repo_root/scripts/check-gate-config-guard.sh"
# The summary calls this through `declare -F` so the accumulation fixture can lift it; that is a
# tolerance for the fixture, not for a real run, so refuse here if the source did not define it.
if ! declare -F gate_config_report >/dev/null || ! declare -F gate_config_snapshot >/dev/null; then
	echo "check.sh: REFUSED -- scripts/check-gate-config-guard.sh defined no gate_config_* functions." >&2
	exit 2
fi
gate_config_snapshot "$repo_root"

bash "$repo_root/scripts/check-no-local-main-commits.sh"
# The meter on the meters. Almost every gate below is prefaced by its own `--selftest`, and the
# whole value of that convention rests on one tool that was itself never run by anything:
# audit-selftest-vacuity.py re-runs each of those selftests with the gate's matcher lobotomised
# (`--mode regex`) or with every file it reads coming back empty (`--mode reads`) and reports which
# selftests do not notice. Its own selftest is sub-second and side-effect-free, so it belongs here;
# the two sweeps take minutes and stay manual. The direct question -- plant the real defect in the
# real tree and require the gate to name it -- is scripts/prove-gate-positive-controls.py, which is
# deliberately not wired because it mutates tracked files while it runs.
python3 "$repo_root/scripts/audit-selftest-vacuity.py" --selftest
# ...and the suite's own execution semantics, which are load-bearing for every verdict below it.
# This file collected no verdict for ~130 of its own steps twice on 2026-08-31 because `set -e`
# aborted it at line 46 on a gate whose subject was mid-edit by another agent. A gate that never
# executed is indistinguishable from a gate that passed, so "does check.sh actually run all of
# check.sh" is itself a gate. It lifts the preamble out of this file (never a copy) and drives it
# over synthetic suites, and its own non-vacuity control deletes the ERR trap and requires the
# cases to go red.
python3 "$repo_root/scripts/test-check-sh-accumulates.py"
# ...and the other half of "did this suite actually check anything": which of its steps could not
# run on the machine it ran on. `.github/workflows/check.yml` ran 9 gates while this file ran 224,
# and nothing said so, because the workflow's step list is hand-written and drifted. The CI set is
# now this file, and the classification of which steps a runner can execute lives in a measured
# ledger. --check refuses a step with no ledger row, so adding a gate forces the question "does
# this run without the game image?" to be answered rather than skipped. --selftest first, because
# a classification gate that cannot catch its own drift is decoration.
python3 "$repo_root/scripts/ci-gate-portability.py" --selftest
python3 "$repo_root/scripts/ci-gate-portability.py" --check
python3 "$repo_root/scripts/check-no-timeouts.py"
# The grammar three things SHARE: scripts/hooks/commit-msg, and both jobs of
# .github/workflows/conventional-commits.yml. Only the selftest runs here -- there is no
# repo-wide "check" mode, because the thing being judged is one line that does not exist yet at
# check time (a PR title, or a commit being written). What this catches is the grammar itself
# rotting: drop `feat` from types and the release path silently stops seeing minor bumps.
python3 "$repo_root/scripts/conventional-commit-subject.py" --selftest
python3 "$repo_root/scripts/check-no-committed-build-artifacts.py" --selftest
python3 "$repo_root/scripts/check-no-committed-build-artifacts.py"
python3 "$repo_root/scripts/test-no-timeouts.py"
bash "$repo_root/scripts/test-git-pre-push-block-main.sh"
# ...and that the hook cannot let any of the ~250 gates below edit the live repository. git
# exports GIT_DIR to hooks in a linked WORKTREE, so a fixture write leaks onto the shared
# checkout; see the header of scripts/test-pre-push-scrubs-git-env.sh.
bash "$repo_root/scripts/test-pre-push-scrubs-git-env.sh"
# The build gates must yield to the person at the keyboard, and three of the four levers that
# make that true are invisible from the process that sets them. See the header of
# scripts/test-cpu-courtesy.sh.
bash "$repo_root/scripts/test-cpu-courtesy.sh"
# Telemetry honesty: no counter may be read to emit an oracle while written nowhere. Selftest first,
# so the gate is never trusted on its own say-so (er-effects-rs-56fx).
python3 "$repo_root/scripts/check-oracle-writers.py" --selftest
python3 "$repo_root/scripts/check-oracle-writers.py"
# ...and the SUPERSET the gate above is blind to by construction. It fires only on
# `writes == 0 and reads > 0`; its own selftest pins the exclusion ("unread counters are out of
# scope"), so a counter that is declared, written nowhere and never `.load()`ed was invisible to it.
# That is where most of the debt was: 85 such counters on 2026-08-31, against the 4 the sibling
# knew about. An unread one is not harmless -- it gets `.load()`ed later by someone who assumes a
# declared counter is a live counter, and the permanent 0 then reads as "the feature ran and did
# nothing". Selftest first, and it carries a frozen negative: a counter written only through an
# identifier a macro constructs makes the census refuse rather than call the counter dead.
python3 "$repo_root/scripts/check-counter-writers.py" --selftest
python3 "$repo_root/scripts/check-counter-writers.py"
# The counter gate above asks whether an oracle has a writer. This one asks where its reads point.
# `standalone_tick` reached four game singletons through `safe_read_usize(base + rva)` with the RVA
# hidden behind a closure parameter, so no name-keyed gate could see it -- and on 1.17 all four
# fields went constant for a whole 4,350-record run rather than going quiet. Selftest first; its
# blinds revert the ledger rows to their 1.16.2 values and confirm the gate turns red.
# RE-armed 2026-08-31. The files it is derived against have landed (da529e95 / 04b16f3a), its
# ratchet docs/recon/ungated-module-base-arithmetic.txt regenerated to zero rows, and both halves
# are green -- so the reason for unwiring it is gone. It shares its vocabulary with
# check-stale-rva-calls.py through scripts/module_base_arith.py: that gate owns `base + NAMED_CONST`
# in any context, this one owns every other right-hand side plus `base.wrapping_add(CONST)`.
python3 "$repo_root/scripts/check-oracle-singleton-globals.py" --selftest
python3 "$repo_root/scripts/check-oracle-singleton-globals.py"
# Seamless Co-op is third-party and the user updates it on their own schedule, so the swap that
# invalidates our pins arrives with no commit, no PR and nothing red. On 2026-09-02 v2.0.0
# replaced v1.9.9 and moved every ersc RVA this repo holds, the session ABI behind them, and the
# packer's section name (`.themida` -> `ERSC`). This is the reader that can see which build is
# actually installed; the gate here is its selftest, which proves the banner parser on synthetic
# bytes (so it is not vacuous in CI, where no game exists) and additionally validates the real
# image when there is one. Only `--selftest` runs: the bare form is a report, and it exits
# non-zero when no game is installed, which is not a repo defect.
python3 "$repo_root/scripts/ersc_identify.py" --selftest
# The workspace uses `../fromsoftware-rs` path dependencies, and CI clones that sibling at one
# pinned revision while a developer's is whatever they have checked out -- often a fork carrying
# types upstream does not have. Everything below compiles against the developer's copy, so it
# cannot see the divergence: PRs #322/#323 were green here and failed CI outright on
# `unresolved import eldenring::cs::MsgRepositoryImp`. This is the cheap text check for that.
python3 "$repo_root/scripts/check-fromsoftware-symbols.py" --selftest
python3 "$repo_root/scripts/check-fromsoftware-symbols.py"

# ...AND that every workflow agrees on *both halves* of that pin. The gate above reads
# FROMSOFTWARE_RS_REV out of check.yml alone. Three workflows clone the sibling, and two of
# them carried the comment "Keep in lockstep with check.yml" while copying only the REV --
# not the FROMSOFTWARE_RS_REMOTE naming the remote that rev exists on. The pin lives only on
# the fork, so those two cloned upstream and died at `git checkout` with `fatal: unable to
# read tree`, at step 2 of ~15, with every Rust check below reporting nothing -- not pass,
# not fail. That is what took `release` red on main at b9109a30 while `check` stayed green.
# Positive control against the break itself: run this gate's audit over the three workflow
# files as of b9109a30 and it returns 4 problems naming both files and both rules.
python3 "$repo_root/scripts/check-sibling-pin-lockstep.py" --selftest
python3 "$repo_root/scripts/check-sibling-pin-lockstep.py"
python3 "$repo_root/scripts/check-launch-guardrails.py" --audit
# Does a launch destroy the previous run'S evidence? (wired 2026-08-31.) A game-directory artifact
# is single-SLOT: `er_game_base::log::begin_fresh_run` renames `<name>` to `<name>.prev` and
# truncates on the first write of each process, so two launches and run N-2 is gone -- and several
# sessions launch concurrently here, which makes that the normal case rather than a race. Measured
# 2026-08-31: an 11:09 launch destroyed the 09:07 run's 5.4 MB continue trace before anyone read
# it, and three artifacts keep zero generations because they are `fs::write`, not a rotating log.
# It ENUMERATES rather than pattern-matches -- it reads the knob table out of the Rust sources (the
# `env::var` call plus its resolver's own fallback, following `const` identifiers) and finds
# launchers by their actual launch command. That distinction is the point: pattern-matching is how
# this class hid.
#
# The live run passes while printing 98 known gaps across 13 launchers, held in
# `scripts/er-artifact-redirect-audit.baseline.txt`. Read that count as a to-do list, not an
# approval: the baseline records what is already broken, and only new drift fails. The count is
# printed on passing runs precisely so it cannot rot unseen. 1.2s each.
# Positive control: deleting one knob from `capture-er-frame.sh` (a launcher that currently
# redirects everything, so the gap is genuinely new) goes red naming the launcher, the knob and the
# consequence -- the baseline does not absorb it. Moving the same export line stays green.
#
# TRANSIENTLY red as of 2026-08-31, and deliberately left that way. Six knobs --
# ER_QUICKLOAD_{CPU_PROFILE,DIAG_HARNESS,INPUT_HARNESS_LOG,INPUT_HARNESS_PHASES,RELOAD_TRACE,
# TIMESERIES}_PATH -- appeared in the working tree while this gate was being wired. Measured with
# `git grep <knob> HEAD`: all six are absent at head, so they are another agent's uncommitted
# in-flight work, and that agent has already converted 6 of the 19 launchers. The remaining 13
# have not caught up, which is what the gate is reporting, correctly and for the first time.
#
# The fix is that agent finishing, not `--write-baseline`. Regenerating now would freeze somebody
# else's half-done change as ~78 rows of accepted debt and call it green -- which is the precise
# failure this whole sweep was called to undo. Leave it red until the launchers land; the suite
# accumulates failures rather than aborting, so a truthful red here costs a line in the summary
# and nothing else.
# RE-armed 2026-08-31, both lines, on the condition the note below asked for: the audit script, its
# baseline and the launcher changes are now all committed -- the launchers landed in 236b00ce and
# the script and baseline land with this re-arm. Measured against a detached worktree pinned to the
# committed tree, not against a working tree holding the producers: `--selftest` pass with all its
# reader cases present, and the live run reports 29 launchers / 19 knobs / 1 stated single-slot and
# no new gaps -- against the 185 gaps and 4 lost selftest cases the previous note recorded.
python3 "$repo_root/scripts/er-artifact-redirect-audit.py" --selftest
python3 "$repo_root/scripts/er-artifact-redirect-audit.py"
python3 "$repo_root/scripts/check-runtime-probe-contract.py" --audit
python3 "$repo_root/scripts/test-runtime-probe-contract.py"
python3 "$repo_root/scripts/test-er-readiness-watch.py"
python3 "$repo_root/scripts/test-save-slot-oracle.py"
python3 "$repo_root/scripts/test-detect-proc.py"
python3 "$repo_root/scripts/test-semaphore-watchdog.py"
python3 "$repo_root/scripts/test-input-harness-static.py"
python3 "$repo_root/scripts/test-wall-of-text-classifier.py"
# The SessionStart/PreCompact prime hook must stay small enough that the harness INLINES it.
# At 2452 memories it emitted 157.4 KB, which Claude Code persisted to a file and replaced
# with a 2 KB preview -- so the priming content never reached the agent while still costing
# a large slice of every session, PreCompact included. Size is the whole feature, so it is a
# gate: this drives the real generator against a synthetic 6000-memory store.
python3 "$repo_root/scripts/test-beads-prime-size.py"
python3 "$repo_root/scripts/check-retired-button-labels.py"
python3 "$repo_root/scripts/check-autoload-happy-path.py"
python3 "$repo_root/scripts/test-autoload-happy-path.py"
# An unresolvable staged save is terminal for the process. Pin the caller-side state transitions and
# the nonzero recurrence semaphore so the 120,959-call identical-rejection loop cannot return.
python3 "$repo_root/scripts/check-own-load-save-rejection-guard.py" --selftest
python3 "$repo_root/scripts/check-own-load-save-rejection-guard.py"
# Two gates that existed and ran nowhere (wired 2026-08-31), 0.08s for the pair.
#
# The first is the test of another gate. `check-ifpe-finalization-proof.py` scores a live run's
# artifacts, so it cannot run here; its test is fully synthetic tempdirs, asserts the four failure
# strings in a red case and an empty verdict in a green one, and needs no game and no corpus.
# Nothing ran it, so the scorer's own logic was unguarded. Control: making its `failures.append`
# calls no-ops turns this red; rewording a failure message leaves it green.
#
# The second holds the line that a blind census cannot pass. `check-save-suppression.py` returns a
# single verdict by crossing an in-process census against an offline byte witness, and the branch
# that matters is the one where the census reports zero writes while the bytes on disk moved -- it
# must come back fail, not pass. Its ~20 red/green fixtures cover every verdict branch; only
# `--selftest` runs here, because the scoring path needs a real run's telemetry and witness
# snapshots. Control: forcing `evaluate()` to always return passed=True turns this red and names
# `blind census` and `no offline witness` by branch; a comment-only edit stays green.
python3 "$repo_root/scripts/test-ifpe-finalization-proof.py"
python3 "$repo_root/scripts/check-save-suppression.py" --selftest
python3 "$repo_root/scripts/check-yk0j-runtime-proof.py" --selftest
python3 "$repo_root/scripts/check-user-release-package.py"
python3 "$repo_root/scripts/check-native-continue-static.py"
python3 "$repo_root/scripts/check-menu-constructor-static.py"
# RVA 0 is the PE header: the 1.16.2 -> 1.17 resolver refuses it every time, forever, at the call
# site's own rate. One `game_rva(0)` used only to fetch the module base sat on the 4 Hz telemetry
# write and logged 339,764 anonymous refusals in a single session. Selftest first.
python3 "$repo_root/scripts/check-no-rva-zero.py" --selftest
python3 "$repo_root/scripts/check-no-rva-zero.py"
# Selftest first (mirrors check-no-rva-zero above). Both gates read Rust and Rego through
# `code_only`; their --selftest carries the frozen controls plus the non-vacuity proof that
# blinding the matcher turns them red. Without it a blinded matcher reports a clean tree.
# UNWIRED 2026-08-31: the committed check-env-gate-comments.py has no --selftest -- the flag lives
# in that script's uncommitted edit, so at this commit the step exits 2 with `unrecognized arguments`
# -- a step that produces no verdict about anything, which is the one thing this file refuses.
# Re-arm it in the commit that lands scripts/check-env-gate-comments.py.
# python3 "$repo_root/scripts/check-env-gate-comments.py" --selftest
python3 "$repo_root/scripts/check-env-gate-comments.py"
python3 "$repo_root/scripts/test-env-gate-comments.py"
# UNWIRED 2026-08-31: same shape as the --selftest two rows up. The committed check-marker-file-gates.py
# has no --selftest either. Re-arm it in the commit that lands scripts/check-marker-file-gates.py.
# python3 "$repo_root/scripts/check-marker-file-gates.py" --selftest
python3 "$repo_root/scripts/check-marker-file-gates.py"
python3 "$repo_root/scripts/test-marker-file-gates.py"
python3 "$repo_root/scripts/check-reload-trace-policy.py" --audit
python3 "$repo_root/scripts/check-windows-proof-render.py"
python3 "$repo_root/scripts/test-windows-proof-render.py"
python3 "$repo_root/scripts/test-windows-proof-render-smoke-verdict.py"
command -v cupcake >/dev/null 2>&1 || {
	echo "missing required command: cupcake" >&2
	exit 127
}
cupcake validate --log-level error
python3 "$repo_root/scripts/test-cupcake-policies.py"
# Every cupcake guard in this REPO was partly or wholly inert until 2026-08-22, and the suite was
# green the whole time. `cupcake eval` does not run policies in the OPA interpreter -- it compiles
# them to WASM and executes them in its own runtime, where a builtin the runtime has no host
# implementation for (`sprintf`, `regex.find_n`) returns undefined instead of raising. The rule body
# fails, the decision set comes back empty, and cupcake reports a clean allow with exit code 0. The
# old coverage could not see it: the signal tests ran the shell scripts alone, the .rego tests ran
# the interpreter, and the only real-binary test used PreToolUse events -- so all five Stop guards
# (36 days), the launch guard's non-`command` payload scan (63 days) and the tmp-script guard's Bash
# branch were dead in production while passing every check.
#
# First gate: every builtin the policies call must be proven to execute in the live WASM runtime, and
# a builtin with no probe recipe is a hard failure -- so a new policy reaching for an unverified
# builtin breaks the build instead of quietly not firing. Selftest first, so the gate is never
# trusted on its own say-so.
python3 "$repo_root/scripts/check-cupcake-wasm-builtins.py" --selftest
python3 "$repo_root/scripts/check-cupcake-wasm-builtins.py"
# Second gate: drive real transcripts through the real hook commands out of .claude/settings.json
# and assert the halt actually comes back -- plus a clean turn that must still be allowed, because a
# guard that halts everything wedges every session just as badly as one that halts nothing. It also
# drives UserPromptSubmit, where wall_of_text now lives: that rule must not halt (a Stop verdict is
# printed to the user, and it fires after the answer is already on screen, so halting buys a third
# reading instead of saving one) and its correction must come back on the invisible
# additionalContext channel.
# Third gate: a permission mode cupcake does not recognise must not silently disable every guard.
# cupcake 0.5.2 exits 1 on `permission_mode: "auto"`, which Claude Code now sends -- so on
# 2026-08-24 every hook in this repo failed and every policy went inert for a whole session, with
# this suite green throughout. scripts/cupcake-hook.sh normalises the mode and pins the log level;
# this proves a denial still denies through it.
python3 "$repo_root/scripts/test-cupcake-hook-shim.py"
python3 "$repo_root/scripts/test-cupcake-stop-guards.py"
python3 "$repo_root/scripts/test-authority-agreement-signal.py"
python3 "$repo_root/scripts/test-idle-hold-signal.py"
python3 "$repo_root/scripts/test-unexecuted-promise-signal.py"
python3 "$repo_root/scripts/test-described-next-step-signal.py"
python3 "$repo_root/scripts/test-native-ownership-vocab-signal.py"
python3 "$repo_root/scripts/test-stall-on-friction-signal.py"
python3 "$repo_root/scripts/test-wall-of-text-signal.py"
opa test "$repo_root/.cupcake/system/commands.rego" "$repo_root/.cupcake/policies/claude/no_authority_agreement.rego" "$repo_root/.cupcake/policies/claude/no_authority_agreement_reminder.rego" "$repo_root/.cupcake/tests/no_authority_agreement_test.rego" "$repo_root/.cupcake/tests/no_authority_agreement_reminder_test.rego" "$repo_root/.cupcake/policies/claude/idle_hold.rego" "$repo_root/.cupcake/policies/claude/idle_hold_reminder.rego" "$repo_root/.cupcake/tests/idle_hold_test.rego" "$repo_root/.cupcake/tests/idle_hold_reminder_test.rego" "$repo_root/.cupcake/policies/claude/native_ownership_vocab_reminder.rego" "$repo_root/.cupcake/tests/native_ownership_vocab_reminder_test.rego" "$repo_root/.cupcake/policies/claude/block_manual_pgrep.rego" "$repo_root/.cupcake/tests/block_manual_pgrep_test.rego" "$repo_root/.cupcake/policies/claude/bash_elden_ring_launch_guard.rego" "$repo_root/.cupcake/tests/bash_elden_ring_launch_guard_test.rego" "$repo_root/.cupcake/policies/claude/block_askuserquestion.rego" "$repo_root/.cupcake/tests/block_askuserquestion_test.rego" "$repo_root/.cupcake/policies/claude/block_askuserquestion_reminder.rego" "$repo_root/.cupcake/tests/block_askuserquestion_reminder_test.rego" "$repo_root/.cupcake/policies/claude/no_stall_on_friction.rego" "$repo_root/.cupcake/tests/no_stall_on_friction_test.rego" "$repo_root/.cupcake/policies/claude/no_unexecuted_promise.rego" "$repo_root/.cupcake/tests/no_unexecuted_promise_test.rego" "$repo_root/.cupcake/policies/claude/wall_of_text.rego" "$repo_root/.cupcake/tests/wall_of_text_test.rego"
opa test "$repo_root/.cupcake/system/commands.rego" "$repo_root/.cupcake/policies/claude/git_block_main_push.rego" "$repo_root/.cupcake/tests/git_block_main_push_test.rego"
opa test "$repo_root/.cupcake/system/commands.rego" "$repo_root/.cupcake/policies/claude/git_block_main_commit.rego" "$repo_root/.cupcake/tests/git_block_main_commit_test.rego"
# The shared executed-text decomposition every git guard now reads (bd
# er-effects-rs-dt2e). It is the one place that decides what counts as executed
# rather than quoted, so a regression here silently re-opens four guards at once.
opa test "$repo_root/.cupcake/system/commands.rego" "$repo_root/.cupcake/tests/commands_test.rego"
# Four suites, 89 assertions, written and committed and never executed once (wired 2026-08-31).
# Among them is every test of the two rules standing between an agent and a root delete. They had
# no runner at all: not here, not in CI, nowhere. 0.16s for all four -- the cost was never why.
#
# They were briefly reachable only through `scripts/test-cupcake-policies.py`'s internals. That
# works, but a suite reached through another script's guts is one refactor from going quiet again,
# which is precisely how it got here, so they get their own lines.
#
# What the fish/csh/tcsh line is about: `(ba|z|k|da|a|fi|c|tc)?sh` in commands.rego is the shell
# alternation the command decomposition recognises, and `fi|c|tc` were missing until 2026-08-31 --
# so `fish -c '<guarded command>'` decomposed to nothing and walked through every executed-text
# guard, while AGENTS.md actively instructs agents to wrap commands for fish. A documented
# workflow straight through the enforcement. 18 commands flipped allow->deny closing it.
# Positive control (run against copies; .cupcake is not edited): dropping `fi|c|tc` back out of
# that alternation takes protected_paths_test from pass 73/73 to fail 2/73 and commands_test from
# 58/58 to fail 3/58. The bypass is genuinely gated, by these suites, now that they run.
opa test "$repo_root/.cupcake/system/commands.rego" "$repo_root/.cupcake/policies/claude/builtins/protected_paths.rego" "$repo_root/.cupcake/tests/protected_paths_test.rego"
opa test "$repo_root/.cupcake/system/commands.rego" "$repo_root/.cupcake/policies/claude/edit_no_tmp_scripts_guard.rego" "$repo_root/.cupcake/tests/edit_no_tmp_scripts_guard_test.rego"
# The write-time half of the comment-caps rule. scripts/check-comment-caps.py is the authority and
# runs above; this guard refuses the edit while the author still has the sentence in hand. Its
# negative cases are the load-bearing ones -- it denies a write, so a false positive costs an edit
# nobody can make -- and the live half runs in test-cupcake-policies.py, because an opa-green
# policy that cupcake's WASM runtime cannot execute returns nothing and reads as a clean turn.
opa test "$repo_root/.cupcake/system/commands.rego" "$repo_root/.cupcake/policies/claude/edit_no_comment_caps_guard.rego" "$repo_root/.cupcake/tests/edit_no_comment_caps_guard_test.rego"
opa test "$repo_root/.cupcake/system/commands.rego" "$repo_root/.cupcake/policies/claude/no_unbacked_claim.rego" "$repo_root/.cupcake/tests/no_unbacked_claim_test.rego"
opa test "$repo_root/.cupcake/system/commands.rego" "$repo_root/.cupcake/policies/claude/no_described_next_step.rego" "$repo_root/.cupcake/tests/no_described_next_step_test.rego"
opa test "$repo_root/.cupcake/system/commands.rego" "$repo_root/.cupcake/policies/claude/git_require_runtime_evidence.rego" "$repo_root/.cupcake/tests/git_require_runtime_evidence_test.rego"
opa test "$repo_root/.cupcake/policies/claude/block_compositor_input_injection.rego" "$repo_root/.cupcake/tests/block_compositor_input_injection_test.rego"
opa test "$repo_root/.cupcake/system/commands.rego" "$repo_root/.cupcake/policies/claude/no_repo_network_banners_prompt_context.rego" "$repo_root/.cupcake/tests/no_repo_network_banners_prompt_context_test.rego"
opa test "$repo_root/.cupcake/system/commands.rego" "$repo_root/.cupcake/policies/claude/require_scoped_cargo.rego" "$repo_root/.cupcake/tests/require_scoped_cargo_test.rego"
# And the half `opa test` cannot reach. A green policy suite does not mean production-allowed or
# production-denied: these suites feed raw multi-line text the engine never delivers -- it
# collapses newlines to spaces outside balanced quoted spans, and the production shim rewrites
# unquoted `\n` to `; ` before cupcake sees the command. Measured: one test was green in `opa test`
# and allow in production, and another asserted production allows a push it has denied since the
# shim landed. A suite that never runs against the delivered shape is a weaker form of the same
# defect as one that never runs at all. 1.0s selftest + 12.9s live.
# Control note, stated honestly: its `--selftest` proves it rejects a fictional fixture, and that
# passes. A real-drift control would have to mutate `.cupcake/**`, which this agent is not
# permitted to edit; a scratch-copy harness came back red in all three states, so it proved
# nothing and is not counted as a control. The fish/csh/tcsh drift above is proven gated, by the
# four opa suites.
python3 "$repo_root/scripts/test-cupcake-delivered-shape.py" --selftest
python3 "$repo_root/scripts/test-cupcake-delivered-shape.py"
# These two had test files but no runner: written, committed, and never executed
# once. Both carried the same wrapper hole as the guards above, and neither test
# suite could have caught it because neither ever ran.
opa test "$repo_root/.cupcake/system/commands.rego" "$repo_root/.cupcake/policies/claude/git_require_fresh_origin_main.rego" "$repo_root/.cupcake/tests/git_require_fresh_origin_main_test.rego"
opa test "$repo_root/.cupcake/system/commands.rego" "$repo_root/.cupcake/policies/claude/builtins/git_block_no_verify.rego" "$repo_root/.cupcake/tests/git_block_no_verify_test.rego"
# ...AND the sweep that makes the ENUMERATION unnecessary (2026-08-31). Every line above names its
# suite, and that is precisely why four of them sat committed for weeks with no runner anywhere and
# 89 assertions that had never executed once -- among them every test of the rules standing between
# an agent and a root delete. A new `.rego` suite is born uncovered and nothing says so, so this
# gate's reach was only ever whatever somebody last remembered to type. `opa test` takes a
# directory and loads it recursively, so this single line covers every suite in .cupcake/ from the
# moment the file exists. It already picks up guard_layer_destructive_guard_test.rego (40 cases),
# which landed today reachable only through scripts/test-cupcake-policies.py's ORPHANED_REGO_SUITES
# table -- a suite reached through another script's internals is one refactor from going quiet
# again, which is exactly how this class started.
#
# The per-suite lines above stay. They attribute a failure to one suite and one row of this file's
# summary table, which a 662-case sweep cannot. Cost is not the trade-off it looks like: measured
# 0.79s wall for all 662 assertions (3.9s CPU, parallel) against 0.16s for the four lines above, in
# a suite whose neighbouring delivered-shape gate spends 19.9s. It also produces the 662/662 figure
# that reports have been quoting from a hand-run command no gate computed.
opa test "$repo_root/.cupcake/"
# What a local run of this file cannot see by construction. `compression.zstd` is stdlib only
# from Python 3.14 (PEP 784); the dev box has it, GitHub's ubuntu-latest does not, and
# scripts/regulation-params.py imported it bare -- so the import blew up at CI and nowhere else,
# taking down consumers that never decompress anything (measured: PR #388, run 33793058851).
# This blinds the interpreter with a sys.meta_path finder instead of hoping someone runs an old
# python, so the regression is catchable here. See scripts/test-regulation-params-zstd-skip.py.
python3 "$repo_root/scripts/test-regulation-params-zstd-skip.py"
python3 "$repo_root/scripts/check-no-lossy-utf8.py"
# Capitals in a comment name something -- a const, an env var, an acronym, a register, an
# upstream symbol -- and are never emphasis. All 38,682 shouted English words this tree carried
# were swept by `--fix` on 2026-09-07; scripts/comment-caps.baseline.json now holds only the 139
# in 37 files that another gate asserts on as contract text (check-windows-proof-render.py wants
# the literal `SEPARATE top-level window` in native_overlay.rs), which backticks cannot rescue
# because the match is a substring. Everything else must be at zero. Re-sweep a file with
# `scripts/check-comment-caps.py --fix <path>`, then read the diff -- a shouted word is sometimes
# load-bearing and wants a rewritten sentence rather than a lowercase letter.
python3 "$repo_root/scripts/check-comment-caps.py" --selftest
python3 "$repo_root/scripts/check-comment-caps.py"
# A NUL-terminator walk over a pointer we did not create is how both testers' games died on
# 2026-08-23 (bd er-effects-rs-uuly): `CStr::from_ptr` -> `strlen` -> AV on a garbage non-null
# `key` from Steam/Seamless, past a guard that only checked for null. Four more sites of the same
# shape were still live when that crash's own fix was reviewed, so the invariant is a gate rather
# than a habit. Selftest first, so the gate is never trusted on its own say-so.
python3 "$repo_root/scripts/check-no-unguarded-cstr-from-ptr.py" --selftest
python3 "$repo_root/scripts/check-no-unguarded-cstr-from-ptr.py"
# Two thread suspenders that do not know about each other is how three boots of the 21-DLL profile
# wedged during hook installation (bd er-effects-rs-1742): an ungated er-cpu-sampler calling
# `SuspendThread` on the game thread about once a millisecond, against twenty-one MinHook
# `Freeze()` calls doing the same thing to it. Nothing can catch this at runtime -- er-hook's
# freeze lock covers only MinHook's own entry points, and its 5s timeout is measured inside a
# thread that a foreign freeze has already suspended, so it can never fire. So the enforcement is
# static: a suspension primitive must be behind an opt-in, under the freeze lock, or in
# scripts/thread-suspension.baseline.json with a reason. Selftest first, so the gate is never
# trusted on its own say-so.
python3 "$repo_root/scripts/check-no-thread-suspension.py" --selftest
python3 "$repo_root/scripts/check-no-thread-suspension.py"
# The pre-push refusal that stops a push of game code nothing has run. Wired here so its own
# selftest is part of the suite: the cupcake policy written for this first passed 17 OPA tests
# and was inert in production, which is why the enforcement is bash the hook calls directly.
bash "$repo_root/scripts/check-runtime-evidence.sh" --selftest
# A detour's expected prologue must be generated from named iced-x86 instructions in a build.rs,
# never hand-typed: `mov rax, rsp` has two legal encodings, the game ships 48 8b c4, an assembler
# left to choose emits 48 89 e0, and a prologue that is one byte off byte-checks its own hook off
# on every launch while looking perfectly built. Selftest first, so the gate is never trusted on
# its own say-so. The shared generator + what verifies it live in build-support/prologue_build.rs;
# rustfmt cannot see that file through `include!`, so it is checked explicitly here.
python3 "$repo_root/scripts/check-prologue-bytes.py" --selftest
python3 "$repo_root/scripts/check-prologue-bytes.py"
rustfmt --edition 2024 --check "$repo_root/build-support/prologue_build.rs"
# LINT parity with ../fromsoftware-rs. Standing user requirement (2026-08-21): this code must
# be at least as strict as the parent project. Cargo cannot inherit that -- `[lints] workspace =
# true` resolves only against this workspace root and lint levels never propagate from a path
# dependency -- so parity is asserted rather than inherited. The gate reads upstream's CI and
# manifests, so it goes red when upstream gets stricter instead of us finding out months later.
# It also fails if a blanket `-Awarnings` returns to .cargo/config.toml, which silently defeats
# `[workspace.lints.rust] warnings = "deny"` (measured, not theorised). Selftest first, so the
# gate is never trusted on its own say-so.
python3 "$repo_root/scripts/check-lint-parity.py" --selftest
python3 "$repo_root/scripts/check-lint-parity.py"
# FNV-1a has one zero-dependency owner below every caller. Prove the scanner catches copied
# implementations before trusting the live ownership check.
python3 "$repo_root/scripts/check-fnv1a-owner.py" --selftest
python3 "$repo_root/scripts/check-fnv1a-owner.py"
# One game address must have exactly one literal declaration. Divergent names for one address are
# divergent claims about what it is; three turned out to be wrong RE facts shipping in the DLL
# (bd rva-67b750-is-save-write-not-continue-load-2026-08-01,
# rva-4852f88-is-saveload2-slsystemimpl-not-fd4-io-worker-2026-08-01). Selftest first, so the gate
# is never trusted on its own say-so.
python3 "$repo_root/scripts/check-rva-alias-drift.py" --selftest
python3 "$repo_root/scripts/check-rva-alias-drift.py"
# The in-memory CS::ProfileSummary record layout is cross-cutting RAM ABI, not feature-owned data.
# Keep its typed definition in er-game-base and reject copied numeric offsets/formulas elsewhere.
python3 "$repo_root/scripts/check-profile-summary-layout.py" --selftest
python3 "$repo_root/scripts/check-profile-summary-layout.py"
# A log describes exactly one process run. er-invasion-warp appended to a fixed filename, so
# twelve launches became one 565KB file and a count over it read as one run's behaviour. Every
# appending opener must route through er-game-base's one-shot truncation. Selftest first, so the
# gate is never trusted on its own say-so.
python3 "$repo_root/scripts/check-fresh-run-logs.py" --selftest
python3 "$repo_root/scripts/check-fresh-run-logs.py"
# The refactor/move DLL byte-identity gate (.github/workflows/refactor-byte-identical.yml) has two
# halves that can each rot silently: the trigger (which PRs it applies to) and the comparator (what
# counts as a difference). Both are tested here -- a gate whose scope and whose normalizer are
# untested is decorative.
bash "$repo_root/scripts/test-pr-refactor-scope.sh"
python3 "$repo_root/scripts/test-dll-byte-identical.py"
python3 "$repo_root/scripts/test-release-workflow.py"
python3 "$repo_root/scripts/check-rust-file-sizes.py"
python3 "$repo_root/scripts/check-experiments-rustfmt.py"
# The experiments ratchet. er-quickload is being extracted into crates until it is a thin
# shim that bundles them, so the line total under crates/er-quickload/src/experiments/** may
# shrink but never grow; the roadmap's ledger row is the high-water mark. It is a ratchet, not
# a freeze: edits are free, only net growth is refused, and `--refresh` accepts growth in one
# command -- the value is that accepting it becomes a reviewable diff to the ledger instead of
# the invisible default. Measured on PR #367, 62% of 1,553 added lines already landed in
# extracted crates with no enforcement, pulled there by the host-seam pattern; what that
# pattern does not catch is a new module born inside the shim, which is what this refuses.
# Selftest first, so the gate is never trusted on its own say-so.
python3 "$repo_root/scripts/check-crate-extraction-roadmap.py" --selftest
python3 "$repo_root/scripts/check-crate-extraction-roadmap.py"
# The stale-call ratchet (2026-08-28). The 1.17 build gate resolves detour addresses and refuses
# the ones it cannot place. Nothing looked at a game address reached as a direct call --
# `transmute(base + SOME_RVA)` -- and that is the worse of the two: a refused detour makes one
# feature inert and logs why, while a stale call transfers control into whatever now occupies
# those bytes and faults with no unwind and no record naming anything of ours. One such site is
# left; this refuses a second while it is converted to er_game_base::mem::game_rva.
#
# Two defects in the gate itself were fixed on 2026-08-30, and both had made it report a number
# that was not the truth. It required the constant to be spelled `*RVA*`, so forty converted-since
# sites in er-build-import-runtime named `GET_MAIN_PLAYER_STATS` were never visible to it; and it
# matched raw file text, so two of the three rows in its baseline were doc comments describing the
# hazard. The baseline was re-derived from scratch rather than edited, because a ratchet is only
# meaningful when every row in it is a finding somebody consciously accepted.
# The shared symbol RESOLVER, gated first because two gates now ask it their central question.
# It answers "which symbol declares this address" by evaluating values -- literal consts and
# statics, enum discriminants, `use X as Y` aliases, `const A = path::B` indirection,
# module-qualified names, arrays, Range bands, bare hex literals in table fields -- rather than by
# searching for one spelling, which is how both callers were wrong on 2026-08-30. Its controls
# include an address declared only as an enum discriminant, checked against the frozen pre-fix
# matcher so the proof that the widening is load-bearing cannot quietly widen with it.
python3 "$repo_root/scripts/rva_symbols.py" --selftest
python3 "$repo_root/scripts/check-stale-rva-calls.py" --selftest
python3 "$repo_root/scripts/check-stale-rva-calls.py"

# ...and the constants that resolver could not see at all. It answers by evaluating values, so a
# declaration whose value is an expression -- `BASE + 0x40`, a `size_of` product, a cast, a
# `match` arm -- fell out of the census silently; and a constant dropped from a census reads
# exactly like a constant that was checked, which is the defect this whole file exists to refuse.
# This gates every declaration in the address and field-offset populations: each one either
# evaluates to a number or is named in the UNRESOLVABLE list with the reason it cannot be. It
# does not judge the value -- that is the neighbouring gates' job -- only that the value is
# visible to them.
# RE-armed 2026-08-31, both lines. The blocker was SELECTOR_CTX_OFFSET_F8: UNRESOLVABLE listed it as
# `not modelled` while the committed sources still evaluated it to 0xf8, so the gate correctly called
# its own exception stale. That constant has settled. Measured against a detached worktree pinned to
# head as well as against this working tree -- Both lines exit 0 on both (1467 gated declarations at
# head, 1340 folded, 24 listed with a reason). Checking only the working tree would have proved
# nothing: red-at-head is precisely what unwired this pair, and it is what still holds four other
# pairs in this file dark.
# Non-vacuity re-proved rather than assumed: `audit-selftest-vacuity.py --script
# scripts/check-expression-constants.py` reports PROVABLE -- neutering all 164 regexes the gate
# compiles takes its selftest from 0 failures to 18, mutant D among them ("blinding the evaluator
# left the gate green -- the positive control passes without any folding happening").
python3 "$repo_root/scripts/check-expression-constants.py" --selftest
python3 "$repo_root/scripts/check-expression-constants.py"

# The hand-row guard (2026-08-30). `select-needed-1170-rows.py --refresh` rewrites
# docs/recon/rva-map-1162-to-1170.needed.tsv wholesale, so until today a pair somebody derived by
# hand and typed in -- exactly the short .pdata records and body-changed functions the machine map
# cannot carry -- was deleted by the next refresh at exit 0, with nothing printed and nothing in
# the diff anyone reads. The loss does not read as a loss afterwards: the address reads as one that
# was never mapped. The selftest asserts on a fabricated table that such a row is carried forward
# and that a pair contradicting the function map stops the write instead of being merged. Only the
# selftest runs here; the `--refresh`/current check is not wired in, because the tracked file
# tracks constants that land continuously and would make this gate red on somebody else's commit.
python3 "$repo_root/scripts/select-needed-1170-rows.py" --selftest
# The row-deletion guard (2026-08-30). The sibling ledger's generator,
# `map-data-rvas-1162-to-1170.py --refresh`, does not merely advise: a row it does not reproduce
# and does not want was dropped, and "does not want" was decided by a name-filtered `*RVA*` regex
# with no bare `rva: 0x..` table-field form and no inline enum-variant form. Measured on a scratch
# copy of the 111-row ledger with two rows injected: the pre-fix tool deleted `0xb0d400
# TITLE_MENU_JOB_WAIT_RVA` at exit 0 while printing "nothing declares 0xb0d400 any more", which is
# false -- `MenuTraceRva::MenuJobWait` declares it and three live autoload sites reach it. The drop
# now needs `scripts/rva_symbols.py` to prove nothing claims the address; anything short of a proof
# is preserved. `--selftest-source` is the part that reads only crates/, so this gate needs no
# capstone and no network; the image calibration still lives behind plain `--selftest`.
# RE-armed 2026-08-31. `--selftest-source` landed in f31fd637, so the exit-2 `unrecognized arguments`
# -- a step that produced no verdict about anything -- is gone. Green against a detached worktree
# pinned to head (545 sources, 5189 address-capable declarations, 0 failures) as well as against this
# working tree. Non-vacuity re-proved rather than assumed: `audit-selftest-vacuity.py --script
# scripts/map-data-rvas-1162-to-1170.py` reports PROVABLE in both modes (regex and reads) -- with its
# 61 regexes neutered the retirement gate drops 0xb0d400 as `PROVEN unclaimed` while three live
# autoload sites still reach it, which is the exact silent deletion this guard exists to refuse.
python3 "$repo_root/scripts/map-data-rvas-1162-to-1170.py" --selftest-source
# The silent-comparison gate (2026-08-30). The two gates above catch stale addresses that are
# DETOURED or called; both of those announce themselves when they fail. A stale address that is
# only compared does not. `trace_first_game_caller_rva()` / `callstack_contains_game_rva()` take a
# return address off the live stack, subtract the module base, and test it against a 1.16.2
# constant -- nothing is resolved, so nothing is refused, and on a moved build the comparison
# simply stops matching with no log line anywhere. Nine such sites existed; two of them were
# user-visible features that were dead on 1.17 in total silence (the three cloned System>Quit rows,
# and the title FadeIn suppression). The constants are mid-function return addresses, which the
# address map structurally cannot carry, so the fix is to name the containing function and add the
# offset at the use site -- see the script's docstring and `scripts/derive-callsite-1170.py`.
python3 "$repo_root/scripts/check-no-stale-callsite-rva.py" --selftest
python3 "$repo_root/scripts/check-no-stale-callsite-rva.py"
python3 "$repo_root/scripts/derive-callsite-1170.py" --selftest
# The silent-refusal gate (2026-08-28). Every cdylib statically links its own er-hook/er-game-base,
# so the log sink is a per-DLL static and a DLL that never installs one says nothing when the build
# gate refuses an address. Measured cost: er-armament-icons reported four
# MH_ERROR_UNSUPPORTED_FUNCTION failures -- a code that means both "MinHook cannot hook this" and
# "the gate refused the address" -- for addresses including one that is in the verified translation
# table, and a whole game run could not tell the two apart.
python3 "$repo_root/scripts/check-hook-log-sink.py" --selftest
python3 "$repo_root/scripts/check-hook-log-sink.py"
# The translated-target audit. `verify-rva-map-1170.py` proves the mapped 1.17 code is the same
# function; this proves the destination is a real function entry, by the calls and pointers the
# 1.17 image itself makes to it, and that MinHook's five-byte patch is safe there. Its selftest
# calibrates on the 27 addresses this project hooks successfully on 1.16.2 today -- the previous
# implementation of the entry check called 20 of those mid-function, and calibration is what
# caught it.
python3 "$repo_root/scripts/audit-1170-hook-targets.py" --selftest
# The class that audit keeps being bitten by, made un-reintroducible (2026-08-31). A linear decode
# in the de-Arxan'd images is trustworthy only inside one function: past the `ret` the gaps hold
# the deobfuscator's leftovers, not `cc`/`90` runs, so the decode resynchronises into instructions
# that were never assembled. Five confirmed instances -- a phantom `jno` conjured out of two
# padding bytes that failed a correct ledger row (the run above), 12 false DIVERGES, 31 false
# shape-DIFFs, a trampoline walk counting past its own `ret`, and a field-offset gate whose window
# had been tuned to keep a `sar dword ptr [rax], 0x6f` read out of a neighbouring function. This
# scans every capstone `.disasm`/`.disasm_lite` in scripts/ by AST and requires any span measured
# from the decode start to be justified in scripts/decode-extent-allowlist.tsv. 0.4s, no images
# needed. Positive control: a planted `blob[off : off + 0x400]` in a new file goes red naming it,
# and an extent-bounded lookalike stays green.
python3 "$repo_root/scripts/check-decode-extent-bounds.py" --selftest
python3 "$repo_root/scripts/check-decode-extent-bounds.py"
# The prologue-mask COMPARATOR, wired 2026-08-31 -- it had a green selftest and no runner, which is
# the same decorative green the gates around it exist to refuse. A build-time prologue check that is
# one byte off byte-checks its own hook off on every launch while looking perfectly built
# (check-prologue-bytes.py above), and this is what proves the mask half of that comparison is not
# simply matching everything. Positive-controlled rather than assumed: making `masked_equal` always
# return True, and making `derive_rip_mask` ignore every byte, each turn the selftest red. Two more
# selftest-carrying gates below it are still unwired, and unwiring is what this line is about, but
# both read the gitignored de-Arxan'd images, so they are left for a change that can also state what
# they should do without them.
python3 "$repo_root/scripts/verify-prologue-masks-1170.py" --selftest
# The module the gate above already imports, now run as a gate itself (wired 2026-08-31).
# `verify-prologue-masks-1170.py` has been importing this file for its spec enumeration all along,
# so half of it was already load-bearing and none of it was checked. It answers whether every AOB
# signature this tree scans for still matches, and matches uniquely, in the 1.17 image -- a pattern
# that silently matches twice, or not at all, turns a feature off with no refusal to log, which is
# the failure mode the whole 1.17 migration is about.
# It was red on three stale `guarded: False` pins claiming that three needle scans lacked a
# zero-needle guard. All three guards do exist and `return None` before any scan runs
# (er-telemetry-core title_binding.rs:200, er-input-harness title_scan.rs:125, er-title-flow
# profile_select_flow.rs:1011) -- and, checked at head, the same commit shipped both the guards and
# the pins contradicting them. Pins corrected; no source defect.
# 13s, the most expensive step added today, and it is the `patterns` section: it scans both 98 MB
# de-Arxan'd images. Deliberately wired without `--require-images`, so a checkout that lacks them
# (they are gitignored) degrades to skipped in 0.05s and exits 0 rather than failing for the wrong
# reason.
# Positive control: neutralising the zero-needle guard in profile_select_flow.rs goes red naming
# `find_title_owner_by_vtable`; swapping the guard's two `||` operands stays green.
python3 "$repo_root/scripts/verify-aob-patterns-1170.py" --selftest
# UNWIRED 2026-08-31: Failed (3 problems, no-row=1) against the committed
# docs/recon/rva-map-1162-to-1170.data.tsv, whose update is uncommitted. The --selftest above stays
# wired and green. Re-arm this line with the ledger.
# python3 "$repo_root/scripts/verify-aob-patterns-1170.py"
# ...AND whether that sweep looked at everything. The sweep above globs build files exactly one
# directory level deep, so a spec declared in `crates/foo/dll/build.rs` is invisible to it and its
# verdict silently covers fewer pins than it appears to -- while still printing `verdict: ok`.
# Sibling worktrees are renaming crates to `*-dll` right now, which is one directory move away from
# exactly that. Proven red against a real plant (bd er-effects-rs-zivc): with a planted
# `crates/er-planted-probe/dll/build.rs` carrying one spec, this gate exits 1 (`UNSWEPT ... 6
# file(s) declare 43 spec(s); the sweep enumerated 42`) while the sweep above exits 0 on the
# identical tree. A check that reports success over work it never looked at is the defect this
# whole file exists to refuse, so the sweep now has to say how much it swept.
#
# `--section coverage` specifically, measured: 4.1s. The tool's other section re-reads both 98 MB
# images and is reporting-only -- it never affects the exit code -- so wiring the whole tool would
# buy that fixed cost for output nothing gates on.
# Wired 2026-09-01 (bd er-effects-rs-zivc). The selftest was left out when the gate above it
# went in, and audit-selftest-vacuity.py sweeps check.sh: a gate with no --selftest line in
# this file is one the vacuity auditor never judges, so the gate that catches an unswept
# build.rs had nothing checking that it still catches anything. Measured with an audit hook:
# 690 files opened, every one a tracked repo source -- no image, no subprocess, no uv.
python3 "$repo_root/scripts/verify-prologue-coverage-1170.py" --selftest
python3 "$repo_root/scripts/verify-prologue-coverage-1170.py" --section coverage
# The mid-function gate. `NEITHER-ENTRY` in a verdict table is two verdicts wearing one name: a
# leaf function the x64 ABI let omit unwind data (safe to hook), and an address inside another
# function (MinHook writes five bytes into a live body). build.rs accepts the word for detours
# because refusing it would throw away every legitimate leaf, and nothing downstream re-checked
# which sense it was. Measured 2026-08-30, in one merge wave: six mid-function addresses reached
# or nearly reached the verified table, every one of them carrying `IDENTICAL` over 20-94
# instructions -- because a mid-function address verifies better than a real entry, sitting in a
# neighbourhood that did not change. One (0x140aec480, +0x360 inside 0x140aec120) was merged with
# its own note saying `containing-fn-offset-0x360`, while the real entry (0x140aec570) was already
# written down in crates/er-title-flow, and crates/er-reload-trace carried a raw `rva: 0xaec480`
# HookSpec that would have consumed the licence (since removed, on the same day, by a different
# agent who reached the same address from the other direction).
#
# The inversion, stated once: that impostor row is identical over 56 instructions and would carry a
# detour; the correct pair 0xaec570 -> 0xaed880 is identical over 9 and is refused one by
# MIN_VERIFIED_INSNS. The wrong address had the better-looking evidence.
#
# So a clean verdict is not evidence of a valid hook target, and no reviewer can be the check.
# Selftest first, so the gate is never trusted on its own say-so: it builds a .pdata table in
# memory and proves both senses of neither-entry are separated, then drives the whole failure path
# on a synthetic mid-function row. The live half skips, saying so, on a checkout without the
# de-Arxan'd images (they are untracked by policy).
python3 "$repo_root/scripts/classify-1170-entry-kind.py" --selftest
python3 "$repo_root/scripts/classify-1170-entry-kind.py" --fail-on-mid
# The hole the gate above leaves (wired 2026-08-31; the script existed since 2026-08-30 and was
# invoked by nothing). `IDENTICAL-LEAF` is the one verdict that issues its own detour licence: a
# leaf has no `.pdata` entry, so it reaches `DETOURABLE_ENTRY_EVIDENCE` through the `NEITHER-ENTRY`
# clause, which asserts nothing about the entry. That is only safe if the address really is a whole
# undescribed function rather than a point in the middle of a described one -- and
# `add_leaf_extents` tests the weaker premise, skipping a VA only when a `.pdata` entry begins
# there. An address 0x10 bytes into a declared function begins nothing, so it gets a leaf extent,
# so it can reach identical-leaf and carry a detour into the middle of a live function.
# `--fail-on-mid` above does not close this: its GATED_MAPS deliberately exclude
# `docs/recon/rva-map-1162-to-1170.tsv`, which this one reads. 2.3s. Skips, saying so, without the
# untracked de-Arxan'd images, and re-execs itself under `uv` when capstone is absent, so the step
# stays spelled `python3` for the accounting above.
# Positive control: an identical-leaf row planted at a real .pdata interior goes red naming the
# address and the containing region; the same address as identical-whole stays green.
python3 "$repo_root/scripts/check-leaf-extent-pdata-coverage.py"
# The other way an address can look like a function start and not be one (wired 2026-08-31; the
# script has existed since 2026-08-30 and was invoked by nothing). A `UNW_FLAG_CHAININFO`
# continuation chunk has a `.pdata` record, so every "does .pdata describe this address" test says
# yes -- but the record points at another record, and the real entry is somewhere else entirely.
# Detour such an address and MinHook writes its five bytes into the middle of a function whose
# prologue it never saw. Held at 0 across all three ledgers (102/411/412 rows), 0.27s.
# It skips, in a line that deliberately refuses the words OK and pass, when the untracked
# de-Arxan'd images are absent -- it used to die on a raw FileNotFoundError instead, which is the
# only reason it could not be wired.
# Positive control: a row planted at 0x140c57666 (a real continuation, primary 0x140c575e0) goes
# red naming the row and its real entry; the same row shape at the root 0x1408c47c0 stays green.
python3 "$repo_root/scripts/check-no-chained-continuation-rows.py" --selftest
python3 "$repo_root/scripts/check-no-chained-continuation-rows.py"
# The duplicate-row gate (2026-08-30). er-game-base/build.rs concatenates four address ledgers and
# finishes with `rows.sort_unstable(); rows.dedup_by_key(|(old, _)| *old)`. `sort_unstable` orders
# by the whole tuple, so among rows sharing a source the survivor is the one with the numerically
# smallest destination -- a choice nobody made, applied silently, with the losing row leaving no
# trace anywhere. Nothing gated ledger duplicates: check-rva-alias-drift.py gates Rust
# declarations, the double-resolve gate below gates a destination that is also somebody's source,
# and a verdict table verifies a pair without asking whether it was written down twice. Measured
# 2026-08-30: the curated ledger declared 0x1408c47c0 and 0x1409b72b0 twice each. Both pairs
# agreed, so the maps were right by luck -- and the two 0x1408c47c0 rows disagreed in prose about
# whether its .pdata record is a chained continuation (it is a root), which is exactly the drift a
# second row hides.
#
# A generated ledger legitimately repeats a source: select-needed-1170-rows.py emits one row per
# declaring name, 85 of them today. So the repeat rule applies only to the curated ledger, and the
# selftest carries a false-positive control that fails if anyone widens it. The selftest plants
# each defect into a copy of the real tracked ledgers and requires the verdict to flip; 7 of 7
# mutations of the gate's own rules are caught by it.
python3 "$repo_root/scripts/check-no-duplicate-ledger-rows.py" --selftest
python3 "$repo_root/scripts/check-no-duplicate-ledger-rows.py"
# The section-kind gate (wired 2026-08-31). The gate above asks whether a ledger says the same
# thing twice; this one asks whether it is talking about code at all. A ledger feeding
# `detourable_pairs` licenses a five-byte MinHook patch, which is only meaningful in EXECUTABLE
# memory; `data.tsv` carries GLOBALS, which is only meaningful outside it. Neither claim had ever
# been checked against the image's own section table.
#
# Measured 2026-08-31 on `docs/recon/rva-1170-detour-audited.tsv`, and it is why that file was
# deleted rather than refreshed: 87 of its 444 rows named non-executable destinations while
# carrying prologue verdicts like `6B relocatable`, and all 85 of the rows promoted on its
# "unwindless leaf" clause were among them -- `.pdata` declares no enclosing function for a `.data`
# global for the same reason it declares none for a leaf, so the clause could not tell them apart.
# Four were 24 bytes of zeros in both images. The four live ledgers are clean: 1047 rows, every
# code row in `.text` and every data row outside it, so this arrives green by separation and not
# by vacuity.
#
# Only --selftest runs here: R1/R2 read the gitignored de-Arxan'd 1.17 image, and the selftest
# builds a synthetic PE so it runs in a checkout that has no game files. Run the bare command where
# the image exists.
python3 "$repo_root/scripts/check-ledger-section-kind.py" --selftest
# The double-resolve gate. A row's 1.17 destination can also be some other row's 1.16.2 source,
# and then translating an address twice does not fail -- it succeeds, returning a third, unrelated
# function. The table is keyed by the 1.16.2 side and an address carries no label saying which side
# it came from, so the second lookup cannot tell an already-translated address from an untranslated
# one: nothing errors, nothing logs, and a hook lands somewhere it was never meant to. Measured
# 2026-08-30: er-reload-trace's `native_submit` resolved 0x7ac890 -> 0x7ad710 in er-hook, which
# handed the resolved address to the product's union register, which resolved again -- and
# 0x7ad710 is itself a tracked source, -> 0x7ae590. Both rows are byte-IDENTICAL/BOTH-entries, so
# no verdict, audit or entry check had anything to object to.
#
# That call path was restructured to resolve exactly once per branch and
# `register_shared_hook_resolved` was deleted, but single-resolve is a convention across six crates
# and the ledgers went from 80 to 470+ rows in a day. This is the machine check the convention did
# not have: er-game-base's `verified_map_is_idempotent` reads like it covers this and cannot -- it
# filters to rows where `from != moved`, then asks a predicate requiring `from == moved`, so it is
# a tautology. Selftest first, so the gate is never trusted on its own say-so; it drives the whole
# path over synthetic ledgers and re-reads every admission rule out of build.rs rather than
# copying it, because a copied `EXHAUSTIVE_VERDICTS` already reported one of these tables as 42
# rows instead of 374.
#
# Its claimed-by-no-feature test was fixed on 2026-08-30, and that one had passed while
# recommending a destructive action: it searched for `const NAME: usize = 0x<addr>;`, and printed
# "claimed by no feature: deleting it removes this collision at zero cost" whenever it found none.
# 0xb0d400 is declared `MenuJobWait = 0x00b0d400` inside an enum, reached as
# TITLE_MENU_JOB_WAIT_RVA, with live uses on the autoload path -- the shape it demanded never
# occurs, so its advice would have deleted a working feature's address. The answer now has three
# values (claimed / proven UNCLAIMED / not proven) and only the middle one licenses a deletion,
# and the baselined notes are held to the same rule since a note is what a reader actually sees.
python3 "$repo_root/scripts/check-1170-translation-collisions.py" --selftest
python3 "$repo_root/scripts/check-1170-translation-collisions.py"

# ...AND the other half of the same BUG: The call sites. The gate above finds the ledger rows whose
# shape makes a second resolve dangerous (`A -> B` and `B -> C` both present, which happens when a
# region's shift equals the local function spacing). This one finds the code that performs that
# second resolve: a value handed to a resolving hook API must not itself be a resolver's output.
#
# Both were needed, and the ledger gate alone would not have caught it. On the 2026-08-30 18:42 run
# three detours were installed on unrelated functions, each with the collision row sitting
# blamelessly in a baselined ledger:
#
#   drive.rs:373            game_rva 0x140614870 -> 0x1406156c0, MhHook::new -> 0x140616510
#   menu_trace_hooks.rs:274 game_rva 0x1407ac890 -> 0x1407ad710, union      -> 0x1407ae590
#   lookat_stage_camera:575 game_rva 0x140bba6e0 -> 0x140bbbd90, MhHook::new -> 0x140bbd440
#
# and each feature then logged the address it meant, which is why nobody noticed for a day.
#
# The resolving APIs and the resolvers are derived by call-graph closure over `resolve_target` /
# `resolve_detour_address` and `resolve_game_address*`, not transcribed, so a fourth entry point is
# covered the day it is added; the taint is followed through parameter forwarding and return values
# too, which is the only reason `mh_install_hook_once` and `save_flow_verify_rva` were seen at all.
# Selftest first: it blinds each half of the matcher in turn and asserts the frozen controls change
# classification, so a gate that has quietly stopped matching fails instead of printing OK.
python3 "$repo_root/scripts/check-double-resolved-hook-targets.py" --selftest
# UNWIRED 2026-08-31: 62 double-resolved arguments in the committed crate sources. Green in this
# working tree, where the conversions are in flight. The --selftest above stays wired and green.
# Re-arm this line with the crate changes.
# python3 "$repo_root/scripts/check-double-resolved-hook-targets.py"
# the second opinion on the data map'S VTABLE rows. `map-data-rvas-1162-to-1170.py` carries every
# datum by the code that references it, so each row depends on the function map being right about
# one function. RTTI depends on none of that: a vtable's [base-8] points at its
# CompleteObjectLocator, whose TypeDescriptor holds the class's mangled name, and a name occurring
# once per image identifies its vtable outright. Two methods with disjoint failure modes.
#
# This is the gate the failure it guards did not have. `TITLE_OWNER_VTABLE_RVA` is `CS::TitleStep`
# in 1.16.2 and not a vtable at all at the same address in 1.17, and its three scans had been
# finding no title owner, forever, with no refusal line and no fault -- a wrong data address does
# not crash, the comparison simply never matches. 31 of the map's rows are vtables and all 31 are
# checked here. The selftest runs first and carries its own negative control (every destination
# shifted onto the next vtable must be rejected); `--prove-selftest-catches-regression` blinds the
# matcher and requires the selftest to go red, so a green here cannot be vacuous. SKIPs at exit 0
# without the two gitignored images.
python3 "$repo_root/scripts/verify-data-rvas-by-rtti.py" --selftest
python3 "$repo_root/scripts/verify-data-rvas-by-rtti.py" > /dev/null
# The MAPPER'S own SELFTEST, wired 2026-09-01; it had existed for weeks and was invoked by nothing.
# It asserts three things no other gate does. (1) The two mappings established by reading the live
# 1.17 process are re-derived from bytes alone. (2) `0x1409a5810` is the 39th of 51 shape matches
# for `0x1409a4670`, which is why `CANDIDATE_CEILING` is not 9 -- the old cap ended the candidate
# list thirty short of the answer and the table recorded the loss as "UNRESOLVED: 9 shape matches".
# (3) The four ProfileSelect/System-Quit addresses er-effects-rs-4uw5.13 was filed about resolve
# one at a time, which they can only do from the anchor rows in
# docs/recon/rva-map-1162-to-1170.verified.tsv -- so deleting one of those rows goes red here
# naming the address, rather than quietly taking a region's coverage with it. Negative control run
# 2026-09-01: with the eight anchor rows removed, 0x875590 and 0x920c90 both go UNMAPPED and this
# exits 1. Re-execs itself under `uv` when capstone is absent, so the step stays spelled `python3`.
python3 "$repo_root/scripts/map-rvas-1162-to-1170.py" --selftest
# The independent opinion on those same anchors. Everything above reads the two flat images with
# one decoder under one normalisation, so a wrong destination that decodes into the right shape is
# invisible to all of it. This asks both Ghidra dumps instead -- 1.16.2 on :8765, 1.17 on :8767 --
# whether each half of a pair is a declared function entry and whether the call graph carries
# across. Its refusals are the control: six destinations that are wrong in a named way (0x10 past
# the entry, or another anchor's entry) must all be refused, and the clause that refuses a
# differing size with too few carried callees exists because the swapped cases passed without it.
# Skips at exit 0 when either daemon is down.
python3 "$repo_root/scripts/confirm-1170-pair-in-dumps.py" --selftest
# The field-offset gate (2026-08-30). The three ways to be wrong about a 1.17 address are not
# equally loud. A stale detour target is refused by er-hook and logged; an unmapped CALL/data RVA
# resolves to 0 and the caller says so; a stale STRUCT field offset returns the neighbouring
# field, plausible and wrong, with no refusal, no fault and no log line, forever. That third class
# had no check at all until the 2026-08-30 struct-offset audit, which found the oracle for it
# already lying on the disk and unused.
#
# `mov r64,[rip+d]` onto a known singleton global, then `[reg+0xNN]` before that register is
# written again: the base PROVABLY holds the singleton, so the displacement is a real field offset
# of whatever class lives there -- no dataflow, no RTTI pairing, no signature matching. Run over
# both flat images it says, per object, which field offsets 1.16.2 reads and 1.17 no longer does.
#
# It reports coverage, not a verdict, and its floors are the point: the per-object field counts
# are frozen exactly (they depend only on the two frozen images and the matcher), so a change that
# blinds the scan goes red instead of reporting a smaller clean set -- the failure nine audits in
# this repo shipped in one week, where `assert bad == 0` passed over an empty set. Selftest first,
# and `--prove-selftest-catches-regression` blinds the matcher and requires the selftest to go red,
# so a green here cannot be vacuous. SKIPs -- loudly, never with the word OK -- without the two
# gitignored de-Arxan'd images.
python3 "$repo_root/scripts/check-singleton-field-offsets.py" --selftest
python3 "$repo_root/scripts/check-singleton-field-offsets.py"

# The per-field half, which the census above deliberately cannot do (2026-08-31: Wired. It had
# been written and left uninvoked, so it caught nothing at all).
#
# A displacement census answers "which offsets does the image read off this object", which cannot
# say which field lives at one and cannot see a move when both the old and new offset are read
# somewhere. This gate instead ALIGNS one function'S two BODIES: when the instruction sequences
# agree except for memory displacements, instruction k is the same access to the same field in
# both builds, so a displacement difference is that field moving, by exactly that much. Each row
# names the witness function pair that produced its number; a row that cannot be re-measured is a
# failure, not a pass. It also pins the repo constants those rows verify, and the allocation size
# and class-identity of every object this repo writes into -- a wrong offset misinforms, a wrong
# object corrupts the heap.
#
# It carries the one failure a drift check structurally cannot reach: an offset that was never
# right in either build. `CS_SYSTEM_STEP_CURRENT_STATE_OFFSET` was 0x40 against a field at 0x48,
# so `oracle_system_step_label` read a pointer's low half and emitted `"?"` with a legal-looking
# i32 on every run since it was written, with nothing to drift.
#
# Selftest first: 93 perturbations, each of which must go red -- every witness row moved by 4 and
# every held row by 8, every allocation size, identity anchor and 1.17 witness address, every write
# bound raised past its allocation, plus a blind matcher, an empty source read and a changed
# constant literal. 2s (it was 82s until the tree scan and the function alignments it repeats ~95
# times over an unchanged tree were memoised; the point of that was not patience but that
# scripts/audit-selftest-vacuity.py allows 25s per script and its blinded replay costs several
# times the plain run, so this gate was UNMEASURABLE for vacuity -- an unjudgeable selftest is an
# unproven one. It now reports PROVABLE).
# scripts/prove-gate-positive-controls.py additionally plants the real defect in the real tree.
# SKIPs its image half -- saying so -- without the two gitignored de-Arxan'd images.
python3 "$repo_root/scripts/check-object-field-offsets-1170.py" --selftest
python3 "$repo_root/scripts/check-object-field-offsets-1170.py"

# The population the two lines above are measured against. `audit-name-derived-offsets.py` is a
# report, not a gate -- it counts every hand-written offset constant in the tree and says how many
# still have no provenance, and a growing count is normal output, not a failure. Only its
# `--selftest` is wired here, and it guards the one part of the report that can go quietly wrong:
# `scripts/offset-census-kinds.tsv`, the table that demotes a row out of the counted population.
#
# An exclusion nobody re-checks is how a real game-object offset gets reclassified as a Windows
# structure and stops being counted -- the report shrinks and reads as progress. The selftest
# refuses a row naming a constant that no longer exists, refuses an OS-ABI row whose constant no
# longer matches the published layout it cites, and re-asserts the name-shape rule against the
# eight names that dragged non-offsets into the first census. ~3s.
# scripts/prove-gate-positive-controls.py --only offset-census-kinds plants both failures for real.
python3 "$repo_root/scripts/audit-name-derived-offsets.py" --selftest

# ...and the attribution half of the same question. An offset whose owning object nobody has
# named cannot be measured by the gate above -- there is no object to measure it against -- so
# the unattributed set is itself a ratchet, in docs/recon/unattributed-field-offsets.txt: rows
# may disappear, they may not appear. Its floors carry the same anti-blindness property as the
# scan above, and for the same reason: a shrinking unattributed list is exactly what a blinded
# matcher or a lost owners import produces, and it would otherwise read as progress. The
# witnesses are frozen function-pair alignments re-measured from the two images on every run, so
# a row that cannot be measured is a failure, not a pass. Do not run --refresh to make it green.
# UNWIRED 2026-08-31: the ratchet refuses 4 new unattributed offsets in the committed crate sources
# (three GXDC_OUTPUT_VEC_* in write_game_module_oracles.rs plus SWORD_ARTS_PARAM_ICON_ID_OFFSET);
# the working tree has already moved them into files this commit does not carry. Both lines re-arm
# together -- a selftest with no live gate behind it proves nothing about the tree.
# python3 "$repo_root/scripts/attribute-field-offset-owners.py" --selftest
# python3 "$repo_root/scripts/attribute-field-offset-owners.py"

# The work-list audit. The inventory that says how much of the 1.17 migration is left classified
# every `*_RVA` constant in a cdylib as an eldenring.exe address, by name. Four of them are
# Seamless Co-op's, added to `GetModuleHandleA("ersc.dll")`, and an ELDEN RING patch does not move
# them: translating one through the game map and detouring the result would have put five bytes of
# jmp into an unrelated game function. Two agents caught it by reading the code and no checker did.
# The selftest pins the four foreign addresses, the plausibility bounds that are not addresses at
# all, and two real game addresses as the control, so an exclusion that ate real work fails too.
python3 "$repo_root/scripts/audit-1170-coverage-inventory.py" --selftest
# The detour-licence gate (2026-08-30). Being the right address is not the same claim as being a
# safe place for MinHook to write five bytes, and `er-hook` enforces the difference at RUNTIME: a
# detour on an address that is in the call map but not the detour map is refused, correctly and
# loudly, once per retry, forever. Nothing static noticed. A user played for seven minutes with the
# loading-screen cover pasted over live gameplay because `LOADING_SCREEN_GFX_FADEOUT_RVA`
# (0x90a0a0) was one of those, and the only evidence was 8,430 `HOOK REFUSED` lines in a 412 MB
# log. Two facts were in the tree the whole time -- "this line detours RVA X" and "X is not
# detour-safe" -- and no gate put them together.
#
# The verdict vocabulary, the ledger paths and the installer list are all PARSED out of
# er-game-base/build.rs and er-hook/src/lib.rs rather than copied, so a renamed verdict or a new
# entry point is tracked instead of silently un-checked. Selftest first, and it carries a frozen
# control (a known detour-safe site the scan must find and name) plus floors on how many sites the
# scan sees -- because a matcher that goes blind reports zero findings, which is exactly what nine
# audits in this repo did while real findings stood.
python3 "$repo_root/scripts/check-detour-rva-coverage.py" --selftest
python3 "$repo_root/scripts/check-detour-rva-coverage.py"
# The other half of the same seven minutes. The gate above keeps the unmappable address out. This
# one keeps one refusal from costing four working hooks: `install_now_loading_helper_observer_hooks`
# queued five detours, shared a single `ok` that every failure arm cleared, and returned on it
# before `MH_ApplyQueued` -- so the four healthy detours were created, queued and never enabled.
# One of them was `CS::LoadingScreen::Update`, the sole writer of `LOADING_SCREEN_UPDATE_HITS`,
# which is both the promoting condition for boot phase 8 (building world) and the source of the
# cover's release predicate: the bar froze at `LOADING SAVE 7/11` and the cover had no exit.
# Proven against the offending commit: run this gate on `git show 7a7f25b3:<that file>` and it
# names line 595. Declared-atomic hook sets are printed on every run, never hidden.
python3 "$repo_root/scripts/check-hook-batch-abort.py" --selftest
# RE-armed 2026-08-31: the two batch-abort sites this note held the line open for have landed.
# dlstring_lookat_math.rs:595 is gone and system_quit_ownership_repro.rs:495 is now declared
# atomic with its reason (the dtor detour skips the game's real destructor for any object absent),
# which the gate prints on every run rather than hiding. Green against a detached worktree pinned
# to head (39 files call MH_ApplyQueued, 21 queue more than one hook, 1 declared atomic) as well
# as against this working tree; checking only the working tree would have re-armed it a commit
# early. Its non-vacuity did not need synthesising: at 475a9963 this gate named both real sites
# and at a04f84ed it names none, so it has been observed going red on the real defect and green
# on the real fix. `audit-selftest-vacuity.py --only check-hook-batch-abort` agrees -- PROVABLE,
# 531 regexes neutered, selftest red on "a reasonless marker exempted anyway".
python3 "$repo_root/scripts/check-hook-batch-abort.py"
# The same question for direct calls. The detour gate above covers addresses that go through
# MinHook. This one covers addresses a crate simply calls, which is a different and larger set --
# and the one that broke on 2026-08-30, when the whole build importer went silently inert on 1.17.
# All 27 game functions er-build-import-runtime calls are spelled without `RVA` in the name
# (`GET_WEAPON_NAME`, `SET_REINFORCEMENT`, ...), every tool that picks addresses to translate
# keyed on the name, so none of the 27 were ever selected, mapped or verified. The six item-name
# getters were refused at runtime, every item name failed to resolve, the exporter dropped all 18
# equipped items, and the telemetry reported success the whole time.
#
# So this gate asks the question by use rather than by spelling: what does the workspace hand to
# the address resolver, and can the map answer it? A crate that resolves game addresses and has
# none of them mapped fails. Selftest first: it carries a frozen floor under the control crate and
# proves non-vacuity by blinding the matcher and observing the control collapse and the gate fail,
# because a scan that goes blind otherwise reports a clean tree over a broken one.
python3 "$repo_root/scripts/check-native-call-rva-coverage.py" --selftest
python3 "$repo_root/scripts/check-native-call-rva-coverage.py"
# The version gate. On 2026-08-29 every product DLL died within a second of loading, and it took
# eight game launches to find out why: `ERGameVersion::from_lang_version` in the sibling
# fromsoftware-rs checkout accepted only "2.6.2.0" and "2.6.2.1", the game had become 2.7.0.0, and
# `eldenring::rva::get()` therefore panicked inside a LazyLock on whichever thread first touched a
# singleton -- surfacing as eight unattributed rust_panics with the message nowhere a human looks.
# Both halves of that comparison are readable off the disk, so it never needed a game to catch.
python3 "$repo_root/scripts/check-game-version-supported.py" --selftest
python3 "$repo_root/scripts/check-game-version-supported.py"
# The same question for seamless co-OP, which needs its own gate because its answer is not a
# version-named file this repo produces. On 2026-09-02 v2.0.0 replaced v1.9.9 under an unchanged
# file name and moved everything -- `show` left 0x180022d30, the session state field went
# S+0x110 -> S+0x150, the state enum was renumbered by +1 -- and nothing announced it. A DLL built
# against v1.9.9 loads into v2.0.0 without complaint and silently does nothing. This workspace
# supports one Seamless build, recorded as ERSC_SUPPORTED_VERSION in
# build-support/prologue_build.rs; the gate reads that constant and the banner in the installed
# module and asks the only useful question, which is whether they are the same build.
python3 "$repo_root/scripts/check-ersc-version-supported.py" --selftest
python3 "$repo_root/scripts/check-ersc-version-supported.py"
# Every ME3-loadable shell must install the panic reporter.
#
# A Rust panic in a cdylib crosses an `extern "system"` boundary and becomes an abort, and an abort
# does not dispatch to a vectored handler -- so `er_crash_logging` writes no record, no `-latest`,
# no module list, and the process just vanishes. Measured 2026-09-04: `er_invasion_warp` killed the
# game repeatedly on the F9 cross-area warp with every run's crash log holding only its build
# header, while the same DLL's illegal-instruction fault produced full records every time. The
# difference was not severity; one raised an exception and the other aborted, and hours went into
# the fault that had evidence. The hook is per-DLL because every cdylib links its own
# er-game-base, so this cannot be satisfied once for everyone -- which is exactly why it is a gate
# and not a convention: the omission compiles, links, loads and runs, and only shows up as silence
# during an incident. The audit was red for 12 of 27 shells when it was first written.
python3 "$repo_root/scripts/check-panic-reporter-installed.py" --selftest
python3 "$repo_root/scripts/check-panic-reporter-installed.py"
python3 "$repo_root/scripts/check-markdown-code-blocks.py" "$repo_root/README.md"
cargo fmt --all --manifest-path "$repo_root/Cargo.toml" -- --check
shellcheck "$repo_root/.githooks/pre-push"
# This file. It stopped being a flat list on 2026-08-31 and became control flow -- two traps, an
# associative array, a summary that decides every step's state and this suite's exit code. A defect
# in that preamble misreports every gate below it at once, which is a larger blast radius than any
# single gate has. test-check-sh-accumulates.py proves the semantics; this catches the shell-level
# mistakes that test would not reach (an unquoted expansion, a masked return value).
shellcheck "$repo_root/scripts/check.sh"
shellcheck "$repo_root/scripts/check-no-local-main-commits.sh"
shellcheck "$repo_root/scripts/git-pre-push-block-main.sh"
shellcheck "$repo_root/scripts/test-git-pre-push-block-main.sh"
shellcheck "$repo_root/scripts/test-pre-push-scrubs-git-env.sh"
shellcheck "$repo_root/scripts/pr-refactor-scope.sh"
shellcheck "$repo_root/scripts/test-pr-refactor-scope.sh"
shellcheck "$repo_root/scripts/probe-dll-build-determinism.sh"
shellcheck "$repo_root/scripts/hooks/pre-push"
shellcheck "$repo_root/scripts/stage-autoload-release.sh"
shellcheck "$repo_root/scripts/run-product-continue-direct-probe.sh"
shellcheck "$repo_root/scripts/run-me3-product-smoke.sh"
shellcheck "$repo_root/scripts/run-windows-proof-render-smoke.sh"
shellcheck "$repo_root/scripts/run-portrait-dll-standalone-smoke.sh"
shellcheck "$repo_root/scripts/build-invasion-warp-profile.sh"
shellcheck "$repo_root/scripts/check-rust-build.sh"
shellcheck "$repo_root/scripts/check-committed-compiles.sh"
shellcheck "$repo_root/scripts/lib/cpu-courtesy.sh"
shellcheck "$repo_root/scripts/test-cpu-courtesy.sh"
shellcheck "$repo_root/scripts/check-git-hooks-installed.sh"
shellcheck "$repo_root/scripts/check-gate-config-guard.sh"
shellcheck "$repo_root/scripts/test-check-config-guard.sh"
shellcheck "$repo_root/scripts/measure-git-hook-env.sh"
shellcheck "$repo_root/scripts/er-stale-run-sentinel.sh"
shellcheck "$repo_root/scripts/er-tree-bisect-run.sh"
shellcheck "$repo_root/scripts/beads-prime.sh"
shellcheck "$repo_root/scripts/test-er-stale-run-sentinel-e2e.sh"
shellcheck "$repo_root/scripts/git-strip-path-from-history.sh"

# The stale-run sentinel kills a live game when an edit feeds a DLL that run loaded, so both
# directions are load-bearing: a name it cannot match is a run it cannot stop, and a path it
# misclassifies is either contaminated evidence or a run killed mid-measurement. The selftest proves
# the classifier in both directions (a crate feeding a loaded DLL and its transitive dependencies
# tear down; host-side scripts, policy, docs and crates building unloaded DLLs do not), plus the
# `/proc/<pid>/comm` 15-character truncation handling end to end against a real process.
#
# It deliberately never calls `teardown` -- a real game may be live while this gate runs. The other
# half (/proc profile discovery + the kill itself) is proven by
# scripts/test-er-stale-run-sentinel-e2e.sh, which is not run here because it is destructive by
# design; run it by hand, and it refuses if a real run is live.
bash "$repo_root/scripts/er-stale-run-sentinel.sh" --selftest

# Launch REACHABILITY gate (2026-08-04). A launch takes the user's screen and yields one recording;
# spending it on a predicate that cannot fire returns a clean-looking run that proves nothing. The
# selftest runs first and includes the concrete regression -- the `requestCode latches 2` terminator
# that shipped and could never execute -- so the gate is never trusted on its own say-so.
python3 "$repo_root/scripts/er-launch-gate.py" --selftest

# Host-buildable GFx codec + derived-movie proof gates. These are the only place the runtime GFx
# transforms are checked (the Windows-target `cargo xwin test --lib` below cannot reach an integration
# test), and they carry the System->Quit grid-geometry gate: the two added rows are navigable only
# because the derived movie names them `Item_1_0`/`Item_1_1`. Movie-reading tests skip when the local
# extraction corpus is absent, so this is safe on a machine without it.
cargo test --manifest-path "$repo_root/Cargo.toml" -p er-gfx

# Scaleform's native hook owner stays host-testable at its dependency-injection seam even
# before R24 moves the first hook family. The er-gfx architecture test above enforces the
# one-way codec dependency; this test proves the narrow callback remains inert-by-default
# and install-once.
cargo test --manifest-path "$repo_root/Cargo.toml" -p er-scaleform-hooks --lib

# er-save-loader's host-portable save decoding: BND4 slot bodies + the PlayerGameData
# stats/vitals reads the loading-screen stats panel sources pre-mount. Save-byte tests are
# corpus-gated (skip when local save-files/ fixtures are absent; game-derived bytes are
# never versioned).
cargo test --manifest-path "$repo_root/Cargo.toml" -p er-save-loader

# Host simulation for the own-load terminal-rejection state machine. It drives the preserved
# 120,959-tick churn shape and requires exactly one resolver call plus zero repeated rejections.
cargo test --manifest-path "$repo_root/Cargo.toml" -p er-save-redirect --lib

# er-loading-portrait-core's host-portable stats-line layer: proves the unified loading-screen
# stats layout (one five-line panel whether the values came from the save slot or live
# PlayerGameData, bd er-effects-rs-qic7). The bitmap-geometry test is corpus-gated on the
# extracted menu font (ER_FONT_GFX_PATH overridable) and skips when absent.
cargo test --manifest-path "$repo_root/Cargo.toml" -p er-loading-portrait-core

# The save-picker crate split (docs/plans/save-picker-crate-extraction.md). The row model
# and the quit-row resolver are pure logic, so the host run is their real coverage -- the
# whole point of the extraction is that state machines which today need a game launch
# become `cargo test`-able. The DLL shells' tests prove the host seam installs exactly
# once. `check-rust-build.sh` keeps all four building for the shipping target.
cargo test --manifest-path "$repo_root/Cargo.toml" \
	-p er-save-picker-core -p er-save-picker -p er-quit-menu-core -p er-quit-menu

# The ProfileSummary crate split. Its two host-portable decisions are the ones that were
# untestable while they lived in the shim: whether a record describes a real character (the
# predicate the whole autoload chain turns on) and the throttle standing between a ~26 MB file
# read and a per-frame ~26 MB file read. `check-rust-build.sh` keeps the windows-only half --
# the serialized-save reader and the record writer -- building and running on the shipping target.
cargo test --manifest-path "$repo_root/Cargo.toml" -p er-profile-summary-core

# The world-map invasion-spawn warp crates (docs/plans/world-map-invasion-warp.md). The
# catalog, the block grouping, the BlockId disk/memory byte-order conversion and the on-disk
# `.aip` decoder are all pure logic, so the host run is their real coverage -- that
# testability is the point of the crate split. The corpus test that decodes the 365 real
# `.aip` files skips when the local extraction is absent (game-derived bytes are never
# versioned). `check-rust-build.sh` keeps both crates building for the shipping target.
cargo test --manifest-path "$repo_root/Cargo.toml" \
	-p er-invasion-warp-core -p er-invasion-warp

# er-hotkey-conflicts is almost entirely host-testable, and deliberately so: its product is a
# warning that names modules, and the ways it can be wrong -- attributing a call to the wrong DLL,
# conflating a whole-keyboard read with a specific key, decoding the game's binding table onto the
# wrong scancode, a settle gate that never fires -- are all pure logic that would otherwise cost a
# game launch to find and would surface as a confident accusation against an innocent mod. The
# crate is windows-only to ship and the workspace pins `default-members` to er-effects-rs, so
# nothing else in any gate would ever run these.
cargo test --manifest-path "$repo_root/Cargo.toml" -p er-hotkey-conflicts --lib

# er-net-effects's host-portable modules. Six of them are ungated with a comment saying
# "so its tests run on the host" -- and until this line existed nothing ran them: the workspace
# pins `default-members` to er-quickload, so a bare `cargo test` never selects this crate and the
# windows-target `cargo xwin test --lib` in check-rust-build.sh selects er-quickload only. 42
# tests sat inert. The load-bearing one now is `selector_gate`: it decides whether this DLL may
# take the player's arrow keys away from the game, which is not a claim to leave to review.
cargo test --manifest-path "$repo_root/Cargo.toml" -p er-net-effects --lib

# er-invasion-path's host-portable half: the world->screen projection, the distance ramp, the
# per-player colour assignment and the config parser. Every one of those can be wrong without
# crashing anything -- a projection off by the aspect ratio just looks like "the overlay is
# broken" -- and none of it is reachable from any other gate: the crate is windows-only to ship,
# and the workspace pins `default-members` to er-quickload, so a bare `cargo test` never selects
# it. The near-plane trim regression this caught on the way in is exactly the class of bug that
# otherwise costs a game launch to find.
cargo test --manifest-path "$repo_root/Cargo.toml" -p er-invasion-path

# The build importer's host half: planner-JSON parsing, the name -> item-id catalogue lookup, the
# grant/equip plan, and the `er-quickload.toml` `build_url` scan. It was absent from this gate while
# it had 23 tests, so the whole mapping could regress silently -- the game-side crates
# (er-build-import-runtime, er-build-import) are windows-only and prove none of it. There is
# nothing to run here for those two: `check-rust-build.sh` keeps them building for the shipping
# target, and the DLL half is proven in game.
cargo test --manifest-path "$repo_root/Cargo.toml" -p er-build-import-core

# ...AND the write half, which had no gate at all. er-build-export is the crate that produces the
# share link, its 93 tests include the acceptance check that runs the planner'S own decoder over a
# payload we built, and `default-members` pins a bare `cargo test` to er-quickload -- so none of
# them had ever run in this suite. What that left unchecked is what the document CONTAINS: the
# encoder tests were green throughout the period when `items.tools` was never assigned, so every
# generated link shipped an empty quickbar and an empty pouch and the only place that showed was
# the planner's website. tests/round_trip.rs is the gate for that class: it decodes a document
# this crate wrote with er-build-import-core's reader and equip planner and asserts each item
# lands back on the `ChrAsmSlot` it started from.
#
# The two decoder gates inside it skip -- loudly, never silently -- without `node` or without the
# unvendored third-party LZ-UTF8 library (`npm --prefix crates/er-build-export/tests/reference
# install lzutf8@0.6.3`), so this is safe on CI and on a fresh checkout.
# UNWIRED 2026-08-31: the four reference_decoder tests need crates/er-build-export/tests/reference/,
# the node reference decoder, which is untracked. Re-arm it in the commit that lands that fixture.
# cargo test --manifest-path "$repo_root/Cargo.toml" -p er-build-export

# Two gates the repo had no equivalent of, both reading the installed regulation.bin. 1.17 added
# CharaInitParam rows 3010/3011 -- two new starting classes -- and nothing in Rust could notice:
# STARTING_CLASSES was a [&str; 10], so build export answered None for an Idus Knight and import
# never set the class (er-effects-rs-d3jz). The effects.json check replaces `er-param-inspect
# validate`, which needs a Smithbox checkout and a dotnet bridge to reach a verdict that needs
# neither (er-effects-rs-7ics). Both are dependency-free and sub-second.
#
# They run here and nowhere else on purpose. Their authority is the regulation.bin of the game
# installed on this machine, so a missing regulation is exit 2 rather than a pass -- "could not
# look" must never read as "agreed". The single escape hatch is an explicit
# ER_ALLOW_MISSING_REGULATION=1, which downgrades that to a printed `SKIPPED: ... was NOT checked`
# line on stderr. Do not set it on a developer machine that has the game: it converts the only
# gate that reads real param data into a line of log noise.
#
# Superseded 2026-08-31. This paragraph used to end: "check.yml does not invoke this script -- it
# re-implements a chosen subset of these steps as its own job steps -- and a GitHub runner has no
# game install, so these two are deliberately absent from CI. Adding them there would print
# skipped on every run forever, which is a green that means nothing." The first clause is no
# longer true (check.yml now runs this file, so the subset it re-implemented is gone), and the
# last clause was the right worry with the wrong remedy: keeping a gate out of CI to avoid a
# meaningless green just moves the hole somewhere nobody counts it. Both steps are now ledgered
# `blocked` on `game-install` in docs/ci-gate-portability.tsv, so on a runner they are skipped --
# named, counted, and stated to be non-passes -- rather than absent or quietly green.
python3 "$repo_root/scripts/diff-regulation-params.py" --effects-json
python3 "$repo_root/scripts/check-starting-classes.py"

# er-telemetry-core's host-portable logic. The workspace pins `default-members` to the DLL crate, so the
# windows-target `cargo xwin test --lib` below selects er-quickload only and never ran these -- a
# telemetry-crate test module could be added and silently never execute in any gate. The load-count
# consistency logic is pure integer arithmetic with no platform semantics, so the host run is the
# real coverage; the cross-compile check in check-rust-build.sh keeps it building for the shipping
# target too.
cargo test --manifest-path "$repo_root/Cargo.toml" -p er-telemetry-core --lib

# er-seamless-bugfixes' registries. The crate's own docs already said the `cfg(not(windows))` allow
# exists so `cargo test -p er-seamless-bugfixes` can build -- but no gate ever ran it, so all 23
# tests were inert: `default-members` pins the workspace to er-quickload, and check-rust-build.sh
# only links this shell. What that left unchecked is the whole safety argument for the code patch.
# The freelist patch rewrites one byte of live game code, and its licence to do so is that the `JZ`
# two bytes earlier already lands past the `INT3`; these tests recompute that landing address the
# way the CPU does, and require the write to be one NOP at the `INT3`'s own offset. The window
# bytes are ground-truthed separately, against eldenring-deobf.bin, by the crate's build.rs.
cargo test --manifest-path "$repo_root/Cargo.toml" -p er-seamless-bugfixes --lib

# er-lockon-filter's rule. The crate is one detour plus one predicate over two integers, and the
# predicate is the whole feature: which character kinds stop being lock-on targets, and which kinds
# you have to be for that to happen. It cannot be exercised offline any other way -- the live check
# needs two players invading one world -- and both of its failure modes are silent, so the host run
# is the only thing standing between a wrong constant and an invasion spent locking the wrong red.
cargo test --manifest-path "$repo_root/Cargo.toml" -p er-lockon-filter --lib

# er-hook's raw code-patch primitives. This crate is linked into 15 of the 23 cdylibs, the shipped
# er_quickload.dll among them, so a defect in a byte-patch primitive here is a defect in all of
# them at once -- and it is the crate least able to report one: it carries a crate-level
# `#![allow(dead_code, ...)]` for MinHook binding parity, so an unused or wrong primitive draws no
# warning, and `default-members` pins a bare `cargo test` to er-quickload so nothing ever selected
# it. The tests cover what a compile check cannot see about `write_code_byte`: that the page is
# relocked to the protection it actually had rather than left `PAGE_EXECUTE_READWRITE`, and that a
# refused `VirtualProtect` returns before the store instead of writing anyway. Each assertion was
# confirmed to go red against a deliberately broken implementation.
cargo test --manifest-path "$repo_root/Cargo.toml" -p er-hook --lib

# The 1.17 UNGATED-address ratchet. `er_game_base::game_build` translates or refuses a known 1.16.2
# address on the running build, and `er-hook`'s `MhHook::new` routes detours through it -- so a hook
# on a function the patch moved fails loudly instead of corrupting the image. That protection only
# covers addresses that go through it, and a hand-built `transmute(base + SOME_RVA)` does not: it
# calls the 1.16.2 address on 1.17 with nothing to refuse it, even when the map already knows where
# that function went. This counts those per cdylib and fails when a count rises.
#
# The property worth protecting most: 0 ungated WRITEs across all 27 cdylibs, so no DLL can corrupt
# the 1.17 image with a stale address. Measured 2026-08-29; this is what keeps it true.
python3 "$repo_root/scripts/audit-1170-readiness.py" --selftest
python3 "$repo_root/scripts/audit-1170-readiness.py" --check
# Eight more ways to reach an address without the gate (wired 2026-08-31). The ratchet above counts
# EXEC/WRITE/READ; this one adds RAW_MINHOOK, PRE_GATE_CHECK, CACHED_ADDR, CONST_FOLD, VTABLE_WRITE,
# INDIRECT_HELPER, DOUBLE_TRANSLATE and UPSTREAM_STATIC, keyed per crate-and-class against
# `scripts/audit-1170-gate-bypass.baseline.json`, failing when any count rises.
# It was red on five keys. All five were adjudicated against the source and all five are benign:
# upstream's `SoloParamRepository::instance()` resolves by runtime `.text` pattern scan through
# FromSingleton, not by a pinned RVA; `mem.rs:101`/`:122` are the gate's own implementation and its
# `game_rva_for_hook`, which returns an address deliberately unresolved so the hook API owns the
# single resolve; `announce.rs:344` and `er-refill-all/runtime.rs` hand raw targets to
# `register_union_hook`/`register_shared_hook`, which is the required form; and
# `er-seamless-bugfixes/lib.rs:430` recomposes `base + rva` from `resolve_call_site_rva`, which
# returns an RVA rather than a VA. Baseline regenerated, and it ratcheted down hard in the process
# -- 116 keys / 251 findings to 87 / 161.
# Worth recording: the baseline committed in 7a7f25b3 did not pass on 7a7f25b3's own tree.
# 1.1s + 5.4s.
# Positive control: injecting a raw `*((base + SOME_RVA) as *mut u8) = 0x90` write goes red naming
# the crate and both classes it trips (CACHED_ADDR 0->1, UNGATED_ARITH 2->3).
python3 "$repo_root/scripts/audit-1170-gate-bypass.py" --selftest
# RE-armed 2026-09-01, after the one key that still drifted was adjudicated and measured.
#
# The comment that stood here from 2026-08-31 to 2026-09-01 named two keys -- er-game-base/mem.rs
# 1->2 and er-seamless-bugfixes/lib.rs 0->1 -- and by the time anyone read it neither drifted any
# more; both are in the baseline at their current counts. That is the failure mode of writing an
# adjudication into a comment beside a disabled line: the prose rots while the line stays off, and
# the next reader cannot tell whether the gate is off for a live reason or a dead one. The
# justification now lives in `_reasons` inside the baseline, keyed by the entry it justifies, and
# `--write-baseline` carries it forward so regenerating the counts cannot silently drop it.
#
# The only key that was still red: er-quickload|VTABLE_WRITE|.../experiments/can_move_probe.rs
# 0 -> 4, the move probe's analog-stick writes at `+0x89c`/`+0x8a0`. Adjudicated in bounds by
# static RE, not by inspection: the object is `DLUID::PadDevice` = HeapAlloc(0xa68) = 2664 bytes,
# and the 1704-bytes-smaller `FD4::FD4PadDevice` (0x3c0 = 960) that made this look like a heap
# overrun is a different class that merely holds the PadDevices in its +0x10 vector. The decisive
# fact is that the game's own device poll -- the very function this repo detours -- Stores both
# floats on that same `this`, so the offsets are in-bounds by construction. Full derivation in the
# baseline's `_reasons`, and frozen against future drift by six new rows in
# check-object-field-offsets-1170.py (the poll pairs 616/616 with 72 offsets and zero moved).
python3 "$repo_root/scripts/audit-1170-gate-bypass.py" --baseline "$repo_root/scripts/audit-1170-gate-bypass.baseline.json"
# The four write classes the ratchet above cannot see (wired 2026-08-31). That one counts ungated
# addresses per cdylib; this one names the shapes that reach a write without any `base + SOMETHING`
# text to count -- a store into a vtable/function-pointer slot, a `game_data_addr(..) + offset` that
# destroys the 0-means-refused sentinel (a refusal on row 3 yields the address 24, which `if addr
# != 0` waves through), and the byte-patch primitives that take `(base, rva)` as separate
# arguments. It was report-only until today and printed six findings that were every one of them
# false: two were the primitives' own `fn` declarations self-matching, and four were call sites of
# primitives that gate internally. Both matcher faults are fixed, the true count is zero, and the
# RVA_ARG class now asks the question actually worth failing over -- does `patch_3byte_stub` /
# `apply_xor_ret_stub` still resolve through `resolve_game_address` -- rather than demanding four
# copies of that rule at the call sites. 1.4s.
# Positive control: deleting the resolve out of `patch_3byte_stub` turns its three call sites red
# by name while `apply_xor_ret_stub`'s stays green; swapping it for `resolve_game_address_fmt`
# stays green.
python3 "$repo_root/scripts/audit-ungated-image-writes.py" --selftest
python3 "$repo_root/scripts/audit-ungated-image-writes.py"

# er-game-base: the shared re-entrancy latch and the bounded wait helpers. Both are load-bearing
# for whether the game survives, not for what it computes -- `wait::poll_until` is what stops an
# unbounded `yield_now` spin from starving the serializing wineserver, and `reentry::ReentryLatch`
# is what stops a crash handler that faults while describing a fault from eating 4704 bytes of
# stack per level until the thread dies unreportably (measured on ELDEN RING 1.17, 2026-08-28).
# Neither failure mode produces a compile error and neither is selectable by a bare `cargo test`,
# because `default-members` pins that to er-quickload.
cargo test --manifest-path "$repo_root/Cargo.toml" -p er-game-base --lib

# er-refill-all: the pad-chord parser, the config reload decision, and the cycle-direction rule are
# all host-buildable on purpose, so the parts that decide whether a press does the right thing are
# testable without the game. The tracker-capacity assertion lives here too -- it is the guard on a
# DLPanic that would crash the game outright.
cargo test --manifest-path "$repo_root/Cargo.toml" -p er-refill-all

# The UNEXECUTED-test gate (2026-08-31). Everything above this line is a list of crates
# somebody remembered to name. `default-members = ["crates/er-quickload"]` means a bare
# `cargo test` selects one of 64 crates, so a crate that is not named here, in
# check-rust-build.sh, or in .github/workflows/check.yml contributes nothing to the suite --
# and reports nothing while doing it. Two crates were found in that state by accident on one
# day (er-save-suppress, whose host build was broken outright, and er-build-export, 93 tests
# including the planner-decoder acceptance check); the audit that followed found 20 more.
#
# Crate-granularity bookkeeping would not have been enough. `cargo test -p er-quit-menu-core`
# prints "ok. 43 passed" over a crate with 73 tests, because 30 of them are behind
# `#[cfg(windows)]` and simply do not exist on the host -- not compiled, not listed, not
# failed. So this gate classifies every `#[test]` by the target able to run it (walking `mod`
# declarations, `include!` splices, `#[path]`, file-level `#![cfg(...)]` and the attributes on
# the function itself) and requires a runner on that target. Selftest first, so the gate is
# never trusted on its own say-so; its non-vacuity proof blinds the windows matcher and
# requires the selftest to go red.
# RE-armed 2026-09-01. The pair below was UNWIRED on 2026-08-31 because both were red against
# the committed tree while their fixes sat only in a working tree: er-build-import-runtime's
# windows-only lib, and the `#![cfg_attr(not(windows), allow(dead_code, unused_imports))]` lines
# that let four windows cdylibs compile for the host at all. Every one of those has since landed,
# so the commented-out state had become the opposite of what its own comment described -- an
# unwiring kept for a reason that no longer held, which is how 251 test functions went on being
# reported and never run. Measured at re-arming: selftest green, live gate green, and the 251
# tests the live gate named all pass. Do not comment these out again to get a red tree green;
# a gate that is only wired when it is already passing is not a gate.
python3 "$repo_root/scripts/check-test-target-coverage.py" --selftest
python3 "$repo_root/scripts/check-test-target-coverage.py" --prove-selftest-catches-regression
python3 "$repo_root/scripts/check-test-target-coverage.py"

# The shipped MOVESET table, crates/er-npc-possess/data/moveset.tbl. Two gates in one script:
# the grammar/invariant checks always run, and the full regeneration-and-diff runs only where the
# unpacked chr corpus exists. It skips loudly rather than passing quietly when it does not -- the
# corpus is game-derived and will never be in the repo, so CI can prove the table is well-formed
# and self-consistent but not that it is what the generator produces. Set ER_CHR_CORPUS_ROOT (or
# pass --root) on a machine with an extraction to get the real check.
python3 "$repo_root/scripts/check-moveset-table.py" --selftest
python3 "$repo_root/scripts/check-moveset-table.py"

# ...AND that the table actually gives creatures their attacks. The regeneration diff above proves
# the shipped file is what the generator produces; it says nothing about whether the generator is
# producing the right answer, and for a third of the roster it was not. The generator joined the
# behaviour graph against `<chr>.tae` by chr id on both sides, and 133 creatures do not own their
# TimeAct -- c4351 Godrick Knight's anibnd is a skeleton and nothing else, because the whole 435x
# family plays out of c4350.tae. Every attack those creatures can fire was denied
# `no-damage-window` and then trimmed by the in-span rule, so they shipped as twelve W_Step walk
# clips with `denied=0`, which reads as "this creature has no attacks" everywhere it is reported.
# This gate opens the TimeAct that describes each attackless creature and fails when it finds
# attack-band animations the table is not offering. Corpus-gated and skips loudly, like the above.
python3 "$repo_root/scripts/er-moveset-coverage.py" --selftest
python3 "$repo_root/scripts/er-moveset-coverage.py" --check

# Every remaining host-TESTABLE crate, in two batches. Up to this line the crates above were
# added one at a time, each by somebody who tripped over the fact that theirs had never run --
# `default-members = ["crates/er-quickload"]` means a bare `cargo test` selects one of 64
# crates, so a crate reaches a gate only by being named here. Nothing checked the naming was
# complete, and on 2026-08-31 an audit of all 64 found 20 more crates in exactly that state:
# 279 host-runnable test functions that had never executed once, in any gate, ever. The list
# is not exotic -- the storage-box refill hook, the inventory-sort defaults, the crash-logging
# core, the hotkey parser, the save-suppression core (31 tests), the TPF/FLVER/object codecs.
# All 279 passed the first time they ran, which is the good outcome and not the point: the
# point is that nothing would have said otherwise.
#
# Batched rather than one line per crate because a single cargo invocation resolves and builds
# them in parallel: measured 2.2s + 4.2s warm for the two batches, against ~20 separate
# invocations. scripts/check-test-target-coverage.py is what keeps the list complete from here.
#
# Batch 1 -- the game-adjacent shells and cores. Four of these (er-better-refills,
# er-inventory-sort, er-loading-bar, er-save-disable) did not compile for the host until the
# same day: a windows cdylib's items read as dead on Linux and `[workspace.lints.rust] warnings
# = "deny"` promotes that to a hard error, so `cargo test -p <crate>` failed outright. Fixed
# with the same crate-level `#![cfg_attr(not(windows), allow(dead_code, unused_imports))]` that
# er-save-suppress, er-seamless-bugfixes and er-armament-icons already carry.
# RE-armed 2026-09-01. Unwired on 2026-08-31 for a real reason -- er-inventory-sort's crate-level
# allow was in a working tree and in no commit, so the batch stopped at 24 deny-by-default
# dead-code errors -- and then left unwired after that lib.rs landed. All fourteen crate-level
# allows the comment above describes are committed now; measured 2026-09-01, this line runs
# 201 tests and every one passes. That is the whole 201: 1 + 8 + 18 + 29 + 44 + 3 + 1 + 12 + 1 +
# 11 + 10 + 2 + 8 + 53, matching the per-crate host-lib counts the coverage gate had been
# printing at nobody for a day.
cargo test --manifest-path "$repo_root/Cargo.toml" \
	-p er-quickload-data -p er-build-watermark-core -p er-enemynpc-effects \
	-p er-crash-logging-core -p er-hotkey-config -p er-loading-bar-core \
	-p er-player-name-filter -p er-safe-input -p er-save-suppress \
	-p er-better-refills -p er-inventory-sort -p er-loading-bar \
	-p er-loading-portrait -p er-save-disable -p er-npc-possess
# er-npc-possess joined this batch when the crate landed. Its whole point is that everything
# above the possession seam -- the TOML reader, the schema, the not-live `[target]` staging, the
# hotkey edge latches and the possession state machine -- is decidable without the game, so it
# would be exactly the kind of crate that reports nothing if it were not named here. The
# er-refill-all pad-chord tests moved into er-hotkey-config at the same time (the module was
# promoted so both DLLs share one edge detector), so that crate's count grew rather than a new
# line appearing for them.

# er-build-export -- the crate that writes the `?i=` share link, and the only one of the fifteen
# unreachable crates that was in neither batch. 50 lib tests plus 37 in five integration targets,
# so it is deliberately not `--lib`: `tests/round_trip.rs` is where the interleave that would put
# a bolt in an arrow slot gets caught, and `--lib` would compile none of it. Two of the five
# targets reach outside the checkout and skip loudly rather than fail when they cannot --
# `python3` for the repository decoder, `node` plus an npm `lzutf8@0.6.3` for the reference
# decoder -- which is why this is one more cargo line and not a row in the portability ledger.
cargo test --manifest-path "$repo_root/Cargo.toml" -p er-build-export

# Batch 2 -- the host-only asset/codec crates and the two that ran only in CI. er-soulsformats
# and er-param-inspect were named in .github/workflows/check.yml and nowhere else, so a
# developer running this script locally got a green suite over 24 tests that had not run on
# their machine; local/CI parity is the whole reason this file exists.
cargo test --manifest-path "$repo_root/Cargo.toml" \
	-p er-flver -p er-objectkit -p er-tpf -p erpx-rs -p er-shaderkit \
	-p er-soulsformats -p er-param-inspect
# The two things `scripts/check-er-flver.sh` covered that nothing else did (moved here 2026-08-31,
# and that script deleted). It was gate-shaped, ran nowhere, and could not have gated anywhere: it
# had `set -u` but no `set -e`, piped every command into `tail`, and ended on an unconditional
# `echo "===== DONE ====="`, so it exited 0 whether its `cargo test` passed or failed -- its
# `EXIT_TEST=0` lines were printed text nobody read. The test half is already covered by the
# invocation above; these two compiles were not. `er-flver`'s `wgpu` feature gates a whole
# rendering path the default feature set never builds, and `er-shader-viewer` was referenced by
# nothing in check.sh or check-rust-build.sh at all. 0.9s for the pair once warm.
cargo check --manifest-path "$repo_root/Cargo.toml" -p er-flver --features wgpu
cargo check --manifest-path "$repo_root/Cargo.toml" -p er-shader-viewer

# Host-target compile of the product crate and its whole host dependency graph. Everything else
# in this file compiles the DLL crates for x86_64-pc-windows-msvc, where the windows-only game
# bindings always resolve -- so a `use windows::...` / `use eldenring::...` written without a
# `#[cfg(windows)]` gate is invisible to every gate here while breaking a plain host
# `cargo test`. er-title-flow shipped exactly that: 31 unresolved-import errors on the host
# (measured 2026-08-23), and the cost was misdirection -- an agent or human reaching for a host
# `cargo test` saw a wall of errors that looked like their own change.
#
# `-p er-quickload --lib` is the reproducer itself: the crate's host build is a single stub fn,
# so this compiles nothing but the dependency graph, which is the surface that rots.
# `-p er-title-flow --lib` additionally runs boot_hold's predicates -- the crate's only
# host-portable logic, and untestable at all until the gates landed.
cargo test --manifest-path "$repo_root/Cargo.toml" -p er-quickload -p er-title-flow --lib

# Rust format + Windows-target build of the injectable DLL (cross-compiled from Linux via
# cargo-xwin). A real build (not just `cargo check`) so codegen/link regressions -- including
# any pre-existing rust breakage -- are caught here, producing the linked er_quickload.dll.
# The linking gate above is only as good as its list. `check-rust-build.sh` carries an
# `me3_shells` array of every ME3-loadable cdylib and links each one, but that array was kept
# correct by a comment saying "keep this list in sync" -- so adding a new DLL crate would leave
# the suite green while nothing ever linked it, which is the same hole the array closed, one
# level up. This makes the list's completeness executable.
python3 "$repo_root/scripts/check-me3-shell-coverage.py" --selftest
python3 "$repo_root/scripts/check-me3-shell-coverage.py"

# Knowing every shell exists is not knowing which of them can share a process. Several pairs
# corrupt each other -- two MinHook instances on one prologue, two D3D12 Present compositors,
# a harness that drives input every frame -- and that knowledge used to live only as prose in
# a hand-written ~/Elden/*.me3. scripts/er-dll-closure.py now reads it as data to decide what a
# generated profile may load, so the table must stay complete: a new cdylib that nobody has
# classified is exactly the one a dependency-closure walk auto-includes. The same file also
# carries the two consent answers -- `[opt_in_only]` withholds a shell until the launch names it,
# `[always]` loads one into every launch without being named -- and this gate refuses a package
# that claims both, or that claims to be unconditional without having been found compatible.
python3 "$repo_root/scripts/check-me3-dll-conflicts.py" --selftest
python3 "$repo_root/scripts/check-me3-dll-conflicts.py"

# ...and the table only helps if it still matches the code. This scans every cdylib for the hook
# targets it claims and fails on any address two of them claim without a [[conflict]] or [[shared]]
# row -- then proves each [[shared]] row's mechanism, so neither side can quietly revert to a
# private MinHook instance. That reversion is the failure this pair of gates exists for: two
# instances on one prologue overwrite each other's trampolines, the loser reports installed and
# never runs, nothing crashes, and the feature merely looks unimplemented. It cost a full day on
# 2026-08-23 before an A/B against a one-DLL profile named it.
python3 "$repo_root/scripts/check-shared-hook-rvas.py" --selftest
python3 "$repo_root/scripts/check-shared-hook-rvas.py"

# The branch-launch pipeline. Each stage refuses rather than guessing, and each carries its own
# selftest for the refusal it exists to make -- a stale DLL, an unrankable conflict, a save with
# no decoded identity, a block printed without the DLL's testimony.
python3 "$repo_root/scripts/er_run_lib.py"
python3 "$repo_root/scripts/er-dll-closure.py" --selftest
# ...and the selection built on that walk: which gate work a diff can invalidate. Its selftest
# is the anti-drift half -- it re-derives, from the build scripts and from this file, that no
# path it waves through is a build input and that no gate here compiles without being declared.
python3 "$repo_root/scripts/er-change-scope.py" --selftest
python3 "$repo_root/scripts/er-dll-provenance.py" --selftest
# (The launch-time half of that pair, `er-dll-freshness.sh --selftest`, would sit here with its
# siblings but CANNOT: it needs a linked DLL to exist. It runs after check-rust-build.sh below.)
python3 "$repo_root/scripts/er-pick-save.py" --selftest
python3 "$repo_root/scripts/er-gen-me3-profile.py" --selftest
python3 "$repo_root/scripts/er-run-reaper.py" --selftest
python3 "$repo_root/scripts/er-run-branch.py" --selftest

# Scoring a DLL by launching it alone. Its verdict is the husk oracle -- thread count and CPU
# burn, not a pid existing -- and its selftest drives every branch of that classification,
# including the two-thread husk that a naive check calls a pass. It launches nothing.
python3 "$repo_root/scripts/er-release-bisect.py" --selftest

# Product D3 contract: the customized quit menu is an rlib dependency inside the one shipped
# er_quickload.dll. Its standalone DLL remains an explicitly-built harness and must never leak into
# the default build, staged product payload, or required ME3 native list.
python3 "$repo_root/scripts/check-single-dll-product-contract.py" --selftest
python3 "$repo_root/scripts/check-single-dll-product-contract.py"

bash "$repo_root/scripts/check-rust-build.sh"

# The launch-time freshness gate, shared by five launch scripts as `require_fresh_dlls`. Its
# selftest was written and left unwired, which is the same decorative green the gates above exist
# to refuse. Positive-controlled 2026-08-31 rather than taken on trust: disabling the refusal, and
# silently skipping an artifact name the workspace does not build, each turn it red; so does making
# er-dll-provenance's `verify` always agree or its source hash a constant.
#
# It lives here, below the link, and not with the other launch-pipeline selftests ~20 lines above,
# because it needs a real linked DLL: the provenance record it exercises fingerprints the artifact's
# code section, so it parses a PE header and a placeholder file raises instead of standing in.
# Measured 2026-09-01 in a fresh agent worktree with an empty target/ -- it went red with "no built
# DLL at .../er_crash_logging.dll" while the change under test was fine, which is a gate teaching
# people to read past it. Above the link this step is a property of the checkout, not of the code.
bash "$repo_root/scripts/er-dll-freshness.sh" --selftest

# Does the committed state compile -- not "does my working tree compile", which is the only
# question every gate above this one, this file included, is able to ask. A pathspec commit is
# the exact mechanism by which a consumer lands without its PRODUCER: the new caller is named on
# the command line, the crate or function it calls is not, and the author's checkout still holds
# the producer, so it compiles for them and for every gate that builds their tree. Measured on
# this branch 2026-08-31 -- 15b32ab0 and 11af0c60 were both that shape, `origin` did not compile
# for hours, a dozen agents built on top of it, and a210af7f had to land an 18-file compile
# closure to make it green again. This type-checks a git worktree pinned to the commit under
# test, so an uncommitted producer is invisible to it, which is the whole point. The selftest is
# two-sided: the two historical failures must go red and head must go green, because a gate
# wedged red is as useless as one wedged green.
# RE-armed and green: the E0533 that broke head while this file was being landed (`er_save_redirect::
# SaveSourceRejection::Unreadable` became a struct variant, its er-quickload and er-save-picker
# consumers landed still matching it as a unit variant) was fixed in 21ec2296. That was the third
# consumer-without-producer commit on this branch in one day, and it is why this line exists.
# scripts/hooks/pre-push runs the same script on every pushed tip; this catches it a commit earlier.
bash "$repo_root/scripts/check-committed-compiles.sh" --selftest
bash "$repo_root/scripts/check-committed-compiles.sh"

# ...and whether any of this runs on a commit at all. This clone's core.hooksPath was the
# absolute pre-rename path left behind by 39a919e0, so for a while no hook ran -- not the
# main-push guard, not this suite -- and nothing said so, because a hook git cannot find
# is indistinguishable from a hook that passed. That is this suite's own defect applied to the
# gates themselves. It asserts the value is set, that it resolves to a real directory holding an
# executable pre-push, and that it is RELATIVE: an absolute path is correct right up until the
# checkout is renamed, and then it is silently wrong.
bash "$repo_root/scripts/check-git-hooks-installed.sh" --selftest
bash "$repo_root/scripts/check-git-hooks-installed.sh"

# ...and whether the gate damages the checkout it is gating. It did, twice on 2026-08-31, from a
# push made in a linked worktree: git exports GIT_DIR to a linked worktree's hooks (but not to a
# main checkout's -- scripts/measure-git-hook-env.sh measures both, which is why this looked
# unreachable for a day), `git -C <fixture>` does not override it, and the fixture commands landed
# on the shared config -- core.bare = true, core.hooksPath gone, a push through the hole. The
# offending script now scrubs its environment; scripts/check-gate-config-guard.sh catches the
# class for this whole suite, and this proves it still fires in both directions.
bash "$repo_root/scripts/test-check-config-guard.sh"


# Dead/unused code in the save-disable DLL, on its shipping target. Scoped to that one
# crate on purpose: the repo builds with a global `-Awarnings`, so this is the narrow
# place where warning-freedom is both achievable today and load-bearing -- the crate's
# whole job is to stop saves, and two dead helpers already survived a refactor unseen.
python3 "$repo_root/scripts/check-save-disable-warnings.py"

# Reached only when every step above has run. The exit trap reads this to tell a completed
# suite apart from one that stopped early -- the difference between a real verdict and silence.
_check_reached_end=1
