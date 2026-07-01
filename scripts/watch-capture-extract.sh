#!/usr/bin/env bash
# Live view of the RenderDoc capture extraction (extract-capture.py under qrenderdoc),
# for a Kitty tab. Shows the log, whether qrenderdoc is still replaying (CPU/MEM), and any
# files written so far. Ctrl-C to stop watching (does not affect the extraction).
LOG="${EXTRACT_LOG:-/tmp/er-extract.log}"
OUT_ROOT="/home/banon/projects/er-effects-rs/target/capture"
while true; do
  clear
  echo "===== RenderDoc extract — live ($(date +%H:%M:%S)) ====="
  echo
  echo "--- $LOG ---"
  tail -30 "$LOG" 2>/dev/null || echo "(no log yet)"
  echo
  echo "--- qrenderdoc (replaying the capture?) ---"
  if pgrep -x qrenderdoc >/dev/null 2>&1; then
    ps -o pid,etime,%cpu,%mem,rss --no-headers -C qrenderdoc \
      | awk '{printf "  pid=%s  elapsed=%s  cpu=%s%%  mem=%s%%  rss=%dMB\n",$1,$2,$3,$4,$5/1024}'
  else
    echo "  (qrenderdoc not running — extraction finished or exited)"
  fi
  echo
  echo "--- extracted files (target/capture/) ---"
  find "$OUT_ROOT" -type f 2>/dev/null | sed "s#$OUT_ROOT/##" | head -40
  [ -d "$OUT_ROOT" ] || echo "  (none yet)"
  sleep 2
done
