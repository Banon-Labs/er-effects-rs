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
set -uo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root" || exit 1

fail=0
ok()   { printf '  ok    %s\n' "$1"; }
bad()  { printf '  FAIL  %s\n' "$1" >&2; fail=1; }
check() { # check <label> <expected> <actual>
	if [[ "$2" == "$3" ]]; then ok "$1 = $3"; else bad "$1: expected $2, got $3"; fi
}

# One subshell per scenario: cpu_courtesy renices and re-masks the CALLING shell, and neither is
# reversible from inside it (renice is one-way without CAP_SYS_NICE), so a second scenario in the
# same process would read the first one's leftovers and pass for the wrong reason.
probe() { # probe <env assignments...> -- prints "nice jobs sweep pyaffinity"
	env "$@" bash -c '
		set -euo pipefail
		. scripts/lib/cpu-courtesy.sh
		cpu_courtesy selftest 2>/dev/null
		printf "%s %s %s %s\n" \
			"$(nice)" "$CARGO_BUILD_JOBS" "$SWEEP_JOBS" \
			"$(python3 -c "import os; print(len(os.sched_getaffinity(0)))")"
	'
}

echo "[test-cpu-courtesy] host reports $(nproc) cores"

echo "explicit cap (ER_BUILD_JOBS=2):"
read -r _ n_jobs n_sweep n_aff < <(probe ER_BUILD_JOBS=2)
check "CARGO_BUILD_JOBS" 2 "$n_jobs"
check "SWEEP_JOBS"       2 "$n_sweep"
# THE LOAD-BEARING ONE. A pool with no env knob at all -- and check-moveset-table.py's default --
# sizes itself from the affinity mask, so this is the assertion that covers every gate nobody
# thought to make configurable.
check "python affinity"  2 "$n_aff"

echo "derived cap (half the machine):"
cores=$(nproc 2>/dev/null || echo 4)
want=$((cores / 2)); ((want < 1)) && want=1
read -r d_nice d_jobs _ d_aff < <(probe ER_JOB_DIVISOR=2)
check "CARGO_BUILD_JOBS" "$want" "$d_jobs"
check "python affinity"  "$want" "$d_aff"

echo "priority floor:"
# Only ever yields. The harness runs agent shells at -4, so this asserts the direction that
# matters: whatever we inherited, we come out at the floor or above it (numerically), never below.
if [[ "$d_nice" -ge 10 ]]; then ok "nice $d_nice >= floor 10"; else bad "nice $d_nice is below the floor"; fi
read -r h_nice _ _ _ < <(probe ER_NICE_FLOOR=3)
if [[ "$h_nice" -ge 3 ]]; then ok "respects ER_NICE_FLOOR=3 (got $h_nice)"; else bad "ER_NICE_FLOOR ignored: $h_nice"; fi

echo "caller's environment wins:"
read -r _ _ s_sweep _ < <(probe SWEEP_JOBS=1 ER_BUILD_JOBS=8)
check "SWEEP_JOBS honoured when preset" 1 "$s_sweep"

if [[ $fail -eq 0 ]]; then
	echo "[test-cpu-courtesy] passed"
else
	echo "[test-cpu-courtesy] FAILED" >&2
fi
exit "$fail"
