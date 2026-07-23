#!/usr/bin/env bash
# Wrapper: load the gated probe env from .envs and run the m28 direct-enqueue TREATMENT probe
# (own_load_continue=1 + own_dispatch=1). Zero-input, save-safe, telemetry-only. See bd er-effects-rs-uvh.
set -euo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
set -a
# shellcheck disable=SC1091
source "$REPO_ROOT/.envs/own-dispatch-probe.env"
set +a
exec bash "$REPO_ROOT/scripts/run-product-continue-direct-probe.sh" \
  --autoload-request "$REPO_ROOT/target/runtime-probe-requests/own-load-dispatch-treatment.txt"
