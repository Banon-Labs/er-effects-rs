#!/usr/bin/env bash
# Launch a named me3 profile on the game's OWN default save, with every DLL artifact redirected
# into a run directory `scripts/check-runtime-evidence.sh` reads.
#
#     bash scripts/er-run-default-save.sh ~/Elden/quicksave.me3
#     bash scripts/er-run-default-save.sh --selftest
#
# Why this exists beside `er-run-branch.py`, which does far more
# -------------------------------------------------------------
# That tool picks a save out of a corpus and stages it privately, which is the right shape for a
# reproducible branch run. It cannot launch the DEFAULT save, and it refuses rather than pretend
# otherwise: `er-gen-me3-profile` will not name the game-owned APPDATA container as a `save_file`,
# because the redirect stage it would create lands inside the directory the redirect matches on.
#
# But the default save is exactly what AGENTS.md's 2026-07-08 order prescribes for release and
# autoload proof -- the user method, `~/Elden/launch.sh` with the real APPDATA save and no
# redirect. Run that way, the DLL logs land in the GAME directory, which the push gate does not
# read, so a run that really did execute the pushed commit leaves no evidence the gate can find.
# That is the whole gap this closes: same launch, artifacts somewhere both the user and the gate
# can see (bd er-effects-rs-rhqv).
#
# What it does NOT do, on purpose: pick or stage a save, write the game-directory
# `er-quickload.toml`, or reap anything. The save is the game's and stays the game's.
set -uo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
run_root="${ER_ME3_RUN_ROOT:-$HOME/.cache/er-me3-runs}"
launch_sh="${ER_LAUNCH_SH:-$HOME/Elden/launch.sh}"

# The artifact knobs, read from the one place that defines them rather than copied. A knob added
# there and not here would silently leave that shell's log in the game directory.
artifact_env_pairs() {
	python3 - "$repo_root" "$1" <<'PY'
import sys
sys.path.insert(0, f"{sys.argv[1]}/scripts")
from er_artifact_env import ARTIFACT_ENV

for var, filename in sorted(ARTIFACT_ENV.items()):
    print(f"{var}={sys.argv[2]}/{filename}")
PY
}

usage() {
	printf 'usage: %s <profile.me3>\n       %s --selftest\n' "$0" "$0" >&2
}

selftest() {
	local tmp status=0
	tmp="$(mktemp -d)"
	local pairs
	pairs="$(artifact_env_pairs "$tmp/run")" || {
		printf 'selftest: could not read ARTIFACT_ENV\n' >&2
		return 1
	}
	local count
	count="$(printf '%s\n' "$pairs" | grep -c '=')"
	if [ "$count" -lt 10 ]; then
		printf 'selftest: expected the full artifact knob set, got %s\n' "$count" >&2
		status=1
	fi
	# The evidence gate globs `er-*.log` inside a run directory, so at least one redirected
	# filename has to match that glob or a run leaves nothing the gate can find.
	if ! printf '%s\n' "$pairs" | grep -q "$tmp/run/er-.*\.log$"; then
		printf 'selftest: no redirected filename matches the evidence gate glob er-*.log\n' >&2
		status=1
	fi
	# Every value must sit under the run directory; a knob that kept an absolute default would
	# still write into the game directory.
	if printf '%s\n' "$pairs" | grep -v "=$tmp/run/" | grep -q '='; then
		printf 'selftest: a knob resolved outside the run directory\n' >&2
		status=1
	fi
	if [ -z "${ER_SELFTEST_SKIP_LAUNCHER:-}" ] && [ ! -x "$launch_sh" ]; then
		printf 'selftest: launcher %s is not executable\n' "$launch_sh" >&2
		status=1
	fi
	rm -rf "$tmp"
	[ "$status" -eq 0 ] && printf 'selftest: ok\n'
	return "$status"
}

main() {
	if [ "${1:-}" = "--selftest" ]; then
		selftest
		return
	fi
	local profile="${1:-}"
	if [ -z "$profile" ]; then
		usage
		return 2
	fi
	if [ ! -f "$profile" ]; then
		printf 'refusing: no profile at %s\n' "$profile" >&2
		return 2
	fi
	if [ ! -x "$launch_sh" ]; then
		printf 'refusing: no launcher at %s (set ER_LAUNCH_SH)\n' "$launch_sh" >&2
		return 2
	fi
	local run_id run_dir
	run_id="br-$(date -u +%Y%m%d-%H%M%S)-default"
	run_dir="$run_root/$run_id"
	mkdir -p "$run_dir" || return 1
	local pairs
	pairs="$(artifact_env_pairs "$run_dir")" || return 1
	printf 'er-run-default-save: profile  %s\n' "$profile"
	printf 'er-run-default-save: save     the game-owned default APPDATA container (untouched by this script)\n'
	printf 'er-run-default-save: run dir  %s\n' "$run_dir"
	printf 'er-run-default-save: %s artifact knobs redirected there\n' "$(printf '%s\n' "$pairs" | grep -c '=')"
	# `env` rather than exporting into this shell: the redirect belongs to the launched process,
	# and leaking it into the caller's environment would silently move a later unrelated run.
	# shellcheck disable=SC2046  # word splitting is the point: one VAR=VALUE argument per knob.
	setsid env $(printf '%s\n' "$pairs") ME3_PROFILE="$profile" "$launch_sh" \
		>"$run_dir/launch.log" 2>&1 &
	local pid=$!
	printf 'er-run-default-save: launched pid %s -- launcher output in %s/launch.log\n' "$pid" "$run_dir"
	printf 'er-run-default-save: the DLL names the commit it was built from on the first line of\n'
	printf '                     %s/er-quickload-autoload-debug.log\n' "$run_dir"
}

main "$@"
