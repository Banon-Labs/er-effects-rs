#!/usr/bin/env bash
# Onscreen variant of the telemetry-only refactor smoke (RUNTIME_ONSCREEN=1):
# renders to a real window so the boot can be watched, still save-safe and
# watcher-bounded (auto-teardown at the canonical cap).
set -euo pipefail
repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
set -a
# shellcheck disable=SC1091
source "$repo_root/.envs/refactor-smoke-onscreen.env"
set +a
exec bash "$repo_root/scripts/run-product-continue-direct-probe.sh"
