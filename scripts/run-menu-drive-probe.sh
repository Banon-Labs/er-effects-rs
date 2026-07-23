#!/usr/bin/env bash
# MENU-DRIVE test: arm own_stepper WITHOUT own_load, so own_stepper_idx10 takes the PHASE_MENU
# menu-drive path (SetState(3=BeginTitle) zero-input main-menu build -> identify Load-Game leaf ->
# STAGE 2 invoke the native Load-Game entry) INSTEAD of the bare-title own_load_drive deserialize that
# overflows the Scaleform MenuWindowJob+0x50 sequence (~20s crash). This validates the RE fix direction
# (route the load through the settled load-menu state, not the bare title). Input is auto-BLOCKED (the
# always-block change blocks whenever own_stepper is armed) so the run is uncontaminable. Save-safe:
# the menu-drive currently does NOT SetState5 (no save write) -- it drives to the Load-Game state and the
# native async job mounts c30; this run shows how far it gets (reaches Load-Game / mounts c30 / crash).
set -euo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
set -a
# shellcheck disable=SC1091
source "$REPO_ROOT/.envs/own-load-pump.env"
set +a
export ARTIFACT_DIR="$REPO_ROOT/target/runtime-probe/menu-drive"
exec bash "$REPO_ROOT/scripts/run-product-continue-direct-probe.sh" \
  --autoload-request "$REPO_ROOT/target/runtime-probe-requests/menu-drive.txt"
