#!/usr/bin/env bash
# Wrapper: source the golden-scout env gates from .envs and run the golden mount-trace scout.
set -euo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
set -a
# shellcheck disable=SC1091
source "$REPO_ROOT/.envs/golden-mount-trace.env"
set +a
exec bash "$REPO_ROOT/scripts/run-golden-mount-trace.sh"
