#!/usr/bin/env bash
# Lifecycle manager for the pre-warmed headless Ghidra MCP server (MCPServeHeadless.java).
# Keeps one analyzeHeadless process alive with a program loaded so MCP tool calls are instant
# and Ghidra is not restarted per operation. The 13bm Go bridge (.mcp.json) connects to port.
#
#   scripts/ghidra/mcp-ghidra-daemon.sh start   [--proj-dir DIR] [--proj-name name] [--port N] [--readonly] [--save-interval N]
#   scripts/ghidra/mcp-ghidra-daemon.sh stop
#   scripts/ghidra/mcp-ghidra-daemon.sh status
#   scripts/ghidra/mcp-ghidra-daemon.sh restart [same flags as start]
#
# Defaults: the symbolized dump project (ermaporch), port 8765, WRITABLE with auto-save.
# MCP edits (rename/struct/comment/bookmark) persist into the project: the daemon closes
# GhidraScript's wrapping transaction (see MCPServeHeadless.java) so mutations commit and a
# periodic save (default every 60s, plus a flush on clean stop) writes them back. A crash loses
# at most the last <save-interval seconds of edits. Pass --readonly for a query-only server, or
# --save-interval 0 to disable periodic save. To serve the deobf-native project instead:
#   ... start --proj-dir /home/banon/ghidra_maporch/proj-deobf --proj-name erdeobf
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# Headless + maporch dir are current-user-aware (matches scripts/ghidra-query.sh). Never hard-code a
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

# Per-port run state, so two dumps (1.16.2 on 8765, 1.17 on 8766) can be live at once -- which is
# the whole point during a version migration: the same question asked of both images in one session.
# Port 8765 deliberately keeps the original unsuffixed filenames, so a daemon started before this
# change stays manageable rather than being orphaned by its pidfile moving out from under it.
SUF=""
[[ "$PORT" != "8765" ]] && SUF="-$PORT"
LOG="$RUN_DIR/daemon${SUF}.log"
STOPFILE="$RUN_DIR/STOP${SUF}"
PIDFILE="$RUN_DIR/daemon${SUF}.pid"

mkdir -p "$RUN_DIR" "$TMP"
export TMPDIR="$TMP"
export GHIDRA_JAVA_OPTIONS="-Djava.io.tmpdir=$TMP"

# Liveness for this port only. The pidfile is authoritative: `setsid bash -c "exec ..."` execs into
# the JVM, so $! is the java process itself. A bare `pgrep -f MCPServeHeadless.java` matches every
# daemon, which previously made do_start refuse to bring up a second dump ("already running") and
# made do_stop's pkill fallback take down both.
daemon_pid() {
  local p
  [[ -f "$PIDFILE" ]] && p="$(cat "$PIDFILE" 2>/dev/null)" || p=""
  if [[ -n "$p" ]] && kill -0 "$p" 2>/dev/null; then printf '%s\n' "$p"; return 0; fi
  # Fallback for a pidfile lost to a reboot/manual start: match the port argument, not the class.
  p="$(pgrep -f "MCPServeHeadless.java $PORT " 2>/dev/null | head -1 || true)"
  [[ -n "$p" ]] && { printf '%s\n' "$p"; return 0; }
  return 1
}
is_running() { daemon_pid >/dev/null 2>&1; }
port_up()    { ss -ltn 2>/dev/null | grep -q ":$PORT "; }

# Self-heal exec bits on the install's native helper binaries (decompile, sleigh, demanglers,
# lzfse). A Ghidra install copied off a Windows/drvfs mount silently loses +x on these; Ghidra's
# Java side still works, but DecompInterface cannot spawn `decompile`, so every MCP decompile
# returns "Decompilation failed" while disasm/xref/symbol queries keep working (root-caused
# 2026-07-28: ~/tools/ghidra_12.1.2_PUBLIC had decompile/sleigh at 0664). chmod is idempotent
# and cheap; run it every start so a re-copied install can never regress decompilation.
fix_native_exec_bits() {
  local root="${HEADLESS%/support/analyzeHeadless}"
  [[ -d "$root" ]] || return 0
  find "$root" -type f -path '*/os/linux_x86_64/*' ! -name '*.txt' ! -perm -u+x \
    -exec chmod a+x {} + 2>/dev/null || true
}

do_start() {
  # Heal exec bits even when the daemon is already live: the decompile process is spawned
  # per-request, so restoring +x fixes decompilation without a restart (verified 2026-07-28).
  fix_native_exec_bits
  if is_running; then echo "already running on :$PORT (pid $(daemon_pid))"; return 0; fi
  rm -f "$STOPFILE"
  echo "starting MCP daemon: $PROJ_NAME on port $PORT ${RO:-(writable)}"
  # Isolate the postScript in a clean script dir. Ghidra builds one OSGi bundle for the entire
  # -scriptPath directory, so a compile error in any sibling .java (scripts/ghidra holds ~40 RE
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
  local stop_pid; stop_pid="$(daemon_pid || true)"
  if [[ -n "$stop_pid" ]]; then
    # Wait (bounded, literal cap) for the daemon to exit after the stop-file is dropped; `tail --pid`
    # returns the instant the process is gone. No polling sleeps.
    timeout 20 tail --pid="$stop_pid" -f /dev/null >/dev/null 2>&1 || true
  fi
  if ! is_running; then echo "stopped"; rm -f "$STOPFILE"; return 0; fi
  echo "clean stop timed out; killing :$PORT only" >&2
  # Never `pkill -f MCPServeHeadless.java` here -- that matches every daemon and would take down
  # the other version's dump alongside this one.
  local kill_pid; kill_pid="$(daemon_pid || true)"
  [[ -n "$kill_pid" ]] && kill -9 "$kill_pid" 2>/dev/null || true
  rm -f "$STOPFILE" "$PIDFILE"
}

case "$CMD" in
  start)   do_start ;;
  stop)    do_stop ;;
  restart) do_stop; do_start ;;
  status)
    if is_running; then echo "running (pid $(daemon_pid)); port $PORT $(port_up && echo up || echo DOWN)"; else echo "stopped (port $PORT)"; fi ;;
  *) echo "usage: $0 {start|stop|status|restart} [--proj-dir DIR] [--proj-name NAME] [--port N] [--readonly]" >&2; exit 2 ;;
esac
