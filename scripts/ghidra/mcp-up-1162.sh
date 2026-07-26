#!/usr/bin/env bash
# Bring up the Ghidra MCP daemon on the ELDEN RING 1.16.2 runtime dump and validate it.
#
# The 1.16.2 gzf requires Ghidra 12.1.2 (x86 language V4.7+); 12.1 fails to import it
# (bd 1162-gzf-needs-ghidra-1212-not-121-2026-07-20). This wrapper uses the current-user
# project path, the known local 1.16.2 gzf when present, and env overrides for every machine path.
# It starts the long-lived headless MCP daemon (mcp-ghidra-daemon.sh) and pings it so callers get
# one lock-free MCP endpoint on :8765.
#
#   scripts/ghidra/mcp-up-1162.sh                    # start existing imported project + validate
#   scripts/ghidra/mcp-up-1162.sh --import            # import/update project from the 1.16.2 gzf first
#   GHIDRA_1162_GZF=/path/to/pc_eldenring_runtime.1.16.2.exe.gzf scripts/ghidra/mcp-up-1162.sh --import
#   scripts/ghidra/mcp-up-1162.sh --port 8766
set -uo pipefail
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# shellcheck source=scripts/ghidra/resolve-ghidra.sh
source "$REPO/scripts/ghidra/resolve-ghidra.sh"

IMPORT_FIRST=0
EXTRA_ARGS=()
while [[ $# -gt 0 ]]; do
	case "$1" in
		--import)
			IMPORT_FIRST=1
			shift
			;;
		--port)
			[[ $# -ge 2 ]] || { echo "--port requires a value" >&2; exit 2; }
			PORT="$2"
			EXTRA_ARGS+=("$1" "$2")
			shift 2
			;;
		*)
			EXTRA_ARGS+=("$1")
			shift
			;;
	esac
done

# Current-user defaults; env overrides remain authoritative for other machines.
DEFAULT_GZF="/mnt/net/terra-Documents/pc_eldenring_runtime.1.16.2.exe.gzf"
GHIDRA_INSTALL_DIR="$(resolve_ghidra_install_dir)" || { echo "no executable Ghidra install found; set GHIDRA_INSTALL_DIR" >&2; exit 2; }
export GHIDRA_INSTALL_DIR
export JAVA_HOME="${JAVA_HOME:-/usr/lib/jvm/java-21-openjdk-amd64}"
GZF="${GHIDRA_1162_GZF:-${GZF:-$DEFAULT_GZF}}"
PROJ_DIR="${GHIDRA_PROJ_DIR:-$HOME/ghidra_maporch/proj1162}"
PROJ_NAME="${GHIDRA_PROJ_NAME:-ermaporch1162}"
PORT="${PORT:-${GHIDRA_MCP_PORT:-8765}}"
TMP="${GHIDRA_TMPDIR:-$HOME/ghidra_maporch/tmp1162}"

[[ -x "$GHIDRA_INSTALL_DIR/support/analyzeHeadless" ]] || {
	echo "Ghidra analyzeHeadless not found under $GHIDRA_INSTALL_DIR" >&2
	echo "Install/unpack the newest Ghidra under ~/tools or set GHIDRA_INSTALL_DIR=/path/to/ghidra_X_PUBLIC" >&2
	exit 2
}
if ! resolve_ghidra_version_at_least "$GHIDRA_INSTALL_DIR" "12.1.2"; then
	echo "Ghidra 1.16.2 dump support requires >= 12.1.2; newest resolved install is $GHIDRA_INSTALL_DIR ($(resolve_ghidra_version "$GHIDRA_INSTALL_DIR"))" >&2
	exit 2
fi
[[ -f "$GZF" ]] || {
	echo "1.16.2 dump gzf not found: $GZF (set GHIDRA_1162_GZF=/path/to/pc_eldenring_runtime.1.16.2.exe.gzf)" >&2
	exit 2
}

mkdir -p "$TMP" "$PROJ_DIR"
export TMPDIR="$TMP"
export GHIDRA_JAVA_OPTIONS="-Djava.io.tmpdir=$TMP"

if [[ "$IMPORT_FIRST" -eq 1 || ! -f "$PROJ_DIR/$PROJ_NAME.gpr" ]]; then
	if [[ "$IMPORT_FIRST" -ne 1 ]]; then
		echo "1.16.2 project not found: $PROJ_DIR/$PROJ_NAME.gpr; importing from $GZF" >&2
	fi
	"$GHIDRA_INSTALL_DIR/support/analyzeHeadless" "$PROJ_DIR" "$PROJ_NAME" \
		-import "$GZF" \
		-noanalysis \
		-overwrite || exit $?
fi

[[ -f "$PROJ_DIR/$PROJ_NAME.gpr" ]] || {
	echo "1.16.2 project still missing after import attempt: $PROJ_DIR/$PROJ_NAME.gpr" >&2
	exit 2
}

bash "$REPO/scripts/ghidra/mcp-ghidra-daemon.sh" start \
	--proj-dir "$PROJ_DIR" --proj-name "$PROJ_NAME" --port "$PORT" "${EXTRA_ARGS[@]}"

# Validate. Loading the 1.16.2 program can exceed the daemon's own short READY wait, so block
# EVENT-DRIVEN on the daemon's READY heartbeat via `tail -F` in bounded 30s segments. No blind sleeps.
LOG="$HOME/ghidra_maporch/mcp/daemon.log"
for _ in 1 2 3 4 5 6 7 8; do
	timeout 30 grep -m1 "MCP_HEADLESS: READY" <(tail -F -n +1 "$LOG" 2>/dev/null) >/dev/null 2>&1 && break
	grep -q "MCP_HEADLESS: FAILED" "$LOG" 2>/dev/null && break
done
if python3 "$REPO/scripts/ghidra/mcp_query.py" ping --port "$PORT" >/dev/null 2>&1; then
	echo "== MCP daemon READY on :$PORT =="
	python3 "$REPO/scripts/ghidra/mcp_query.py" getContext --port "$PORT"
	exit 0
fi
echo "== daemon did not answer ping on :$PORT; see $LOG ==" >&2
exit 1
