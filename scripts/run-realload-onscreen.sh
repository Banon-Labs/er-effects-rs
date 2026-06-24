#!/usr/bin/env bash
# REAL gold-load smoke, ONSCREEN: stages the gold save and runs the actual
# zero-input autoload (NOT telemetry-only), so success == the character reaching
# the playable world. Onscreen so it can be watched; watcher-bounded (early-exit
# on world reach, else teardown at the canonical cap).
set -euo pipefail
repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
set -a
# shellcheck disable=SC1091
source "$repo_root/.envs/refactor-realload-onscreen.env"
set +a
exec bash "$repo_root/scripts/run-product-continue-direct-probe.sh"
