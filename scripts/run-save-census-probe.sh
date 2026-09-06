#!/usr/bin/env bash
# Agent-owned census probe: run ELDEN RING with ONLY the save-disable DLL loaded and
# record every call site that touches save data on disk.
#
# Split into two fast subcommands so neither exceeds the repo's 30s cap on non-game
# operations; the GAME portion is bounded separately by the caller against
# .auto/runtime_timeout_cap_seconds.
#
#   bash scripts/run-save-census-probe.sh start   # preflight, snapshot, launch
#   bash scripts/run-save-census-probe.sh finish  # snapshot, verdict, teardown
#   bash scripts/run-save-census-probe.sh resolve <name> <game-dir> <artifact-dir> <launch-epoch>
#                                                 # which file is THIS run's copy of <name>
#
# `start` prints the artifact directory; `finish` reuses the most recent one unless
# ARTIFACT_DIR is set. Env overrides: ME3_STEAM_DIR, GAME_EXE, ME3_BIN, ARTIFACT_DIR.
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO" || exit 2

ME3_BIN="${ME3_BIN:-me3}"
ME3_STEAM_DIR="${ME3_STEAM_DIR:-$HOME/.local/share/Steam}"
GAME_EXE="${GAME_EXE:-$ME3_STEAM_DIR/steamapps/common/ELDEN RING/Game/eldenring.exe}"
PROBE_ROOT="$REPO/target/runtime-probe"
LATEST_LINK="$PROBE_ROOT/save-census-latest"

TELEMETRY_NAME="er-save-disable-telemetry.json"
DLL_LOG_NAME="er-save-disable.log"
HARNESS_LOG_NAME="er-input-harness.log"
HARNESS_PHASES_NAME="er-input-harness-phases.jsonl"

resolve_artifact_dir() {
	if [[ -n "${ARTIFACT_DIR:-}" ]]; then
		echo "$ARTIFACT_DIR"
	elif [[ -L "$LATEST_LINK" || -d "$LATEST_LINK" ]]; then
		readlink -f "$LATEST_LINK"
	else
		echo ""
	fi
}

# resolve_run_artifact <name> <game_dir> <artifact_dir> <launch_epoch>
#
# THIS RUN's copy of <name>: the redirect if the DLL honoured it, otherwise the game-directory
# fallback -- and never a file older than the launch. Both halves are load-bearing.
#
# The redirect is set from this script's side, but the DLL only obeys it if the env survives
# me3 -> Proton; when it does not, it writes into the game directory rather than nowhere, and a
# reader that knows only the run directory would report a healthy run as SILENT.
#
# `newer_than` is the other half, and it is what makes it safe to have stopped deleting the game
# directory's copy. That copy is the PREVIOUS run's file, complete and readable, so a reader that
# resolves by existence alone binds to it and scores a finished run as this one's. A stale file's
# mtime never changes either, so a freshness check downstream calls a perfectly healthy game
# frozen. The floor makes an old candidate unresolvable, and the honest answer -- the run
# directory's not-yet-written path -- is what is returned instead.
#
# The `env VAR=... "$ME3_BIN"` prefix used at launch does NOT put the redirects in this script's
# own environment, so `prefer` is passed explicitly: the artifact directory is what this script
# knows first-hand, and it beats anything inherited.
resolve_run_artifact() {
	python3 - "$REPO" "$1" "$2" "$3" "$4" <<-'PY'
		import os, sys

		repo, name, game_dir, artifact_dir, launch_epoch = sys.argv[1:6]
		sys.path.insert(0, os.path.join(repo, "scripts"))
		from er_artifact_env import resolve_artifact  # noqa: E402 - repo-local, path set above

		print(resolve_artifact(name, game_dir, prefer=artifact_dir, newer_than=float(launch_epoch)))
	PY
}

preflight() {
	# Sanctioned Steam check: raw pgrep false-negatives on this setup and is guard-blocked.
	# shellcheck source=/dev/null
	source "$REPO/scripts/steam-running.sh"
	if ! steam_running; then
		echo "PREFLIGHT FAIL: Steam is not running. The offline eldenring.exe launch reuses" >&2
		echo "Steam's environment (wineprefix, save dir, account id); with Steam down the run" >&2
		echo "is not representative. Start Steam and retry." >&2
		exit 1
	fi
	if [[ ! -f "$GAME_EXE" ]]; then
		echo "PREFLIGHT FAIL: game executable not found: $GAME_EXE" >&2
		exit 1
	fi
	if ! command -v "$ME3_BIN" >/dev/null 2>&1; then
		echo "PREFLIGHT FAIL: me3 not found (ME3_BIN=$ME3_BIN)" >&2
		exit 1
	fi
}

cmd_start() {
	preflight
	local game_dir stamp artifact profile
	game_dir="$(cd -- "$(dirname -- "$GAME_EXE")" && pwd)"
	stamp="$(date +%Y%m%d-%H%M%S)"
	artifact="$PROBE_ROOT/save-census-$stamp"
	mkdir -p "$artifact" || exit 1

	if ! bash "$REPO/scripts/build-save-census-profile.sh" >"$artifact/build.log" 2>&1; then
		echo "BUILD FAIL -- see $artifact/build.log" >&2
		exit 1
	fi
	profile="$REPO/target/save-census/save-census.me3"

	# NOTHING IS CLEARED FROM THE GAME DIRECTORY ANY MORE, so nothing needs rescuing from it
	# either. Every artifact this probe reads is redirected into `$artifact` below, and the reader
	# resolves with a `newer_than` floor, so a leftover in `$game_dir` can no longer be mistaken
	# for this run's evidence -- which is all the old sweep bought. The census pair is off this
	# list for that reason; the harness pair stays only for the case where the env does not survive
	# me3 -> Proton, the DLL falls back to `$game_dir`, and the previous run's copy is about to be
	# rotated to `.prev` by ours.
	local carried="$artifact/carried-over-from-previous-run"
	local carried_any=0
	for stale in "$HARNESS_LOG_NAME" "$HARNESS_PHASES_NAME"; do
		if [[ -s "$game_dir/$stale" ]]; then
			mkdir -p "$carried"
			cp -f "$game_dir/$stale" "$carried/$stale" 2>/dev/null && carried_any=1
		fi
	done
	if [[ "$carried_any" == "1" ]]; then
		echo "NOTE: a previous run left un-finished artifacts; archived to $carried" >&2
	fi
	# NOTHING IS DELETED FROM THE GAME DIRECTORY HERE. This used to be
	#     rm -f "$game_dir/$TELEMETRY_NAME" "$game_dir/$TELEMETRY_NAME".tmp* "$game_dir/$DLL_LOG_NAME"
	#     rm -f "$game_dir/$HARNESS_LOG_NAME" "$game_dir/$HARNESS_PHASES_NAME"
	# so the files read at `finish` were guaranteed to be this run's. It destroyed two runs at a
	# time: the live file, and -- because `er_game_base::log::begin_fresh_run` removes a stale
	# `<name>.prev` unconditionally when the live file is absent -- the generation behind it.
	# Neither was this run's, since several sessions launch concurrently here. The census telemetry
	# was the worst of them, being the run-stopping ORACLE of a suppression proof and kept in ZERO
	# copies (it publishes tmp-then-rename, so there is no `.prev` at all).
	#
	# The freshness those deletes bought comes from the redirect instead: both files are written
	# into `$artifact` (below), a directory no other run has ever touched, and `finish` resolves
	# them by existence with a `newer_than` floor of this launch, so it can never bind to a
	# leftover in `$game_dir`. The `.tmp.<serial>` publish files follow the same redirect.

	# The harness reads its drive mode from a CWD-relative flag file in the game dir.
	# `full` is boot -> PRESS ANY BUTTON -> Continue -> in-world -> System->Quit, which is
	# what reaches the return-to-title save; booting alone only ever shows the system-slot save.
	if [[ "${WITH_INPUT_HARNESS:-0}" == "1" ]]; then
		printf '%s\n' "${HARNESS_DRIVE_MODE:-full}" >"$game_dir/er-harness-drive-mode.txt"
		# Take the NativeQuit branch of `full`: Startup -> PressAnyButton -> Continue ->
		# WaitLoadIn -> NativeQuit -> QuitTeardown -> PressAnyButton -> Continue -> WaitLoadIn.
		# The menu-driven default would navigate the in-world pause menu with injected input,
		# which does not reliably reach the Scaleform menu; the native path invokes the quit
		# job's two slots directly instead (save request, then return-to-title).
		printf 'save-census: drive System->Quit natively, not by menu input\n' \
			>"$game_dir/er-harness-native-quit.txt"
	else
		rm -f "$game_dir/er-harness-drive-mode.txt" "$game_dir/er-harness-native-quit.txt"
	fi

	python3 "$REPO/scripts/save-write-witness.py" snapshot --out "$artifact/before.json" \
		>"$artifact/before.log" 2>&1 || exit 1

	# THE LAUNCH CLOCK, WHICH `finish` CANNOT DO WITHOUT. The game-directory copy of both census
	# files is now the PREVIOUS run's, sitting there with its final contents because nothing
	# deletes it any more. A reader that resolves by existence alone binds to it and scores a run
	# that has already ended as this one's -- and a stale file's mtime never changes, so a
	# freshness check downstream would call a perfectly healthy game frozen. `finish` therefore
	# refuses any candidate older than this timestamp.
	local launch_epoch
	launch_epoch="$(date +%s)"

	# Shell-quoted: the game directory is "ELDEN RING" -- an unquoted value here makes
	# `source` split it and silently leaves game_dir unset in `finish`.
	{
		printf 'game_dir=%q\n' "$game_dir"
		printf 'profile=%q\n' "$profile"
		printf 'launch_epoch=%q\n' "$launch_epoch"
		# The redirect targets, not the game directory: that is where the DLL is being told to
		# write, and recording the old fixed path here made this file describe the bug.
		printf 'telemetry=%q\n' "$artifact/$TELEMETRY_NAME"
		printf 'dll_log=%q\n' "$artifact/$DLL_LOG_NAME"
	} >"$artifact/run-context.env"

	ln -sfn "$artifact" "$LATEST_LINK"

	(
		cd "$game_dir" || exit 1
		# EVERY per-run artifact goes into THIS run's directory. A GAME_DIR artifact is
		# SINGLE-SLOT -- the DLL rotates `<name>` to `<name>.prev` on its first write -- so two
		# launches lose the run before last, and several sessions launch concurrently here. A copy
		# at `finish` cannot fix that (this run clobbered the last one's file at LAUNCH) and never
		# runs at all when the game crashes or the operator walks away, which is exactly the case
		# the `carried-over-from-previous-run` archive above exists to mop up.
		nohup env ER_QUICKLOAD_SAVE_MODE_HINT=vanilla \
			ER_QUICKLOAD_TELEMETRY_PATH="$artifact/er-quickload-telemetry.json" \
			ER_QUICKLOAD_AUTOLOAD_DEBUG_PATH="$artifact/er-quickload-autoload-debug.log" \
			ER_QUICKLOAD_CRASH_LOG_PATH="$artifact/er-quickload-crash-log.txt" \
			ER_QUICKLOAD_TRACE_CONTINUE_PATH="$artifact/er-quickload-continue-trace.log" \
			ER_QUICKLOAD_INPUT_TRACE_PATH="$artifact/er-quickload-input-trace.jsonl" \
			ER_QUICKLOAD_BOOTSTRAP_PATH="$artifact/er-quickload-bootstrap.jsonl" \
			ER_QUICKLOAD_BOOTSTRAP_STATE_PATH="$artifact/er-quickload-bootstrap-state.json" \
			ER_QUICKLOAD_PROFILE_PATH="$artifact/er-quickload-profile.jsonl" \
			ER_QUICKLOAD_RELOAD_TRACE_PATH="$artifact/er-reload-trace.log" \
			ER_QUICKLOAD_DIAG_HARNESS_PATH="$artifact/er-diag-harness.log" \
			ER_QUICKLOAD_TIMESERIES_PATH="$artifact/er-telemetry-timeseries.jsonl" \
			ER_QUICKLOAD_CPU_PROFILE_PATH="$artifact/er-cpu-profile.txt" \
			ER_QUICKLOAD_ARMAMENT_ICONS_PATH="$artifact/er-armament-icons.log" \
			ER_QUICKLOAD_CRASH_LOGGING_LOG_PATH="$artifact/er-crash-log.txt" \
			ER_QUICKLOAD_CRASH_LOGGING_LATEST_PATH="$artifact/er-crash-latest.txt" \
			ER_QUICKLOAD_CRASH_LOGGING_BREADCRUMB_PATH="$artifact/er-crash-breadcrumb-latest.txt" \
			ER_QUICKLOAD_CRASH_LOGGING_MODULES_PATH="$artifact/er-crash-modules.txt" \
			ER_QUICKLOAD_SAVE_DISABLE_LOG_PATH="$artifact/$DLL_LOG_NAME" \
			ER_QUICKLOAD_SAVE_DISABLE_TELEMETRY_PATH="$artifact/$TELEMETRY_NAME" \
			ER_QUICKLOAD_LOADING_PORTRAIT_PATH="$artifact/er-loading-portrait.log" \
			ER_QUICKLOAD_LOADING_PORTRAIT_CRASH_LOG_PATH="$artifact/er-loading-portrait-crash-log.txt" \
			ER_QUICKLOAD_CRASH_LOGGING_LOG_PATH="$artifact/er-crash-log.txt" \
			ER_QUICKLOAD_CRASH_LOGGING_LATEST_PATH="$artifact/er-crash-latest.txt" \
			ER_QUICKLOAD_CRASH_LOGGING_BREADCRUMB_PATH="$artifact/er-crash-breadcrumb-latest.txt" \
			ER_QUICKLOAD_CRASH_LOGGING_MODULES_PATH="$artifact/er-crash-modules.txt" \
			ER_QUICKLOAD_INPUT_HARNESS_LOG_PATH="$artifact/$HARNESS_LOG_NAME" \
			ER_QUICKLOAD_INPUT_HARNESS_PHASES_PATH="$artifact/$HARNESS_PHASES_NAME" \
			"$ME3_BIN" \
			--steam-dir "$ME3_STEAM_DIR" launch \
			-p "$profile" -g eldenring -e "$GAME_EXE" \
			>"$artifact/me3.log" 2>&1 &
		echo $! >"$artifact/me3.pid"
	)

	echo "== census probe started =="
	echo "artifact:  $artifact"
	# Where the DLL was TOLD to write. The game-directory copy of either name is the previous
	# run's, and printing it here is how an operator ends up tailing somebody else's census.
	echo "telemetry: $artifact/$TELEMETRY_NAME"
	echo "dll log:   $artifact/$DLL_LOG_NAME"
	echo "me3 log:   $artifact/me3.log"
}

cmd_finish() {
	local artifact game_dir
	artifact="$(resolve_artifact_dir)"
	if [[ -z "$artifact" || ! -d "$artifact" ]]; then
		echo "no census artifact directory found; set ARTIFACT_DIR" >&2
		exit 2
	fi
	# shellcheck source=/dev/null
	source "$artifact/run-context.env"

	# RESOLVE, DO NOT COPY-THEN-HOPE. The unconditional `cp -f "$game_dir/..." "$artifact/..."`
	# that used to stand here was actively destructive once the redirect landed: it overwrote the
	# census this run had just written into `$artifact` with the PREVIOUS run's game-directory
	# copy, quietly swapping the verdict for somebody else's. Every candidate is resolved against
	# the launch clock instead, so a leftover can never win.
	local census_telemetry census_log resolved
	census_telemetry="$(resolve_run_artifact "$TELEMETRY_NAME" "$game_dir" "$artifact" "${launch_epoch:-0}")"
	census_log="$(resolve_run_artifact "$DLL_LOG_NAME" "$game_dir" "$artifact" "${launch_epoch:-0}")"
	# FALLBACK ONLY, and it copies INTO the run directory, never over it: when the env did not
	# survive me3 -> Proton the DLL wrote into the game directory, and this is the one chance to
	# bring a copy home. The source is left exactly where it is -- it is the next run's `.prev`.
	for resolved in "$census_telemetry" "$census_log" \
		"$(resolve_run_artifact "$HARNESS_LOG_NAME" "$game_dir" "$artifact" "${launch_epoch:-0}")" \
		"$(resolve_run_artifact "$HARNESS_PHASES_NAME" "$game_dir" "$artifact" "${launch_epoch:-0}")"; do
		if [[ -f "$resolved" && "$(dirname "$resolved")" != "$artifact" ]]; then
			cp -f "$resolved" "$artifact/$(basename "$resolved")"
		fi
	done

	python3 "$REPO/scripts/save-write-witness.py" snapshot --out "$artifact/after.json" \
		>"$artifact/after.log" 2>&1

	echo "== census verdict =="
	echo "telemetry: $census_telemetry"
	echo "dll log:   $census_log"
	python3 "$REPO/scripts/check-save-suppression.py" \
		--telemetry "$census_telemetry" \
		--before "$artifact/before.json" \
		--after "$artifact/after.json" | tee "$artifact/verdict.txt"
	local verdict_status=${PIPESTATUS[0]}

	echo
	echo "artifact: $artifact"
	exit "$verdict_status"
}

case "${1:-}" in
start) cmd_start ;;
finish) cmd_finish ;;
# Which file is THIS run's copy of <name>. Exposed as a subcommand for two reasons: an operator
# asking "where did the census actually land" gets the same answer `finish` will use, and the
# resolution -- the half of the redirect that decides whether a verdict is drawn from this run or
# the last one -- becomes testable without a game. `er-artifact-redirect-audit.py --selftest`
# drives it across all four states (redirect pending, redirect live, env-lost fallback, stale only).
resolve)
	shift
	if (($# != 4)); then
		echo "usage: $0 resolve <artifact-name> <game-dir> <artifact-dir> <launch-epoch>" >&2
		exit 2
	fi
	resolve_run_artifact "$1" "$2" "$3" "$4"
	;;
*)
	echo "usage: $0 {start|finish|resolve <name> <game-dir> <artifact-dir> <launch-epoch>}" >&2
	exit 2
	;;
esac
