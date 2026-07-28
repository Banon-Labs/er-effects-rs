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

	# Stale artifacts from a previous run must not be read as this run's evidence -- but they
	# must not be DESTROYED either. A previous run whose `finish` never ran (killed, crashed, or
	# still being driven by hand) has its only in-process census sitting in the game directory;
	# deleting it silently threw away real measurements. Archive, then clear.
	local carried="$artifact/carried-over-from-previous-run"
	local carried_any=0
	for stale in "$TELEMETRY_NAME" "$DLL_LOG_NAME" "$HARNESS_LOG_NAME" "$HARNESS_PHASES_NAME"; do
		if [[ -s "$game_dir/$stale" ]]; then
			mkdir -p "$carried"
			cp -f "$game_dir/$stale" "$carried/$stale" 2>/dev/null && carried_any=1
		fi
	done
	if [[ "$carried_any" == "1" ]]; then
		echo "NOTE: a previous run left un-finished artifacts; archived to $carried" >&2
	fi
	# Glob the tmp files: each publish now uses a unique suffix
	# (er-save-disable-telemetry.json.tmp.<serial>) so concurrent publishers on the
	# install/game/worker threads cannot truncate each other's file mid-rename. The old
	# single-name sweep would leave every one of them behind.
	rm -f "$game_dir/$TELEMETRY_NAME" "$game_dir/$TELEMETRY_NAME".tmp* "$game_dir/$DLL_LOG_NAME"
	rm -f "$game_dir/$HARNESS_LOG_NAME" "$game_dir/$HARNESS_PHASES_NAME"

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

	# Shell-quoted: the game directory is "ELDEN RING" -- an unquoted value here makes
	# `source` split it and silently leaves game_dir unset in `finish`.
	{
		printf 'game_dir=%q\n' "$game_dir"
		printf 'profile=%q\n' "$profile"
		printf 'telemetry=%q\n' "$game_dir/$TELEMETRY_NAME"
		printf 'dll_log=%q\n' "$game_dir/$DLL_LOG_NAME"
	} >"$artifact/run-context.env"

	ln -sfn "$artifact" "$LATEST_LINK"

	(
		cd "$game_dir" || exit 1
		nohup env ER_EFFECTS_SAVE_MODE_HINT=vanilla "$ME3_BIN" \
			--steam-dir "$ME3_STEAM_DIR" launch \
			-p "$profile" -g eldenring -e "$GAME_EXE" \
			>"$artifact/me3.log" 2>&1 &
		echo $! >"$artifact/me3.pid"
	)

	echo "== census probe started =="
	echo "artifact:  $artifact"
	echo "telemetry: $game_dir/$TELEMETRY_NAME"
	echo "dll log:   $game_dir/$DLL_LOG_NAME"
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

	# Preserve the in-process evidence before teardown: the DLL writes into the game
	# directory, which a later run clears.
	cp -f "$game_dir/$TELEMETRY_NAME" "$artifact/$TELEMETRY_NAME" 2>/dev/null
	cp -f "$game_dir/$DLL_LOG_NAME" "$artifact/$DLL_LOG_NAME" 2>/dev/null
	cp -f "$game_dir/$HARNESS_LOG_NAME" "$artifact/$HARNESS_LOG_NAME" 2>/dev/null
	cp -f "$game_dir/$HARNESS_PHASES_NAME" "$artifact/$HARNESS_PHASES_NAME" 2>/dev/null

	python3 "$REPO/scripts/save-write-witness.py" snapshot --out "$artifact/after.json" \
		>"$artifact/after.log" 2>&1

	echo "== census verdict =="
	python3 "$REPO/scripts/check-save-suppression.py" \
		--telemetry "$artifact/$TELEMETRY_NAME" \
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
*)
	echo "usage: $0 {start|finish}" >&2
	exit 2
	;;
esac
