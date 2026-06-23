#!/usr/bin/env bash
# Memory-bounded, RESUMABLE RF function-start finder (wraps FindFunctionStartsRFChunked.java).
# Classifies the undefined space in chunks, appending hits to a JSONL file and checkpointing a
# state file after each chunk -- so it runs at a modest heap (default 4G) and a kill/crash simply
# resumes. Designed for RAM-constrained boxes where the one-shot finder OOMs.
#
#   scripts/ghidra/find-functions-rf-chunked.sh [--proj-dir DIR] [--proj-name NAME]
#       [--threshold 0.80] [--max-starts 500] [--min-range 16] [--chunk-size 20000]
#       [--max-mem 4G] [--out FILE.jsonl] [--reset]
#
# Idempotent: re-running continues from the checkpoint. --reset starts fresh.
# Results stream to the JSONL out file; a deduped/sorted summary prints at the end.
set -uo pipefail

PROJ_DIR=/home/banon/ghidra_maporch/proj-deobf
PROJ_NAME=erdeobf
THRESHOLD=0.80
# Training fits in ~2G regardless (it was never the OOM -- only the monolithic classify was), so
# use the full training set for good recall; chunking keeps classify memory bounded either way.
MAX_STARTS=1000
MIN_RANGE=16
CHUNK=20000
MAXMEM=4G
OUT=/home/banon/ghidra_maporch/tmp/erdeobf-rf.jsonl
RESET=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --proj-dir)   PROJ_DIR="$2"; shift 2 ;;
    --proj-name)  PROJ_NAME="$2"; shift 2 ;;
    --threshold)  THRESHOLD="$2"; shift 2 ;;
    --max-starts) MAX_STARTS="$2"; shift 2 ;;
    --min-range)  MIN_RANGE="$2"; shift 2 ;;
    --chunk-size) CHUNK="$2"; shift 2 ;;
    --max-mem)    MAXMEM="$2"; shift 2 ;;
    --out)        OUT="$2"; shift 2 ;;
    --reset)      RESET=1; shift ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HEADLESS=/home/banon/tools/ghidra_12.1_PUBLIC/support/analyzeHeadless
TMP=/home/banon/ghidra_maporch/tmp
STATE="$OUT.state"
LOG="$TMP/$(basename "$OUT").run.log"

mkdir -p "$TMP"
export TMPDIR="$TMP"
export GHIDRA_JAVA_OPTIONS="-Djava.io.tmpdir=$TMP"
export GHIDRA_MAXMEM="$MAXMEM"

if [[ "$RESET" == 1 ]]; then
  rm -f "$OUT" "$STATE"
  echo "reset: cleared $OUT and $STATE"
fi

echo "RF chunked: $PROJ_NAME chunk=$CHUNK heap=$MAXMEM out=$OUT (resumable)"
"$HEADLESS" "$PROJ_DIR" "$PROJ_NAME" \
  -process -noanalysis -readOnly \
  -scriptPath "$SCRIPT_DIR" \
  -postScript FindFunctionStartsRFChunked.java "$THRESHOLD" "$MAX_STARTS" "$MIN_RANGE" "$CHUNK" "$OUT" "$STATE" \
  >"$LOG" 2>&1
rc=$?

if grep -q "OutOfMemory" "$LOG" 2>/dev/null; then
  echo "OOM at heap=$MAXMEM -- rerun with a larger --max-mem or smaller --chunk-size; progress is checkpointed in $STATE so it resumes." >&2
fi
if [[ $rc -ne 0 ]]; then
  echo "run exited rc=$rc (see $LOG). If interrupted, just rerun the same command to resume." >&2
fi

if [[ -f "$OUT" ]]; then
  n=$(sort -u "$OUT" | grep -c '"va"' || true)
  echo "candidates so far (unique): $n  in $OUT"
  echo "# top by score:"
  sort -u "$OUT" | python3 -c "import sys,json; rows=[json.loads(l) for l in sys.stdin if l.strip()]; rows.sort(key=lambda r:-r['score']);
import itertools
[print('  ',r['va'],r['score']) for r in rows[:10]]" 2>/dev/null || true
fi
echo "# full log: $LOG"
