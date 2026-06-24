#!/usr/bin/env bash
# Gated runner for the native-continue (zero-input Continue member-node) moment-of-truth probe.
# Loads .envs/native-continue-probe.env then runs the direct/offline eldenring.exe readiness probe.
# The GAME_DIR trigger files (er-effects-pab-advance.txt + er-effects-offline.txt +
# er-effects-native-continue.txt) drive: zero-input menu open -> fire native Continue run once the
# menu validates -> observe the full native load (deser -> SetState5 -> world stream). Save-safe.
set -euo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
set -a
# shellcheck disable=SC1091
[ -f "$REPO_ROOT/.envs/native-continue-probe.env" ] && . "$REPO_ROOT/.envs/native-continue-probe.env"
set +a
exec bash "$REPO_ROOT/scripts/run-product-continue-direct-probe.sh"
