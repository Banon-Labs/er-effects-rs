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
# HARD 45s CAP, ENFORCED: never run no-teardown; route through the probe watcher cap AND an independent
# watchdog that hard-kills eldenring.exe at the canonical cap (single source of truth) no matter what.
export RUNTIME_NO_TEARDOWN=0
CAP="$(python3 -c 'import sys; sys.path.insert(0,"scripts"); from runtime_timeout_cap import runtime_timeout_cap_seconds as f; print(f())' 2>/dev/null || true)"
case "$CAP" in ''|*[!0-9]*) CAP=45 ;; esac
echo "lookat-smoke: ARTIFACT_DIR=$ARTIFACT_DIR (HARD ${CAP}s cap)"
( python3 - "$CAP" <<'PY'
import sys, threading
threading.Event().wait(float(sys.argv[1]))
PY
pkill -x eldenring.exe >/dev/null 2>&1; pkill -f 'eldenring.exe' >/dev/null 2>&1 ) &
disown || true
exec bash scripts/run-product-continue-direct-probe.sh
