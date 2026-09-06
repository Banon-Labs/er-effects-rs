#!/usr/bin/env bash
# ARMAMENT-ICONS live user-inspection launch (bd er-effects-rs-pe98 / er-effects-rs-jogu).
#
# This is the USER-INSPECTION counterpart to run-armament-icons-smoke.sh: it launches the
# badge DLL through me3 and LEAVES THE GAME RUNNING for the user to drive by hand. Per
# AGENTS.md it therefore uses NO watcher that owns shutdown, NO autopilot/repro driver, and
# NO input blocking -- the user is genuinely in control.
#
# The badge covers the equip menu, the inventory tabs and the sort chest (the movies in
# er_gfx::arts_badge::TARGETS), so every armament/shield tile in those lists shows its Ash of
# War in the bottom-left corner.
#
#   bash scripts/run-armament-icons-live.sh                # badge + telemetry + frida
#   FRIDA=0 bash scripts/run-armament-icons-live.sh         # no frida gadget
#   ARTIFACT_DIR=... bash scripts/run-armament-icons-live.sh
#
# MOD_PACKAGE=<dir>[:<dir>...] adds third-party me3 asset-override packages (a directory
# holding `menu/*.gfx` etc.) ALONGSIDE the badge DLL. That is the side-by-side compatibility
# smoke: the DLL must derive the badge from whatever menu movies the other mod supplies
# rather than only from vanilla bytes.
#
#   MOD_PACKAGE=/path/to/minimal-hud bash scripts/run-armament-icons-live.sh
#
# Teardown is the user's (or a later agent turn's) call: the launcher PID and the artifact
# dir are printed on exit.
set -uo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

ARTIFACT_DIR="${ARTIFACT_DIR:-$REPO_ROOT/target/runtime-probe/armament-icons-live-$(date +%Y%m%d-%H%M%S)}"
TELEM_DLL="$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_telemetry.dll"
BADGE_DLL="$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_armament_icons.dll"
FRIDA_DLL="$REPO_ROOT/target/frida-gadget/frida-gadget.dll"

fail() {
	echo "run-armament-icons-live: $*" >&2
	exit 2
}

# --- GAME_DIR resolution (current-user-aware; never hard-code /home/<user>) ---
if [[ -z "${GAME_DIR:-}" ]]; then
	for c in \
		"$HOME/.local/share/Steam/steamapps/common/ELDEN RING/Game" \
		"$HOME/.steam/steam/steamapps/common/ELDEN RING/Game" \
		"/mnt/c/SteamLibrary/steamapps/common/ELDEN RING/Game"; do
		[[ -f "$c/eldenring.exe" ]] && {
			GAME_DIR="$c"
			break
		}
	done
fi
[[ -n "${GAME_DIR:-}" && -f "$GAME_DIR/eldenring.exe" ]] || fail \
	"GAME_DIR not resolved. Set GAME_DIR=<linux path to the '.../ELDEN RING/Game' dir with eldenring.exe>."

# shellcheck source=scripts/steam-running.sh
# shellcheck disable=SC1091
source "$REPO_ROOT/scripts/steam-running.sh"
steam_running || fail "Steam is not running. Start Steam (interactive login) first."

[[ -f "$BADGE_DLL" ]] || fail "badge DLL not built: $BADGE_DLL (cargo xwin build --release --target x86_64-pc-windows-msvc -p er-armament-icons)"

if python3 "$REPO_ROOT/scripts/detect-proc.py" 'eldenring\.exe|start_protected_game\.exe' >/dev/null 2>&1; then
	fail "An Elden Ring process is already running. Tear it down before launching (never a blanket kill)."
fi

if [[ -z "${ME3:-}" ]]; then
	command -v me3 >/dev/null 2>&1 && ME3="$(command -v me3)"
fi
[[ -n "${ME3:-}" ]] || fail "me3 not found on PATH (set ME3=<path>)"
ME3_STEAM_DIR="${ME3_STEAM_DIR:-$(cd "$GAME_DIR/../../../.." && pwd)}"

mkdir -p "$ARTIFACT_DIR"

# --- stage the DLLs into the game dir (me3 natives load from there) ---
BADGE_GAMEDIR="$GAME_DIR/er_armament_icons.dll"
cp -f "$BADGE_DLL" "$BADGE_GAMEDIR"
TELEM_GAMEDIR=""
if [[ -f "$TELEM_DLL" ]]; then
	TELEM_GAMEDIR="$GAME_DIR/er_telemetry.dll"
	cp -f "$TELEM_DLL" "$TELEM_GAMEDIR"
fi

PROFILE="$ARTIFACT_DIR/armament-icons-live.me3"
{
	echo 'profileVersion = "v1"'
	echo 'start_online = false'
	echo
	echo '[[supports]]'
	echo 'game = "eldenring"'
	# Third-party asset-override packages (schema verified against `me3 profile create
	# --package`, me3 0.11.0). Listed BEFORE the natives so the movies they override are the
	# ones our DLL sees when it derives the badge.
	if [[ -n "${MOD_PACKAGE:-}" ]]; then
		IFS=':' read -r -a _pkgs <<<"$MOD_PACKAGE"
		for _pkg in "${_pkgs[@]}"; do
			[[ -n "$_pkg" ]] || continue
			[[ -d "$_pkg" ]] || fail "MOD_PACKAGE entry is not a directory: $_pkg"
			echo
			echo '[[packages]]'
			echo 'enabled = true'
			echo "path = '$_pkg'"
			echo 'load_after = []'
			echo 'load_before = []'
		done
	fi
	if [[ -n "$TELEM_GAMEDIR" ]]; then
		echo
		echo '[[natives]]'
		echo "path = '$TELEM_GAMEDIR'"
	fi
	echo
	echo '[[natives]]'
	echo "path = '$BADGE_GAMEDIR'"
	# frida-gadget listens on 127.0.0.1:27042 so the badge can still be poked live.
	if [[ "${FRIDA:-1}" != "0" && -f "$FRIDA_DLL" ]]; then
		echo
		echo '[[natives]]'
		echo "path = '$FRIDA_DLL'"
	fi
} >"$PROFILE"

# Sweep stale MARKERS so this run's behaviour is unambiguous. No drive-mode marker and no
# diagnostic overrides: this is a hands-on run, not an agent-driven probe.
#
# THE LOG IS NO LONGER IN THIS LIST, AND THAT IS THE POINT. This line used to begin
# `rm -f "$GAME_DIR"/er-armament-icons.log`, which destroyed TWO prior runs at once and neither of
# them this one's: `er_game_base::log::begin_fresh_run` removes `<name>.prev` unconditionally when
# the live file is absent, so clearing the live file takes the generation behind it as well. Several
# sessions launch concurrently here. The log is redirected into ARTIFACT_DIR at launch, so it starts
# empty by construction and there is nothing of anyone's in the game directory to clear.
rm -f "$GAME_DIR"/er-harness-drive-mode.txt \
	"$GAME_DIR"/er-armament-icons-force-icon.txt "$GAME_DIR"/er-armament-icons-target.txt \
	2>/dev/null

echo "======================================================================"
echo "==  LAUNCHING ELDEN RING (offline, me3) -- LIVE, USER-DRIVEN         =="
echo "==  A GAME WINDOW WILL OPEN ON YOUR DESKTOP AND STAY OPEN.           =="
echo "==  No autopilot, no injected input, no input blocking: you drive.   =="
echo "==  Badge movies: equip menu, inventory tabs, sort chest.            =="
[[ -n "${MOD_PACKAGE:-}" ]] && echo "==  Third-party packages: $MOD_PACKAGE"
echo "==  profile   -> $PROFILE"
echo "==  artifacts -> $ARTIFACT_DIR"
echo "==  DLL log   -> $GAME_DIR/er-armament-icons.log"
echo "======================================================================"

# EVERY per-run artifact goes into THIS run's directory. A GAME_DIR artifact is SINGLE-SLOT: the DLL
# rotates `<name>` to `<name>.prev` on its first write, so two launches lose the run before last,
# and several sessions launch concurrently here. A copy after the run cannot fix that -- by then
# this run has clobbered the previous one's file -- and a crashed run never reaches the copy.
(cd "$GAME_DIR" && env \
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
	"$ME3" --steam-dir "$ME3_STEAM_DIR" launch -p "$PROFILE" -g eldenring -e "$GAME_DIR/eldenring.exe") \
	>"$ARTIFACT_DIR/me3-live.log" 2>&1 &
LAUNCHER_PID=$!

echo "run-armament-icons-live: launcher pid=$LAUNCHER_PID (left running for inspection)"
echo "run-armament-icons-live: me3 log -> $ARTIFACT_DIR/me3-live.log"
