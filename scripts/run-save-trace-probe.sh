#!/usr/bin/env bash
# Runner for the vanilla save-read TRACE probe (see .envs/save-trace-probe.env). Loads the env then
# runs the direct/offline eldenring.exe probe. A char-present save MUST already be staged in the real
# appdata (EldenRing/<steamid>/ER0000.sl2) so the game reads it -- this captures the working open seq.
set -euo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
set -a
# shellcheck disable=SC1091
[ -f "$REPO_ROOT/.envs/save-trace-probe.env" ] && . "$REPO_ROOT/.envs/save-trace-probe.env"
set +a
exec bash "$REPO_ROOT/scripts/run-product-continue-direct-probe.sh"
