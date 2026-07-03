#!/usr/bin/env bash
# Diagnostic: burst-capture the ER window through the ProfileSelect-confirm window so we can SEE the
# MessageBox(es) the System->Quit switch drive shows (~+31-33s), plus the final loaded world.
# Validated ER-window capture only (capture-er-window.py). Tears ER down after the reload commits or
# a hard cap. Artifacts land under the probe artifact dir (not versioned).
set -u
REPO=/home/banon/projects/er-effects-rs
ART="${ARTIFACT_DIR:-$REPO/target/runtime-probe/system-quit-repro-selfdrive}"
TELEM="$ART/er-effects-telemetry.json"
OUT="$ART/msgbox-capture"
CAP="$REPO/scripts/capture-er-window.py"
mkdir -p "$OUT"
BURST_START=${BURST_START:-27}   # seconds after watcher start to begin bursting
BURST_END=${BURST_END:-37}       # stop bursting
DEADLINE_S=${DEADLINE_S:-85}
POLL_S=${POLL_S:-2}

er_alive() { python3 -c "
import glob
def comm(p):
    try: return open(p).read().strip()
    except OSError: return ''
print('1' if any(comm(p)=='eldenring.exe' for p in glob.glob('/proc/[0-9]*/comm')) else '0')
"; }
tf() { python3 -c "
import json,sys
try: t=json.load(open('$TELEM'))
except Exception: print(0); sys.exit()
print(t.get('$1',0))
"; }

start=$SECONDS
saw_er=0
n=0
final_done=0
while (( SECONDS - start < DEADLINE_S )); do
  t=$(( SECONDS - start ))
  alive=$(er_alive)
  [[ "$alive" == "1" ]] && saw_er=1
  if [[ "$saw_er" == "1" && "$alive" == "0" ]]; then
    echo "[cap +${t}s] ER exited"
    break
  fi
  # Burst window: capture every loop iteration (fast) to catch the brief msgbox.
  if (( t >= BURST_START && t <= BURST_END )); then
    f=$(printf "%s/frame-%02d-t%03ds.jpg" "$OUT" "$n" "$t")
    python3 "$CAP" "$f" >/dev/null 2>&1
    mbox=$(tf oracle_msgbox_total_builds)
    echo "[cap +${t}s] burst frame $n -> $(basename "$f")  msgbox_builds=$mbox"
    n=$((n+1))
    continue   # tight loop during burst (no long sleep)
  fi
  deser=$(tf system_quit_continue_confirm_fresh_deser_count)
  mbox=$(tf oracle_msgbox_total_builds)
  echo "[cap +${t}s] alive=$alive deser_ok=$deser msgbox_builds=$mbox"
  # After the reload commits + a moment to stream, grab the final world then tear down.
  if [[ "${deser:-0}" -ge 1 && "$final_done" == "0" && "$t" -ge "$((BURST_END+3))" ]]; then
    python3 "$CAP" "$OUT/final-loaded-world.jpg" >/dev/null 2>&1
    echo "[cap +${t}s] captured final-loaded-world.jpg (deser_ok=$deser)"
    final_done=1
    break
  fi
  sleep "$POLL_S"
done

echo "[cap] tearing down eldenring.exe"
pkill -x eldenring.exe 2>/dev/null
sleep 1
echo "[cap] final ER alive=$(er_alive); frames in $OUT:"
ls -1 "$OUT" 2>/dev/null | tail -40
