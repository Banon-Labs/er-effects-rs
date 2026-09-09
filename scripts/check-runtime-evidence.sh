#!/usr/bin/env bash
# Refuse a push of code that runs inside ELDEN RING when nothing has run it.
#
# Called by scripts/hooks/pre-push before the gate suite. Run its selftest directly:
#
#     bash scripts/check-runtime-evidence.sh --selftest
#
# The question it asks is whether a run artifact exists whose DLL says it was built from the commit
# being pushed. The evidence is the first line every shell in this workspace writes to its own log:
#
#     build git=b6b459560dfa module=er_invasion_warp.dll base=0x... pe=0x... (2026-09-09T02:05:18Z)
#
# That sha is what ties a run to code, and a file timestamp does not. The first version of this
# check compared mtimes and answered "evidence present" for HEAD 0e084240 on a log whose own first
# line read `build git=b6b45956` -- a DLL two commits older, still running and still writing, so its
# file was newer than the commit it could not possibly have executed. Reading a clock and calling it
# provenance is the exact mistake this exists to stop.
#
# `+dirty` on that line disqualifies the run: the tree carried uncommitted changes when the DLL was
# built, so the binary is not the commit even when the sha matches.
#
# Three things it deliberately leaves alone:
#   * a push that changes no crate. Docs, scripts and policies have nothing for a run to prove, and
#     a guard that fires on everything is one the next agent overrides by reflex.
#   * whether the run went well. A run that executed the code and went badly is a fact worth
#     pushing with; a run that never executed it is not evidence of anything.
#   * a tree it cannot measure. No run root, no git, no readable log -- it says so and allows,
#     because a check that cannot see must not invent a verdict.
#
# The override is `ER_ALLOW_UNPROVEN_PUSH=1`, and it prints what is being waived.
set -uo pipefail

# evidence_for <tip sha> -> 0 evidence, 1 no evidence, 2 unmeasurable. Prints one line of prose.
evidence_for() {
	local tip="$1"
	local run_root="${ER_ME3_RUN_ROOT:-$HOME/.cache/er-me3-runs}"
	if [ ! -d "$run_root" ]; then
		printf 'no run root at %s -- cannot measure\n' "$run_root"
		return 2
	fi
	python3 - "$run_root" "$tip" <<'PY'
import pathlib
import re
import sys

run_root = pathlib.Path(sys.argv[1])
tip = sys.argv[2]
BUILD_LINE = re.compile(r"^build git=([0-9a-f]+)(\+dirty)?\b")

seen = []
for run in sorted(run_root.iterdir()):
    if not run.is_dir():
        continue
    for artifact in sorted(run.glob("er-*.log")):
        try:
            with artifact.open(encoding="utf-8", errors="replace") as handle:
                first = handle.readline()
        except OSError:
            continue
        found = BUILD_LINE.match(first)
        if not found:
            continue
        built, dirty = found.group(1), bool(found.group(2))
        seen.append((run.name, artifact.name, built, dirty))
        if not dirty and (built.startswith(tip) or tip.startswith(built)):
            print(f"{run.name}/{artifact.name} was built from {tip} and ran")
            raise SystemExit(0)

if not seen:
    print("no DLL log under any run directory carries a build line")
else:
    run_name, name, built, dirty = seen[-1]
    state = "a dirty tree at " if dirty else ""
    print(f"the newest run {run_name} ran {name}, built from {state}{built[:8]}, not {tip}")
raise SystemExit(1)
PY
}

selftest() {
	local failures=0
	local tmp
	tmp="$(mktemp -d)"
	# Cleaned up explicitly at every exit below rather than with a `RETURN` trap. A `RETURN` trap
	# set here stays armed for the caller's own return, and `main` has no `tmp` -- so the selftest
	# passed and then killed the script with `tmp: unbound variable` on the way out.

	expect() { # expect <wanted rc> <description> ... runs evidence_for with the caller's env
		local wanted="$1" description="$2"
		shift 2
		"$@" >/dev/null 2>&1
		local got=$?
		if [ "$got" = "$wanted" ]; then
			printf '  ok    %s\n' "$description"
		else
			printf '  FAIL  %s (wanted rc %s, got %s)\n' "$description" "$wanted" "$got"
			failures=$((failures + 1))
		fi
	}

	mkdir -p "$tmp/runs/br-good"
	printf 'build git=deadbeef1234 module=er_invasion_warp.dll base=0x1 pe=0x2 (t)\nmore\n' \
		>"$tmp/runs/br-good/er-invasion-warp.log"
	ER_ME3_RUN_ROOT="$tmp/runs" expect 0 "a log whose build sha is the commit counts as evidence" \
		evidence_for "deadbeef1234"
	ER_ME3_RUN_ROOT="$tmp/runs" expect 0 "an abbreviated tip sha matches the log's full sha" \
		evidence_for "deadbeef"
	ER_ME3_RUN_ROOT="$tmp/runs" expect 1 "a log from a different build is refused" \
		evidence_for "0e0842402b18"

	mkdir -p "$tmp/dirty"
	printf 'build git=deadbeef1234+dirty module=er_invasion_warp.dll\n' \
		>"$tmp/dirty/er-invasion-warp.log"
	ER_ME3_RUN_ROOT="$tmp/dirty" expect 1 "a dirty build is refused even with a matching sha" \
		evidence_for "deadbeef1234"

	# The mtime trap, as a regression: a file newer than the commit, from an older build.
	mkdir -p "$tmp/mtime"
	printf 'build git=aaaaaaaaaaaa module=er_invasion_warp.dll\n' >"$tmp/mtime/er-old-build.log"
	touch -d '+1 hour' "$tmp/mtime/er-old-build.log" 2>/dev/null ||
		touch "$tmp/mtime/er-old-build.log"
	ER_ME3_RUN_ROOT="$tmp/mtime" expect 1 "a newer file from an older build is still refused" \
		evidence_for "deadbeef1234"

	ER_ME3_RUN_ROOT="$tmp/nonexistent" expect 2 "an absent run root reports unmeasurable, not refused" \
		evidence_for "deadbeef1234"

	mkdir -p "$tmp/nobuild"
	printf 'some other log\n' >"$tmp/nobuild/er-thing.log"
	ER_ME3_RUN_ROOT="$tmp/nobuild" expect 1 "a log with no build line is not evidence" \
		evidence_for "deadbeef1234"

	rm -rf "$tmp"
	if [ "$failures" -eq 0 ]; then
		printf 'check-runtime-evidence selftest: PASS\n'
		return 0
	fi
	printf 'check-runtime-evidence selftest: %d failure(s)\n' "$failures"
	return 1
}

main() {
	if [ "${1:-}" = "--selftest" ]; then
		selftest
		return $?
	fi

	command -v git >/dev/null 2>&1 || return 0
	git rev-parse --git-dir >/dev/null 2>&1 || return 0

	local tip
	tip="$(git rev-parse --short HEAD 2>/dev/null)" || return 0
	[ -n "$tip" ] || return 0

	# Only code that ends up inside the game can be proven by a run.
	local changed
	if git rev-parse --verify --quiet refs/remotes/origin/main >/dev/null 2>&1; then
		changed="$(git diff --name-only refs/remotes/origin/main...HEAD 2>/dev/null)"
	else
		changed="$(git show --name-only --format= HEAD 2>/dev/null)"
	fi
	if ! printf '%s\n' "$changed" | grep -q '^crates/'; then
		return 0
	fi

	local note rc
	note="$(evidence_for "$tip")"
	rc=$?

	if [ "$rc" -eq 0 ]; then
		printf 'pre-push: runtime evidence for %s -- %s\n' "$tip" "$note" >&2
		return 0
	fi
	if [ "$rc" -eq 2 ]; then
		printf 'pre-push: runtime evidence unmeasurable for %s -- %s. Allowing.\n' "$tip" "$note" >&2
		return 0
	fi

	if [ "${ER_ALLOW_UNPROVEN_PUSH:-}" = "1" ]; then
		printf 'pre-push: pushing unproven code by ER_ALLOW_UNPROVEN_PUSH=1 -- %s\n' "$note" >&2
		return 0
	fi

	{
		printf '\n'
		printf 'pre-push: REFUSING -- this push changes code under crates/ that has never run.\n'
		printf '  tip       %s\n' "$tip"
		printf '  evidence  %s\n' "$note"
		printf '\n'
		printf '  A DLL log names the commit it was built from on its first line, and no log names\n'
		printf '  this one. Build it, launch it, and let the DLL write:\n'
		printf '\n'
		printf '    bash scripts/er-build-dlls.sh --all\n'
		printf '    python3 scripts/er-run-branch.py --save <save>:<slot>\n'
		printf '\n'
		printf '  Or push a commit that does not change game code.\n'
		printf '  Deliberate override, which says so in the log: ER_ALLOW_UNPROVEN_PUSH=1\n'
		printf '\n'
	} >&2
	return 1
}

main "$@"
