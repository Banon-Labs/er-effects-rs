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
source .envs/postcontinue-lookat.env
set +a
: "${ARTIFACT_DIR:=$PWD/target/runtime-probe/postcontinue-lookat-smoke}"
export ARTIFACT_DIR
# Clean the previous run's logo-replacement capture artifacts: maybe_capture_logo_replacement() returns
# early if any of these already exist (capture-once guard), so a stale file from a prior run silently
# suppresses this run's screenshot -- the whole point of the run. Remove them so each run captures fresh.
rm -f "$ARTIFACT_DIR"/logo-replacement-screenshot.jpg \
      "$ARTIFACT_DIR"/logo-replacement-screenshot.txt \
      "$ARTIFACT_DIR"/logo-replacement-screenshot-event.json \
      "$ARTIFACT_DIR"/logo-replacement-screenshot-analysis.json 2>/dev/null || true
# HARD 45s CAP, ENFORCED (not prose): never run no-teardown -- route through the probe's watcher cap...
export RUNTIME_NO_TEARDOWN=0
# ...AND a belt-and-suspenders independent watchdog that hard-kills eldenring.exe at the canonical cap
# regardless of what the probe/watcher does. The cap value comes from the single source of truth.
CAP="$(python3 -c 'import sys; sys.path.insert(0,"scripts"); from runtime_timeout_cap import runtime_timeout_cap_seconds as f; print(f())' 2>/dev/null || true)"
case "$CAP" in ''|*[!0-9]*) CAP=45 ;; esac
echo "postcontinue-lookat-smoke: ARTIFACT_DIR=$ARTIFACT_DIR (autoload ON, HARD ${CAP}s cap)"
( sleep "$CAP"; pkill -x eldenring.exe >/dev/null 2>&1; pkill -f 'eldenring.exe' >/dev/null 2>&1 ) &
disown || true
exec bash scripts/run-product-continue-direct-probe.sh
