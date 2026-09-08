#!/usr/bin/env bash
# Bring up the Ghidra MCP daemon on the ELDEN RING 1.17 runtime dump and validate it.
#
# Sibling of mcp-up-1162.sh. The two are meant to run at the same TIME: 1.16.2 on :8765 and 1.17
# on :8767, so the same question can be asked of both images in one session. That is the entire
# point during this migration -- "where did this function go" is a two-image question.
#
# :8766 is deliberately not used. It belongs to an unrelated DarkSoulsII.exe daemon on this
# machine; taking it would collide with a live user session.
#
#   scripts/ghidra/mcp-up-1170.sh            # start + validate
#   scripts/ghidra/mcp-up-1170.sh --port 8768
#
# Project comes from scripts/ghidra/import-runtime-gzf.sh:
#   --gzf ~/pc_eldenring_runtime.1.17.0.exe.gzf --proj-dir ~/ghidra_maporch/proj1170 \
#   --proj-name ermaporch1170
set -uo pipefail
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

# 12.1.2 is required, not preferred: 12.1 cannot read a 1.16.2-or-newer gzf (x86 language V4.7+).
# Prefer the local copy under $HOME/tools (native FS) over the drvfs mount, which may be unmounted.
if [[ -z "${GHIDRA_INSTALL_DIR:-}" ]]; then
	for c in "$HOME/tools/ghidra_12.1.2_PUBLIC" /mnt/d/ghidra/ghidra_12.1.2_PUBLIC /opt/ghidra_12.1.2_PUBLIC; do
		[[ -x "$c/support/analyzeHeadless" ]] && { GHIDRA_INSTALL_DIR="$c"; break; }
	done
fi
export GHIDRA_INSTALL_DIR="${GHIDRA_INSTALL_DIR:-$HOME/tools/ghidra_12.1.2_PUBLIC}"
export JAVA_HOME="${JAVA_HOME:-/usr/lib/jvm/java-21-openjdk-amd64}"
PROJ_DIR="${GHIDRA_PROJ_DIR:-$HOME/ghidra_maporch/proj1170}"
PROJ_NAME="${GHIDRA_PROJ_NAME:-ermaporch1170}"
PORT="${GHIDRA_MCP_PORT:-8767}"

[[ -x "$GHIDRA_INSTALL_DIR/support/analyzeHeadless" ]] || {
	echo "12.1.2 analyzeHeadless not found under $GHIDRA_INSTALL_DIR" >&2; exit 2; }
[[ -f "$PROJ_DIR/$PROJ_NAME.gpr" ]] || {
	echo "1.17 project not found: $PROJ_DIR/$PROJ_NAME.gpr" >&2
	echo "import it first: scripts/ghidra/import-runtime-gzf.sh --gzf ~/pc_eldenring_runtime.1.17.0.exe.gzf --proj-dir $PROJ_DIR --proj-name $PROJ_NAME" >&2
	exit 2; }

bash "$REPO/scripts/ghidra/mcp-ghidra-daemon.sh" start \
	--proj-dir "$PROJ_DIR" --proj-name "$PROJ_NAME" --port "$PORT" "$@"

# Per-port log, matching the daemon's own naming (8765 keeps the legacy unsuffixed name).
SUF=""; [[ "$PORT" != "8765" ]] && SUF="-$PORT"
LOG="$HOME/ghidra_maporch/mcp/daemon${SUF}.log"

# Loading a ~26G project exceeds the daemon's own 30s ready wait, so block event-driven on its
# ready heartbeat in bounded 30s segments (per-op 30s cap). No polling sleeps. Bail early on failed.
for _ in 1 2 3 4 5 6 7 8; do
	timeout 30 grep -m1 "MCP_HEADLESS: READY" <(tail -F -n +1 "$LOG" 2>/dev/null) >/dev/null 2>&1 && break
	grep -q "MCP_HEADLESS: FAILED" "$LOG" 2>/dev/null && break
done

if python3 "$REPO/scripts/ghidra/mcp_query.py" ping --port "$PORT" >/dev/null 2>&1; then
	echo "== MCP daemon READY on :$PORT =="
	# getContext names the program, which is the check that matters: a wrong-project daemon answers
	# every query confidently and wrongly. Confirm it says 1.17 before trusting anything it returns.
	python3 "$REPO/scripts/ghidra/mcp_query.py" getContext --port "$PORT"
	exit 0
fi
echo "== daemon did not answer ping on :$PORT; see $LOG ==" >&2
exit 1
