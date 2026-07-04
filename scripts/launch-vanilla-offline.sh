#!/usr/bin/env bash
# USER-INTERACTIVE vanilla offline boot for the privacy-policy persistence experiment. Launches the
# approved direct/offline eldenring.exe via Proton (NOT Steam applaunch, NOT the protected launcher,
# NOT me3 -- a vanilla boot must not inject the me3 mod host). Genuinely vanilla: no DLL, no
# fail-closed abort, no input block, no agent teardown. The USER drives it (accept the privacy
# policy, quit). LazyLoader was removed 2026-07-04 (me3 is the product loader); the dinput8
# disable/restore below is a DEFENSIVE guard against a leftover proxy so the boot stays vanilla.
# Modes:
#   launch   -- stage away any leftover dinput8 proxy, launch detached, print pid
#   teardown -- kill any eldenring.exe (if the user wants the agent to close it)
#   restore  -- put back a staged-away leftover proxy
set -uo pipefail
MODE="${1:?usage: launch-vanilla-offline.sh launch|teardown|restore}"
GAME_DIR="${GAME_DIR:-$HOME/.local/share/Steam/steamapps/common/ELDEN RING/Game}"
PROTON="${PROTON:-$HOME/.local/share/Steam/steamapps/common/Proton - Experimental/proton}"
STEAM_COMPAT_DATA_PATH="${STEAM_COMPAT_DATA_PATH:-$HOME/.local/share/Steam/steamapps/compatdata/1245620}"
STEAM_COMPAT_CLIENT_INSTALL_PATH="${STEAM_COMPAT_CLIENT_INSTALL_PATH:-$HOME/.local/share/Steam}"
KEEP_LAZYLOADER="${KEEP_LAZYLOADER:-0}"
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
    dll_state="OFF"
    if [[ "$KEEP_LAZYLOADER" == "1" ]]; then
      echo "KEEP_LAZYLOADER=1 is obsolete: LazyLoader was removed 2026-07-04; for a DLL run use scripts/run-me3-product-smoke.sh or scripts/run-product-continue-direct-probe.sh (me3 native)" >&2
      exit 2
    fi
    # Stage away any LEFTOVER proxy so the boot is genuinely vanilla (no DLL).
    [[ -f "$DINPUT" ]] && mv -f "$DINPUT" "$DINPUT_OFF" && echo "staged away leftover dinput8 proxy ($DINPUT -> $DINPUT_OFF)"
    (
      cd "$GAME_DIR"
      STEAM_COMPAT_CLIENT_INSTALL_PATH="$STEAM_COMPAT_CLIENT_INSTALL_PATH" \
      STEAM_COMPAT_DATA_PATH="$STEAM_COMPAT_DATA_PATH" \
      "$PROTON" run "$GAME_DIR/eldenring.exe" > "$GAME_DIR/er-vanilla-proton.out" 2>&1 &
      echo "$!" > "$GAME_DIR/er-vanilla.pid"
    )
    echo "launched vanilla offline eldenring.exe (proton pid $(cat "$GAME_DIR/er-vanilla.pid" 2>/dev/null)). DLL is $dll_state."
    echo "Accept the privacy policy and quit when done; then tell the agent."
    ;;
  teardown)
    kill_game
    echo "sent kill to eldenring.exe (if running)"
    ;;
  restore)
    [[ -f "$DINPUT_OFF" ]] && mv -f "$DINPUT_OFF" "$DINPUT" && echo "restored staged-away dinput8 proxy ($DINPUT_OFF -> $DINPUT)"
    ;;
  *) echo "unknown mode: $MODE" >&2; exit 2 ;;
esac
