#!/usr/bin/env bash
# PATH B moment-of-truth: build the LoadGame MenuJob with real mss-derived ctx and PRIVATELY PUMP its
# Run every frame (own_load_pump lever) until Success -> guarded SetState5 transition -> the player
# world streams. Zero simulated input. Save-safe (deser reads; only the gated SetState5 writes). Crash
# logging ON; overlay OFF (er-effects-no-overlay.txt); world-load deadline OFF so a real load isn't cut.
set -euo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
set -a
# shellcheck disable=SC1091
source "$REPO_ROOT/.envs/own-load-pump.env"
set +a
exec bash "$REPO_ROOT/scripts/run-product-continue-direct-probe.sh" \
  --autoload-request "$REPO_ROOT/target/runtime-probe-requests/own-load-pump.txt"
