#!/usr/bin/env bash
# Gated runner for the pab_advance (zero-input press-any-button) validation probe. Loads the authorized
# runtime env from .envs/pab-advance-probe.env and runs the direct/offline eldenring.exe readiness probe
# with no autoload-request: the GAME_DIR gate `er-quickload-pab-advance.txt` drives the readiness-gated
# press-any-button advance (hook 0x1407ad1c0 -> set [job+0x1e8]=2) + maybe_auto_open_menu, with no
# selector fire. Telemetry/DLL-debug-log only (no save write expected). bd
# press-any-button-golden-lever-job1e8-readiness-2026-06-23.
set -euo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export RUNTIME_SKIP_VISUAL_CAPTURE="${RUNTIME_SKIP_VISUAL_CAPTURE:-1}"
set -a
# shellcheck disable=SC1091
[ -f "$REPO_ROOT/.envs/pab-advance-probe.env" ] && . "$REPO_ROOT/.envs/pab-advance-probe.env"
set +a
exec bash "$REPO_ROOT/scripts/run-product-continue-direct-probe.sh"
