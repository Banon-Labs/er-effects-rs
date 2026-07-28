#!/usr/bin/env bash
# Launch Elden Ring through approved direct/offline ME3 with only er_better_refills.dll loaded,
# then immediately leave the game open for manual QA.
# This script deliberately does NOT install an EXIT trap that kills the run: the live process is
# handed to the user, and the printed teardown command is the explicit cleanup trail. Hook-active
# waiting defaults to 0 because readiness proof must not block a user-inspection handoff.
set -euo pipefail

REPO_ROOT="${REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
GAME_DIR="${GAME_DIR:-/mnt/c/SteamLibrary/steamapps/common/ELDEN RING/Game}"
# shellcheck source=scripts/me3-launch-lib.sh
# shellcheck disable=SC1091
source "$REPO_ROOT/scripts/me3-launch-lib.sh"

ARTIFACT_DIR="${ARTIFACT_DIR:-$REPO_ROOT/target/runtime-probe/better-refills-user-qa-$(date +%Y%m%d-%H%M%S)}"
BUILT_DLL="${BUILT_DLL:-$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_better_refills.dll}"
ME3_PROFILE="${ME3_PROFILE:-$ARTIFACT_DIR/better-refills-user-qa.me3}"
PID_FILE="${PID_FILE:-$ARTIFACT_DIR/me3-launch.pid}"
GAME_LOG="${GAME_LOG:-$GAME_DIR/er-better-refills.log}"
CRASH_LOG="${CRASH_LOG:-$GAME_DIR/er-better-refills-crash-log.txt}"
ARTIFACT_LOG="${ARTIFACT_LOG:-$ARTIFACT_DIR/er-better-refills.log}"
ARTIFACT_CRASH_LOG="${ARTIFACT_CRASH_LOG:-$ARTIFACT_DIR/er-better-refills-crash-log.txt}"
HOOK_READY_TIMEOUT_SECONDS="${HOOK_READY_TIMEOUT_SECONDS:-${MAX_WAIT_SECONDS:-0}}"

fatal() {
	echo "better-refills-user-qa: $*" >&2
	exit 1
}

runtime_pids() {
	local proc pid comm cmdline
	for proc in /proc/[0-9]*; do
		pid=${proc##*/}
		[[ -r "$proc/comm" ]] || continue
		comm=$(tr -d '\0' 2>/dev/null <"$proc/comm") || continue
		if [[ "$comm" == "eldenring.exe" ]]; then
			printf '%s\n' "$pid"
			continue
		fi
		[[ -r "$proc/cmdline" ]] || continue
		cmdline=$(tr '\0' ' ' 2>/dev/null <"$proc/cmdline") || continue
		if [[ "$cmdline" == *"$GAME_DIR/eldenring.exe"* ]]; then
			printf '%s\n' "$pid"
			continue
		fi
		if [[ "$cmdline" == *"ELDEN RING\\Game\\eldenring.exe"* ]]; then
			printf '%s\n' "$pid"
			continue
		fi
		if [[ "$cmdline" == *"windows-bin/me3-launcher.exe"* || "$cmdline" == *"windows-bin\\me3-launcher.exe"* ]]; then
			printf '%s\n' "$pid"
		fi
	done
}

win_pids_for() {
	command -v tasklist.exe >/dev/null 2>&1 || return 0
	tasklist.exe /FI "IMAGENAME eq $1" /FO CSV /NH 2>/dev/null |
		python3 -c "import csv,sys; print(' '.join(r[1] for r in csv.reader(sys.stdin) if len(r)>1 and r[1].isdigit()))"
}

wait_for_hook_active() {
	local deadline
	deadline=$((SECONDS + HOOK_READY_TIMEOUT_SECONDS))
	while ((SECONDS < deadline)); do
		if timeout 1 grep --line-buffered -m1 "SetItemReplenishState hook ACTIVE" < <(tail -n +1 -F -- "$GAME_LOG" 2>/dev/null) >"$ARTIFACT_DIR/hook-active.log"; then
			return 0
		fi
	done
	return 1
}

preflight() {
	[[ "$HOOK_READY_TIMEOUT_SECONDS" =~ ^[0-9]+$ ]] || fatal "HOOK_READY_TIMEOUT_SECONDS must be an integer"
	((HOOK_READY_TIMEOUT_SECONDS >= 0 && HOOK_READY_TIMEOUT_SECONDS <= 300)) || fatal "HOOK_READY_TIMEOUT_SECONDS must be 0..300"
	[[ -f "$BUILT_DLL" ]] || fatal "built DLL not found: $BUILT_DLL"
	[[ -d "$GAME_DIR" ]] || fatal "GAME_DIR not found: $GAME_DIR"
	bash "$REPO_ROOT/scripts/steam-running.sh" >/dev/null || fatal "Steam is not running according to scripts/steam-running.sh"
	me3_preflight
	me3_require_no_lazyloader "$GAME_DIR"
	if [[ -n "$(runtime_pids)" ]]; then
		fatal "Elden Ring/me3 runtime process already exists; refusing to launch over it"
	fi
	local existing_windows_pids
	existing_windows_pids="$(win_pids_for eldenring.exe)$(win_pids_for me3.exe)$(win_pids_for me3-launcher.exe)"
	if [[ -n "$existing_windows_pids" ]]; then
		fatal "Windows Elden Ring/me3 process already exists; refusing to launch over it"
	fi
}

launch_me3_detached() {
	local launch_steam_dir launch_profile_path
	launch_steam_dir=$(me3_to_host_path "$ME3_STEAM_DIR")
	launch_profile_path=$(me3_to_host_path "$ME3_PROFILE")
	nohup "$ME3_BIN" --steam-dir "$launch_steam_dir" launch -g eldenring -p "$launch_profile_path" \
		>"$ARTIFACT_DIR/me3-launch.stdout.log" 2>"$ARTIFACT_DIR/me3-launch.stderr.log" &
	echo $! >"$PID_FILE"
}

preflight
mkdir -p "$ARTIFACT_DIR"
cp -f "$BUILT_DLL" "$ARTIFACT_DIR/er_better_refills.dll"
rm -f "$GAME_LOG" "$CRASH_LOG" "$ARTIFACT_LOG" "$ARTIFACT_CRASH_LOG"
me3_write_profile "$ME3_PROFILE" "$ARTIFACT_DIR/er_better_refills.dll"

launch_me3_detached

echo "better-refills-user-qa: ARTIFACT_DIR=$ARTIFACT_DIR"
echo "better-refills-user-qa: ME3_PROFILE=$ME3_PROFILE"
echo "better-refills-user-qa: DLL=$ARTIFACT_DIR/er_better_refills.dll"
echo "better-refills-user-qa: launch_pid=$(<"$PID_FILE")"

if ((HOOK_READY_TIMEOUT_SECONDS > 0)); then
	echo "better-refills-user-qa: optional hook proof requested; waiting up to ${HOOK_READY_TIMEOUT_SECONDS}s for hook ACTIVE log"
	if ! wait_for_hook_active; then
		cp -f "$GAME_LOG" "$ARTIFACT_LOG" 2>/dev/null || true
		cp -f "$CRASH_LOG" "$ARTIFACT_CRASH_LOG" 2>/dev/null || true
		echo "better-refills-user-qa: FAIL hook-active log not observed within ${HOOK_READY_TIMEOUT_SECONDS}s" >&2
		echo "better-refills-user-qa: artifacts=$ARTIFACT_DIR" >&2
		exit 1
	fi
	cp -f "$GAME_LOG" "$ARTIFACT_LOG"
	cp -f "$CRASH_LOG" "$ARTIFACT_CRASH_LOG" 2>/dev/null || true
	echo "better-refills-user-qa: PASS hook installed; game left running for user QA"
	echo "better-refills-user-qa: log=$ARTIFACT_LOG"
else
	echo "better-refills-user-qa: hook readiness wait skipped (HOOK_READY_TIMEOUT_SECONDS=0); game handed off immediately"
	echo "better-refills-user-qa: inspect later: $GAME_LOG"
fi
echo "better-refills-user-qa: live_wsl_pids=$(runtime_pids | tr '\n' ' ')"
echo "better-refills-user-qa: live_windows_eldenring_pids=$(win_pids_for eldenring.exe)"
echo "better-refills-user-qa: live_windows_me3_pids=$(win_pids_for me3.exe) $(win_pids_for me3-launcher.exe)"
echo "better-refills-user-qa: teardown: taskkill.exe /F /PID <listed spawned Windows PID(s)>"
