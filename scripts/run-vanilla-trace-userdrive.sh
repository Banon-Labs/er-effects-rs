#!/usr/bin/env bash
# VANILLA USER-DRIVEN load1 baseline (bd vanilla-userdrive-trace-only-baseline-load1-safety-2026-07-20).
# me3 OFFLINE with ONLY er_reload_trace.dll (log-only, standalone MinHook -- NO product DLL, NO
# autoload/quickload/system-quit, NO input harness/autodrive, NO save redirect). The game boots pure
# vanilla; the USER drives to angrE via the normal Load Game menu using their real APPDATA save. The
# trace DLL logs the native load-path sequence + a RAM snapshot to er-reload-trace.log. NO monitor / NO
# teardown -- the game stays LIVE for the user; collect the log afterward.
set -uo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

TRACE_DLL="$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_reload_trace.dll"
ARTIFACT_DIR="${ARTIFACT_DIR:-$REPO_ROOT/target/runtime-probe/vanilla-trace-userdrive-$(date +%Y%m%d-%H%M%S)}"

fail() { echo "run-vanilla-trace-userdrive: $*" >&2; exit 2; }

# --- GAME_DIR resolution (current-user-aware) ---
if [[ -z "${GAME_DIR:-}" ]]; then
	for c in \
		"/mnt/c/SteamLibrary/steamapps/common/ELDEN RING/Game" \
		"$HOME/.local/share/Steam/steamapps/common/ELDEN RING/Game" \
		"$HOME/.steam/steam/steamapps/common/ELDEN RING/Game"; do
		[[ -f "$c/eldenring.exe" ]] && { GAME_DIR="$c"; break; }
	done
fi
[[ -n "${GAME_DIR:-}" && -f "$GAME_DIR/eldenring.exe" ]] || fail \
	"GAME_DIR not resolved. Set GAME_DIR=<linux path to '.../ELDEN RING/Game' with eldenring.exe>."

# shellcheck source=scripts/steam-running.sh
# shellcheck disable=SC1091
source "$REPO_ROOT/scripts/steam-running.sh"
steam_running || fail "Steam is not running. Start Steam (interactive login) first."
[[ -f "$TRACE_DLL" ]] || fail "trace DLL not built: $TRACE_DLL (cargo xwin build --release --target x86_64-pc-windows-msvc -p er-reload-trace)"

ME3="${ME3:-/mnt/c/Users/$USER/AppData/Local/garyttierney/me3/bin/me3.exe}"
[[ -f "$ME3" ]] || fail "Windows me3.exe not found at $ME3 (set ME3=<path to me3.exe>)"

mkdir -p "$ARTIFACT_DIR"
win_path() { python3 -c "import sys;p=sys.argv[1];print((p[5].upper()+':\\\\'+p[7:].replace('/','\\\\')) if p.startswith('/mnt/') and len(p)>6 and p[6]=='/' else p)" "$1"; }

# --- stage ONLY the trace DLL + a single-native me3 profile ---
TRACE_GAMEDIR="$GAME_DIR/er_reload_trace.dll"
cp -f "$TRACE_DLL" "$TRACE_GAMEDIR"
PROFILE="$ARTIFACT_DIR/vanilla-trace.me3"
{
	echo 'profileVersion = "v1"'
	echo
	echo '[[supports]]'
	echo 'game = "eldenring"'
	echo
	echo '[[natives]]'
	echo "path = '$(win_path "$TRACE_GAMEDIR")'"
} >"$PROFILE"

# --- PURE VANILLA: back up + remove any product save-redirect TOML so nothing redirects the save ---
if [[ -f "$GAME_DIR/er-quickload.toml" ]]; then
	cp -f "$GAME_DIR/er-quickload.toml" "$ARTIFACT_DIR/er-quickload.toml.bak"
	rm -f "$GAME_DIR/er-quickload.toml"
	echo "== backed up + removed er-quickload.toml (pure vanilla, no save redirect) -> $ARTIFACT_DIR/er-quickload.toml.bak"
fi

# --- clean slate: reset the trace log so this run is isolated ---
# The trace is redirected into ARTIFACT_DIR (see the launch below), so it starts empty by
# construction. This line used to clear the game directory's copy, which belongs to whichever session
# wrote it -- and took its `.prev` along, since `begin_fresh_run` drops a stale `.prev` when the live
# file is absent.

echo "======================================================================"
echo "== LAUNCHING ELDEN RING (offline me3) -- VANILLA, USER-DRIVEN load1 baseline"
echo "==   native: er_reload_trace.dll ONLY (log-only; NO product, NO autodrive, NO save redirect)"
echo "==   YOU drive (CONTINUE is the product-matching path): PRESS ANY BUTTON -> Continue -> into the world."
echo "==   Nothing auto-tears-down; the game stays live. Tell me when you've reached a stable, movable world"
echo "==   (or if anything crashes / a message box appears)."
echo "==   trace log -> $ARTIFACT_DIR/er-reload-trace.log   (artifacts: $ARTIFACT_DIR)"
echo "======================================================================"

# EVERY per-run artifact goes into THIS run's directory. A GAME_DIR artifact is SINGLE-SLOT: the DLL
# rotates `<name>` to `<name>.prev` on its first write, so two launches lose the run before last,
# and several sessions launch concurrently here. A copy after the run cannot fix that -- by then
# this run has clobbered the previous one's file -- and a crashed run never reaches the copy.
nohup env \
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
echo "me3 pid $! ; launch log: $ARTIFACT_DIR/me3-launch.log"
echo "ARTIFACT_DIR=$ARTIFACT_DIR"
