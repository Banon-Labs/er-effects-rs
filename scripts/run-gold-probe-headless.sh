#!/usr/bin/env bash
# Run the gold-save runtime probe with Elden Ring rendered on a DEDICATED Hyprland
# HEADLESS virtual output, so the game gets continuous frame callbacks and runs full
# speed WITHOUT stealing focus from (or appearing on) the user's real monitor. This
# lets a probe run concurrently while the user plays another game.
#
# Mechanism (chosen-surface-at-launch, NOT move-after):
#   1. create a headless output (a virtual monitor wlroots refreshes continuously)
#   2. bind a dedicated workspace to it
#   3. a class-scoped windowrule routes the ER window onto that workspace SILENTLY at
#      open -- it never lands on the user's monitor, so there is nothing to "move".
# Fully reversible: the windowrule is unset and the headless output removed on exit.
#
# Privacy: only the ER window class (steam_app_1245620) is ever referenced; the user's
# other windows are never enumerated. Only `hyprctl monitors` (outputs, not clients) is
# queried, and just for the headless monitor name.
set -euo pipefail

REPO_ROOT="${REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
ER_CLASS="steam_app_1245620"
ER_WS="${ER_WS:-73}"
ENV_FILE="${ENV_FILE:-$REPO_ROOT/.envs/gold-probe.env}"

[[ -n "${HYPRLAND_INSTANCE_SIGNATURE:-}" ]] || { echo "not a Hyprland session (HYPRLAND_INSTANCE_SIGNATURE unset)" >&2; exit 2; }
command -v hyprctl >/dev/null 2>&1 || { echo "hyprctl not found" >&2; exit 2; }
command -v jq >/dev/null 2>&1 || { echo "jq not found" >&2; exit 2; }
[[ -f "$ENV_FILE" ]] || { echo "missing env file: $ENV_FILE" >&2; exit 2; }

headless_names() { hyprctl monitors -j | jq -r '.[].name' | grep '^HEADLESS-' | sort; }

before="$(headless_names || true)"
hyprctl output create headless >/dev/null
# Identify the newly-created headless monitor (set difference before/after).
after="$(headless_names || true)"
NEW_MON="$(comm -13 <(printf '%s\n' "$before") <(printf '%s\n' "$after") | head -1)"
[[ -n "$NEW_MON" ]] || { echo "failed to identify new headless output" >&2; exit 2; }
echo "headless output created: $NEW_MON (ER workspace $ER_WS bound to it)"

cleanup() {
  # Stop the routing poller if still alive, then reverse compositor mutations.
  [[ -n "${ROUTER_PID:-}" ]] && kill "$ROUTER_PID" >/dev/null 2>&1 || true
  hyprctl keyword windowrulev2 "unset,class:^(${ER_CLASS})\$" >/dev/null 2>&1 || true
  hyprctl output remove "$NEW_MON" >/dev/null 2>&1 || true
  echo "compositor restored: removed windowrule + headless output $NEW_MON"
}
trap cleanup EXIT INT TERM HUP

# Bind a dedicated workspace to the headless output, and route ER onto it silently at open.
hyprctl keyword workspace "${ER_WS},monitor:${NEW_MON},default:true" >/dev/null
hyprctl keyword windowrulev2 "workspace ${ER_WS} silent,class:^(${ER_CLASS})\$" >/dev/null

# Routing fallback: Proton sets the ER window class (steam_app_1245620) LATE, so the at-open rule
# above can miss and the window opens on the user's normal workspace (occluded -> Present blocks ->
# game-loop stalls, the verdict of unfocused-throttle-is-present-block-not-code-2026-06-23). A short
# poller re-issues a SILENT move targeting ONLY the ER class (never enumerating other windows, per
# privacy rules) so the window reaches the always-rendering headless output within ~0.2s of mapping
# and starts getting frame callbacks. Idempotent: once moved, re-issues are no-ops. Self-terminates.
(
  for _ in $(seq 1 60); do
    hyprctl dispatch movetoworkspacesilent "${ER_WS},class:^(${ER_CLASS})\$" >/dev/null 2>&1 || true
    sleep 0.25
  done
) &
ROUTER_PID=$!

# Focus-independent run: the watcher relies on in-process telemetry (no screenshot/focus
# dependency); the game renders on the headless output regardless of the user's monitor.
set -a
# shellcheck disable=SC1090
. "$ENV_FILE"
set +a
export RUNTIME_SKIP_VISUAL_CAPTURE=1

bash "$REPO_ROOT/scripts/run-product-continue-direct-probe.sh"
