#!/usr/bin/env bash
# USER-DRIVEN golden/observe launcher: launches the approved offline eldenring.exe
# via Proton with the observer DLL (LazyLoader chainload), and runs NO readiness
# watcher -- so the user can drive a normal load at their own pace while the DLL's
# recurring observer logs world-stream state to GAME_DIR/er-effects-autoload-debug.log.
# Tear down with: pkill -x eldenring.exe  (the script also self-kills at SAFETY_SECONDS).
# Save-safety is the caller's responsibility (back up + restore the .sl2).
set -uo pipefail

REPO_ROOT="${REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
GAME_DIR="${GAME_DIR:-$HOME/.local/share/Steam/steamapps/common/ELDEN RING/Game}"
PROTON="${PROTON:-$HOME/.local/share/Steam/steamapps/common/Proton - Experimental/proton}"
STEAM_COMPAT_DATA_PATH="${STEAM_COMPAT_DATA_PATH:-$HOME/.local/share/Steam/steamapps/compatdata/1245620}"
STEAM_COMPAT_CLIENT_INSTALL_PATH="${STEAM_COMPAT_CLIENT_INSTALL_PATH:-$HOME/.local/share/Steam}"
DLL="$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_effects_rs.dll"
SAFETY_SECONDS="${SAFETY_SECONDS:-300}"

fatal() { echo "run-golden-observe: $*" >&2; exit 2; }
pgrep -x steam >/dev/null 2>&1 || fatal "Steam is not running; start Steam first"
[[ -x "$PROTON" ]] || fatal "missing Proton: $PROTON"
[[ -f "$GAME_DIR/eldenring.exe" ]] || fatal "missing eldenring.exe: $GAME_DIR/eldenring.exe"
[[ -f "$DLL" ]] || fatal "missing DLL (build it first): $DLL"
if pgrep -x eldenring.exe >/dev/null 2>&1; then
  fatal "eldenring.exe already running; tear it down first"
fi

# Deploy the observer DLL via LazyLoader chainload (same mechanism as the gated harness).
mkdir -p "$GAME_DIR/dllMods"
cp -f "$DLL" "$GAME_DIR/er_effects_rs.dll"
cat > "$GAME_DIR/lazyLoad.ini" <<'INI'
; LazyLoader by Church Guard
[LAZYLOAD]
dllModFolderName=dllMods
[LOADORDER]
[CHAINLOAD]
dll=er_effects_rs.dll
INI

echo "run-golden-observe: launching offline eldenring.exe (observer-only, no watcher); safety kill in ${SAFETY_SECONDS}s"

# Anti-strand safety: if left running past SAFETY_SECONDS, kill the exact game process. Implemented as
# bounded literal <=30s waits on this launcher's own PID -- `tail --pid` returns instantly when the
# launcher exits, so the watchdog self-cancels the moment the run ends, with no blind sleep.
(
  watchdog_waited=0
  while kill -0 "$$" 2>/dev/null && (( watchdog_waited < SAFETY_SECONDS )); do
    timeout 20 tail --pid="$$" -f /dev/null >/dev/null 2>&1 || true
    watchdog_waited=$(( watchdog_waited + 20 ))
  done
  kill -0 "$$" 2>/dev/null && pkill -x eldenring.exe >/dev/null 2>&1 || true
) &
SAFETY_PID=$!

cd "$GAME_DIR"
export STEAM_COMPAT_CLIENT_INSTALL_PATH STEAM_COMPAT_DATA_PATH
"$PROTON" run "$GAME_DIR/eldenring.exe"
RC=$?

# Proton returned (game exited or was killed): cancel the safety timer.
kill "$SAFETY_PID" >/dev/null 2>&1 || true
echo "run-golden-observe: eldenring.exe exited rc=$RC"
exit "$RC"
