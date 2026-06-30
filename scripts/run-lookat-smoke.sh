#!/usr/bin/env bash
# Manual-drive look-at smoke launcher. Same on-screen RUNTIME_NO_TEARDOWN path as run-camera-smoke.sh,
# but with its own ARTIFACT_DIR so the bone dump (er-effects-autoload-debug.log "lookat-bones:"),
# telemetry (oracle_profile_lookat_*), and portrait-capture-slot*.bin dumps don't clobber the camera run.
#
# Stage the gates in GAME_DIR first: er-effects-{no-autoload,force-profile-render,portrait-real-pixels,
# portrait-lookat}.txt. The human drives to LOAD GAME, holds, and MOVES THE MOUSE over the window so the
# portrait head/eyes follow the cursor. Tear down with `pkill -x eldenring.exe`; remove the flag files.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
set -a
# shellcheck disable=SC1091
source .envs/manual-portrait-drive.env
set +a
: "${ARTIFACT_DIR:=$PWD/target/runtime-probe/lookat-smoke}"
export ARTIFACT_DIR
echo "lookat-smoke: ARTIFACT_DIR=$ARTIFACT_DIR"
exec bash scripts/run-product-continue-direct-probe.sh
