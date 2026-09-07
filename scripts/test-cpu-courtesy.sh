#!/usr/bin/env bash
# Prove scripts/lib/cpu-courtesy.sh actually binds -- all four levers, not just the two with
# obvious env vars.
#
# WHY A TEST AND NOT AN EYEBALL: the library's whole claim is that it caps parallelism the caller
# never asked about. Three of its four levers are invisible to the process that sets them
# (`CARGO_BUILD_JOBS` and `SWEEP_JOBS` are read by children; the affinity mask is read by
# `os.sched_getaffinity` inside an unrelated Python pool), so "it printed a line" proves nothing.
# Each assertion below therefore observes the lever from where it is actually consumed.
#
# MEASURED 2026-09-06, the failure this exists to keep fixed: with every gate process reniced to
# 19, scripts/check-moveset-table.py still held 81.4% of a 16-core box, because it sizes its pool
# from os.cpu_count(). Priority decides who wins a contended core; it does nothing about how many
# cores are contended.
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
probe() { # probe <env assignments...> -- prints "nice jobs sweep pyaffinity"
	# shellcheck disable=SC2016  # the $-expansions belong to the inner shell, deliberately
	env -u SWEEP_JOBS -u CARGO_BUILD_JOBS -u ER_CPU_COURTESY_APPLIED \
		-u ER_BUILD_JOBS -u ER_JOB_DIVISOR -u ER_NICE_FLOOR "$@" bash -c '
		set -euo pipefail
		. scripts/lib/cpu-courtesy.sh
		cpu_courtesy selftest 2>/dev/null
		printf "%s %s %s %s\n" \
			"$(nice)" "$CARGO_BUILD_JOBS" "$SWEEP_JOBS" \
			"$(python3 -c "import os; print(len(os.sched_getaffinity(0)))")"
	'
}

echo "[test-cpu-courtesy] host reports $cores cores"

echo "explicit cap (ER_BUILD_JOBS=$small):"
read -r _ n_jobs n_sweep n_aff < <(probe "ER_BUILD_JOBS=$small")
check "CARGO_BUILD_JOBS" "$small" "$n_jobs"
check "SWEEP_JOBS"       "$small" "$n_sweep"
# THE LOAD-BEARING ONE. A pool with no env knob at all -- and check-moveset-table.py's default --
# sizes itself from the affinity mask, so this is the assertion that covers every gate nobody
# thought to make configurable.
check "python affinity"  "$small" "$n_aff"

echo "derived cap (half the machine):"
want=$((cores / 2)); ((want < 1)) && want=1
read -r d_nice d_jobs _ d_aff < <(probe ER_JOB_DIVISOR=2)
check "CARGO_BUILD_JOBS" "$want" "$d_jobs"
check "python affinity"  "$want" "$d_aff"

echo "priority floor:"
# Only ever yields. The harness runs agent shells at -4, so this asserts the direction that
# matters: whatever we inherited, we come out at the floor or above it, never below.
if [[ "$d_nice" -ge 10 ]]; then ok "nice $d_nice >= floor 10"; else bad "nice $d_nice is below the floor"; fi
read -r h_nice _ _ _ < <(probe ER_NICE_FLOOR=3)
if [[ "$h_nice" -ge 3 ]]; then ok "respects ER_NICE_FLOOR=3 (got $h_nice)"; else bad "ER_NICE_FLOOR ignored: $h_nice"; fi

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
read -r _ _ s_sweep _ < <(probe "ER_BUILD_JOBS=$cores" SWEEP_JOBS=1)
check "SWEEP_JOBS honoured when preset" 1 "$s_sweep"

if [[ $fail -eq 0 ]]; then
	echo "[test-cpu-courtesy] passed"
else
	echo "[test-cpu-courtesy] FAILED" >&2
fi
exit "$fail"
