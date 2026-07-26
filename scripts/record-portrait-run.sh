#!/usr/bin/env bash
# Record the ER portrait run: capture ONLY the Elden Ring window at 60fps native res, then extract
# 60fps frames, then open the output folder. Privacy: captures only the steam_app_1245620 window region.
# TIME-BOUNDED so it can never hang (the previous version stuck on an unbounded `wait`).
set -u
SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
REPO_ROOT=$(cd -- "$SCRIPT_DIR/.." && pwd)
OUT=${ER_EFFECTS_PORTRAIT_VIDEO_OUT:-"$REPO_ROOT/target/portrait-video"}
# Clear IN PLACE (do NOT rm -rf the dir -- that swaps the inode and any open file-manager window goes empty).
mkdir -p "$OUT/frames"
rm -f "$OUT"/frames/*.jpg "$OUT"/run-60fps-native.mkv "$OUT"/*.log 2>/dev/null || true
VIDEO="$OUT/run-60fps-native.mkv"

echo "launching product smoke (deploys fresh DLL, kills stale ER, on-screen)..."
bash "$SCRIPT_DIR/run-product-continue-direct-probe.sh" > "$OUT/smoke.log" 2>&1 &
SMOKE_PID=$!

pause_s() {
  python3 - "$1" <<'PY'
import sys, threading
threading.Event().wait(float(sys.argv[1]))
PY
}

geom_of_er() {
  hyprctl clients -j 2>/dev/null | python3 -c "
import json,sys
try: cs=json.load(sys.stdin)
except: sys.exit()
for c in cs:
    if c.get('class')=='steam_app_1245620' and c.get('mapped'):
        x,y=c['at']; w,h=c['size']
        if w>0 and h>0: print(f'{x},{y} {w}x{h}'); break
"
}

echo "polling for ER window (max 40s)..."
GEOM=""
for _ in $(seq 1 80); do
  GEOM=$(geom_of_er)
  [ -n "$GEOM" ] && break
  kill -0 "$SMOKE_PID" 2>/dev/null || { echo "smoke ended before window"; break; }
  pause_s 0.5
done
if [ -z "$GEOM" ]; then echo "ERROR: ER window never mapped"; exit 1; fi

echo "ER window: $GEOM -- recording (hard 30s cap via timeout)"
# timeout guarantees wf-recorder exits even if SIGINT is missed; -r 60 = constant 60fps.
timeout --signal=INT 30 wf-recorder -g "$GEOM" -r 60 -f "$VIDEO" >> "$OUT/wf-recorder.log" 2>&1 &
REC_PID=$!

# Record until the smoke run ends OR a 55s hard cap -- never an unbounded wait.
for _ in $(seq 1 30); do
  kill -0 "$SMOKE_PID" 2>/dev/null || break
  pause_s 1
done
# Stop the recorder regardless of how we got here.
kill -INT "$REC_PID" 2>/dev/null
for _ in $(seq 1 6); do kill -0 "$REC_PID" 2>/dev/null || break; pause_s 0.5; done
kill -9 "$REC_PID" 2>/dev/null
pause_s 1

if [ ! -s "$VIDEO" ]; then echo "ERROR: no video captured"; exit 1; fi
echo "video: $(du -h "$VIDEO" | cut -f1) -- extracting first 30s at 60fps"
ffmpeg -y -t 30 -i "$VIDEO" -vf fps=60 -q:v 9 "$OUT/frames/frame_%05d.jpg" >> "$OUT/ffmpeg.log" 2>&1
NFRAMES=$(find "$OUT/frames" -maxdepth 1 -type f -name '*.jpg' | wc -l)
echo "extracted $NFRAMES frames -> $OUT/frames"
setsid dolphin "$OUT" >/dev/null 2>&1 < /dev/null & disown
echo "DONE video=$VIDEO frames=$NFRAMES dir=$OUT"
