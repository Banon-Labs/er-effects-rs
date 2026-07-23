#!/usr/bin/env bash
# One-shot launcher for the SAVE-DIR cold-mount fix verification probe.
# Sources the gated env, launches the approved offline eldenring.exe Proton probe with the
# cold_char_mount autoload request, and lets the readiness watcher tear down on evidence.
set -uo pipefail
cd /home/banon/projects/er-effects-rs
set -a
# shellcheck disable=SC1091
source .envs/savedir-fix-probe.env
set +a
exec bash scripts/run-product-continue-direct-probe.sh \
  --autoload-request target/runtime-probe/b80probe-autoload-request.txt
