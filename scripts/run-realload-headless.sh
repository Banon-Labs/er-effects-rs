#!/usr/bin/env bash
# Headless (offscreen gamescope) REAL gold-load: stages the gold save and runs the
# zero-input autoload with splash-skip + input-block active. Used to validate
# in-process oracles (e.g. oracle_splash_skip_armed) without putting the game on
# the user's monitor. Watcher-bounded; save-safe (isolated staged copy).
set -euo pipefail
repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
set -a
# shellcheck disable=SC1091
source "$repo_root/.envs/gold-probe.env"
set +a
exec bash "$repo_root/scripts/run-product-continue-direct-probe.sh"
