#!/usr/bin/env bash
# Lifecycle manager for the PRE-WARMED headless Ghidra MCP server (MCPServeHeadless.java).
# Keeps one analyzeHeadless process alive with a program loaded so MCP tool calls are instant
# and Ghidra is NOT restarted per operation. The 13bm Go bridge (.mcp.json) connects to PORT.
#
#   scripts/ghidra/mcp-ghidra-daemon.sh start   [--proj-dir DIR] [--proj-name NAME] [--port N] [--writable]
#   scripts/ghidra/mcp-ghidra-daemon.sh stop
#   scripts/ghidra/mcp-ghidra-daemon.sh status
#   scripts/ghidra/mcp-ghidra-daemon.sh restart [same flags as start]
#
# Defaults: the symbolized DUMP project (ermaporch), port 8765, READ-ONLY. Use --writable only
# when you intend the agent's rename/struct edits to persist (mutates the shared project!).
# To serve the deobf-native project instead:
#   ... start --proj-dir /home/banon/ghidra_maporch/proj-deobf --proj-name erdeobf
set -euo pipefail

HEADLESS=/home/banon/tools/ghidra_12.1_PUBLIC/support/analyzeHeadless
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RUN_DIR=/home/banon/ghidra_maporch/mcp
TMP=/home/banon/ghidra_maporch/tmp
LOG="$RUN_DIR/daemon.log"
STOPFILE="$RUN_DIR/STOP"
PIDFILE="$RUN_DIR/daemon.pid"

PROJ_DIR=/home/banon/ghidra_maporch/proj
PROJ_NAME=ermaporch
PORT=8765
RO="-readOnly"

CMD="${1:-}"; shift || true
while [[ $# -gt 0 ]]; do
  case "$1" in
    --proj-dir)  PROJ_DIR="$2"; shift 2 ;;
    --proj-name) PROJ_NAME="$2"; shift 2 ;;
    --port)      PORT="$2"; shift 2 ;;
    --writable)  RO=""; shift ;;
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
  # Fully detach so the daemon outlives this shell/session; the stop-file is the clean exit.
  setsid bash -c "exec '$HEADLESS' '$PROJ_DIR' '$PROJ_NAME' -process -noanalysis $RO \
    -scriptPath '$SCRIPT_DIR' -postScript MCPServeHeadless.java '$PORT' '$STOPFILE'" \
    >"$LOG" 2>&1 < /dev/null &
  echo $! > "$PIDFILE"
  # Wait (bounded) for readiness rather than sleeping blindly.
  for _ in $(seq 1 25); do
    if grep -q "MCP_HEADLESS: READY" "$LOG" 2>/dev/null; then
      echo "READY: $(grep 'MCP_HEADLESS: READY' "$LOG" | tail -1)"; return 0
    fi
    if grep -q "MCP_HEADLESS: FAILED" "$LOG" 2>/dev/null; then
      echo "FAILED to start; see $LOG" >&2; tail -20 "$LOG" >&2; return 1
    fi
    sleep 1
  done
  echo "timed out waiting for READY; see $LOG" >&2; tail -20 "$LOG" >&2; return 1
}

do_stop() {
  if ! is_running; then echo "not running"; rm -f "$STOPFILE"; return 0; fi
  echo "stopping (clean) ..."
  touch "$STOPFILE"
  for _ in $(seq 1 15); do is_running || { echo "stopped"; rm -f "$STOPFILE"; return 0; }; sleep 1; done
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
  *) echo "usage: $0 {start|stop|status|restart} [--proj-dir DIR] [--proj-name NAME] [--port N] [--writable]" >&2; exit 2 ;;
esac
