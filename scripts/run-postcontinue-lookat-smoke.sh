#!/usr/bin/env bash
# Post-Continue realtime look-at smoke. Same on-screen no-teardown probe as run-lookat-smoke.sh, but with
# the product AUTOLOAD ENABLED (NO er-effects-no-autoload.txt staged), so the zero-input Continue fires and
# the teardown-spare hook keeps the loaded character's portrait renderer alive PAST Continue. The DLL then
# drives realtime look-at + rasterizes that spared (persistent-model) renderer and the in-process pixel
# oracle samples it -- the cycling 10-slot menu can't show a stable portrait, but the post-Continue loaded
# character (= the loading-screen portrait) persists.
#
# Stage in GAME_DIR first (and REMOVE er-effects-no-autoload.txt + er-effects-portrait-real-pixels.txt):
#   er-effects-force-profile-render.txt  (build the menu model so a built renderer exists to spare)
#   er-effects-portrait-lookat.txt       (the look-at lever)
#   er-effects-portrait-lookat-selftest.txt (zero-input sinusoid drive + RT-readback oracle)
# Read the oracle in the debug log line "lookat-phase-sweep: ... spared[ptr=.. model_ok=.. draws=.. hits=..]
# rt[samples=.. nonblack=.. changed=..]". Tear down with `pkill -x eldenring.exe`.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
set -a
# shellcheck disable=SC1091
source .envs/manual-portrait-drive.env
set +a
: "${ARTIFACT_DIR:=$PWD/target/runtime-probe/postcontinue-lookat-smoke}"
export ARTIFACT_DIR
echo "postcontinue-lookat-smoke: ARTIFACT_DIR=$ARTIFACT_DIR (autoload ON -- drives the post-Continue spared renderer)"
exec bash scripts/run-product-continue-direct-probe.sh
