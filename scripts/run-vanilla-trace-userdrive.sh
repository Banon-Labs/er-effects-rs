#!/usr/bin/env bash
# VANILLA USER-DRIVEN load1 baseline (bd vanilla-userdrive-trace-only-baseline-load1-safety-2026-07-20).
# me3 OFFLINE with ONLY er_reload_trace_dll.dll (log-only, standalone MinHook -- NO product DLL, NO
# autoload/quickload/system-quit, NO input harness/autodrive, NO save redirect). The game boots pure
# vanilla; the USER drives to angrE via the normal Load Game menu using their real APPDATA save. The
# trace DLL logs the native load-path sequence + a RAM snapshot to er-reload-trace.log. NO monitor / NO
# teardown -- the game stays LIVE for the user; collect the log afterward.
set -uo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

TRACE_DLL="$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_reload_trace_dll.dll"
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
[[ -f "$TRACE_DLL" ]] || fail "trace DLL not built: $TRACE_DLL (cargo xwin build --release --target x86_64-pc-windows-msvc -p er-reload-trace-dll)"

ME3="${ME3:-/mnt/c/Users/$USER/AppData/Local/garyttierney/me3/bin/me3.exe}"
[[ -f "$ME3" ]] || fail "Windows me3.exe not found at $ME3 (set ME3=<path to me3.exe>)"

mkdir -p "$ARTIFACT_DIR"
win_path() { python3 -c "import sys;p=sys.argv[1];print((p[5].upper()+':\\\\'+p[7:].replace('/','\\\\')) if p.startswith('/mnt/') and len(p)>6 and p[6]=='/' else p)" "$1"; }

# --- stage ONLY the trace DLL + a single-native me3 profile ---
TRACE_GAMEDIR="$GAME_DIR/er_reload_trace_dll.dll"
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
if [[ -f "$GAME_DIR/er-effects.toml" ]]; then
	cp -f "$GAME_DIR/er-effects.toml" "$ARTIFACT_DIR/er-effects.toml.bak"
	rm -f "$GAME_DIR/er-effects.toml"
	echo "== backed up + removed er-effects.toml (pure vanilla, no save redirect) -> $ARTIFACT_DIR/er-effects.toml.bak"
fi

# --- clean slate: reset the trace log so this run is isolated ---
rm -f "$GAME_DIR/er-reload-trace.log" 2>/dev/null

echo "======================================================================"
echo "== LAUNCHING ELDEN RING (offline me3) -- VANILLA, USER-DRIVEN load1 baseline"
echo "==   native: er_reload_trace_dll.dll ONLY (log-only; NO product, NO autodrive, NO save redirect)"
echo "==   YOU drive (CONTINUE is the product-matching path): PRESS ANY BUTTON -> Continue -> into the world."
echo "==   Nothing auto-tears-down; the game stays live. Tell me when you've reached a stable, movable world"
echo "==   (or if anything crashes / a message box appears)."
echo "==   trace log -> $GAME_DIR/er-reload-trace.log   (artifacts: $ARTIFACT_DIR)"
echo "======================================================================"

nohup "$ME3" launch -g eldenring --online false -p "$(wslpath -w "$PROFILE")" >"$ARTIFACT_DIR/me3-launch.log" 2>&1 &
echo "me3 pid $! ; launch log: $ARTIFACT_DIR/me3-launch.log"
echo "ARTIFACT_DIR=$ARTIFACT_DIR"
