#!/usr/bin/env bash
# Wrapper: golden scout with BOTH low-frequency breakpoints armed in one user-driven native Continue:
#   826510 = LoadGame-job factory (the native confirm's real ctx args + caller chain -- the reference
#            our own-load-pump must match; informs why our menu-free path conflicts with the menu system)
#   1efc00 = MountEblArchive (the native world-map m28 EBL-mount caller chain our load must replicate)
# Sources the golden-mount-trace env (auth gates + 120s window so the user has time to navigate), then
# overrides BREAKPOINTS_RVAS to arm both. SAVE-SAFE w.r.t. our DLL (no SetState5/own-load/autoload --
# read-only INT3 loggers only); the user drives their own save the normal way (Continue autosaves as in
# normal play). Input is NOT blocked (no levers armed -> the user navigates).
set -euo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
set -a
# shellcheck disable=SC1091
source "$REPO_ROOT/.envs/golden-mount-trace.env"
set +a
export BREAKPOINTS_RVAS="826510 1efc00"
exec bash "$REPO_ROOT/scripts/run-golden-mount-trace.sh"
