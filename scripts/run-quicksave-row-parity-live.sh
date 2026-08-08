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
Usage: $0 [--dry-run] [--seamless] [--skip-save-check]

Launch the ProfileSelect row-parity ME3 profile through the user's approved
~/Elden/launch.sh path, but route ME3 through scripts/me3-quiet.sh so ME3's
non-fatal mount_ebl temp-path spam does not dominate live-inspection logs.
Use the normal launcher directly for diagnostic runs where full ME3 tracing is needed.

  --seamless   launch the SEAMLESS profile (launch.sh -s), which sets
               ER_EFFECTS_SAVE_MODE_HINT=seamless and loads the game-installed
               ersc.dll alongside this repo's DLL. Without this the run is
               vanilla, and a Seamless-shaped save will not be the one opened.
  --dry-run    print the launch command without starting the game
  --skip-save-check  bypass the staged-save preflight (you are on your own)

Environment:
  ME3_PROFILE      profile path (default: $PROFILE; ignored with --seamless)
  RUN_DIR          artifact directory (default: $RUN_DIR)
  ELDEN_LAUNCH_SH  launcher script (default: $LAUNCH_SH)
  Existing Elden Ring processes are allowed. This launcher records only the wrapper PID it starts;
  unknown preexisting game PIDs are user-owned and must not be touched by agent teardown.
EOF
}

SEAMLESS=0
SKIP_SAVE_CHECK=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --dry-run) DRY_RUN=1; shift ;;
    --seamless|-s) SEAMLESS=1; shift ;;
    --skip-save-check) SKIP_SAVE_CHECK=1; shift ;;
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

# The profile lives under target/, so a `cargo clean` deletes it. It used to be hand-made, which
# meant the next run failed with "profile not found" and no way to tell what the file should have
# contained. Generate it instead: it is two `[[natives]]` entries whose values this script already
# knows. Per AGENTS.md the Seamless DLL is REFERENCED where the game installed it and never copied,
# staged, or bundled into the repo.
DLL="${ER_EFFECTS_DLL:-$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_effects_rs.dll}"
ERSC_DLL="${ERSC_DLL:-$HOME/.local/share/Steam/steamapps/common/ELDEN RING/Game/SeamlessCoop/ersc.dll}"

# --seamless hands the profile choice to the launcher's own -s, which is the ONLY thing that also
# sets ER_EFFECTS_SAVE_MODE_HINT=seamless. Generating a local profile that merely INCLUDES ersc.dll
# is not equivalent and is how the 2026-08-07 runs ended up vanilla-mode with Seamless loaded: the
# DLL then staged the save under the vanilla container name while the game opened the Seamless one.
LAUNCH_ARGS=()
if [[ "$SEAMLESS" == "1" ]]; then
  LAUNCH_ARGS+=(-s)
  PROFILE=""
fi

if [[ -z "$PROFILE" ]]; then
  : # the launcher picks the profile (and the matching save-mode hint) for us
elif [[ ! -f "$PROFILE" ]]; then
  if [[ ! -f "$DLL" ]]; then
    echo "row-parity-live: cannot generate $PROFILE -- built DLL not found: $DLL" >&2
    echo "row-parity-live: build it with: cargo xwin build -p er-effects-rs --release --target x86_64-pc-windows-msvc" >&2
    exit 1
  fi
  if [[ ! -f "$ERSC_DLL" ]]; then
    echo "row-parity-live: cannot generate $PROFILE -- game-installed Seamless DLL not found: $ERSC_DLL" >&2
    exit 1
  fi
  mkdir -p "$(dirname "$PROFILE")"
  cat > "$PROFILE" <<PROFILE_EOF
profileVersion = "v1"
start_online = false

[[supports]]
game = "eldenring"

[[natives]]
path = '$DLL'

[[natives]]
path = '$ERSC_DLL'
PROFILE_EOF
  echo "row-parity-live: generated $PROFILE" >&2
fi
if [[ ! -x "$LAUNCH_SH" ]]; then
  echo "row-parity-live: launcher not executable: $LAUNCH_SH" >&2
  exit 1
fi
if [[ ! -x "$REPO_ROOT/scripts/me3-quiet.sh" ]]; then
  echo "row-parity-live: quiet ME3 wrapper not executable: $REPO_ROOT/scripts/me3-quiet.sh" >&2
  exit 1
fi

# Staged-save preflight. The configured autoload save is staged into a private root, and a container
# left there by an EARLIER run can be the one the game actually opens -- three launches on
# 2026-08-07 softlocked on exactly that, each one costing a screen and a boot. The failure is
# invisible in the DLL log, which prints a successful "staged N bytes" either way, so it has to be
# checked here, before the launch. See scripts/check-staged-save.py.
SAVE_CHECK="$REPO_ROOT/scripts/check-staged-save.py"
if [[ "$SKIP_SAVE_CHECK" != "1" && -f "$SAVE_CHECK" ]]; then
  if ! python3 "$SAVE_CHECK"; then
    echo "row-parity-live: REFUSING to launch -- the staged save would load the wrong character." >&2
    echo "row-parity-live: fix it with: python3 $SAVE_CHECK --fix" >&2
    echo "row-parity-live: or bypass with --skip-save-check if you know why." >&2
    exit 1
  fi
fi

mkdir -p "$RUN_DIR/tmp"
mkdir -p "$EDITOR_DIR"
EDITOR_DIR_WIN="$(wine_path_for "$EDITOR_DIR")"

# With --seamless the launcher owns the profile AND the save-mode hint, so ME3_PROFILE must be left
# unset rather than passed empty (an empty ME3_PROFILE overrides the launcher's own choice with
# nothing and me3 then has no profile at all).
launch_env=(
  "TMPDIR=$RUN_DIR/tmp"
  "ER_PROFILE_05_010_EDITOR_DIR=$EDITOR_DIR_WIN"
  "ME3_BIN=$REPO_ROOT/scripts/me3-quiet.sh"
)
if [[ -n "$PROFILE" ]]; then
  launch_env+=("ME3_PROFILE=$PROFILE")
fi

printf '[agent] ME3_PROFILE=%s\n[agent] ME3_BIN=%s\n[agent] TMPDIR=%s\n[agent] ER_PROFILE_05_010_EDITOR_DIR=%s (unix: %s)\n[agent] launch args:%s\n' \
  "${PROFILE:-<launcher-chosen>}" "$REPO_ROOT/scripts/me3-quiet.sh" "$RUN_DIR/tmp" "$EDITOR_DIR_WIN" "$EDITOR_DIR" " ${LAUNCH_ARGS[*]:-<none>}" > "$LOG"

if [[ "$DRY_RUN" == "1" ]]; then
  env "${launch_env[@]}" ME3_DRY_RUN=1 "$LAUNCH_SH" "${LAUNCH_ARGS[@]}" >> "$LOG" 2>&1
  cat "$LOG"
  exit 0
fi

require_user_runtime_go
env "${launch_env[@]}" nohup "$LAUNCH_SH" "${LAUNCH_ARGS[@]}" >> "$LOG" 2>&1 &
pid=$!
printf '%s\n' "$pid" > "$RUN_DIR/wrapper.pid"
printf 'wrapper_pid=%s\nlaunch_log=%s\n' "$pid" "$LOG"
