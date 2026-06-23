#!/usr/bin/env bash
# ACCEPT-BYTE + FOCUS advance test (user guidance 2026-06-22): advance press-any-button by TRICKING THE
# DECODED "a button was pressed" bit (the global accept byte 0x144589bdc=1, set ONCE -- not raw input,
# not state-machine manipulation, so no logo-replay/loop), WHILE keeping ER's input-accept flag forced
# (STAY_ACTIVE -> [DLUID+0x88d]=1 every tick) so the advance registers even though the probe window is
# UNFOCUSED (the user noted press-any-button needs focus). No own_load / own_stepper / product_core
# (all of which loop/crash). Goal: does the title advance CLEANLY to the main menu (no loop) this time?
# User raw input is force-BLOCKED (the accept byte is a decoded flag, unaffected by the block).
set -euo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
set -a
# shellcheck disable=SC1091
source "$REPO_ROOT/.envs/own-load-pump.env"
set +a
export ER_EFFECTS_TITLE_ACCEPT_BYTE=1
export ER_EFFECTS_STAY_ACTIVE=1
export ER_EFFECTS_BLOCK_INPUT=1
GAME_DIR="${GAME_DIR:-$HOME/.local/share/Steam/steamapps/common/ELDEN RING/Game}"
: > "$GAME_DIR/er-effects-block-input.txt"
export ARTIFACT_DIR="$REPO_ROOT/target/runtime-probe/accept-byte-focus"
exec bash "$REPO_ROOT/scripts/run-product-continue-direct-probe.sh" \
  --autoload-request "$REPO_ROOT/target/runtime-probe-requests/accept-byte-focus.txt"
