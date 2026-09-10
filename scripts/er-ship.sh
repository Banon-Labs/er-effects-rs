#!/usr/bin/env bash
# Commit-then-build-then-run-then-push, in that order, because the other order does not work.
#
# The DLL stamps the git hash at build time. Build first and then commit, and the artifact carries
# the previous commit -- so `scripts/check-runtime-evidence.sh` correctly refuses the push, naming
# a run that predates the code being pushed. That is not the guard being awkward: it is the guard
# catching an ordering mistake, and on 2026-09-10 it caught the same one four times in a row
# because the ordering lived in an agent's head instead of in a script.
#
# Usage:
#   bash scripts/er-ship.sh            check the ordering and push if it holds
#   bash scripts/er-ship.sh --fix      rebuild, relaunch and then push, if the newest run is stale
#   bash scripts/er-ship.sh --selftest exercise the staleness comparison with no game and no push
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
runs_dir="${ER_ME3_RUNS_DIR:-$HOME/.cache/er-me3-runs}"

# The git hash the newest run's DLL was built from, or empty.
newest_run_build() {
	local newest
	newest="$(find "$runs_dir" -maxdepth 1 -name 'br-*' -printf '%T@ %p\n' 2>/dev/null | sort -rn | head -1 | cut -d' ' -f2- || true)"
	[ -n "$newest" ] || return 0
	[ -f "$newest/er-invasion-warp.log" ] || return 0
	head -1 "$newest/er-invasion-warp.log" | sed -n 's/^build git=\([0-9a-f]*\).*/\1/p'
}

# Does `built` name the same commit as `head`? Compared on the shorter of the two, because the log
# carries twelve characters and `git rev-parse` can be asked for any width.
same_commit() {
	local built="$1" head="$2" width
	[ -n "$built" ] || return 1
	width="${#built}"
	[ "${#head}" -lt "$width" ] && width="${#head}"
	[ "${built:0:width}" = "${head:0:width}" ]
}

selftest() {
	local failures=0
	same_commit "7ac6a383a435" "7ac6a383a435c0ffee" || { echo "  FAIL a prefix must match"; failures=1; }
	same_commit "7ac6a383a435" "7ac6a383a435" || { echo "  FAIL identical must match"; failures=1; }
	! same_commit "7ac6a383a435" "425f00dd48ba" || { echo "  FAIL a different commit must not"; failures=1; }
	! same_commit "" "7ac6a383a435" || { echo "  FAIL an unbuilt run must not match"; failures=1; }
	if [ "$failures" -eq 0 ]; then
		echo "er-ship selftest: OK (4 cases)"
		return 0
	fi
	echo "er-ship selftest: FAILED"
	return 1
}

case "${1:-}" in
--selftest)
	selftest
	exit $?
	;;
esac

head_commit="$(git -C "$repo_root" rev-parse HEAD)"
built="$(newest_run_build)"

if same_commit "$built" "$head_commit"; then
	echo "er-ship: the newest run was built from ${built}, which is HEAD -- pushing"
else
	echo "er-ship: the newest run was built from '${built:-<none>}', HEAD is ${head_commit:0:12}."
	echo "         The DLL stamps its hash at BUILD time, so a run older than HEAD cannot"
	echo "         evidence this push. Commit first, THEN build, THEN launch."
	if [ "${1:-}" != "--fix" ]; then
		echo "         Re-run with --fix to rebuild, relaunch and push."
		exit 1
	fi
	bash "$repo_root/scripts/er-build-dlls.sh" --all
	python3 "$repo_root/scripts/er-teardown.py"
	python3 "$repo_root/scripts/er-run-branch.py" --no-fetch
	built="$(newest_run_build)"
	same_commit "$built" "$head_commit" || {
		echo "er-ship: still stale after a rebuild (${built:-<none>}) -- not pushing"
		exit 1
	}
fi

git -C "$repo_root" push origin "$(git -C "$repo_root" rev-parse --abbrev-ref HEAD)"
