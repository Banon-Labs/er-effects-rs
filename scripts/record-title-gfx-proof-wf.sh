#!/usr/bin/env bash
set -euo pipefail

# Stable/overwrite-by-default visual proof capture for title GFX experiments.
# This intentionally reuses the same artifact directory unless ARTIFACT_DIR is set by the caller.
# It prevents target/runtime-probe from accumulating one-off recording folders during iterative
# frame hunting. Runtime still stages the gold save and tears down eldenring.exe after capture.

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
ARTIFACT_DIR=${ARTIFACT_DIR:-$REPO_ROOT/target/runtime-probe/title-gfx-proof-latest}
DESIRED_ARTIFACT_DIR=$ARTIFACT_DIR
SECONDS_TO_RECORD=${1:-23}
FPS=${2:-30}
ENV_FILE=${3:-${ENV_FILE:-$REPO_ROOT/.envs/title-resource-embedded-golden-onscreen.env}}

rm -rf "$DESIRED_ARTIFACT_DIR"
mkdir -p "$DESIRED_ARTIFACT_DIR"

(
  cd "$REPO_ROOT"
  set -a
  # shellcheck source=/dev/null
  source "$ENV_FILE"
  RUNTIME_NO_TEARDOWN=1
  RUNTIME_TELEMETRY_ONLY=0
  # The env file may carry an older ARTIFACT_DIR. Force the stable/latest
  # directory after sourcing so the launcher, Hypr placer, recorder, logs,
  # and screenshots all rendezvous in the same wiped folder.
  ARTIFACT_DIR="$DESIRED_ARTIFACT_DIR"
  set +a
  bash scripts/run-product-continue-direct-probe.sh
) > "$DESIRED_ARTIFACT_DIR/launcher-wrapper.out" 2>&1 &
echo $! > "$DESIRED_ARTIFACT_DIR/launcher-wrapper.pid"

(
  cd "$REPO_ROOT"
  python3 scripts/record-er-window-wf.py "$DESIRED_ARTIFACT_DIR" "$SECONDS_TO_RECORD" "$FPS" > "$DESIRED_ARTIFACT_DIR/wf-capture.out" 2>&1 || true
  python3 - <<'PY'
import subprocess, time
p=subprocess.run(['pgrep','-x','eldenring.exe'],text=True,stdout=subprocess.PIPE)
pids=[x for x in p.stdout.split() if x.strip()]
print('teardown_pids', pids)
for pid in pids:
    subprocess.run(['kill',pid])
deadline=time.time()+5
while time.time()<deadline:
    p=subprocess.run(['pgrep','-x','eldenring.exe'],text=True,stdout=subprocess.PIPE)
    if not p.stdout.split():
        print('teardown_done')
        break
    time.sleep(0.2)
else:
    for pid in p.stdout.split():
        subprocess.run(['kill','-9',pid])
    print('teardown_forced')
PY
) > "$DESIRED_ARTIFACT_DIR/capture-and-teardown.out" 2>&1 &
echo $! > "$DESIRED_ARTIFACT_DIR/capture-and-teardown.pid"

echo "artifact=$DESIRED_ARTIFACT_DIR"
echo "capture_pid=$(cat "$DESIRED_ARTIFACT_DIR/capture-and-teardown.pid")"
