#!/usr/bin/env bash
# USER-DRIVEN golden/observe launcher: launches the approved offline eldenring.exe
# via me3 (the observer DLL loaded as an me3 native; LazyLoader removed 2026-07-04),
# and runs NO readiness watcher -- so the user can drive a normal load at their own
# pace while the DLL's recurring observer logs world-stream state to
# GAME_DIR/er-effects-autoload-debug.log.
# Tear down with: pkill -x eldenring.exe  (the script also self-kills at SAFETY_SECONDS).
# Save-safety is the caller's responsibility (back up + restore the .sl2).
set -uo pipefail

REPO_ROOT="${REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
GAME_DIR="${GAME_DIR:-$HOME/.local/share/Steam/steamapps/common/ELDEN RING/Game}"
# shellcheck source=scripts/me3-launch-lib.sh
source "$REPO_ROOT/scripts/me3-launch-lib.sh"
DLL="$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_effects_rs.dll"
SAFETY_SECONDS="${SAFETY_SECONDS:-300}"
OBSERVE_DIR="${OBSERVE_DIR:-$REPO_ROOT/target/runtime-probe/golden-observe-$(date +%Y%m%d-%H%M%S)}"

fatal() { echo "run-golden-observe: $*" >&2; exit 2; }
pgrep -x steam >/dev/null 2>&1 || fatal "Steam is not running; start Steam first"
me3_preflight || fatal "me3 preflight failed"
me3_require_no_lazyloader "$GAME_DIR" || fatal "leftover LazyLoader proxy in $GAME_DIR"
[[ -f "$GAME_DIR/eldenring.exe" ]] || fatal "missing eldenring.exe: $GAME_DIR/eldenring.exe"
[[ -f "$DLL" ]] || fatal "missing DLL (build it first): $DLL"
if pgrep -x eldenring.exe >/dev/null 2>&1; then
  fatal "eldenring.exe already running; tear it down first"
fi

# Stage the observer DLL as an me3 native (per-run immutable payload).
mkdir -p "$OBSERVE_DIR"
cp -f "$DLL" "$OBSERVE_DIR/er_effects_rs.dll"
me3_write_profile "$OBSERVE_DIR/er-effects-observe.me3" "$OBSERVE_DIR/er_effects_rs.dll"

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

cd "$GAME_DIR" || fatal "cannot cd to $GAME_DIR"
me3_launch "$OBSERVE_DIR/er-effects-observe.me3"
RC=$?

# me3 returned (game exited or was killed): cancel the safety timer.
kill "$SAFETY_PID" >/dev/null 2>&1 || true
echo "run-golden-observe: eldenring.exe exited rc=$RC"
exit "$RC"
