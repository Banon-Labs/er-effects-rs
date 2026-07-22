#!/usr/bin/env bash
# Bring up the Ghidra MCP daemon on the ELDEN RING 1.16.2 runtime dump and validate it.
#
# The 1.16.2 gzf requires Ghidra 12.1.2 (x86 language V4.7+); 12.1 fails to import it
# (bd 1162-gzf-needs-ghidra-1212-not-121-2026-07-20). This wrapper pins the 12.1.2 install,
# the imported project (ermaporch1162 in ~/ghidra_maporch/proj1162), starts the long-lived
# headless MCP daemon (mcp-ghidra-daemon.sh), and pings it so callers get one lock-free MCP
# endpoint on :8765 (bd prefer-ghidra-mcp-daemon-over-perquery-headless-to-avoid-lock).
#
#   scripts/ghidra/mcp-up-1162.sh            # start + validate
#   scripts/ghidra/mcp-up-1162.sh --port 8766
set -uo pipefail
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

export GHIDRA_INSTALL_DIR="${GHIDRA_INSTALL_DIR:-/mnt/d/ghidra/ghidra_12.1.2_PUBLIC}"
export JAVA_HOME="${JAVA_HOME:-/usr/lib/jvm/java-21-openjdk-amd64}"
PROJ_DIR="${GHIDRA_PROJ_DIR:-$HOME/ghidra_maporch/proj1162}"
PROJ_NAME="${GHIDRA_PROJ_NAME:-ermaporch1162}"
PORT="${GHIDRA_MCP_PORT:-8765}"

[[ -x "$GHIDRA_INSTALL_DIR/support/analyzeHeadless" ]] || {
	echo "12.1.2 analyzeHeadless not found under $GHIDRA_INSTALL_DIR" >&2; exit 2; }
[[ -f "$PROJ_DIR/$PROJ_NAME.gpr" ]] || {
	echo "1.16.2 project not found: $PROJ_DIR/$PROJ_NAME.gpr (import the gzf first)" >&2; exit 2; }

bash "$REPO/scripts/ghidra/mcp-ghidra-daemon.sh" start \
	--proj-dir "$PROJ_DIR" --proj-name "$PROJ_NAME" --port "$PORT" "$@"

# Validate. Loading the 1.16.2 program takes longer than the daemon's own 30s READY wait, so block
# EVENT-DRIVEN on the daemon's READY heartbeat via `tail -F` (no polling sleeps -- same pattern the
# daemon uses), then ping + fetch program info over the lock-free direct client.
LOG="$HOME/ghidra_maporch/mcp/daemon.log"
# Wait for the daemon's READY heartbeat in <=30s bounded segments (per-op 30s cap; the 1.16.2 program
# load exceeds one segment). Event-driven via `tail -F`, no polling sleeps. Bail early on FAILED.
for _ in 1 2 3 4 5 6 7 8; do
	timeout 30 grep -m1 "MCP_HEADLESS: READY" <(tail -F -n +1 "$LOG" 2>/dev/null) >/dev/null 2>&1 && break
	grep -q "MCP_HEADLESS: FAILED" "$LOG" 2>/dev/null && break
done
if python3 "$REPO/scripts/ghidra/mcp_query.py" ping --port "$PORT" >/dev/null 2>&1; then
	echo "== MCP daemon READY on :$PORT =="
	python3 "$REPO/scripts/ghidra/mcp_query.py" get_program_info --port "$PORT"
	exit 0
fi
echo "== daemon did not answer ping on :$PORT; see $LOG ==" >&2
exit 1
