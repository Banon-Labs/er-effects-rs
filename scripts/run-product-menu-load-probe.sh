#!/usr/bin/env bash
# PRODUCT-CONTINUE foundation test (strategy pivot 2026-06-22): drive the REAL native menu flow via the
# product_core DirectMenuLoad lane (self-fires open_menu 0x1409b24e0 + the native Continue MenuWindowJob,
# the "flag Continue ASAP" auto-Continue) instead of the bare-title own-load (crash) or own_stepper
# SetState (logo-replay). Establishes whether the auto-Continue LOADS the save (rides the flow -> no ToS,
# the foundation for speed+customize) or still surfaces the ToS (save-load timing still off). Input is
# auto-BLOCKED (product_autoload armed -> the always-block change blocks). 120s safety cap, early teardown
# on world-stable / messagebox.
set -euo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
set -a
# shellcheck disable=SC1091
source "$REPO_ROOT/.envs/own-load-pump.env"
set +a
export ARTIFACT_DIR="$REPO_ROOT/target/runtime-probe/product-menu-load"
exec bash "$REPO_ROOT/scripts/run-product-continue-direct-probe.sh" \
  --autoload-request "$REPO_ROOT/target/runtime-probe-requests/product-menu-load.txt"
