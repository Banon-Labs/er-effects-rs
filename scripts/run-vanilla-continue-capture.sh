#!/usr/bin/env bash
# VANILLA-CONTINUE oracle-reference capture (user 2026-07-20): the correct baseline to diff load2
# against is the NATIVE menu-driven Continue, NOT our custom autoload (load1). This run:
#   - loads the PRODUCT + trace DLLs (so the rich oracle_* telemetry is emitted), but
#   - sets ER_QUICKLOAD_TELEMETRY_ONLY=1, which DISARMS the custom autoload
#     (product_autoload_gates.rs:62 arms only if !save_override_telemetry_only()), so the game boots
#     to the TITLE normally with NO autoload/redirect, and
#   - records the full telemetry timeseries in OBSERVE-ONLY mode (no probe/verdict/stall teardowns).
# The USER drives: boot -> title -> CONTINUE (or Load Game -> angrE). Input is LIVE (menu nav is
# RawInput, unaffected by the DInput/XInput block). The captured timeseries is the vanilla-continue
# reference imprint. bd vanilla-continue-telemetry-capture-via-telemetry-only-disarms-autoload-2026-07-20.
set -uo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

ARTIFACT_DIR="${ARTIFACT_DIR:-$REPO_ROOT/target/runtime-probe/vanilla-continue-$(date +%Y%m%d-%H%M%S)}"
PRODUCT_DLL="$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_quickload.dll"
TRACE_DLL="$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_reload_trace.dll"
CAP_SECONDS="$(cat "$REPO_ROOT/.auto/runtime_timeout_cap_seconds" 2>/dev/null || echo 300)"
OBSERVE_SECONDS="${OBSERVE_SECONDS:-280}"

fail() {
	echo "run-vanilla-continue-capture: $*" >&2
	exit 2
}

if [[ -z "${GAME_DIR:-}" ]]; then
	for c in \
		"/mnt/c/SteamLibrary/steamapps/common/ELDEN RING/Game" \
		"$HOME/.local/share/Steam/steamapps/common/ELDEN RING/Game" \
		"$HOME/.steam/steam/steamapps/common/ELDEN RING/Game"; do
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
[[ -f "$PRODUCT_DLL" ]] || fail "product DLL not built: $PRODUCT_DLL"
[[ -f "$TRACE_DLL" ]] || fail "trace DLL not built: $TRACE_DLL"

ME3="${ME3:-/mnt/c/Users/$USER/AppData/Local/garyttierney/me3/bin/me3.exe}"
[[ -f "$ME3" ]] || fail "Windows me3.exe not found at $ME3 (set ME3=<path>)"

mkdir -p "$ARTIFACT_DIR"
win_path() { python3 -c "import sys;p=sys.argv[1];print((p[5].upper()+':\\\\'+p[7:].replace('/','\\\\')) if p.startswith('/mnt/') and len(p)>6 and p[6]=='/' else p)" "$1"; }

PRODUCT_GAMEDIR="$GAME_DIR/er_quickload.dll"
TRACE_GAMEDIR="$GAME_DIR/er_reload_trace.dll"
cp -f "$PRODUCT_DLL" "$PRODUCT_GAMEDIR"
cp -f "$TRACE_DLL" "$TRACE_GAMEDIR"
PROFILE="$ARTIFACT_DIR/vanilla-continue.me3"
{
	echo 'profileVersion = "v1"'
	echo
	echo '[[supports]]'
	echo 'game = "eldenring"'
	echo
	echo '[[natives]]'
	echo "path = '$(win_path "$PRODUCT_GAMEDIR")'"
	echo
	echo '[[natives]]'
	echo "path = '$(win_path "$TRACE_GAMEDIR")'"
} >"$PROFILE"

# TELEMETRY-ONLY: disarms the custom autoload; product still emits oracle_* telemetry. NO save redirect
# (pure vanilla APPDATA save -- the user's real save; they Continue/Load-Game into angrE).
printf '1\n' >"$GAME_DIR/er-quickload-telemetry-only.txt"
rm -f "$GAME_DIR/er-quickload.toml" 2>/dev/null
# Sweep every autoload-drive / harness / move-probe / sq-repro marker so nothing drives the run.
rm -f "$GAME_DIR"/er-quickload-system-quit-repro.txt "$GAME_DIR"/er-quickload-system-quit-load-switch.txt \
	"$GAME_DIR"/er-quickload-sq-target-switches.txt "$GAME_DIR"/er-quickload-sq-target-slots.txt \
	"$GAME_DIR"/er-quickload-prove-movement.txt "$GAME_DIR"/er-quickload-stay-active.txt \
	"$GAME_DIR"/er-quickload-probe-foreground.txt "$GAME_DIR"/er-quickload-switch-slot.txt 2>/dev/null
# Clean slate for logs/telemetry so this run is not polluted by a prior one.
# THE GAME_DIR LOG SWEEP IS GONE ON PURPOSE. It used to read
#   rm -f "$GAME_DIR"/er-quickload-*.log "$GAME_DIR"/er-reload-trace.log \
#         "$GAME_DIR"/er-quickload-telemetry.json
# and it destroyed TWO generations of somebody ELSE's run: `begin_fresh_run` removes `<name>.prev`
# unconditionally when the live file is absent. Every log this run writes now lands in a fresh
# per-run ARTIFACT_DIR, so the clean slate is free and takes nobody else's evidence with it.

# shellcheck disable=SC2317
cleanup() {
	taskkill.exe /F /IM eldenring.exe >/dev/null 2>&1
	taskkill.exe /F /IM me3.exe >/dev/null 2>&1
	rm -f "$GAME_DIR/er-quickload-telemetry-only.txt" 2>/dev/null
}
trap cleanup EXIT

echo "======================================================================"
echo "== LAUNCHING ELDEN RING (offline, me3) -- VANILLA CONTINUE capture   =="
echo "==   product(TELEMETRY-ONLY, autoload DISARMED) + trace              =="
echo "==   boots to the TITLE -- NO autoload, NO redirect, NO driving      =="
echo "==   YOUR input is LIVE: drive to the title, press CONTINUE (or Load =="
echo "==   Game -> angrE). The oracle_* timeseries records the NATIVE load =="
echo "==   observe window ${OBSERVE_SECONDS}s  cap=${CAP_SECONDS}s  artifacts -> $ARTIFACT_DIR"
echo "======================================================================"

# EVERY per-run artifact goes into THIS run's directory. A GAME_DIR artifact is SINGLE-SLOT: the DLL
# rotates `<name>` to `<name>.prev` on its first write, so two launches lose the run before last --
# and several sessions launch concurrently here, which makes that normal rather than a race. A copy
# after the run cannot fix it (this run clobbered the previous one's file at LAUNCH) and never runs
# at all when the game crashes, which is exactly the run whose evidence matters.
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

python3 "$REPO_ROOT/scripts/capture-samechar-3x.py" \
	--game-dir "$GAME_DIR" \
	--artifact-dir "$ARTIFACT_DIR" \
	--max-seconds "$CAP_SECONDS" \
	--report "$ARTIFACT_DIR/vanilla-continue-report.md" \
	--observe-only --observe-seconds "$OBSERVE_SECONDS"
RC=$?

# FALLBACK ONLY -- the trace is redirected into ARTIFACT_DIR at launch. This copy covers the case
# where the env did not survive me3 -> Proton and the DLL fell back to the game directory.
[[ -f "$ARTIFACT_DIR/er-reload-trace.log" ]] ||
	{ [[ -f "$GAME_DIR/er-reload-trace.log" ]] && cp -f "$GAME_DIR/er-reload-trace.log" "$ARTIFACT_DIR/er-reload-trace.log"; }
echo "== vanilla-continue capture done rc=$RC ; timeseries -> $ARTIFACT_DIR/telemetry-timeseries.jsonl =="
exit "$RC"
