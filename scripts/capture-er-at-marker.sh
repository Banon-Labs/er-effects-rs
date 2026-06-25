#!/usr/bin/env bash
# Capture an on-screen ER screenshot timed off a DLL TELEMETRY MARKER (not launch_epoch).
# The launch_epoch->DLL-epoch offset (wine/proton + chainload) makes launch-relative offsets
# land in the wrong place in the game timeline; this waits for a regex to appear in the live
# er-effects-autoload-debug.log, optionally waits POST_DELAY_MS more, captures, then tears down.
#
#   $1 MARKER_REGEX : python regex matched against each new debug-log line (required)
#   $2 POST_DELAY_MS: ms to wait AFTER the marker before capturing (default 0)
#   HARD_CAP_S      : absolute teardown deadline after launch (default 40)
#   ARTIFACT_DIR    : run dir (default target/runtime-probe/marker-capture-<ts>)
set -uo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MARKER_REGEX="${1:?usage: capture-er-at-marker.sh <marker_regex> [post_delay_ms]}"
POST_DELAY_MS="${2:-0}"
HARD_CAP_S="${HARD_CAP_S:-40}"
ARTIFACT_DIR="${ARTIFACT_DIR:-$REPO_ROOT/target/runtime-probe/marker-capture-$(date +%Y%m%d-%H%M%S)}"
ARTIFACT_DIR="$(realpath -m "$ARTIFACT_DIR")"
mkdir -p "$ARTIFACT_DIR"
SAFE_TAG="$(printf '%s' "$MARKER_REGEX" | tr -c 'A-Za-z0-9' '_' | cut -c1-32)"
SHOT="$ARTIFACT_DIR/at-${SAFE_TAG}-plus${POST_DELAY_MS}ms.png"
DBG="$ARTIFACT_DIR/er-effects-autoload-debug.log"
RUNNER_PIDFILE="$ARTIFACT_DIR/runner.sid"

teardown() {
  echo "[marker] teardown" >&2
  pkill -x eldenring.exe 2>/dev/null || true
  if [[ -s "$RUNNER_PIDFILE" ]]; then
    local sid; IFS= read -r sid < "$RUNNER_PIDFILE" || sid=""
    [[ -n "$sid" ]] && kill -- "-$sid" 2>/dev/null || true
  fi
  for _ in $(seq 1 12); do
    pgrep -x eldenring.exe >/dev/null 2>&1 || break
    timeout 1 tail --pid="$(pgrep -x eldenring.exe | head -1)" -f /dev/null >/dev/null 2>&1 || true
  done
  pgrep -x eldenring.exe >/dev/null 2>&1 && pkill -9 -x eldenring.exe 2>/dev/null || true
}
trap teardown EXIT INT TERM HUP

setsid env ARTIFACT_DIR="$ARTIFACT_DIR" bash "$REPO_ROOT/scripts/run-watch-onscreen.sh" \
  > "$ARTIFACT_DIR/orchestrator-runner.out" 2>&1 &
RUNNER_PID=$!
echo "$RUNNER_PID" > "$RUNNER_PIDFILE"
echo "[marker] runner pid=$RUNNER_PID artifact_dir=$ARTIFACT_DIR marker='$MARKER_REGEX' post_delay=${POST_DELAY_MS}ms" >&2

# Wait (bounded by HARD_CAP_S) for the marker to appear in the live debug log, then post-delay.
MATCH_MS="$(python3 - "$DBG" "$MARKER_REGEX" "$HARD_CAP_S" "$POST_DELAY_MS" <<'PY'
import sys, time, re, os
dbg, pat, cap, post = sys.argv[1], re.compile(sys.argv[2]), float(sys.argv[3]), float(sys.argv[4])/1000.0
deadline = time.time() + cap
pos = 0
matched = None
while time.time() < deadline:
    if os.path.exists(dbg):
        with open(dbg, encoding="utf-8", errors="replace") as f:
            f.seek(pos)
            chunk = f.read()
            pos = f.tell()
        for line in chunk.splitlines():
            if pat.search(line):
                m = re.match(r"\[\+(\d+)ms\]", line)
                matched = m.group(1) if m else "?"
                break
    if matched is not None:
        break
    time.sleep(0.02)
if matched is None:
    sys.exit(3)
if post > 0:
    time.sleep(post)
print(matched, end="")
PY
)"
RC=$?
if [[ $RC -ne 0 ]]; then
  echo "[marker] FAILED: marker '$MARKER_REGEX' not seen within ${HARD_CAP_S}s -- aborting" >&2
  exit 3
fi
echo "[marker] matched at debug-log +${MATCH_MS}ms; capturing (+${POST_DELAY_MS}ms) -> $SHOT" >&2
python3 "$REPO_ROOT/scripts/capture-er-window-fullres.py" "$SHOT" || true

teardown
trap - EXIT INT TERM HUP
echo "[marker] done. screenshot: $SHOT (marker debug-log +${MATCH_MS}ms)" >&2
ls -l "$SHOT" 2>/dev/null || echo "[marker] NO screenshot (see ${SHOT%.png}.txt)" >&2
