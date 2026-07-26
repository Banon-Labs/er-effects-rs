#!/usr/bin/env bash
# Vanilla-reload FPS comparison (2026-07-22, bd USER-chose-vanilla-reload-comparison).
# Loads ONLY the telemetry-only DLL (er_telemetry_dll -- no product hooks, no reload driver, no
# autopilot), launches offline ER LIVE for the USER to drive, and polls er-telemetry-standalone.json to
# a timeseries. The USER drives: title -> Continue (loads angrE = the BOOT-equivalent load), play +
# walk forward, then System -> Quit to Title -> Continue (the RELOAD), play + walk forward ~3s. We then
# compare the game frame time (flip task_delta) between the boot-continue and the reload -- to isolate
# whether OUR reload path (own_load_switch_reload_fire) causes the ~20fps game-side slowdown or it is
# inherent to game reloads in this WSLg/Proton env. No agent input/autopilot: the user owns the input.
set -uo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ARTIFACT_DIR="${ARTIFACT_DIR:-$REPO_ROOT/target/runtime-probe/vanilla-reload-fps-$(date +%Y%m%d-%H%M%S)}"
TELEM_DLL="$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_telemetry_dll.dll"
HARNESS_DLL="$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_input_harness_dll.dll"

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
[[ -f "$TELEM_DLL" ]] || fail "telemetry DLL not built: $TELEM_DLL (cargo xwin build --release --target x86_64-pc-windows-msvc -p er-telemetry-dll)"
[[ -f "$HARNESS_DLL" ]] || fail "input-harness DLL not built: $HARNESS_DLL (cargo xwin build --release --target x86_64-pc-windows-msvc -p er-input-harness-dll)"

ME3="${ME3:-/mnt/c/Users/$USER/AppData/Local/garyttierney/me3/bin/me3.exe}"
[[ -f "$ME3" ]] || fail "Windows me3.exe not found at $ME3 (set ME3=<path to me3.exe>)"
mkdir -p "$ARTIFACT_DIR"
cp -f "$TELEM_DLL" "$GAME_DIR/er_telemetry_dll.dll"
cp -f "$HARNESS_DLL" "$GAME_DIR/er_input_harness_dll.dll"
TS_GAME="$GAME_DIR/er-telemetry-timeseries.jsonl"
rm -f "$TS_GAME" "$GAME_DIR/er-input-harness.log"

winpath() { python3 -c "p='$1'; print((p[5].upper()+':\\\\'+p[7:].replace('/','\\\\')) if p.startswith('/mnt/') else p)"; }
WIN_TELEM="$(winpath "$GAME_DIR/er_telemetry_dll.dll")"
WIN_HARNESS="$(winpath "$GAME_DIR/er_input_harness_dll.dll")"
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

"$ME3" launch -g eldenring --online false -p "$(wslpath -w "$PROFILE")" >"$ARTIFACT_DIR/me3-launch.log" 2>&1 &
ME3_PID=$!
echo "== ER launching (me3 pid $ME3_PID). The telemetry-only DLL APPENDS a timeseries to:"
echo "==   $TS_GAME"
echo "== (no poller: the DLL writes it every 4th frame). Drive the reload, then analyze that jsonl:"
echo "==   python3 scripts/analyze-vanilla-reload-fps.py '$TS_GAME'"
echo "== me3-launch.log -> $ARTIFACT_DIR/me3-launch.log ; artifacts -> $ARTIFACT_DIR"
