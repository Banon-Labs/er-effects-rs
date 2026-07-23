#!/usr/bin/env bash
# Lifecycle manager for the PRE-WARMED headless Ghidra MCP server (MCPServeHeadless.java).
# Keeps one analyzeHeadless process alive with a program loaded so MCP tool calls are instant
# and Ghidra is NOT restarted per operation. The 13bm Go bridge (.mcp.json) connects to PORT.
#
#   scripts/ghidra/mcp-ghidra-daemon.sh start   [--proj-dir DIR] [--proj-name NAME] [--port N] [--readonly] [--save-interval N]
#   scripts/ghidra/mcp-ghidra-daemon.sh stop
#   scripts/ghidra/mcp-ghidra-daemon.sh status
#   scripts/ghidra/mcp-ghidra-daemon.sh restart [same flags as start]
#
# Defaults: the symbolized DUMP project (ermaporch), port 8765, WRITABLE with auto-save.
# MCP edits (rename/struct/comment/bookmark) PERSIST into the project: the daemon closes
# GhidraScript's wrapping transaction (see MCPServeHeadless.java) so mutations commit and a
# periodic save (default every 60s, plus a flush on clean stop) writes them back. A crash loses
# at most the last <save-interval seconds of edits. Pass --readonly for a query-only server, or
# --save-interval 0 to disable periodic save. To serve the deobf-native project instead:
#   ... start --proj-dir /home/banon/ghidra_maporch/proj-deobf --proj-name erdeobf
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# HEADLESS + maporch dir are current-user-aware (matches scripts/ghidra-query.sh). NEVER hard-code a
# user (bd prefer-ghidra-mcp-daemon-over-perquery-headless / Reusable Tooling path-correction rule).
resolve_headless() {
  if [[ -n "${GHIDRA_HEADLESS:-}" && -x "${GHIDRA_HEADLESS}" ]]; then printf '%s\n' "$GHIDRA_HEADLESS"; return 0; fi
  if [[ -n "${GHIDRA_INSTALL_DIR:-}" && -x "$GHIDRA_INSTALL_DIR/support/analyzeHeadless" ]]; then
    printf '%s\n' "$GHIDRA_INSTALL_DIR/support/analyzeHeadless"; return 0; fi
  local c
  for c in "$HOME"/tools/ghidra*/support/analyzeHeadless /mnt/d/ghidra/ghidra*/support/analyzeHeadless \
    /opt/ghidra*/support/analyzeHeadless /home/banon/tools/ghidra*/support/analyzeHeadless; do
    [[ -x "$c" ]] && { printf '%s\n' "$c"; return 0; }
  done
  return 1
}
HEADLESS="$(resolve_headless)" || { echo "analyzeHeadless not found; set GHIDRA_HEADLESS or GHIDRA_INSTALL_DIR" >&2; exit 3; }
GH_MAPORCH="${GHIDRA_MAPORCH_DIR:-$HOME/ghidra_maporch}"
[[ -d "$GH_MAPORCH" ]] || GH_MAPORCH="/home/banon/ghidra_maporch"
RUN_DIR="$GH_MAPORCH/mcp"
TMP="${GHIDRA_TMPDIR:-$GH_MAPORCH/tmp}"
LOG="$RUN_DIR/daemon.log"
STOPFILE="$RUN_DIR/STOP"
PIDFILE="$RUN_DIR/daemon.pid"

PROJ_DIR="${GHIDRA_PROJ_DIR:-$GH_MAPORCH/proj}"
PROJ_NAME="${GHIDRA_PROJ_NAME:-ermaporch}"
PORT=8765
RO=""        # default writable; edits persist via the daemon's periodic save. --readonly to opt out.
SAVE_SEC=60  # periodic auto-save interval (seconds); 0 disables. Edits also flush on clean stop.

CMD="${1:-}"; shift || true
while [[ $# -gt 0 ]]; do
  case "$1" in
    --proj-dir)      PROJ_DIR="$2"; shift 2 ;;
    --proj-name)     PROJ_NAME="$2"; shift 2 ;;
    --port)          PORT="$2"; shift 2 ;;
    --readonly)      RO="-readOnly"; shift ;;
    --save-interval) SAVE_SEC="$2"; shift 2 ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

mkdir -p "$RUN_DIR" "$TMP"
export TMPDIR="$TMP"
export GHIDRA_JAVA_OPTIONS="-Djava.io.tmpdir=$TMP"

is_running() { pgrep -f "MCPServeHeadless.java" >/dev/null 2>&1; }
port_up()    { ss -ltn 2>/dev/null | grep -q ":$PORT "; }

do_start() {
  if is_running; then echo "already running (pid $(pgrep -f MCPServeHeadless.java | tr '\n' ' '))"; return 0; fi
  rm -f "$STOPFILE"
  echo "starting MCP daemon: $PROJ_NAME on port $PORT ${RO:-(writable)}"
  # Isolate the postScript in a CLEAN script dir. Ghidra builds ONE OSGi bundle for the ENTIRE
  # -scriptPath directory, so a compile error in ANY sibling .java (scripts/ghidra holds ~40 RE
  # scripts) fails the whole bundle and MCPServeHeadless never loads ("Failed to get OSGi bundle
  # containing script"). Staging only this script sidesteps sibling-compile coupling.
  local MCP_SCRIPT_DIR="$GH_MAPORCH/mcp-script"
  mkdir -p "$MCP_SCRIPT_DIR"
  cp -f "$SCRIPT_DIR/MCPServeHeadless.java" "$MCP_SCRIPT_DIR/MCPServeHeadless.java"
  # Fully detach so the daemon outlives this shell/session; the stop-file is the clean exit.
  setsid bash -c "exec '$HEADLESS' '$PROJ_DIR' '$PROJ_NAME' -process -noanalysis $RO \
    -scriptPath '$MCP_SCRIPT_DIR' -postScript MCPServeHeadless.java '$PORT' '$STOPFILE' '$SAVE_SEC'" \
    >"$LOG" 2>&1 < /dev/null &
  echo $! > "$PIDFILE"
  # Event-driven readiness: block on the daemon's own READY/FAILED heartbeat line via `tail -F`
  # (retries until the log appears), bounded by a literal safety cap. No polling sleeps.
  timeout 30 grep -m1 -E "MCP_HEADLESS: (READY|FAILED)" <(tail -F -n +1 "$LOG" 2>/dev/null) >/dev/null 2>&1 || true
  if grep -q "MCP_HEADLESS: READY" "$LOG" 2>/dev/null; then
    echo "READY: $(grep 'MCP_HEADLESS: READY' "$LOG" | tail -1)"; return 0
  fi
  if grep -q "MCP_HEADLESS: FAILED" "$LOG" 2>/dev/null; then
    echo "FAILED to start; see $LOG" >&2; tail -20 "$LOG" >&2; return 1
  fi
  echo "timed out waiting for READY; see $LOG" >&2; tail -20 "$LOG" >&2; return 1
}

do_stop() {
  if ! is_running; then echo "not running"; rm -f "$STOPFILE"; return 0; fi
  echo "stopping (clean) ..."
  touch "$STOPFILE"
  local stop_pid; stop_pid="$(pgrep -f 'MCPServeHeadless.java' | head -1 || true)"
  if [[ -n "$stop_pid" ]]; then
    # Wait (bounded, literal cap) for the daemon to exit after the stop-file is dropped; `tail --pid`
    # returns the instant the process is gone. No polling sleeps.
    timeout 20 tail --pid="$stop_pid" -f /dev/null >/dev/null 2>&1 || true
  fi
  if ! is_running; then echo "stopped"; rm -f "$STOPFILE"; return 0; fi
  echo "clean stop timed out; killing" >&2
  pkill -f "MCPServeHeadless.java" || true
  rm -f "$STOPFILE"
}

case "$CMD" in
  start)   do_start ;;
  stop)    do_stop ;;
  restart) do_stop; do_start ;;
  status)
    if is_running; then echo "running (pid $(pgrep -f MCPServeHeadless.java | tr '\n' ' ')); port $PORT $(port_up && echo up || echo DOWN)"; else echo "stopped"; fi ;;
  *) echo "usage: $0 {start|stop|status|restart} [--proj-dir DIR] [--proj-name NAME] [--port N] [--readonly]" >&2; exit 2 ;;
esac
