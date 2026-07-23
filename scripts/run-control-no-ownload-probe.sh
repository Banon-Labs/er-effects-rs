#!/usr/bin/env bash
# CONTROL run for the ~20-26s Scaleform menu-job FixOrderJobSequence crash isolation.
# DLL attached, splash-skip ON (via env, since own_load is off), foreground-force always-on, but NO
# own-load / own-stepper levers (so the title is NOT manipulated by us). If the same assert still fires
# at ~20s -> the crash is the NATIVE offline title flow (not our levers). If it boots clean to the title
# and sits -> our own-load levers trigger it.
set -euo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
set -a
# shellcheck disable=SC1091
source "$REPO_ROOT/.envs/own-load-pump.env"
set +a
# Override: splash-skip via env (own_load is off, so the splash_skip_enabled() own_load arm won't fire);
# separate artifact dir; keep the same authorization + cap from own-load-pump.env.
export ER_EFFECTS_SPLASH_SKIP=1
# ZERO-INPUT INVARIANT (always-block-input-zero-input-invariant-2026-06-22): with own_load=0 the DLL's
# block_input_enabled() would NOT auto-arm (it gates on own_stepper), so foreign input could reach the
# game and contaminate the control. FORCE the unconditional block by planting the game-dir gate file so
# block_input_enabled() returns true for the whole run -- the game cannot receive any keyboard/mouse/
# gamepad input. Also force it via env for belt-and-braces.
export ER_EFFECTS_BLOCK_INPUT=1
GAME_DIR="${GAME_DIR:-$HOME/.local/share/Steam/steamapps/common/ELDEN RING/Game}"
: > "$GAME_DIR/er-effects-block-input.txt"
export ARTIFACT_DIR="$REPO_ROOT/target/runtime-probe/control-no-ownload"
exec bash "$REPO_ROOT/scripts/run-product-continue-direct-probe.sh" \
  --autoload-request "$REPO_ROOT/target/runtime-probe-requests/control-no-ownload.txt"
