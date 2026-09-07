#!/usr/bin/env bash
# Prove scripts/lib/cpu-courtesy.sh actually binds -- all five levers, not just the two with
# obvious env vars.
#
# WHY A TEST AND NOT AN EYEBALL: the library's whole claim is that it caps parallelism the caller
# never asked about. Four of its five levers are invisible to the process that sets them
# (`CARGO_BUILD_JOBS` and `SWEEP_JOBS` are read by children; the affinity mask is read by
# `os.sched_getaffinity` inside an unrelated Python pool; the scheduling policy is read back via
# `chrt -p` in a child, not from the setter), so "it printed a line" proves nothing. Each assertion
# below therefore observes the lever from where it is actually consumed.
#
# MEASURED 2026-09-06, the failure this exists to keep fixed: with every gate process reniced to
# 19, scripts/check-moveset-table.py still held 81.4% of a 16-core box, because it sizes its pool
# from os.cpu_count(). Priority decides who wins a contended core; it does nothing about how many
# cores are contended.
#
# THE SCHED-IDLE LEVER GETS A DIFFERENT SHAPE OF ASSERTION, NOT A LITERAL ONE. On this machine
# `chrt -i` is the lever that actually survives ananicy-cpp's periodic renice (see
# scripts/lib/cpu-courtesy.sh); on a GitHub runner or a locked-down container `chrt` may be
# absent, or present but refused. The probe therefore re-attempts the same idempotent `chrt -i -p
# 0` call itself and reports whether IT could set the policy; the assertion only expects
# `SCHED_IDLE` when the probe just proved the syscall is available, so a runner without it self-
# reports "nothing to assert" instead of failing.
#
# EVERY NUMBER HERE IS DERIVED FROM THE LIVE MACHINE, NEVER A LITERAL. A GitHub runner has 2-4
# cores, and `taskset -c 0-3` on a 2-core box fails (best-effort, so it silently changes nothing)
# -- an assertion written as "affinity == 4" would then fail on the runner while passing here.
set -uo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root" || exit 1

fail=0
ok()  { printf '  ok    %s\n' "$1"; }
bad() { printf '  FAIL  %s\n' "$1" >&2; fail=1; }
check() { # check <label> <expected> <actual>
	if [[ "$2" == "$3" ]]; then ok "$1 = $3"; else bad "$1: expected $2, got $3"; fi
}

cores=$(nproc 2>/dev/null || echo 4)
# A cap this run can actually be granted: at least 1, never more than the machine has.
small=2; ((cores < 2)) && small=1

# One subshell per scenario, and a SCRUBBED one.
#
# THE ENVIRONMENT THIS TEST RUNS IN HAS USUALLY ALREADY BEEN CAPPED. scripts/check.sh calls
# cpu_courtesy before it reaches this gate, so SWEEP_JOBS/CARGO_BUILD_JOBS are already exported
# and ER_CPU_COURTESY_APPLIED already set. Without `env -u`, `SWEEP_JOBS="${SWEEP_JOBS:-$cap}"`
# correctly preserves the inherited 8 and the assertion below reads it as a failure to apply the
# cap. Measured: this gate failed inside check.sh ("SWEEP_JOBS: expected 2, got 8") while passing
# standalone -- the test was wrong, not the library.
#
# Renice and the affinity mask are also one-way and inherited, so each scenario must be its own
# process or it reads the previous one's leftovers and passes for the wrong reason.
probe() { # probe <env assignments...> -- prints "nice jobs sweep pyaffinity sched settable"
	# shellcheck disable=SC2016  # the $-expansions belong to the inner shell, deliberately
	env -u SWEEP_JOBS -u CARGO_BUILD_JOBS -u ER_CPU_COURTESY_APPLIED \
		-u ER_BUILD_JOBS -u ER_JOB_DIVISOR -u ER_NICE_FLOOR -u ER_SCHED_IDLE "$@" bash -c '
		set -euo pipefail
		. scripts/lib/cpu-courtesy.sh
		cpu_courtesy selftest 2>/dev/null
		sched=$(chrt -p $$ 2>/dev/null | sed -n "s/.*scheduling policy: //p")
		# Re-attempt the very call cpu_courtesy already made (idempotent -- SCHED_IDLE set twice
		# is still SCHED_IDLE) to find out, from right here, whether THIS host can grant it at
		# all. That is what tells the test whether an unmet "expected SCHED_IDLE" is a real
		# regression or just an environment (missing/refused chrt) that never had the lever.
		if command -v chrt >/dev/null 2>&1 && chrt -i -p 0 $$ >/dev/null 2>&1; then
			settable=1
		else
			settable=0
		fi
		printf "%s %s %s %s %s %s\n" \
			"$(nice)" "$CARGO_BUILD_JOBS" "$SWEEP_JOBS" \
			"$(python3 -c "import os; print(len(os.sched_getaffinity(0)))")" \
			"${sched:-none}" "$settable"
	'
}

echo "[test-cpu-courtesy] host reports $cores cores"

echo "explicit cap (ER_BUILD_JOBS=$small):"
read -r _ n_jobs n_sweep n_aff _ < <(probe "ER_BUILD_JOBS=$small")
check "CARGO_BUILD_JOBS" "$small" "$n_jobs"
check "SWEEP_JOBS"       "$small" "$n_sweep"
# THE LOAD-BEARING ONE. A pool with no env knob at all -- and check-moveset-table.py's default --
# sizes itself from the affinity mask, so this is the assertion that covers every gate nobody
# thought to make configurable.
check "python affinity"  "$small" "$n_aff"

echo "derived cap (half the machine):"
want=$((cores / 2)); ((want < 1)) && want=1
read -r d_nice d_jobs _ d_aff _ < <(probe ER_JOB_DIVISOR=2)
check "CARGO_BUILD_JOBS" "$want" "$d_jobs"
check "python affinity"  "$want" "$d_aff"

echo "priority floor (best-effort by construction -- see below):"
# THIS CANNOT BE ASSERTED AS AN OUTCOME ON THIS CLASS OF MACHINE, and pretending otherwise made
# this gate fail its own push. ananicy-cpp runs as root with CAP_SYS_NICE and re-nices every
# `bash` back to -4 every 15 seconds (its bash rule assigns the Doc-View type, which declares
# nice: -4). A probe that renices itself to 10 can be dragged to -4 before it reads the value
# back, and it was: `FAIL nice -4 is below the floor`, on the very branch that documents why
# nice is unreliable here. Asserting a nice VALUE is asserting the thing this library exists to
# tell you is not enforceable.
#
# What cpu_courtesy actually guarantees is one-way movement: it raises niceness toward the floor
# and never lowers it (lowering needs CAP_SYS_NICE, which it does not have and does not want).
# That is the property worth pinning, and it holds whether or not a daemon interferes: either we
# reached the floor, or something with more privilege moved it -- and in the second case the
# value is not ours to defend.
floor_ok=0
[[ "$d_nice" -ge 10 ]] && floor_ok=1
# `nice -n 0 nice` reports what a fresh child of THIS shell inherits, i.e. what the probe started
# from. If the observed value is not above the floor, it must at least not be below where we began.
inherited_nice=$(nice)
if ((floor_ok)); then
	ok "nice $d_nice >= floor 10"
elif [[ "$d_nice" -ge "$inherited_nice" ]]; then
	ok "nice $d_nice was reverted below the floor by a privileged daemon, but never lowered by us (inherited $inherited_nice)"
else
	bad "cpu_courtesy LOWERED priority: $inherited_nice -> $d_nice, which it must never do"
fi
read -r h_nice _ _ _ _ _ < <(probe ER_NICE_FLOOR=3)
if [[ "$h_nice" -ge 3 || "$h_nice" -ge "$inherited_nice" ]]; then
	ok "ER_NICE_FLOOR=3 honoured or externally reverted (got $h_nice)"
else
	bad "ER_NICE_FLOOR ignored and priority lowered: $h_nice"
fi

echo "sched-idle lever (survives an ananicy-cpp-style renice reversion; see scripts/lib/cpu-courtesy.sh):"
read -r _ _ _ _ i_sched i_settable < <(probe)
if [[ "$i_settable" == "1" ]]; then
	# The probe just proved, from inside its own child, that THIS host can grant SCHED_IDLE --
	# so a mismatch here is a real regression in cpu_courtesy, not an environment gap.
	check "sched policy (ER_SCHED_IDLE=1 default)" "SCHED_IDLE" "$i_sched"
else
	ok "chrt is absent or refused here -- sched-idle is a documented best-effort no-op, nothing to assert"
fi
# ...and the opt-out leaves the policy AS INHERITED -- which is not the same as "SCHED_OTHER",
# and asserting the literal is what broke this gate inside check.sh on 2026-09-06.
#
# The scheduling policy is INHERITED PROCESS STATE, not an environment variable. check.sh calls
# cpu_courtesy on itself long before reaching this gate, so every child here is ALREADY
# SCHED_IDLE -- and a SCHED_IDLE process cannot raise itself back to SCHED_OTHER without
# CAP_SYS_NICE. "ER_SCHED_IDLE=0 => SCHED_OTHER" is therefore unsatisfiable in the environment
# this gate actually runs in, and the earlier simulation missed it because it reproduced the
# suite's env vars (SWEEP_JOBS, CARGO_BUILD_JOBS, ER_CPU_COURTESY_APPLIED) but could not
# reproduce its process state.
#
# The real invariant, true in both environments: the opt-out CHANGES NOTHING. Measure what this
# shell already is, then require the opted-out child to match it.
inherited_sched=$(chrt -p $$ 2>/dev/null | sed -n 's/.*scheduling policy: //p')
read -r _ _ _ _ o_sched _ < <(probe ER_SCHED_IDLE=0)
if [[ -z "$inherited_sched" ]]; then
	ok "chrt cannot report this shell's policy -- nothing to compare the opt-out against"
else
	check "ER_SCHED_IDLE=0 leaves the policy as inherited" "$inherited_sched" "$o_sched"
fi

echo "nesting does not ratchet the cap:"
# THE REGRESSION THIS PINS: er_cpu_count calls nproc, which reports the affinity MASK. A second
# cpu_courtesy inside a nested script therefore sees the cores the first one granted and halves
# them again -- check.sh sources this library and then invokes check-rust-build.sh, which sources
# it too, so an unguarded version walks 8 -> 4 -> 2 toward serial.
#
# Compared against the OUTER call's own result rather than a literal, so the assertion says
# "nesting changed nothing" on any size of machine.
# shellcheck disable=SC2016  # the $-expansions belong to the inner shell, deliberately
nested=$(env -u SWEEP_JOBS -u CARGO_BUILD_JOBS -u ER_CPU_COURTESY_APPLIED bash -c '
	set -euo pipefail
	. scripts/lib/cpu-courtesy.sh
	cpu_courtesy outer 2>/dev/null
	before="$CARGO_BUILD_JOBS $SWEEP_JOBS $(python3 -c "import os; print(len(os.sched_getaffinity(0)))")"
	unset ER_BUILD_JOBS ER_JOB_DIVISOR   # a nested script has no idea what the outer one chose
	cpu_courtesy inner 2>/dev/null
	after="$CARGO_BUILD_JOBS $SWEEP_JOBS $(python3 -c "import os; print(len(os.sched_getaffinity(0)))")"
	printf "%s|%s\n" "$before" "$after"
')
check "jobs/sweep/affinity unchanged by a nested call" "${nested%%|*}" "${nested##*|}"

echo "caller's environment wins:"
read -r _ _ s_sweep _ _ _ < <(probe "ER_BUILD_JOBS=$cores" SWEEP_JOBS=1)
check "SWEEP_JOBS honoured when preset" 1 "$s_sweep"

if [[ $fail -eq 0 ]]; then
	echo "[test-cpu-courtesy] passed"
else
	echo "[test-cpu-courtesy] FAILED" >&2
fi
exit "$fail"
