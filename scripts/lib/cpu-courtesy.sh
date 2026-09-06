# shellcheck shell=bash
# Make a long build or gate yield to the human at the keyboard. Sourced, never executed.
#
# WHY THIS EXISTS
# ---------------
# MEASURED 2026-09-06: the user reported the machine was unusable at 100% CPU while a pre-push
# `check.sh` ran. The hogs were ten `scripts/check-*.py` workers -- and they were running at
# **nice -4**, i.e. at HIGHER priority than the desktop they were starving. Nothing in this repo
# asked for that. The agent harness runs its shells at -4, and nice is inherited across fork and
# exec, so every gate, every cargo invocation and every rustc the agent ever spawned silently
# outranked the compositor, the browser and the game.
#
# That is the whole defect: the build did not merely use the CPU, it was given priority OVER the
# person trying to use the computer. Load average hit 34 on 16 cores and the desktop stopped
# responding.
#
# SO THE FIX BELONGS HERE, NOT IN THE CALLER. A wrapper the caller has to remember (`nice -n19
# bash scripts/check.sh`) is not enforcement -- it fails exactly when someone forgets, which is
# every time an agent invokes the script directly or a git hook does. These scripts therefore
# renice THEMSELVES at startup, whatever they inherited, so no caller can hand them a priority
# they should not have.
#
# WHY RENICE IS SAFE AND ONE-WAY
# ------------------------------
# Raising niceness (lower priority) needs no privilege and is always permitted; LOWERING it below
# 0 needs CAP_SYS_NICE, which this repo does not have and does not want. `cpu_courtesy` therefore
# only ever moves in the yielding direction, and is a no-op when the process is already at or
# below the floor. It cannot escalate anything.
#
# WHY A JOBS CAP TOO, AND WHY IT IS NOT ENOUGH ON ITS OWN
# -------------------------------------------------------
# `nice` fixes WHO WINS a contended core; it does not reduce how many cores are contended, so a
# 16-job build still pins every core and the desktop stutters even while winning. `CARGO_BUILD_JOBS`
# bounds the rustc processes cargo spawns. Neither one covers the other:
#
#   * cargo's `jobs` does NOTHING for the ~190 non-cargo gates in check.sh, which were the actual
#     hogs in the measurement above, nor for threads INSIDE a single rustc, nor for the linker.
#   * `nice` alone leaves every core saturated.
#
# Hence both, and hence the cap is a fraction of the machine rather than `nproc - 1`: leaving one
# core free does not make a desktop responsive when the other fifteen are pinned.

# Lowest priority this repo's long jobs may run at. 10 rather than 19 so a build still makes
# real progress on an otherwise idle machine while losing every contested slice to the user.
: "${ER_NICE_FLOOR:=10}"

# Fraction of the machine a build may take, as a divisor. 2 = half the cores. Chosen so the user
# keeps enough parallelism for a browser, a compositor and a game while a cold cross-compile runs.
: "${ER_JOB_DIVISOR:=2}"

# How many cores this machine has, or a conservative guess when it cannot be read.
er_cpu_count() {
	local count
	count=$(nproc 2>/dev/null) || count=""
	[[ "$count" =~ ^[0-9]+$ && "$count" -gt 0 ]] || count=4
	printf '%s' "$count"
}

# The job cap: half the cores, never below 1. `ER_BUILD_JOBS` overrides outright, for a machine
# where the operator wants the whole box (CI, or a build nobody is waiting behind).
er_job_cap() {
	if [[ -n "${ER_BUILD_JOBS:-}" ]]; then
		printf '%s' "$ER_BUILD_JOBS"
		return
	fi
	local cores cap
	cores=$(er_cpu_count)
	cap=$((cores / ER_JOB_DIVISOR))
	((cap < 1)) && cap=1
	printf '%s' "$cap"
}

# Yield the CPU, cap the parallelism, and say so once.
#
# `$1` is the caller's name, used only in the one line this prints. Printing it matters: a build
# that is quietly slower than the reader expects is a bug report, and the line is the answer.
#
# Every step is best-effort. A missing `renice`, a refused `ionice` or an unreadable `nproc` must
# degrade to "ran at the priority it inherited", never to a failed build -- the courtesy is for
# the user's comfort and is not worth breaking a gate over.
cpu_courtesy() {
	local who="${1:-build}" current floor cap
	floor="$ER_NICE_FLOOR"
	cap=$(er_job_cap)

	# Bound the rustc processes cargo will spawn. Exported rather than passed as `-j` so it
	# reaches every nested cargo invocation, including the ones inside other scripts.
	export CARGO_BUILD_JOBS="$cap"

	# AND THE PYTHON WORKER POOLS, which is the half `nice` cannot reach and the half that
	# actually pinned this machine. MEASURED 2026-09-06: with every gate process already
	# reniced to 19, `scripts/check-moveset-table.py` still held 81.4% of a 16-core box,
	# because it sizes its pool `int(os.environ.get('SWEEP_JOBS', os.cpu_count() or 8))` --
	# one worker per core, each at ~90% CPU. Priority decides who WINS a contended core; it
	# does nothing about how many cores are contended, so the desktop stayed unusable while
	# formally losing every race. `SWEEP_JOBS` is the knob that gate already reads.
	export SWEEP_JOBS="${SWEEP_JOBS:-$cap}"

	# `nproc` is what a pool with no env knob calls, and a Python `os.cpu_count()` respects the
	# affinity mask rather than the core count -- so restricting the mask caps every pool at
	# once, including ones that were written without a knob. Best-effort: `taskset` may be
	# absent, and a container may already restrict us, in which case this changes nothing.
	if command -v taskset >/dev/null 2>&1; then
		taskset -cp "0-$((cap - 1))" $$ >/dev/null 2>&1 || true
	fi

	current=$( (nice) 2>/dev/null || echo 0)
	[[ "$current" =~ ^-?[0-9]+$ ]] || current=0
	if ((current < floor)); then
		# `$$` is this shell; children inherit, which is the entire point.
		renice -n "$floor" -p $$ >/dev/null 2>&1 || true
	fi

	# Disk is the other resource a cold build monopolises. Idle-class IO is best-effort and is
	# skipped silently where the scheduler or the container does not allow it.
	command -v ionice >/dev/null 2>&1 && ionice -c 3 -p $$ >/dev/null 2>&1 || true

	echo "[$who] cpu courtesy: nice $current -> $(nice), CARGO_BUILD_JOBS=$cap of $(er_cpu_count) cores" >&2
	echo "[$who]   override with ER_BUILD_JOBS=<n> ER_NICE_FLOOR=<n>; see scripts/lib/cpu-courtesy.sh" >&2
}
