#!/usr/bin/env bash
# USER-INTERACTIVE vanilla offline boot for the privacy-policy persistence experiment. Launches the
# approved direct/offline eldenring.exe via Proton (NOT Steam applaunch, NOT the protected launcher),
# with our LazyLoader DLL DISABLED (so it is genuinely vanilla: no fail-closed abort, no input block,
# no agent teardown). The USER drives it (accept the privacy policy, quit). Modes:
#   launch   -- disable DLL, launch detached, print pid
#   teardown -- kill any eldenring.exe (if the user wants the agent to close it)
#   restore  -- re-enable our LazyLoader DLL
set -uo pipefail
MODE="${1:?usage: launch-vanilla-offline.sh launch|teardown|restore}"
GAME_DIR="${GAME_DIR:-$HOME/.local/share/Steam/steamapps/common/ELDEN RING/Game}"
PROTON="${PROTON:-$HOME/.local/share/Steam/steamapps/common/Proton - Experimental/proton}"
STEAM_COMPAT_DATA_PATH="${STEAM_COMPAT_DATA_PATH:-$HOME/.local/share/Steam/steamapps/compatdata/1245620}"
STEAM_COMPAT_CLIENT_INSTALL_PATH="${STEAM_COMPAT_CLIENT_INSTALL_PATH:-$HOME/.local/share/Steam}"
DINPUT="$GAME_DIR/dinput8.dll"
DINPUT_OFF="$GAME_DIR/dinput8.dll.er-disabled"

kill_game() {
  for proc in /proc/[0-9]*; do
    [[ -r "$proc/comm" ]] || continue
    [[ "$(<"$proc/comm")" == "eldenring.exe" ]] && kill "${proc##*/}" 2>/dev/null || true
  done
}

case "$MODE" in
  launch)
    pgrep -x steam >/dev/null 2>&1 || { echo "Steam is not running; start it first" >&2; exit 2; }
    [[ -x "$PROTON" ]] || { echo "missing proton: $PROTON" >&2; exit 2; }
    [[ -f "$GAME_DIR/eldenring.exe" ]] || { echo "missing eldenring.exe" >&2; exit 2; }
    for proc in /proc/[0-9]*; do
      [[ -r "$proc/comm" && "$(<"$proc/comm" 2>/dev/null)" == "eldenring.exe" ]] && { echo "eldenring.exe already running; teardown first" >&2; exit 2; }
    done
    # Disable our LazyLoader so the boot is genuinely vanilla (no DLL).
    [[ -f "$DINPUT" ]] && mv -f "$DINPUT" "$DINPUT_OFF" && echo "disabled LazyLoader ($DINPUT -> $DINPUT_OFF)"
    (
      cd "$GAME_DIR"
      STEAM_COMPAT_CLIENT_INSTALL_PATH="$STEAM_COMPAT_CLIENT_INSTALL_PATH" \
      STEAM_COMPAT_DATA_PATH="$STEAM_COMPAT_DATA_PATH" \
      "$PROTON" run "$GAME_DIR/eldenring.exe" > "$GAME_DIR/er-vanilla-proton.out" 2>&1 &
      echo "$!" > "$GAME_DIR/er-vanilla.pid"
    )
    echo "launched vanilla offline eldenring.exe (proton pid $(cat "$GAME_DIR/er-vanilla.pid" 2>/dev/null)). DLL is OFF."
    echo "Accept the privacy policy and quit when done; then tell the agent."
    ;;
  teardown)
    kill_game
    echo "sent kill to eldenring.exe (if running)"
    ;;
  restore)
    [[ -f "$DINPUT_OFF" ]] && mv -f "$DINPUT_OFF" "$DINPUT" && echo "re-enabled LazyLoader ($DINPUT_OFF -> $DINPUT)"
    ;;
  *) echo "unknown mode: $MODE" >&2; exit 2 ;;
esac
