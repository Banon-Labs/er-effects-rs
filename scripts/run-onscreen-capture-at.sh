#!/usr/bin/env bash
# ON-SCREEN run that captures a FULL-RES screenshot of the ER window at a target
# wall-clock offset after launch, then tears the game down. Self-bounded.
#
#   CAPTURE_AT_MS  : offset after launch-epoch to capture (default 15000)
#   ARTIFACT_DIR   : run dir (default target/runtime-probe/onscreen-capture-<ts>)
#   HARD_CAP_S     : absolute safety teardown deadline after launch (default 40)
#
# Tears down eldenring.exe + the runner session unconditionally on exit.
set -uo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Optional first arg overrides the capture offset in ms.
CAPTURE_AT_MS="${1:-${CAPTURE_AT_MS:-15000}}"
HARD_CAP_S="${HARD_CAP_S:-40}"
ARTIFACT_DIR="${ARTIFACT_DIR:-$REPO_ROOT/target/runtime-probe/onscreen-capture-$(date +%Y%m%d-%H%M%S)}"
ARTIFACT_DIR="$(realpath -m "$ARTIFACT_DIR")"
mkdir -p "$ARTIFACT_DIR"
SHOT="$ARTIFACT_DIR/capture-at-${CAPTURE_AT_MS}ms.png"
RUNNER_PIDFILE="$ARTIFACT_DIR/runner.sid"

teardown() {
  echo "[orchestrator] teardown" >&2
  pkill -x eldenring.exe 2>/dev/null || true
  if [[ -s "$RUNNER_PIDFILE" ]]; then
    local sid; IFS= read -r sid < "$RUNNER_PIDFILE" || sid=""
    [[ -n "$sid" ]] && kill -- "-$sid" 2>/dev/null || true
  fi
  # bounded wait for the game to actually go away
  for _ in $(seq 1 12); do
    pgrep -x eldenring.exe >/dev/null 2>&1 || break
    timeout 1 tail --pid="$(pgrep -x eldenring.exe | head -1)" -f /dev/null >/dev/null 2>&1 || true
  done
  pgrep -x eldenring.exe >/dev/null 2>&1 && pkill -9 -x eldenring.exe 2>/dev/null || true
}
trap teardown EXIT INT TERM HUP

# Launch the on-screen no-teardown runner in its OWN session so we can kill the
# whole tree. It sources .envs/native-continue-probe.env (gold save + triggers).
setsid env ARTIFACT_DIR="$ARTIFACT_DIR" bash "$REPO_ROOT/scripts/run-watch-onscreen.sh" \
  > "$ARTIFACT_DIR/orchestrator-runner.out" 2>&1 &
RUNNER_PID=$!
# session id == the setsid child's pid (it's the session leader)
echo "$RUNNER_PID" > "$RUNNER_PIDFILE"
echo "[orchestrator] runner pid=$RUNNER_PID artifact_dir=$ARTIFACT_DIR" >&2

EPOCH_FILE="$ARTIFACT_DIR/launch-epoch.txt"
# Bounded poll for launch-epoch.txt, then sleep precisely to epoch+CAPTURE_AT_MS
# (capped by HARD_CAP_S). One python block handles wait + precise sleep.
LAUNCH_EPOCH="$(python3 - "$EPOCH_FILE" "$CAPTURE_AT_MS" "$HARD_CAP_S" <<'PY'
import sys, time, os
epoch_file, at_ms, cap = sys.argv[1], float(sys.argv[2]), float(sys.argv[3])
epoch = None
# wait up to 30s for the probe to write the launch epoch
wait_deadline = time.time() + 30.0
while time.time() < wait_deadline:
    try:
        with open(epoch_file) as f:
            s = f.read().strip()
        if s:
            epoch = float(s); break
    except Exception:
        pass
    time.sleep(0.25)
if epoch is None:
    print("", end=""); sys.exit(3)
target = epoch + at_ms / 1000.0
hard = epoch + cap
end = min(target, hard)
now = time.time()
if end > now:
    time.sleep(end - now)
print(f"{epoch:.6f}", end="")
PY
)"
RC=$?
if [[ $RC -ne 0 || -z "$LAUNCH_EPOCH" ]]; then
  echo "[orchestrator] FAILED: no launch-epoch.txt -- aborting" >&2
  exit 3
fi
echo "[orchestrator] launch_epoch=$LAUNCH_EPOCH" >&2

echo "[orchestrator] capturing at +${CAPTURE_AT_MS}ms -> $SHOT" >&2
python3 "$REPO_ROOT/scripts/capture-er-window-fullres.py" "$SHOT" || true

# Hold briefly is unnecessary; tear down now (trap also covers it).
teardown
trap - EXIT INT TERM HUP
echo "[orchestrator] done. screenshot: $SHOT" >&2
ls -l "$SHOT" 2>/dev/null || echo "[orchestrator] NO screenshot (see ${SHOT%.png}.txt)" >&2
