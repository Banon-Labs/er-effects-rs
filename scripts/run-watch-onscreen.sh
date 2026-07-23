#!/usr/bin/env bash
# ON-SCREEN, NO-TEARDOWN watch+test run of the zero-input gold autoload.
#
# Reuses the native-continue probe env (gold save + the pab-advance / offline / native-continue
# trigger files that drive the zero-input menu-open -> accept-byte -> Continue -> gold load), but:
#   * renders to a REAL on-screen window (RUNTIME_ONSCREEN=1, drops gamescope headless), and
#   * does NOT run the readiness watcher / auto-teardown (RUNTIME_NO_TEARDOWN=1),
# so a human can WATCH the autoload and then PLAY the loaded character. The DLL's input block
# releases automatically once in-world (IN_WORLD_REACHED), so you take control after the load.
#
# SAVE-SAFE: the save-override keeps the user's gold (save-files/150-Banon) untouched -- it is only
# READ; the %APPDATA% save dir is redirected to the isolated staged copy, so any in-session autosave
# lands there (or in the pre-wiped default dir), never on the gold.
#
# TEAR DOWN when done:  pkill -x eldenring.exe
set -euo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
set -a
# shellcheck disable=SC1091
[ -f "$REPO_ROOT/.envs/native-continue-probe.env" ] && . "$REPO_ROOT/.envs/native-continue-probe.env"
# Optional boot-profiler overlay: when present, enables/tunes the CPU+RIP sampler for this run.
# shellcheck disable=SC1091
[ -f "$REPO_ROOT/.envs/boot-profile.env" ] && . "$REPO_ROOT/.envs/boot-profile.env"
RUNTIME_ONSCREEN=1
RUNTIME_NO_TEARDOWN=1
set +a
exec bash "$REPO_ROOT/scripts/run-product-continue-direct-probe.sh"
