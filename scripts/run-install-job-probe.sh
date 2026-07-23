#!/usr/bin/env bash
# Moment-of-truth: build+install the LoadGame MenuJob (own_load_install_job lever) on the menu-free
# path and let STEP_MenuJobWait tick it -> the player world should stream. Zero-input, save-safe
# (deser reads only). Crash logging ON by default (VEH logs any AV). World-load deadline DISABLED so a
# slow-but-working ER load isn't killed; the 3s per-phase stall watchdog + 120s cap still bound it.
set -euo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
set -a
# shellcheck disable=SC1091
source "$REPO_ROOT/.envs/install-job-probe.env"
set +a
exec bash "$REPO_ROOT/scripts/run-product-continue-direct-probe.sh" \
  --autoload-request "$REPO_ROOT/target/runtime-probe-requests/own-load-install-job.txt"
