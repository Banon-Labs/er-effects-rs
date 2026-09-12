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

# The git hash the newest run's DLLs were built from, or empty.
#
# Every shell in the run writes its own `build git=` first line -- four of them in a typical
# profile -- so this reads whichever one has appeared rather than one named file. It used to read
# only er-invasion-warp.log, which made the answer depend on a DLL being in the profile at all.
#
# `+dirty` disqualifies a log here, which is stricter than the push guard: since 2026-09-11
# `scripts/er-runtime-evidence.py` accepts a dirty build line when a provenance record proves the
# shell's own dependency closure was committed, and this reader does not implement that proof. The
# difference only ever makes this rebuild when the guard would not have asked for one, which is the
# safe direction for a script whose job is to order commit-build-run-push.
newest_run_dir() {
	find "$runs_dir" -maxdepth 1 -name 'br-*' -printf '%T@ %p\n' 2>/dev/null | sort -rn | head -1 | cut -d' ' -f2- || true
}

newest_run_build() {
	local newest artifact built
	newest="$(newest_run_dir)"
	[ -n "$newest" ] || return 0
	for artifact in "$newest"/er-*.log; do
		[ -f "$artifact" ] || continue
		built="$(head -1 "$artifact" | sed -n 's/^build git=\([0-9a-f]*\)\([^ ]*\) .*/\1\2/p')"
		case "$built" in
		"" | *+dirty) continue ;;
		*)
			printf '%s' "$built"
			return 0
			;;
		esac
	done
}

# Wait for the relaunched run to write a build line, then answer.
#
# er-run-branch.py returns once the QUICKLOAD dll's own log line confirms it loaded; the other
# shells write their first line a moment later. Reading once at that instant is a race, and it
# lost twice on 2026-09-10 -- both times reporting `<none>` and refusing to push a run that was
# seconds away from proving itself.
#
# The waiting is done by scripts/wait-for-run-build-line.py, which blocks on inotify rather than
# polling: a poll loop is a sleep, and scripts/check-no-timeouts.py bans those in every language
# here, correctly -- the readiness signal is the file changing, and 30s is only the cap.
await_run_build() {
	local newest
	newest="$(newest_run_dir)"
	[ -n "$newest" ] || return 0
	built="$(python3 "$repo_root/scripts/wait-for-run-build-line.py" "$newest" --timeout-seconds "$BUILD_LINE_WAIT_SECONDS" || true)"
}

# The cap scripts/wait-for-run-build-line.py is given, not an interval it waits out.
BUILD_LINE_WAIT_SECONDS=30

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
	# The reader half, against a run directory built here rather than against the real cache.
	local sandbox
	sandbox="$(mktemp -d)"
	mkdir -p "$sandbox/br-00000000-000000-aaaa"
	echo "nothing here" >"$sandbox/br-00000000-000000-aaaa/er-quickload-continue-trace.log"
	echo "build git=7ac6a383a435+dirty module=er_npc_possess.dll" >"$sandbox/br-00000000-000000-aaaa/er-npc-possess.log"
	runs_dir="$sandbox"
	[ -z "$(newest_run_build)" ] || { echo "  FAIL a dirty build line must not count"; failures=1; }
	echo "build git=7ac6a383a435 module=er_invasion_warp.dll base=0x1 pe=0x2 (t)" >"$sandbox/br-00000000-000000-aaaa/er-invasion-warp.log"
	[ "$(newest_run_build)" = "7ac6a383a435" ] || { echo "  FAIL a clean line in any er-*.log must count"; failures=1; }
	rm -rf "$sandbox"
	if [ "$failures" -eq 0 ]; then
		echo "er-ship selftest: OK (6 cases)"
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
	await_run_build
	same_commit "$built" "$head_commit" || {
		echo "er-ship: still stale after a rebuild (${built:-<none>}) -- not pushing"
		exit 1
	}
fi

git -C "$repo_root" push origin "$(git -C "$repo_root" rev-parse --abbrev-ref HEAD)"
