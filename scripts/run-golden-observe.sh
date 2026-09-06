#!/usr/bin/env bash
# USER-DRIVEN golden/observe launcher: launches the approved offline eldenring.exe
# via me3 (the observer DLL loaded as an me3 native; LazyLoader removed 2026-07-04),
# and runs NO readiness watcher -- so the user can drive a normal load at their own
# pace while the DLL's recurring observer logs world-stream state to
# OBSERVE_DIR/er-quickload-autoload-debug.log (redirected there; the game directory's
# copy is single-slot and the next launch destroys it).
# Tear down with: pkill -x eldenring.exe  (the script also self-kills at SAFETY_SECONDS).
# Save-safety is the caller's responsibility (back up + restore the .sl2).
set -uo pipefail

REPO_ROOT="${REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
GAME_DIR="${GAME_DIR:-$HOME/.local/share/Steam/steamapps/common/ELDEN RING/Game}"
# shellcheck source=scripts/me3-launch-lib.sh
source "$REPO_ROOT/scripts/me3-launch-lib.sh"
DLL="$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_quickload.dll"
SAFETY_SECONDS="${SAFETY_SECONDS:-300}"
OBSERVE_DIR="${OBSERVE_DIR:-$REPO_ROOT/target/runtime-probe/golden-observe-$(date +%Y%m%d-%H%M%S)}"

fatal() { echo "run-golden-observe: $*" >&2; exit 2; }
# THE SANCTIONED STEAM CHECK. This was a raw `pgrep -x steam`, which AGENTS.md forbids: it
# false-negatives on this setup and the OPA guard blocks it outright, so the script refused to run
# with "Steam is not running" while Steam was running.
# shellcheck source=scripts/steam-running.sh disable=SC1091
source "$REPO_ROOT/scripts/steam-running.sh"
steam_running || fatal "Steam is not running; start Steam first (interactive login)"
me3_preflight || fatal "me3 preflight failed"
me3_require_no_lazyloader "$GAME_DIR" || fatal "leftover LazyLoader proxy in $GAME_DIR"
[[ -f "$GAME_DIR/eldenring.exe" ]] || fatal "missing eldenring.exe: $GAME_DIR/eldenring.exe"
[[ -f "$DLL" ]] || fatal "missing DLL (build it first): $DLL"
# Name-grepping for the game is guard-denied here and, worse, blind: a Wine process's `comm` is the
# WINDOWS executable name while its `exe` symlink points at wine64-preloader, so a naive scan misses
# the Proton container stack entirely -- scripts/er-teardown.py's docstring records a sweep that
# reported itself clean and left 93 processes alive. Ask that tool instead; it also tells a live game
# from a two-thread husk at 0% CPU, which a bare pid check cannot.
python3 - "$REPO_ROOT" <<'PY' || fatal "eldenring.exe already running; tear it down first"
import importlib.util, sys
spec = importlib.util.spec_from_file_location("er_teardown", sys.argv[1] + "/scripts/er-teardown.py")
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)
sys.exit(1 if any(row["comm"] == "eldenring.exe" for row in module.survey()) else 0)
PY

# Stage the observer DLL as an me3 native (per-run immutable payload).
mkdir -p "$OBSERVE_DIR"
cp -f "$DLL" "$OBSERVE_DIR/er_quickload.dll"
me3_write_profile "$OBSERVE_DIR/er-quickload-observe.me3" "$OBSERVE_DIR/er_quickload.dll"

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

# EVERY per-run artifact goes into THIS run's directory. A GAME_DIR artifact is SINGLE-SLOT:
# the DLL rotates `<name>` to `<name>.prev` on its first write, so two launches lose the run
# before last, and several sessions launch concurrently here. `me3_launch` is a shell FUNCTION,
# so an `env VAR=... me3_launch` prefix would not work -- the redirects are exported instead.
# `ER_RUN_ARTIFACT_DIR` is what a watcher reads to find this run rather than the game directory.
export ER_RUN_ARTIFACT_DIR="$OBSERVE_DIR"
export ER_QUICKLOAD_TELEMETRY_PATH="$OBSERVE_DIR/er-quickload-telemetry.json"
export ER_QUICKLOAD_AUTOLOAD_DEBUG_PATH="$OBSERVE_DIR/er-quickload-autoload-debug.log"
export ER_QUICKLOAD_CRASH_LOG_PATH="$OBSERVE_DIR/er-quickload-crash-log.txt"
export ER_QUICKLOAD_TRACE_CONTINUE_PATH="$OBSERVE_DIR/er-quickload-continue-trace.log"
export ER_QUICKLOAD_INPUT_TRACE_PATH="$OBSERVE_DIR/er-quickload-input-trace.jsonl"
export ER_QUICKLOAD_BOOTSTRAP_PATH="$OBSERVE_DIR/er-quickload-bootstrap.jsonl"
export ER_QUICKLOAD_BOOTSTRAP_STATE_PATH="$OBSERVE_DIR/er-quickload-bootstrap-state.json"
export ER_QUICKLOAD_PROFILE_PATH="$OBSERVE_DIR/er-quickload-profile.jsonl"
export ER_QUICKLOAD_RELOAD_TRACE_PATH="$OBSERVE_DIR/er-reload-trace.log"
export ER_QUICKLOAD_INPUT_HARNESS_LOG_PATH="$OBSERVE_DIR/er-input-harness.log"
export ER_QUICKLOAD_INPUT_HARNESS_PHASES_PATH="$OBSERVE_DIR/er-input-harness-phases.jsonl"
export ER_QUICKLOAD_DIAG_HARNESS_PATH="$OBSERVE_DIR/er-diag-harness.log"
export ER_QUICKLOAD_TIMESERIES_PATH="$OBSERVE_DIR/er-telemetry-timeseries.jsonl"
export ER_QUICKLOAD_CPU_PROFILE_PATH="$OBSERVE_DIR/er-cpu-profile.txt"
export ER_QUICKLOAD_ARMAMENT_ICONS_PATH="$OBSERVE_DIR/er-armament-icons.log"
export ER_QUICKLOAD_SAVE_DISABLE_LOG_PATH="$OBSERVE_DIR/er-save-disable.log"
export ER_QUICKLOAD_SAVE_DISABLE_TELEMETRY_PATH="$OBSERVE_DIR/er-save-disable-telemetry.json"
export ER_QUICKLOAD_LOADING_PORTRAIT_PATH="$OBSERVE_DIR/er-loading-portrait.log"
export ER_QUICKLOAD_LOADING_PORTRAIT_CRASH_LOG_PATH="$OBSERVE_DIR/er-loading-portrait-crash-log.txt"
export ER_QUICKLOAD_CRASH_LOGGING_LOG_PATH="$OBSERVE_DIR/er-crash-log.txt"
export ER_QUICKLOAD_CRASH_LOGGING_LATEST_PATH="$OBSERVE_DIR/er-crash-latest.txt"
export ER_QUICKLOAD_CRASH_LOGGING_BREADCRUMB_PATH="$OBSERVE_DIR/er-crash-breadcrumb-latest.txt"
export ER_QUICKLOAD_CRASH_LOGGING_MODULES_PATH="$OBSERVE_DIR/er-crash-modules.txt"

cd "$GAME_DIR" || fatal "cannot cd to $GAME_DIR"
me3_launch "$OBSERVE_DIR/er-quickload-observe.me3"
RC=$?

# me3 returned (game exited or was killed): cancel the safety timer.
kill "$SAFETY_PID" >/dev/null 2>&1 || true
echo "run-golden-observe: eldenring.exe exited rc=$RC"
exit "$RC"
