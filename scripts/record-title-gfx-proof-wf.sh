#!/usr/bin/env bash
set -euo pipefail

# Stable/overwrite-by-default visual proof capture for title GFX experiments.
# This intentionally reuses the same artifact directory unless ARTIFACT_DIR is set by the caller.
# It prevents target/runtime-probe from accumulating one-off recording folders during iterative
# frame hunting. Runtime still stages the gold save and tears down eldenring.exe after capture.

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
ARTIFACT_DIR=${ARTIFACT_DIR:-$REPO_ROOT/target/runtime-probe/title-gfx-proof-latest}
DESIRED_ARTIFACT_DIR=$ARTIFACT_DIR
if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  cat <<EOF
Usage: $0 [max-record-seconds] [fps] [env-file]

Launches the approved direct/offline Elden Ring path, waits for a real stable
steam_app_1245620 Hyprland window, records that exact window geometry, keeps
recording until oracle_player_present plus a post-confirm hold when possible,
then tears down the owned runtime.

Environment:
  ARTIFACT_DIR              Output dir (default target/runtime-probe/title-gfx-proof-latest)
  WINDOW_TIMEOUT_SECONDS    Max wait for stable ER window (default 45)
  STABLE_WINDOW_SAMPLES     Consecutive stable geometry samples required (default 8)
  MIN_SECONDS_TO_RECORD     Minimum record duration after stable window (default 8)
  POST_PLAYER_HOLD_SECONDS  Extra recording after oracle_player_present (default 8)
  RECORD_ALLOW_HYPR_PLACER  Set 1 to allow launcher-side pre-placement (default disabled)
EOF
  exit 0
fi

SECONDS_TO_RECORD=${1:-45}
FPS=${2:-30}
ENV_FILE=${3:-${ENV_FILE:-$REPO_ROOT/.envs/title-resource-embedded-golden-onscreen.env}}
WINDOW_TIMEOUT_SECONDS=${WINDOW_TIMEOUT_SECONDS:-45}
STABLE_WINDOW_SAMPLES=${STABLE_WINDOW_SAMPLES:-8}
MIN_SECONDS_TO_RECORD=${MIN_SECONDS_TO_RECORD:-8}
POST_PLAYER_HOLD_SECONDS=${POST_PLAYER_HOLD_SECONDS:-8}

# The artifact directory is intentionally wiped for stable latest-style runs. If the caller staged
# the env file inside that same directory, preserve it before the wipe and restore it afterward.
ENV_FILE_SNAPSHOT=""
if [[ -f "$ENV_FILE" ]]; then
  ENV_FILE_SNAPSHOT=$(mktemp "${TMPDIR:-/tmp}/er-record-env.XXXXXX")
  cp -f "$ENV_FILE" "$ENV_FILE_SNAPSHOT"
fi

rm -rf "$DESIRED_ARTIFACT_DIR"
mkdir -p "$DESIRED_ARTIFACT_DIR"

if [[ -n "$ENV_FILE_SNAPSHOT" ]]; then
  cp -f "$ENV_FILE_SNAPSHOT" "$DESIRED_ARTIFACT_DIR/record.env"
  rm -f "$ENV_FILE_SNAPSHOT"
  ENV_FILE="$DESIRED_ARTIFACT_DIR/record.env"
fi

(
  cd "$REPO_ROOT"
  set -a
  # shellcheck source=/dev/null
  source "$ENV_FILE"
  # shellcheck disable=SC2034 # exported by set -a for run-product-continue-direct-probe.sh
  RUNTIME_NO_TEARDOWN=1
  # shellcheck disable=SC2034 # exported by set -a for run-product-continue-direct-probe.sh
  RUNTIME_TELEMETRY_ONLY=0
  # Visual recording proof must not pick a crop/size before the ER window exists. Unless explicitly
  # requested by RECORD_ALLOW_HYPR_PLACER=1, disable the launcher-side Hypr placer and let
  # record-er-window-wf.py wait for the real mapped steam_app_1245620 geometry to stabilize.
  if [[ "${RECORD_ALLOW_HYPR_PLACER:-0}" != "1" ]]; then
    # shellcheck disable=SC2034 # exported by set -a for run-product-continue-direct-probe.sh
    ER_EFFECTS_HYPR_PLACE_WINDOW=0
  fi
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
  set +e
  python3 scripts/record-er-window-wf.py \
    "$DESIRED_ARTIFACT_DIR" "$SECONDS_TO_RECORD" "$FPS" \
    --window-timeout "$WINDOW_TIMEOUT_SECONDS" \
    --stable-samples "$STABLE_WINDOW_SAMPLES" \
    --stop-after-player-present \
    --min-seconds "$MIN_SECONDS_TO_RECORD" \
    --post-confirm-seconds "$POST_PLAYER_HOLD_SECONDS" \
    > "$DESIRED_ARTIFACT_DIR/wf-capture.out" 2>&1
  record_rc=$?
  set -e
  # If recording never started, do not pretend a visual run existed. Still clean up the owned direct
  # runtime if it spawned, but leave a clear machine-readable reason for callers.
  if [[ ! -f "$DESIRED_ARTIFACT_DIR/wf-recorder-request.json" ]]; then
    printf '{"recording_started":false,"record_rc":%s,"reason":"no stable ER target window before recording"}\n' "$record_rc" > "$DESIRED_ARTIFACT_DIR/recording-not-started.json"
  fi
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
