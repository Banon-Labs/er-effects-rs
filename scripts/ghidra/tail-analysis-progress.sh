#!/usr/bin/env bash
# Live percentage tail for the headless Ghidra analysis heartbeat (AnalyzeWithProgress.java).
# Follows the import log and prints an updating "% complete" line. The percentage is an ESTIMATE:
# functions-discovered / expected-total, where the expected total defaults to the 1.16.1 runtime
# dump's function count (~366744). The deobf binary's true total may differ, so the bar is
# capped at 99% until the analyzer prints DONE (then it shows 100%).
#
#   scripts/ghidra/tail-analysis-progress.sh [LOG] [TARGET_FUNC_COUNT]
LOG="${1:-/home/banon/ghidra_maporch/tmp/deobf-import.log}"
TARGET="${2:-366744}"

echo "tailing $LOG  (estimated total funcs ~ $TARGET)"
echo "Ctrl-C to stop watching (does not affect the analysis)."
echo

# Event-driven: `tail -F` waits for the log to appear (it retries on a missing file) and then follows
# each heartbeat line as it is written -- so no explicit file-existence poll/sleep is needed.
tail -n +1 -F "$LOG" 2>/dev/null | awk -v target="$TARGET" '
  /ANALYZE_PROGRESS:.*funcs=/ {
    f=""; ins="";
    for (i=1;i<=NF;i++) {
      if ($i ~ /^funcs=/)  { split($i,a,"="); f=a[2] }
      if ($i ~ /^instrs=/) { split($i,b,"="); ins=b[2] }
    }
    if (f != "") {
      pct = (target>0) ? f*100.0/target : 0;
      if (pct > 99 && $0 !~ /DONE/) pct = 99;
      printf "\r%6.2f%%   funcs=%-8s instrs=%-12s (target~%s)        ", pct, f, ins, target;
      fflush();
    }
  }
  /ANALYZE_PROGRESS: DONE/ {
    printf "\n100.00%%  analysis COMPLETE  -> %s\n", $0; fflush();
  }
'
