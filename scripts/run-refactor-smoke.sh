#!/usr/bin/env bash
# Telemetry-only runtime smoke for the experiments-split refactor: sources the
# auth/telemetry-only env from .envs/ and runs the approved gated offline probe.
# Save-safe and self-tearing-down (see run-product-continue-direct-probe.sh).
set -euo pipefail
repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
set -a
# shellcheck disable=SC1091
source "$repo_root/.envs/refactor-smoke.env"
set +a
exec bash "$repo_root/scripts/run-product-continue-direct-probe.sh"
