#!/usr/bin/env bash
set -euo pipefail
set -a
source .envs/portrait-source-runtime.env
set +a
scripts/run-product-continue-direct-probe.sh "$@"
