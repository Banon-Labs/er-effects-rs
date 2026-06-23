#!/usr/bin/env bash
# Run the read-only Random-Forest function-start finder (FindFunctionStartsRF.java) against a
# persistent, already-analyzed Ghidra project and print the discovered candidate VAs as JSON.
#
#   scripts/ghidra/find-functions-rf.sh [--proj-dir DIR] [--proj-name NAME] \
#       [--threshold 0.80] [--max-starts 1000] [--min-range 16] [--log FILE]
#
# Defaults target the persistent ER runtime DUMP project (ermaporch). NOTE: the dump is already
# heavily symbolized, so it is mainly a smoke-test target -- few functions remain undiscovered,
# and its addresses carry the ~0x10 dump-vs-deobf shift (see AGENTS.md). For real address-bearing
# results, point --proj-dir/--proj-name at the deobf-binary project (scripts/ghidra/import-deobf.sh).
#
# Same env gotchas as ghidra-query.sh: java.io.tmpdir is forced onto /home (the /tmp tmpfs
# is a near-full 32G and overflows when Ghidra unpacks program data).
set -euo pipefail

# Default to the deobf-native project (erdeobf): RF is only useful for call/patch-able VAs, and
# the dump (ermaporch) is both slow to classify and exclusively locked by a running MCP daemon.
PROJ_DIR=/home/banon/ghidra_maporch/proj-deobf
PROJ_NAME=erdeobf
THRESHOLD=0.80
MAX_STARTS=1000
MIN_RANGE=16
LOG=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --proj-dir)   PROJ_DIR="$2"; shift 2 ;;
    --proj-name)  PROJ_NAME="$2"; shift 2 ;;
    --threshold)  THRESHOLD="$2"; shift 2 ;;
    --max-starts) MAX_STARTS="$2"; shift 2 ;;
    --min-range)  MIN_RANGE="$2"; shift 2 ;;
    --log)        LOG="$2"; shift 2 ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HEADLESS=/home/banon/tools/ghidra_12.1_PUBLIC/support/analyzeHeadless
TMP=/home/banon/ghidra_maporch/tmp

mkdir -p "$TMP"
export TMPDIR="$TMP"
export GHIDRA_JAVA_OPTIONS="-Djava.io.tmpdir=$TMP"

if [[ -z "$LOG" ]]; then
  LOG="$(mktemp "$TMP/rf-finder.XXXXXX.log")"
fi

# -process (no -import) reopens the SAVED program; the RF script is read-only so -readOnly is safe.
"$HEADLESS" "$PROJ_DIR" "$PROJ_NAME" \
  -process \
  -noanalysis \
  -readOnly \
  -scriptPath "$SCRIPT_DIR" \
  -postScript FindFunctionStartsRF.java "$THRESHOLD" "$MAX_STARTS" "$MIN_RANGE" \
  >"$LOG" 2>&1 || {
    if grep -q "Unable to lock project" "$LOG"; then
      echo "ERROR: project '$PROJ_NAME' is locked -- an MCP daemon (or other Ghidra process)" >&2
      echo "       holds it. Stop it first: scripts/ghidra/mcp-ghidra-daemon.sh stop" >&2
    else
      echo "analyzeHeadless failed; see $LOG" >&2
      tail -40 "$LOG" >&2
    fi
    exit 1
  }

# Slice the JSON payload out of the headless log between the markers.
if ! sed -n '/RF_RESULT_JSON_BEGIN/,/RF_RESULT_JSON_END/p' "$LOG" \
     | sed '1d;$d'; then
  echo "no RF_RESULT_JSON block found; full log at $LOG" >&2
  exit 1
fi
echo "# full headless log: $LOG" >&2
