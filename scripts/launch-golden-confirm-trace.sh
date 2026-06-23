#!/usr/bin/env bash
# Wrapper: source the confirm-trace env and run the golden scout (sw-bp at the LoadGame-build factory
# 826510; overlay off; user drives Continue). Captures the native confirm's real ctx args + caller chain.
set -euo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
set -a
# shellcheck disable=SC1091
source "$REPO_ROOT/.envs/golden-confirm-trace.env"
set +a
exec bash "$REPO_ROOT/scripts/run-golden-mount-trace.sh"
