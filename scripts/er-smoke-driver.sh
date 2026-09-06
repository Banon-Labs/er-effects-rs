#!/usr/bin/env bash
set -euo pipefail

if [[ -f "$HOME/.cargo/env" ]]; then
  # Non-interactive agent shells may not have Cargo on PATH.
  # shellcheck disable=SC1091
  . "$HOME/.cargo/env"
fi

REPO_ROOT="${REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
GAME_DIR="${GAME_DIR:-$HOME/.local/share/Steam/steamapps/common/ELDEN RING/Game}"
SCREENSHOT_HELPER="${SCREENSHOT_HELPER:-/home/banon/projects/scripts/hypr-window-screenshot.sh}"
# me3 delivers the DLL as a native (LazyLoader removed 2026-07-04).
# shellcheck source=scripts/me3-launch-lib.sh
source "$REPO_ROOT/scripts/me3-launch-lib.sh"
ARTIFACT_DIR="${ARTIFACT_DIR:-$REPO_ROOT/target/smoke/driver-$(date +%Y%m%d-%H%M%S)}"
# EVERY per-run artifact is redirected into ARTIFACT_DIR. These four used to default into GAME_DIR,
# which is SINGLE-SLOT: the DLL rotates `<name>` to `<name>.prev` on its first write, so run N-2 is
# already gone -- and the `rm -f` this script ran before launching finished the job, deleting the
# PREVIOUS run's telemetry, autoload log and continue trace outright. Measured 2026-08-31: two
# launches destroyed a 5.4 MB continue trace nobody had read. Copying them out at teardown (see
# `copy_runtime_logs`) never fixed that: by then this run had already clobbered the last one's
# files, and a crashed or killed run never reaches the copy at all.
#
# COMMAND_PATH / AUTOLOAD_PATH stay in GAME_DIR deliberately -- they are INPUT channels this driver
# writes and the DLL reads, not evidence.
TELEMETRY_PATH="${TELEMETRY_PATH:-$ARTIFACT_DIR/er-quickload-telemetry.json}"
COMMAND_PATH="${COMMAND_PATH:-$GAME_DIR/er-quickload-command.txt}"
AUTOLOAD_PATH="${AUTOLOAD_PATH:-$GAME_DIR/er-quickload-autoload.txt}"
AUTOLOAD_DEBUG_PATH="${AUTOLOAD_DEBUG_PATH:-$ARTIFACT_DIR/er-quickload-autoload-debug.log}"
CRASH_LOG_PATH="${CRASH_LOG_PATH:-$ARTIFACT_DIR/er-quickload-crash-log.txt}"
TRACE_CONTINUE_PATH="${TRACE_CONTINUE_PATH:-$ARTIFACT_DIR/er-quickload-continue-trace.log}"
INPUT_TRACE_PATH="${INPUT_TRACE_PATH:-$ARTIFACT_DIR/er-quickload-input-trace.jsonl}"
BOOTSTRAP_PATH="${BOOTSTRAP_PATH:-$ARTIFACT_DIR/er-quickload-bootstrap.jsonl}"
BOOTSTRAP_STATE_PATH="${BOOTSTRAP_STATE_PATH:-$ARTIFACT_DIR/er-quickload-bootstrap-state.json}"
BOOT_PROFILE_PATH="${BOOT_PROFILE_PATH:-$ARTIFACT_DIR/er-quickload-profile.jsonl}"
YDOTOOL_SOCKET="${YDOTOOL_SOCKET:-/run/user/$(id -u)/.ydotool_socket}"
SCREENSHOT_EXT="${SCREENSHOT_EXT:-jpg}"
SCREENSHOT_MAX_WIDTH="${SCREENSHOT_MAX_WIDTH:-900}"
SCREENSHOT_JPEG_QUALITY="${SCREENSHOT_JPEG_QUALITY:-45}"
MAX_NUDGES=0
CAPTURE_EVERY_POLLS=1000
NUDGE_EVERY_POLLS=2000
ALLOW_POINTER_INPUT=0
BUILD=1
INSTALL=1
LAUNCH=1
LAUNCH_MODE=direct
COMMAND=drive
CALL_INDEX=0

usage() {
  cat <<EOF
Usage: $0 [drive|preflight] [options]

The drive command is runtime-disruptive and is disabled fail-closed unless
ER_QUICKLOAD_ALLOW_RUNTIME_DRIVER=1 is set for the exact invocation.

Options:
  --artifact-dir DIR    Capture/log output directory (default: target/smoke/driver-<timestamp>)
  --game-dir DIR        Elden Ring Game directory
  --telemetry PATH      JSON telemetry path written by er_quickload.dll
  --command-path PATH   Text command path consumed by er_quickload.dll
  --autoload-path PATH  Text autoload request path consumed by er_quickload.dll
  --autoload-debug PATH Debug log path written by er_quickload.dll autoload path
  --max-nudges N        Max Enter nudges while waiting (default: 0; disabled)
  --allow-pointer-input Allow legacy center-click OK fallback (default: off)
  --call-index N        Overlay named-call index to toggle for proof (default: 0)
  --screenshot-ext EXT  Screenshot extension: jpg (default) or png
  --no-build            Skip cargo xwin build
  --no-install          Skip staging er_quickload.dll into the me3 profile
  --launch-mode MODE    direct (default) only; Steam/protected launcher modes are policy-blocked
  --no-launch           Skip launch and drive existing game process/window
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    drive|preflight) COMMAND="$1"; shift ;;
    --artifact-dir) ARTIFACT_DIR="$2"; shift 2 ;;
    --game-dir) GAME_DIR="$2"; shift 2 ;;
    --telemetry) TELEMETRY_PATH="$2"; shift 2 ;;
    --command-path) COMMAND_PATH="$2"; shift 2 ;;
    --autoload-path) AUTOLOAD_PATH="$2"; shift 2 ;;
    --autoload-debug) AUTOLOAD_DEBUG_PATH="$2"; shift 2 ;;
    --max-nudges) MAX_NUDGES="$2"; shift 2 ;;
    --allow-pointer-input) ALLOW_POINTER_INPUT=1; shift ;;
    --call-index) CALL_INDEX="$2"; shift 2 ;;
    --screenshot-ext) SCREENSHOT_EXT="$2"; shift 2 ;;
    --no-build) BUILD=0; shift ;;
    --no-install) INSTALL=0; shift ;;
    --launch-mode) LAUNCH_MODE="$2"; shift 2 ;;
    --no-launch) LAUNCH=0; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

log() { printf '[er-smoke-driver] %s\n' "$*"; }
require() { command -v "$1" >/dev/null 2>&1 || { echo "missing required command: $1" >&2; exit 127; }; }

require_runtime_driver_opt_in() {
  if [[ "${ER_QUICKLOAD_ALLOW_RUNTIME_DRIVER:-0}" != "1" ]]; then
    cat >&2 <<'EOF'
Runtime driver execution is disabled fail-closed.

Set ER_QUICKLOAD_ALLOW_RUNTIME_DRIVER=1 only for a deliberate non-autoresearch
runtime validation with observable completion/failure evidence. Do not use
outer agent/tool wall-clock budgets as the safety mechanism.
EOF
    exit 2
  fi
}

preflight() {
  require cargo
  require jq
  require realpath
  require tail
  if [[ "$LAUNCH_MODE" != direct ]]; then
    echo "unsupported launch mode: $LAUNCH_MODE (only direct offline eldenring.exe is allowed)" >&2
    exit 2
  fi
  me3_preflight || { echo "me3 preflight failed" >&2; exit 127; }
  me3_require_no_lazyloader "$GAME_DIR" || { echo "leftover LazyLoader proxy in $GAME_DIR" >&2; exit 127; }
  require ydotool
  require hyprctl
  [[ -x "$SCREENSHOT_HELPER" ]] || { echo "missing screenshot helper: $SCREENSHOT_HELPER" >&2; exit 127; }
  [[ -d "$GAME_DIR" ]] || { echo "missing game dir: $GAME_DIR" >&2; exit 1; }
  [[ -S "$YDOTOOL_SOCKET" ]] || { echo "missing ydotool socket: $YDOTOOL_SOCKET" >&2; exit 1; }
  log "preflight ok"
}

telemetry_source_path() {
  if [[ -s "$TELEMETRY_PATH" ]]; then
    printf '%s\n' "$TELEMETRY_PATH"
    return 0
  fi
  if [[ -s "$GAME_DIR/er-quickload-telemetry.json" ]]; then
    printf '%s\n' "$GAME_DIR/er-quickload-telemetry.json"
    return 0
  fi
  printf '%s\n' "$TELEMETRY_PATH"
}

telemetry_bool() {
  local key="$1" telemetry_source
  telemetry_source=$(telemetry_source_path)
  [[ -s "$telemetry_source" ]] || return 1
  jq -e ".${key} == true" "$telemetry_source" >/dev/null
}

call_active() {
  local telemetry_source
  telemetry_source=$(telemetry_source_path)
  [[ -s "$telemetry_source" ]] || return 1
  jq -e --argjson index "$CALL_INDEX" '.calls[] | select(.index == $index) | .active == true' "$telemetry_source" >/dev/null
}

game_pids() {
  pgrep -f 'eldenring.exe|start_protected_game.exe' || true
}

game_running() {
  [[ -n "$(game_pids)" ]]
}

launcher_pids() {
  local pid_file pid
  for pid_file in "$ARTIFACT_DIR"/*.pid; do
    [[ -s "$pid_file" ]] || continue
    IFS= read -r pid < "$pid_file" || continue
    [[ -n "$pid" ]] || continue
    printf '%s\n' "$pid"
  done
}

launcher_running() {
  local pid
  while IFS= read -r pid; do
    if kill -0 "$pid" 2>/dev/null; then
      return 0
    fi
  done < <(launcher_pids)
  return 1
}

runtime_in_flight() {
  game_running || launcher_running
}

await_telemetry_event() {
  local telemetry_source pid
  local -a tail_pid_args=()
  telemetry_source=$(telemetry_source_path)
  while IFS= read -r pid; do
    [[ -n "$pid" ]] || continue
    tail_pid_args+=("--pid=$pid")
  done < <(game_pids)
  if (( ${#tail_pid_args[@]} == 0 )); then
    echo "Elden Ring process exited while waiting for telemetry" >&2
    return 1
  fi
  if IFS= read -r _ < <(tail -n 0 -F "${tail_pid_args[@]}" "$telemetry_source" 2>/dev/null); then
    return 0
  fi
  echo "Elden Ring process exited while waiting for telemetry" >&2
  return 1
}

copy_runtime_logs() {
  # The DLL now writes these straight into ARTIFACT_DIR (see the redirect block at the top), so this
  # is no longer how the evidence is preserved -- it is a FALLBACK for the case where the env did not
  # reach the game through me3/Proton and the DLL fell back to GAME_DIR. Copying a GAME_DIR file here
  # can only ever recover THIS run's output, never the previous run's; that is why the fix is the
  # redirect and not this function.
  [[ -d "$ARTIFACT_DIR" ]] || return 0
  cp -f "$(telemetry_source_path)" "$ARTIFACT_DIR/telemetry.json" 2>/dev/null || true
  [[ -e "$AUTOLOAD_DEBUG_PATH" ]] ||
    cp -f "$GAME_DIR/er-quickload-autoload-debug.log" "$ARTIFACT_DIR/autoload-debug-default.log" 2>/dev/null || true
  [[ -e "$TRACE_CONTINUE_PATH" ]] ||
    cp -f "$GAME_DIR/er-quickload-continue-trace.log" "$ARTIFACT_DIR/continue-trace.log" 2>/dev/null || true
}

capture() {
  local name="$1" output
  output="$ARTIFACT_DIR/$name.$SCREENSHOT_EXT"
  if [[ "$SCREENSHOT_EXT" =~ ^[jJ][pP][eE]?[gG]$ ]]; then
    "$SCREENSHOT_HELPER" --class steam_app_1245620 --output "$output" --max-width "$SCREENSHOT_MAX_WIDTH" --jpeg-quality "$SCREENSHOT_JPEG_QUALITY" > "$ARTIFACT_DIR/$name.capture.txt" 2>&1 || true
  else
    "$SCREENSHOT_HELPER" --class steam_app_1245620 --output "$output" > "$ARTIFACT_DIR/$name.capture.txt" 2>&1 || true
  fi
}

wait_window() {
  while true; do
    if "$SCREENSHOT_HELPER" --list | rtk grep -qi 'ELDEN RING|eldenring'; then
      return 0
    fi
    if ! runtime_in_flight; then
      echo "Elden Ring process and launcher exited before a window appeared" >&2
      return 1
    fi
  done
}

send_enter() {
  hyprctl dispatch "hl.dsp.focus({window = 'class:steam_app_1245620'})" >/dev/null 2>&1 || true
  YDOTOOL_SOCKET="$YDOTOOL_SOCKET" ydotool key 28:1 28:0 >/dev/null 2>&1 || true
}

click_center_ok() {
  local geometry x y width height
  geometry=$(hyprctl clients -j | jq -r '.[] | select(.class == "steam_app_1245620") | "\(.at[0]) \(.at[1]) \(.size[0]) \(.size[1])"' | head -1)
  [[ -n "$geometry" ]] || return 0
  read -r x y width height <<<"$geometry"
  YDOTOOL_SOCKET="$YDOTOOL_SOCKET" ydotool mousemove -a -x $((x + width / 2)) -y $((y + height * 58 / 100)) >/dev/null 2>&1 || true
  YDOTOOL_SOCKET="$YDOTOOL_SOCKET" ydotool click 0xC0 >/dev/null 2>&1 || true
}

send_driver_command() {
  local command="$1"
  printf '%s\n' "$command" > "$COMMAND_PATH"
}

click_call_checkbox() {
  local geometry x y width height click_x click_y
  geometry=$(hyprctl clients -j | jq -r '.[] | select(.class == "steam_app_1245620") | "\(.at[0]) \(.at[1]) \(.size[0]) \(.size[1])"' | head -1)
  [[ -n "$geometry" ]] || { echo "missing Elden Ring window geometry" >&2; return 1; }
  read -r x y width height <<<"$geometry"
  # Historical UI default was 24x24 points. These coordinates target the named-call
  # checkbox rows in logical Hyprland coordinates, avoiding OCR/template matching.
  click_x=$((x + 42))
  click_y=$((y + 236 + CALL_INDEX * 20))
  YDOTOOL_SOCKET="$YDOTOOL_SOCKET" ydotool mousemove -a -x "$click_x" -y "$click_y" >/dev/null
  YDOTOOL_SOCKET="$YDOTOOL_SOCKET" ydotool click 0xC0 >/dev/null
}

wait_for_player() {
  local polls=0 last_capture_poll=0 last_nudge_poll=0 nudges=0
  while true; do
    if telemetry_bool player_available; then
      log "player_available=true"
      return 0
    fi
    polls=$((polls + 1))
    if (( polls - last_capture_poll >= CAPTURE_EVERY_POLLS )); then
      capture "nav-poll-$polls"
      last_capture_poll=$polls
    fi
    # Do not mash. Use only a few telemetry-poll-spaced nudges to dismiss known
    # boot/title prompts while the hook-side telemetry remains authoritative.
    if (( nudges < MAX_NUDGES && polls - last_nudge_poll >= NUDGE_EVERY_POLLS )); then
      send_enter
      if (( ALLOW_POINTER_INPUT )); then
        click_center_ok
      fi
      last_nudge_poll=$polls
      nudges=$((nudges + 1))
    fi
    await_telemetry_event || return 1
  done
}

wait_for_call_state() {
  local desired="$1"
  while true; do
    if [[ "$desired" == active ]]; then
      call_active && return 0
    else
      call_active || return 0
    fi
    await_telemetry_event || return 1
  done
}

drive() {
  require_runtime_driver_opt_in
  preflight
  ARTIFACT_DIR=$(realpath -m "$ARTIFACT_DIR")
  TELEMETRY_PATH=$(realpath -m "$TELEMETRY_PATH")
  COMMAND_PATH=$(realpath -m "$COMMAND_PATH")
  AUTOLOAD_PATH=$(realpath -m "$AUTOLOAD_PATH")
  AUTOLOAD_DEBUG_PATH=$(realpath -m "$AUTOLOAD_DEBUG_PATH")
  CRASH_LOG_PATH=$(realpath -m "$CRASH_LOG_PATH")
  TRACE_CONTINUE_PATH=$(realpath -m "$TRACE_CONTINUE_PATH")
  INPUT_TRACE_PATH=$(realpath -m "$INPUT_TRACE_PATH")
  BOOTSTRAP_PATH=$(realpath -m "$BOOTSTRAP_PATH")
  BOOTSTRAP_STATE_PATH=$(realpath -m "$BOOTSTRAP_STATE_PATH")
  BOOT_PROFILE_PATH=$(realpath -m "$BOOT_PROFILE_PATH")
  mkdir -p "$ARTIFACT_DIR"
  # THE GAME_DIR ENTRIES ARE GONE FROM THIS LINE ON PURPOSE. They used to read
  #   rm -f ... "$GAME_DIR/er-quickload-telemetry.json" \
  #             "$GAME_DIR/er-quickload-autoload-debug.log" \
  #             "$GAME_DIR/er-quickload-continue-trace.log"
  # which deleted ANOTHER run's evidence -- this driver has no claim on those files, and several
  # sessions launch concurrently in this repo. Everything this run writes now lands in a fresh
  # per-run ARTIFACT_DIR, so there is nothing of ours in GAME_DIR left to clear.
  rm -f "$COMMAND_PATH" "$AUTOLOAD_PATH"
  trap copy_runtime_logs EXIT

  if [[ -n "${ER_QUICKLOAD_AUTOLOAD_SAVE_EXT:-}${ER_QUICKLOAD_AUTOLOAD_SLOT:-}${ER_QUICKLOAD_AUTOLOAD_METHOD:-}" ]]; then
    {
      [[ -z "${ER_QUICKLOAD_AUTOLOAD_SAVE_EXT:-}" ]] || printf 'save_ext=%s\n' "$ER_QUICKLOAD_AUTOLOAD_SAVE_EXT"
      [[ -z "${ER_QUICKLOAD_AUTOLOAD_SLOT:-}" ]] || printf 'slot=%s\n' "$ER_QUICKLOAD_AUTOLOAD_SLOT"
      [[ -z "${ER_QUICKLOAD_AUTOLOAD_METHOD:-}" ]] || printf 'method=%s\n' "$ER_QUICKLOAD_AUTOLOAD_METHOD"
    } > "$AUTOLOAD_PATH"
    cp -f "$AUTOLOAD_PATH" "$ARTIFACT_DIR/autoload-request.txt"
  fi

  if (( BUILD )); then
    log "building DLL"
    (cd "$REPO_ROOT" && cargo xwin build --target x86_64-pc-windows-msvc --release) | tee "$ARTIFACT_DIR/build.log"
  fi

  if (( INSTALL )); then
    log "staging DLL as an me3 native (LazyLoader removed 2026-07-04)"
    cp -f "$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_quickload.dll" "$ARTIFACT_DIR/er_quickload.dll"
    me3_write_profile "$ARTIFACT_DIR/er-quickload-driver.me3" "$ARTIFACT_DIR/er_quickload.dll"
  fi

  if (( LAUNCH )); then
    if [[ "$LAUNCH_MODE" != direct ]]; then
      echo "unsupported launch mode: $LAUNCH_MODE (only direct offline eldenring.exe is allowed)" >&2
      exit 2
    fi
    log "launching Elden Ring through me3 (DLL as me3 native)"
    (
      cd "$GAME_DIR" &&
        ER_QUICKLOAD_TELEMETRY_PATH="$TELEMETRY_PATH" \
        ER_QUICKLOAD_COMMAND_PATH="$COMMAND_PATH" \
        ER_QUICKLOAD_AUTOLOAD_PATH="$AUTOLOAD_PATH" \
        ER_QUICKLOAD_AUTOLOAD_DEBUG_PATH="$AUTOLOAD_DEBUG_PATH" \
        ER_QUICKLOAD_CRASH_LOG_PATH="$CRASH_LOG_PATH" \
        ER_QUICKLOAD_TRACE_CONTINUE_PATH="$TRACE_CONTINUE_PATH" \
        ER_QUICKLOAD_INPUT_TRACE_PATH="$INPUT_TRACE_PATH" \
        ER_QUICKLOAD_BOOTSTRAP_PATH="$BOOTSTRAP_PATH" \
        ER_QUICKLOAD_BOOTSTRAP_STATE_PATH="$BOOTSTRAP_STATE_PATH" \
        ER_QUICKLOAD_PROFILE_PATH="$BOOT_PROFILE_PATH" \
        ER_QUICKLOAD_RELOAD_TRACE_PATH="$ARTIFACT_DIR/er-reload-trace.log" \
        ER_QUICKLOAD_INPUT_HARNESS_LOG_PATH="$ARTIFACT_DIR/er-input-harness.log" \
        ER_QUICKLOAD_INPUT_HARNESS_PHASES_PATH="$ARTIFACT_DIR/er-input-harness-phases.jsonl" \
        ER_QUICKLOAD_DIAG_HARNESS_PATH="$ARTIFACT_DIR/er-diag-harness.log" \
        ER_QUICKLOAD_TIMESERIES_PATH="$ARTIFACT_DIR/er-telemetry-timeseries.jsonl" \
        ER_QUICKLOAD_CPU_PROFILE_PATH="$ARTIFACT_DIR/er-cpu-profile.txt" \
        ER_QUICKLOAD_CRASH_LOGGING_LOG_PATH="$ARTIFACT_DIR/er-crash-log.txt" \
        ER_QUICKLOAD_CRASH_LOGGING_LATEST_PATH="$ARTIFACT_DIR/er-crash-latest.txt" \
        ER_QUICKLOAD_CRASH_LOGGING_BREADCRUMB_PATH="$ARTIFACT_DIR/er-crash-breadcrumb-latest.txt" \
        ER_QUICKLOAD_CRASH_LOGGING_MODULES_PATH="$ARTIFACT_DIR/er-crash-modules.txt" \
        ER_QUICKLOAD_ARMAMENT_ICONS_PATH="$ARTIFACT_DIR/er-armament-icons.log" \
        ER_QUICKLOAD_SAVE_DISABLE_LOG_PATH="$ARTIFACT_DIR/er-save-disable.log" \
        ER_QUICKLOAD_SAVE_DISABLE_TELEMETRY_PATH="$ARTIFACT_DIR/er-save-disable-telemetry.json" \
        ER_QUICKLOAD_LOADING_PORTRAIT_PATH="$ARTIFACT_DIR/er-loading-portrait.log" \
        ER_QUICKLOAD_LOADING_PORTRAIT_CRASH_LOG_PATH="$ARTIFACT_DIR/er-loading-portrait-crash-log.txt" \
        "$ME3_BIN" --steam-dir "$ME3_STEAM_DIR" launch -g eldenring -p "$ARTIFACT_DIR/er-quickload-driver.me3" > "$ARTIFACT_DIR/me3-launch.out" 2>&1 & echo $! > "$ARTIFACT_DIR/me3-launch.pid"
    )
  fi

  wait_window
  capture 00-window
  wait_for_player
  capture 01-before-toggle

  if call_active; then
    log "call $CALL_INDEX initially active; commanding it off first"
    send_driver_command "set $CALL_INDEX off"
    wait_for_call_state inactive
    capture 02-after-initial-off
  fi

  log "commanding checkbox/effect on"
  send_driver_command "set $CALL_INDEX on"
  wait_for_call_state active
  capture 03-after-checkbox-on

  log "commanding checkbox/effect off"
  send_driver_command "set $CALL_INDEX off"
  wait_for_call_state inactive
  capture 04-after-checkbox-off

  cp -f "$(telemetry_source_path)" "$ARTIFACT_DIR/final-telemetry.json" 2>/dev/null || true
  copy_runtime_logs
  log "artifacts: $ARTIFACT_DIR"
}

case "$COMMAND" in
  preflight) preflight ;;
  drive) drive ;;
  *) usage >&2; exit 2 ;;
esac
