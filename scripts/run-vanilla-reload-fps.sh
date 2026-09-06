#!/usr/bin/env bash
# Vanilla-reload FPS comparison (2026-07-22, bd USER-chose-vanilla-reload-comparison).
# Loads ONLY the telemetry-only DLL (er_telemetry -- no product hooks, no reload driver, no
# autopilot), launches offline ER LIVE for the USER to drive, and polls er-telemetry-standalone.json to
# a timeseries. The USER drives: title -> Continue (loads angrE = the BOOT-equivalent load), play +
# walk forward, then System -> Quit to Title -> Continue (the RELOAD), play + walk forward ~3s. We then
# compare the game frame time (flip task_delta) between the boot-continue and the reload -- to isolate
# whether OUR reload path (own_load_switch_reload_fire) causes the ~20fps game-side slowdown or it is
# inherent to game reloads in this WSLg/Proton env. No agent input/autopilot: the user owns the input.
set -uo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ARTIFACT_DIR="${ARTIFACT_DIR:-$REPO_ROOT/target/runtime-probe/vanilla-reload-fps-$(date +%Y%m%d-%H%M%S)}"
TELEM_DLL="$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_telemetry.dll"
HARNESS_DLL="$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_input_harness.dll"

fail() {
	echo "run-vanilla-reload-fps: $*" >&2
	exit 2
}

if [[ -z "${GAME_DIR:-}" ]]; then
	for c in \
		"/mnt/c/SteamLibrary/steamapps/common/ELDEN RING/Game" \
		"$HOME/.local/share/Steam/steamapps/common/ELDEN RING/Game"; do
		[[ -f "$c/eldenring.exe" ]] && {
			GAME_DIR="$c"
			break
		}
	done
fi
[[ -n "${GAME_DIR:-}" && -f "$GAME_DIR/eldenring.exe" ]] || fail "GAME_DIR not resolved."

# shellcheck source=scripts/steam-running.sh
# shellcheck disable=SC1091
source "$REPO_ROOT/scripts/steam-running.sh"
steam_running || fail "Steam is not running. Start Steam (interactive login) first."
# Fail closed if an ER is already running -- a second launch on top double-loads the DLLs and
# contaminates the run (observed 2026-07-22). tasklist.exe not resolving just yields no match (safe);
# do NOT guard on `command -v` (it failed in the script PATH and silently skipped this check).
if tasklist.exe 2>/dev/null | grep -qiE 'eldenring\.exe|start_protected_game\.exe'; then
	fail "An Elden Ring process is already running. Tear it down (taskkill.exe /F /IM eldenring.exe) before launching."
fi
# FRESHNESS, NOT EXISTENCE. The profile below points me3 straight at target/.../release, so
# "it exists" was never a statement about which code loads. This run's whole output is a frame-time
# COMPARISON against a product run, and a comparison drawn between two different builds of the
# telemetry DLL measures the build, not the reload path. Refuse rather than launch.
# shellcheck source=scripts/er-dll-freshness.sh
# shellcheck disable=SC1091
source "$REPO_ROOT/scripts/er-dll-freshness.sh"
require_fresh_dlls "$TELEM_DLL" "$HARNESS_DLL" ||
	fail "refusing to launch against DLLs that are not this source tree (see above)"

ME3="${ME3:-/mnt/c/Users/$USER/AppData/Local/garyttierney/me3/bin/me3.exe}"
[[ -f "$ME3" ]] || fail "Windows me3.exe not found at $ME3 (set ME3=<path to me3.exe>)"
mkdir -p "$ARTIFACT_DIR"
cp -f "$TELEM_DLL" "$GAME_DIR/er_telemetry.dll"
cp -f "$HARNESS_DLL" "$GAME_DIR/er_input_harness.dll"
# Redirected into ARTIFACT_DIR (see the launch below), so the timeseries this run reads is its own
# and starts empty by construction. It used to be the game directory's shared copy, cleared here --
# which destroyed another session's run, and its `.prev` with it.
TS_GAME="$ARTIFACT_DIR/er-telemetry-timeseries.jsonl"

winpath() { python3 -c "p='$1'; print((p[5].upper()+':\\\\'+p[7:].replace('/','\\\\')) if p.startswith('/mnt/') else p)"; }
WIN_TELEM="$(winpath "$GAME_DIR/er_telemetry.dll")"
WIN_HARNESS="$(winpath "$GAME_DIR/er_input_harness.dll")"
PROFILE="$ARTIFACT_DIR/vanilla-telemetry.me3"
cat >"$PROFILE" <<EOF
profileVersion = "v1"

[[supports]]
game = "eldenring"

[[natives]]
path = '$WIN_TELEM'

[[natives]]
path = '$WIN_HARNESS'
EOF

echo "======================================================================"
echo "== LAUNCHING ELDEN RING (offline, me3) -- VANILLA telemetry-only run =="
echo "==   telemetry DLL (fps) + input-harness DLL (drives NATIVE boot + reload via"
echo "==   direct input-memory injection -- NO product, NO user, NO mouse)"
echo "==   harness drives: title->Continue (BOOT) then System->Quit->Continue (RELOAD)"
echo "==   artifacts -> $ARTIFACT_DIR"
echo "======================================================================"

# EVERY per-run artifact goes into THIS run's directory. A GAME_DIR artifact is SINGLE-SLOT: the DLL
# rotates `<name>` to `<name>.prev` on its first write, so two launches lose the run before last,
# and several sessions launch concurrently here. A copy after the run cannot fix that -- by then
# this run has clobbered the previous one's file -- and a crashed run never reaches the copy.
env \
	ER_QUICKLOAD_TELEMETRY_PATH="$ARTIFACT_DIR/er-quickload-telemetry.json" \
	ER_QUICKLOAD_AUTOLOAD_DEBUG_PATH="$ARTIFACT_DIR/er-quickload-autoload-debug.log" \
	ER_QUICKLOAD_CRASH_LOG_PATH="$ARTIFACT_DIR/er-quickload-crash-log.txt" \
	ER_QUICKLOAD_TRACE_CONTINUE_PATH="$ARTIFACT_DIR/er-quickload-continue-trace.log" \
	ER_QUICKLOAD_INPUT_TRACE_PATH="$ARTIFACT_DIR/er-quickload-input-trace.jsonl" \
	ER_QUICKLOAD_BOOTSTRAP_PATH="$ARTIFACT_DIR/er-quickload-bootstrap.jsonl" \
	ER_QUICKLOAD_BOOTSTRAP_STATE_PATH="$ARTIFACT_DIR/er-quickload-bootstrap-state.json" \
	ER_QUICKLOAD_PROFILE_PATH="$ARTIFACT_DIR/er-quickload-profile.jsonl" \
	ER_QUICKLOAD_RELOAD_TRACE_PATH="$ARTIFACT_DIR/er-reload-trace.log" \
	ER_QUICKLOAD_INPUT_HARNESS_LOG_PATH="$ARTIFACT_DIR/er-input-harness.log" \
	ER_QUICKLOAD_INPUT_HARNESS_PHASES_PATH="$ARTIFACT_DIR/er-input-harness-phases.jsonl" \
	ER_QUICKLOAD_DIAG_HARNESS_PATH="$ARTIFACT_DIR/er-diag-harness.log" \
	ER_QUICKLOAD_TIMESERIES_PATH="$ARTIFACT_DIR/er-telemetry-timeseries.jsonl" \
	ER_QUICKLOAD_CPU_PROFILE_PATH="$ARTIFACT_DIR/er-cpu-profile.txt" \
	ER_QUICKLOAD_ARMAMENT_ICONS_PATH="$ARTIFACT_DIR/er-armament-icons.log" \
	ER_QUICKLOAD_SAVE_DISABLE_LOG_PATH="$ARTIFACT_DIR/er-save-disable.log" \
	ER_QUICKLOAD_SAVE_DISABLE_TELEMETRY_PATH="$ARTIFACT_DIR/er-save-disable-telemetry.json" \
	ER_QUICKLOAD_LOADING_PORTRAIT_PATH="$ARTIFACT_DIR/er-loading-portrait.log" \
	ER_QUICKLOAD_LOADING_PORTRAIT_CRASH_LOG_PATH="$ARTIFACT_DIR/er-loading-portrait-crash-log.txt" \
	ER_QUICKLOAD_CRASH_LOGGING_LOG_PATH="$ARTIFACT_DIR/er-crash-log.txt" \
	ER_QUICKLOAD_CRASH_LOGGING_LATEST_PATH="$ARTIFACT_DIR/er-crash-latest.txt" \
	ER_QUICKLOAD_CRASH_LOGGING_BREADCRUMB_PATH="$ARTIFACT_DIR/er-crash-breadcrumb-latest.txt" \
	ER_QUICKLOAD_CRASH_LOGGING_MODULES_PATH="$ARTIFACT_DIR/er-crash-modules.txt" \
	"$ME3" launch -g eldenring --online false -p "$(wslpath -w "$PROFILE")" >"$ARTIFACT_DIR/me3-launch.log" 2>&1 &
ME3_PID=$!
echo "== ER launching (me3 pid $ME3_PID). The telemetry-only DLL APPENDS a timeseries to:"
echo "==   $TS_GAME"
echo "== (no poller: the DLL writes it every 4th frame). Drive the reload, then analyze that jsonl:"
echo "==   python3 scripts/analyze-vanilla-reload-fps.py '$TS_GAME'"
echo "== me3-launch.log -> $ARTIFACT_DIR/me3-launch.log ; artifacts -> $ARTIFACT_DIR"
