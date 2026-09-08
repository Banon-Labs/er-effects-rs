#!/usr/bin/env bash
# Does the CONFIG guard actually catch a gate that REWRITES the repository CONFIG?
#
# scripts/check-gate-config-guard.sh compares core.bare and core.hooksPath in the shared git dir
# before and after the gate suite, because on 2026-08-31 the suite itself flipped core.bare to true
# and unset core.hooksPath in the live checkout -- twice, silently, from a push made in a linked
# worktree (bd hooks-selftest-under-git-hook-blanks-the-live-config-2026-08-31).
#
# A guard nobody has watched fail is a guard nobody knows is wired up. This proves it in both
# directions against the real text: the fixture sources the guard file rather than copying it, so a
# rename, a rewrite or a deletion changes this test's behaviour or stops it finding the guard at
# all -- it cannot drift into testing a stale copy of the logic. (Before 2026-09-03 the guard was
# the opening trap of scripts/ci-local-check.sh and this test copied that file's head; the hook now
# runs scripts/check.sh, the same suite CI runs, and the guard moved into its own sourced file.)
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
guard="$repo_root/scripts/check-gate-config-guard.sh"
label='[test-check-config-guard]'

if [[ ! -f "$guard" ]]; then
	echo "$label FAIL: $guard does not exist -- the guard this test proves is gone, or was renamed." >&2
	exit 1
fi
for required in gate_config_snapshot gate_config_report gate_config_key; do
	if ! grep -q "^$required()" "$guard"; then
		echo "$label FAIL: $guard defines no $required -- check.sh's calls into it cannot work." >&2
		exit 1
	fi
done

# The fixtures below run git commands in throwaway repositories. If this script inherits a git
# environment -- which is exactly the bug under test -- those commands would retarget the real
# checkout and the test would corrupt the thing it is defending.
# shellcheck disable=SC2046  # word splitting is the point: one variable name per word.
unset $(git rev-parse --local-env-vars)

tmp=$(mktemp -d "${TMPDIR:-/tmp}/er-gate-config-guard-test.XXXXXX")
trap 'rm -rf -- "$tmp"' EXIT

# A fixture "suite": snapshot, run one line standing in for the gates, report. The real suite is
# ~150 gates; the guard is indifferent to what happens between the snapshot and the report, which
# is the whole point of comparing endpoints rather than watching writes.
build_fixture() {
	local dir=$1 body=$2
	rm -rf -- "$dir"
	mkdir -p "$dir/scripts"
	git init -q "$dir"
	git -C "$dir" config core.hooksPath scripts/hooks
	cp -f "$guard" "$dir/scripts/check-gate-config-guard.sh"
	cat >"$dir/scripts/fixture-suite.sh" <<FIXTURE
#!/usr/bin/env bash
set -uo pipefail
repo_root=\$(cd -- "\$(dirname -- "\${BASH_SOURCE[0]}")/.." && pwd)
source "\$repo_root/scripts/check-gate-config-guard.sh"
gate_config_snapshot "\$repo_root"
$body
gate_config_report || exit 1
exit 0
FIXTURE
}

run_fixture() { (cd "$1" && bash scripts/fixture-suite.sh 2>&1); }

fail=0

# --- negative control: a gate that touches nothing must stay green ------------------------------
build_fixture "$tmp/clean" 'echo "gates ran"'
if run_fixture "$tmp/clean" >/dev/null; then
	echo "$label ok -- an honest suite passes"
else
	echo "$label FAIL: the guard rejected a suite that changed nothing." >&2
	echo "  A guard that fires on a clean run gets disabled by the next person to hit it." >&2
	fail=1
fi

# --- the 2026-08-31 damage, one key at a time --------------------------------------------------
# Two separate cases rather than one combined mutation: a guard that only noticed core.bare would
# still miss the ninety-minute ungated-push window, which was the hooksPath half.
check_case() {
	local name=$1 body=$2 expect=$3
	build_fixture "$tmp/$name" "$body"
	local out rc
	out=$(run_fixture "$tmp/$name") && rc=0 || rc=$?
	if ((rc == 0)); then
		echo "$label FAIL: $name was ACCEPTED. The gate rewrote the repository config and the" >&2
		echo "  suite exited 0 -- the silent failure, restored." >&2
		fail=1
		return
	fi
	if ! printf '%s' "$out" | grep -q "$expect"; then
		echo "$label FAIL: $name was refused, but the message never named the key that changed" >&2
		echo "  (expected to see: $expect). Got:" >&2
		printf '%s\n' "$out" | sed 's/^/    /' >&2
		fail=1
		return
	fi
	echo "$label ok -- $name is refused, and the message names the key"
}

check_case bare_flipped 'git config core.bare true' 'core.bare      false -> true'
check_case hookspath_unset 'git config --unset core.hooksPath' 'core.hooksPath scripts/hooks -> <unset>'

if ((fail)); then
	exit 1
fi
echo "$label passed"
