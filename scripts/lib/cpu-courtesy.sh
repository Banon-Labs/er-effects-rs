# shellcheck shell=bash
# Make a long build or gate yield to the human at the keyboard. Sourced, never executed.
#
# Why this exists
# ---------------
# Measured 2026-09-06: the user reported the machine was unusable at 100% CPU while a pre-push
# `check.sh` ran. The hogs were ten `scripts/check-*.py` workers -- and they were running at
# **nice -4**, i.e. at higher priority than the desktop they were starving. Nothing in this repo
# asked for that. The agent harness runs its shells at -4, and nice is inherited across fork and
# exec, so every gate, every cargo invocation and every rustc the agent ever spawned silently
# outranked the compositor, the browser and the game.
#
# That is the whole defect: the build did not merely use the CPU, it was given priority over the
# person trying to use the computer. Load average hit 34 on 16 cores and the desktop stopped
# responding.
#
# So the fix belongs here, not in the caller. A wrapper the caller has to remember (`nice -n19
# bash scripts/check.sh`) is not enforcement -- it fails exactly when someone forgets, which is
# every time an agent invokes the script directly or a git hook does. These scripts therefore
# renice themselves at startup, whatever they inherited, so no caller can hand them a priority
# they should not have.
#
# Why RENICE is safe and one-way
# ------------------------------
# Raising niceness (lower priority) needs no privilege and is always permitted; Lowering it below
# 0 needs CAP_SYS_NICE, which this repo does not have and does not want. `cpu_courtesy` therefore
# only ever moves in the yielding direction, and is a no-op when the process is already at or
# below the floor. It cannot escalate anything.
#
# Why a jobs cap too, and why it is not enough on its own
# -------------------------------------------------------
# `nice` fixes who wins a contended core; it does not reduce how many cores are contended, so a
# 16-job build still pins every core and the desktop stutters even while winning. `CARGO_BUILD_JOBS`
# bounds the rustc processes cargo spawns. Neither one covers the other:
#
#   * cargo's `jobs` does nothing for the ~190 non-cargo gates in check.sh, which were the actual
#     hogs in the measurement above, nor for threads inside a single rustc, nor for the linker.
#   * `nice` alone leaves every core saturated.
#
# Hence both, and hence the cap is a fraction of the machine rather than `nproc - 1`: leaving one
# core free does not make a desktop responsive when the other fifteen are pinned.
#
# Why a SCHED-policy lever too, and why nice above is not enough on this class of machine
# ----------------------------------------------------------------------------------------
# Measured 2026-09-06 on this CachyOS box, live from /proc during a real gate run: the renice
# above is inert here. `git push` sat at nice 11 (a relative `nice -n 15` on a -4 parent), but its
# child `bash check.sh` was back at nice -4 -- a child cannot lower its own nice below what it
# inherited -- and the eight `check-moveset-table.py` workers it forked inherited that -4 too. The
# `cpu_courtesy` banner had printed "nice 11 -> 11" for that very shell, correctly reading 11 and
# leaving it; something with `CAP_SYS_NICE` moved it afterwards, unprompted.
#
# That something is `ananicy-cpp`, a systemd daemon (`apply_nice = true`, `check_freq = 15` in
# `/etc/ananicy.d/ananicy.conf`) that classifies every process literally named `bash` as type
# `Doc-View` (`/etc/ananicy.d/bash.rules`) and pins that type to nice -4
# (`/etc/ananicy.d/00-types.types`). It re-applies that pin to every bash on this machine roughly
# every 15 seconds, including every gate script and everything it forks, so the renice above wins
# for at most one sweep before losing again -- it is not merely weak here, it is actively
# reversed. Confirmed on a single pid across one 20s window: `renice -n 10` plus `chrt -i` at t=0
# read back as `nice=10, SCHED_IDLE`; at t=20s (past one `check_freq`) the same pid read back as
# `nice=-4, SCHED_IDLE` -- ananicy reverted the nice, unchanged, and left the scheduling policy
# alone. The `Doc-View` type carries no `sched` key, so it has nothing to reapply there.
#
# Hence `chrt -i` (`SCHED_IDLE`): the process only runs when no other runnable task wants that
# core, which is strictly below anything `nice` alone can guarantee and survives a daemon that
# actively fights the nice value on this class of machine. It is additive to, not a replacement
# for, the renice above -- a machine without ananicy (or any daemon like it) still benefits from
# both, and the renice is what a `nice`-only reader of `/proc` will see until they check
# `chrt -p` too.

# Lowest priority this repo's long jobs may run at. 10 rather than 19 so a build still makes
# real progress on an otherwise idle machine while losing every contested slice to the user.
: "${ER_NICE_FLOOR:=10}"

# Fraction of the machine a build may take, as a divisor. 2 = half the cores. Chosen so the user
# keeps enough parallelism for a browser, a compositor and a game while a cold cross-compile runs.
: "${ER_JOB_DIVISOR:=2}"

# Drop to SCHED_IDLE (`chrt -i`) in addition to the renice above. Set to 0 for a CI runner or an
# operator who wants the whole box -- SCHED_IDLE means "only run when nothing else wants this
# core," which is the wrong trade on a machine nobody is sitting at.
: "${ER_SCHED_IDLE:=1}"

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

	# IDEMPOTENT, because the cap reads a number it itself changed. `er_cpu_count` calls `nproc`,
	# which reports the affinity mask, not the socket -- so a second call inside a nested script
	# sees the 8 cores the first call granted and halves them again. check.sh sources this and
	# then invokes check-rust-build.sh, which sources it too: without this guard that run would
	# have proceeded on 4 cores, then 2, ratcheting toward serial. Measured in this file's own
	# banner on 2026-09-06, which printed "CARGO_BUILD_JOBS=8 of 8 cores" -- the 8 was already
	# the masked count, one nesting level away from being wrong rather than merely confusing.
	#
	# Re-entry is a no-op rather than an error: nice and the mask are inherited by children, so
	# the nested caller already has everything this would grant it.
	if [[ -n "${ER_CPU_COURTESY_APPLIED:-}" ]]; then
		echo "[$who] cpu courtesy: already applied by $ER_CPU_COURTESY_APPLIED (nice $(nice), jobs ${CARGO_BUILD_JOBS:-?})" >&2
		return 0
	fi

	floor="$ER_NICE_FLOOR"
	# Read the machine before masking it, and remember it, so the banner below reports the cap
	# against the real core count instead of against itself.
	local cores
	cores=$(er_cpu_count)
	cap=$(er_job_cap)
	export ER_CPU_COURTESY_APPLIED="$who"

	# Bound the rustc processes cargo will spawn. Exported rather than passed as `-j` so it
	# reaches every nested cargo invocation, including the ones inside other scripts.
	export CARGO_BUILD_JOBS="$cap"

	# And the Python worker pools, which is the half `nice` cannot reach and the half that
	# actually pinned this machine. Measured 2026-09-06: with every gate process already
	# reniced to 19, `scripts/check-moveset-table.py` still held 81.4% of a 16-core box,
	# because it sizes its pool `int(os.environ.get('SWEEP_JOBS', os.cpu_count() or 8))` --
	# one worker per core, each at ~90% CPU. Priority decides who wins a contended core; it
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

	# SCHED_IDLE, because on this class of machine the renice above is reversed within seconds --
	# see the header comment. Best-effort exactly like `ionice` above: a missing `chrt`, a refused
	# call, or a container that forbids it must leave the process at whatever policy it inherited,
	# never fail the gate. `$$` again, for the same reason: children inherit scheduling policy.
	local sched=""
	if [[ "$ER_SCHED_IDLE" != "0" ]] && command -v chrt >/dev/null 2>&1; then
		chrt -i -p 0 $$ >/dev/null 2>&1 || true
	fi
	command -v chrt >/dev/null 2>&1 && sched=$(chrt -p $$ 2>/dev/null | sed -n 's/.*scheduling policy: //p')

	echo "[$who] cpu courtesy: nice $current -> $(nice), CARGO_BUILD_JOBS=$cap of $cores cores, SWEEP_JOBS=$SWEEP_JOBS, sched=${sched:-unknown}" >&2
	echo "[$who]   override with ER_BUILD_JOBS=<n> ER_NICE_FLOOR=<n> ER_SCHED_IDLE=0; see scripts/lib/cpu-courtesy.sh" >&2
}
