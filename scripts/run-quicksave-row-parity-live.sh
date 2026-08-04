#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="${REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
PROFILE="${ME3_PROFILE:-$REPO_ROOT/target/pi-local/quicksave-row-parity-worktree.me3}"
LAUNCH_SH="${ELDEN_LAUNCH_SH:-$HOME/Elden/launch.sh}"
RUN_NAME="${RUN_NAME:-live-launch-quiet-$(date +%Y%m%d-%H%M%S)}"
RUN_DIR="${RUN_DIR:-$REPO_ROOT/target/pi-local/$RUN_NAME}"
LOG="$RUN_DIR/launch.log"
DRY_RUN=0

usage() {
  cat <<EOF
Usage: $0 [--dry-run]

Launch the worktree ProfileSelect row-parity ME3 profile through the user's
approved ~/Elden/launch.sh path, but route ME3 through scripts/me3-quiet.sh so
ME3's non-fatal mount_ebl temp-path spam does not dominate live-inspection logs.
Use the normal launcher directly for diagnostic runs where full ME3 tracing is needed.

Environment:
  ME3_PROFILE      profile path (default: $PROFILE)
  RUN_DIR          artifact directory (default: $RUN_DIR)
  ELDEN_LAUNCH_SH  launcher script (default: $LAUNCH_SH)
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --dry-run) DRY_RUN=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

if [[ ! -f "$PROFILE" ]]; then
  echo "row-parity-live: profile not found: $PROFILE" >&2
  exit 1
fi
if [[ ! -x "$LAUNCH_SH" ]]; then
  echo "row-parity-live: launcher not executable: $LAUNCH_SH" >&2
  exit 1
fi
if [[ ! -x "$REPO_ROOT/scripts/me3-quiet.sh" ]]; then
  echo "row-parity-live: quiet ME3 wrapper not executable: $REPO_ROOT/scripts/me3-quiet.sh" >&2
  exit 1
fi

mkdir -p "$RUN_DIR"
printf '[agent] ME3_PROFILE=%s\n[agent] ME3_BIN=%s\n' "$PROFILE" "$REPO_ROOT/scripts/me3-quiet.sh" > "$LOG"

if [[ "$DRY_RUN" == "1" ]]; then
  ME3_PROFILE="$PROFILE" ME3_BIN="$REPO_ROOT/scripts/me3-quiet.sh" ME3_DRY_RUN=1 "$LAUNCH_SH" >> "$LOG" 2>&1
  cat "$LOG"
  exit 0
fi

ME3_PROFILE="$PROFILE" ME3_BIN="$REPO_ROOT/scripts/me3-quiet.sh" nohup "$LAUNCH_SH" >> "$LOG" 2>&1 &
pid=$!
printf '%s\n' "$pid" > "$RUN_DIR/wrapper.pid"
printf 'wrapper_pid=%s\nlaunch_log=%s\n' "$pid" "$LOG"
