#!/usr/bin/env bash
# Manual-drive camera-lever smoke launcher.
#
# Boots the approved direct/offline eldenring.exe (RUNTIME_NO_TEARDOWN on-screen) with the freshly
# built DLL and the manual-portrait-drive env, pinning ARTIFACT_DIR to a known path so the DLL's
# portrait-capture-slot*.bin dumps (written next to ER_QUICKLOAD_AUTOLOAD_DEBUG_PATH) are easy to find.
#
# Prereqs (the probe's own preflight re-checks these and fails closed): Steam running; no eldenring.exe
# already running; the diagnostic flag files staged in GAME_DIR
# (er-quickload-{no-autoload,force-profile-render,portrait-real-pixels}.txt). The human drives to load
# game and holds ~20s; the DLL applies the custom camera and dumps each slot once. Tear down with
# `pkill -x eldenring.exe` and remove the flag files when done.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
set -a
# shellcheck disable=SC1091
source .envs/manual-portrait-drive.env
set +a
: "${ARTIFACT_DIR:=$PWD/target/runtime-probe/camera-smoke}"
export ARTIFACT_DIR
echo "camera-smoke: ARTIFACT_DIR=$ARTIFACT_DIR"
# er-artifact-redirect: delegates-to scripts/run-product-continue-direct-probe.sh
# That script owns the ER_QUICKLOAD_*_PATH redirects that keep this run's artifacts out of GAME_DIR
# (where they are single-slot and the next launch destroys them). Do not add a second copy of that
# list here; add to the delegate. `scripts/er-artifact-redirect-audit.py` reads the marker above.
exec bash scripts/run-product-continue-direct-probe.sh
