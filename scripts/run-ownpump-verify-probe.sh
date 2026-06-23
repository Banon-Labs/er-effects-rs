#!/usr/bin/env bash
# Gated runner for the own_load_pump VERIFY probe: loads the authorized-runtime env from
# .envs/ownpump-probe.env and runs the direct/offline eldenring.exe readiness probe with the
# own_load_pump autoload request. The verify gate file (er-effects-own-load-pump-verify.txt) in the
# game dir makes the pump skip the SetState5 transition -> READ-ONLY, no save write.
set -euo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Telemetry-only probe: the own_load_pump oracle is in-process telemetry, not screenshots, so the
# watcher must NOT require a focused, screenshot-safe target window -- otherwise an agent-launched
# (unfocused) run is torn down at ~3.6s with target_window_capture_unsafe before the title advances.
# bd runtime-probe-needs-skip-visual-capture-when-headless-2026-06-22. Override-able from the env file.
export RUNTIME_SKIP_VISUAL_CAPTURE="${RUNTIME_SKIP_VISUAL_CAPTURE:-1}"
# Clear the stale FORCE-BLOCK override file: er-effects-block-input.txt unconditionally blocks ALL
# input (block_input_enabled FORCE-BLOCK branch). It is a manual falsification tool, never wanted for
# these runs (the input block already auto-engages from any armed lever). A leftover from a prior
# session silently blocked a user-driven trace drive (observed 2026-06-22). Always remove it so the
# drive/probe input state is derived purely from the armed levers, not a stale file.
rm -f "${GAME_DIR:-$HOME/.local/share/Steam/steamapps/common/ELDEN RING/Game}/er-effects-block-input.txt"
set -a
# shellcheck disable=SC1091
[ -f "$REPO_ROOT/.envs/ownpump-probe.env" ] && . "$REPO_ROOT/.envs/ownpump-probe.env"
set +a
exec bash "$REPO_ROOT/scripts/run-product-continue-direct-probe.sh" \
  --autoload-request "${1:-/tmp/autoload-ownpump.txt}"
