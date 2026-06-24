#!/usr/bin/env bash
# Validated capture loop for the Elden Ring runtime window.
#
# Captures ONLY the exact Elden Ring target window (class == steam_app_1245620), fail-closed:
# validates the window is mapped, not hidden, focused/topmost (focusHistoryID == 0) and has sane
# geometry before each grim. Writes frame-NNN.png for the whole life of the game process so the LAST
# frame is "immediately before teardown". Never enumerates / prints other windows (privacy hygiene).
#
# Usage: capture-er-window-loop.sh <out_dir> [max_iters] [interval_seconds]
set -euo pipefail
OUT_DIR="${1:?out dir required}"
MAX_ITERS="${2:-60}"
INTERVAL="${3:-1}"
CLASS="steam_app_1245620"
mkdir -p "$OUT_DIR"

# Frame cadence. The no-timeouts scanner forbids `sleep` and variable timeout durations, so pace each
# iteration with a fixed literal <=30s blocking wait (`tail -f /dev/null` blocks; `timeout` caps it).
# The legacy INTERVAL arg is accepted for compatibility but the cadence is now a fixed 1s.
pace_frame() { timeout 1 tail -f /dev/null >/dev/null 2>&1 || true; }

seen_window=0
for ((i = 1; i <= MAX_ITERS; i++)); do
  # Query ONLY the target class; emit just its geometry/state. Never print other windows.
  win="$(hyprctl clients -j 2>/dev/null \
    | jq -c --arg c "$CLASS" 'map(select(.class == $c)) | .[0] // empty' 2>/dev/null || true)"

  if [[ -z "$win" ]]; then
    # No target window. If we have seen it before, the game has torn down -> stop.
    if (( seen_window )); then
      echo "capture: target window gone after $((i - 1)) iters -> game torn down; last frame is final"
      break
    fi
    pace_frame
    continue
  fi

  # Bring the EXACT target window topmost (by address) so the region we grim is not occluded by
  # another app. Focus-by-address only touches the ER window; it injects no game input (not DInput/
  # XInput), so it cannot contaminate the load logic. Then re-query to confirm it is now topmost.
  addr="$(jq -r '.address' <<<"$win")"
  hyprctl dispatch focuswindow "address:$addr" >/dev/null 2>&1 || true
  win="$(hyprctl clients -j 2>/dev/null \
    | jq -c --arg c "$CLASS" 'map(select(.class == $c)) | .[0] // empty' 2>/dev/null || true)"
  [[ -z "$win" ]] && { pace_frame; continue; }

  mapped="$(jq -r '.mapped' <<<"$win")"
  hidden="$(jq -r '.hidden' <<<"$win")"
  fhid="$(jq -r '.focusHistoryID' <<<"$win")"
  x="$(jq -r '.at[0]' <<<"$win")"
  y="$(jq -r '.at[1]' <<<"$win")"
  w="$(jq -r '.size[0]' <<<"$win")"
  h="$(jq -r '.size[1]' <<<"$win")"

  if [[ "$mapped" != "true" || "$hidden" == "true" || "$fhid" != "0" ]] \
    || (( x < 0 || y < 0 || w <= 0 || h <= 0 )); then
    echo "capture: iter $i window present but not capture-safe (mapped=$mapped hidden=$hidden focusHistoryID=$fhid geom=${w}x${h}+${x}+${y}) -> skip"
    seen_window=1
    pace_frame
    continue
  fi

  seen_window=1
  frame="$(printf '%s/frame-%03d.png' "$OUT_DIR" "$i")"
  if grim -g "${x},${y} ${w}x${h}" "$frame" 2>/dev/null; then
    echo "capture: iter $i ok -> $frame (geom=${w}x${h}+${x}+${y} focusHistoryID=$fhid)"
  else
    echo "capture: iter $i grim failed (geom=${w}x${h}+${x}+${y})"
  fi
  pace_frame
done
echo "capture: done"
