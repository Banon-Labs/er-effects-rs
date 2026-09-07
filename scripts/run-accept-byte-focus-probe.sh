#!/usr/bin/env bash
# Accept-byte + focus advance test (user guidance 2026-06-22): advance press-any-button by tricking the
# DECODED "a button was pressed" bit (the global accept byte 0x144589bdc=1, set once -- not raw input,
# not state-machine manipulation, so no logo-replay/loop), while keeping ER's input-accept flag forced
# (STAY_ACTIVE -> [DLUID+0x88d]=1 every tick) so the advance registers even though the probe window is
# UNFOCUSED (the user noted press-any-button needs focus). No own_load / own_stepper / product_core
# (all of which loop/crash). Goal: does the title advance cleanly to the main menu (no loop) this time?
# User raw input is force-blocked (the accept byte is a decoded flag, unaffected by the block).
set -euo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
set -a
# shellcheck disable=SC1091
source "$REPO_ROOT/.envs/own-load-pump.env"
set +a
export ER_QUICKLOAD_TITLE_ACCEPT_BYTE=1
export ER_QUICKLOAD_STAY_ACTIVE=1
export ER_QUICKLOAD_BLOCK_INPUT=1
GAME_DIR="${GAME_DIR:-$HOME/.local/share/Steam/steamapps/common/ELDEN RING/Game}"
: > "$GAME_DIR/er-quickload-block-input.txt"
export ARTIFACT_DIR="$REPO_ROOT/target/runtime-probe/accept-byte-focus"
exec bash "$REPO_ROOT/scripts/run-product-continue-direct-probe.sh" \
  --autoload-request "$REPO_ROOT/target/runtime-probe-requests/accept-byte-focus.txt"
