#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="${REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
# shellcheck source=scripts/user-runtime-gate.sh
# shellcheck disable=SC1091
source "$REPO_ROOT/scripts/user-runtime-gate.sh"
PROFILE="${ME3_PROFILE:-$REPO_ROOT/target/pi-local/quicksave-row-parity-worktree.me3}"
LAUNCH_SH="${ELDEN_LAUNCH_SH:-$HOME/Elden/launch.sh}"
RUN_NAME="${RUN_NAME:-live-launch-quiet-$(date +%Y%m%d-%H%M%S)}"
RUN_DIR="${RUN_DIR:-$REPO_ROOT/target/pi-local/$RUN_NAME}"
LOG="$RUN_DIR/launch.log"
EDITOR_DIR="${ER_PROFILE_05_010_EDITOR_DIR:-$REPO_ROOT/target/pi-local/profile-editor}"
DRY_RUN=0

# The DLL runs under Proton and takes ER_PROFILE_05_010_EDITOR_DIR VERBATIM into Windows file APIs
# (profile_05_010_editor_runtime.rs `editor_dir()` -> PathBuf -> std::fs), with no translation. A
# unix path is rooted on the CURRENT DRIVE there, and the game's cwd is the install dir on S:, so
# "/home/..." resolves to "S:\home\..." and every control-file read misses -- the editor then sits at
# "live runtime requested but disconnected/no ack" with no error anywhere, because a missing control
# file is a legal "nothing to do". The prefix maps z: -> /, so hand the DLL the Z: form of the very
# same directory the Python editor writes to. Verified against the live prefix's dosdevices.
wine_path_for() {
  local unix_path="$1"
  printf 'Z:%s' "${unix_path//\//\\}"
}

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
  Existing Elden Ring processes are allowed. This launcher records only the wrapper PID it starts;
  unknown preexisting game PIDs are user-owned and must not be touched by agent teardown.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --dry-run) DRY_RUN=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

if [[ "$RUN_DIR" != /* ]]; then
  RUN_DIR="$REPO_ROOT/$RUN_DIR"
fi
if [[ "$PROFILE" != /* ]]; then
  PROFILE="$REPO_ROOT/$PROFILE"
fi

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

mkdir -p "$RUN_DIR/tmp"
mkdir -p "$EDITOR_DIR"
EDITOR_DIR_WIN="$(wine_path_for "$EDITOR_DIR")"
printf '[agent] ME3_PROFILE=%s\n[agent] ME3_BIN=%s\n[agent] TMPDIR=%s\n[agent] ER_PROFILE_05_010_EDITOR_DIR=%s (unix: %s)\n' "$PROFILE" "$REPO_ROOT/scripts/me3-quiet.sh" "$RUN_DIR/tmp" "$EDITOR_DIR_WIN" "$EDITOR_DIR" > "$LOG"

if [[ "$DRY_RUN" == "1" ]]; then
  TMPDIR="$RUN_DIR/tmp" ER_PROFILE_05_010_EDITOR_DIR="$EDITOR_DIR_WIN" ME3_PROFILE="$PROFILE" ME3_BIN="$REPO_ROOT/scripts/me3-quiet.sh" ME3_DRY_RUN=1 "$LAUNCH_SH" >> "$LOG" 2>&1
  cat "$LOG"
  exit 0
fi

require_user_runtime_go
TMPDIR="$RUN_DIR/tmp" ER_PROFILE_05_010_EDITOR_DIR="$EDITOR_DIR_WIN" ME3_PROFILE="$PROFILE" ME3_BIN="$REPO_ROOT/scripts/me3-quiet.sh" nohup "$LAUNCH_SH" >> "$LOG" 2>&1 &
pid=$!
printf '%s\n' "$pid" > "$RUN_DIR/wrapper.pid"
printf 'wrapper_pid=%s\nlaunch_log=%s\n' "$pid" "$LOG"
