#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: scripts/triage-er-softlock.sh [--close] [--artifact-dir DIR] [--me3-log PATH] [--telemetry PATH]

Collect Elden Ring soft-lock evidence without visual inspection, then optionally close only exact
Elden Ring/ME3 processes identified by target path. This is the default agent route when a user
reports a soft lock during an er-effects-rs runtime smoke.

Steps:
  1. create an artifact directory,
  2. copy product telemetry, crash log, and ME3 launch log when present,
  3. summarize autoload/menu/native/load experiment semaphores,
  4. snapshot only target Elden Ring/ME3 processes,
  5. with --close, terminate those exact PIDs and verify none remain.
EOF
}

CLOSE=0
ARTIFACT_DIR=""
ME3_LOG=""
TELEMETRY_OVERRIDE=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --close) CLOSE=1; shift ;;
    --artifact-dir) ARTIFACT_DIR="${2:?missing --artifact-dir value}"; shift 2 ;;
    --me3-log) ME3_LOG="${2:?missing --me3-log value}"; shift 2 ;;
    --telemetry) TELEMETRY_OVERRIDE="${2:?missing --telemetry value}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
GAME_DIR="${ER_GAME_DIR:-$HOME/.local/share/Steam/steamapps/common/ELDEN RING/Game}"
TELEMETRY="${TELEMETRY_OVERRIDE:-$GAME_DIR/er-effects-telemetry.json}"
CRASH_LOG="$GAME_DIR/er-effects-crash-log.txt"
if [[ -z "$ARTIFACT_DIR" ]]; then
  ARTIFACT_DIR="$ROOT/target/runtime-probe/softlock-triage-$(date +%Y%m%d-%H%M%S)"
fi
mkdir -p "$ARTIFACT_DIR"
SUMMARY="$ARTIFACT_DIR/summary.txt"
PROCESS_SNAPSHOT="$ARTIFACT_DIR/processes.tsv"

if [[ -z "$ME3_LOG" ]]; then
  ME3_LOG=$(python3 - <<'PY' "$ROOT"
from pathlib import Path
import sys
root=Path(sys.argv[1])
logs=list((root/'target/runtime-probe').glob('*/me3-launch.log'))
if logs:
    print(max(logs, key=lambda p: p.stat().st_mtime))
PY
)
fi

copy_if_present() {
  local src="$1" name="$2"
  if [[ -n "$src" && -f "$src" ]]; then
    cp -f -- "$src" "$ARTIFACT_DIR/$name"
    printf '%s\n' "$ARTIFACT_DIR/$name"
  fi
}

{
  printf 'softlock triage artifact_dir=%s\n' "$ARTIFACT_DIR"
  printf 'timestamp=%s\n' "$(date --iso-8601=seconds)"
  printf 'game_dir=%s\n' "$GAME_DIR"
  printf 'telemetry_copy=%s\n' "$(copy_if_present "$TELEMETRY" er-effects-telemetry.json || true)"
  printf 'crash_log_copy=%s\n' "$(copy_if_present "$CRASH_LOG" er-effects-crash-log.txt || true)"
  printf 'me3_log_copy=%s\n' "$(copy_if_present "$ME3_LOG" me3-launch.log || true)"
  printf '\n## telemetry summary\n'
} >"$SUMMARY"

python3 - <<'PY' "$TELEMETRY" >>"$SUMMARY"
import json, sys
from pathlib import Path
p=Path(sys.argv[1])
if not p.exists():
    print('telemetry missing')
    raise SystemExit(0)
o=json.loads(p.read_text(errors='replace'))
keys=[
 'dll_hash_tag','product_autoload_armed','product_core_autoload_ticks','product_core_ready_successes',
 'product_core_ready_blocker','product_core_last_branch','autoload_attempts','autoload_commits','oracle_native_submit_hits',
 'oracle_continue_phase','oracle_continue_mount_c30','oracle_player_present','oracle_can_move',
 'oracle_msgbox_total_builds','oracle_msgbox_any_seen','oracle_menu_continue_candidate_hits',
 'oracle_menu_continue_candidate_native_accept_hits','oracle_menu_continue_candidate_idle_accept_hits',
 'oracle_menu_item_update_semantic_hits','oracle_menu_window_ctor_semantic_hits',
 'oracle_menu_window_native_ctor_b_hits','oracle_menu_window_native_ctor_b_continue_hits',
 'product_core_last_menu_opened_latch','oracle_menu_window_native_ctor_b_last_item',
 'oracle_menu_window_native_ctor_b_last_accept','oracle_menu_window_native_ctor_b_last_docall',
 'oracle_load_game_fallback_calls','oracle_load_game_fallback_last_item',
 'oracle_load_game_fallback_last_docall','oracle_load_game_fallback_last_blocker',
 'oracle_load_game_fallback_stack_first_external_kind',
 'oracle_load_game_fallback_stack_first_external_label',
 'oracle_load_game_fallback_stack_first_external_name',
 'oracle_load_game_fallback_stack_first_external_base',
 'oracle_load_game_fallback_stack_first_external_offset',
 'oracle_load_game_fallback_stack_self_frames','oracle_load_game_fallback_stack_ersc_frames',
 'oracle_load_game_fallback_stack_me3_frames','oracle_load_game_fallback_stack_other_user_frames',
 'oracle_load_game_fallback_stack_game_frames',
 'oracle_menu_continue_candidate_stack_first_external_kind',
 'oracle_menu_continue_candidate_stack_first_external_label',
 'oracle_menu_continue_candidate_stack_first_external_name',
 'oracle_menu_continue_candidate_stack_first_external_base',
 'oracle_menu_continue_candidate_stack_first_external_offset',
 'oracle_menu_continue_candidate_stack_self_frames','oracle_menu_continue_candidate_stack_ersc_frames',
 'oracle_menu_continue_candidate_stack_me3_frames','oracle_menu_continue_candidate_stack_other_user_frames',
 'oracle_menu_continue_candidate_stack_game_frames',
 'oracle_own_stepper_s2_invoke_calls','oracle_own_stepper_s2_invoke_last_item',
 'oracle_own_stepper_s2_invoke_last_ret','oracle_own_stepper_s2_invoke_last_functor',
 'oracle_own_stepper_s2_invoke_last_ctx10','oracle_own_stepper_s2_invoke_last_pre130',
 'oracle_own_stepper_s2_invoke_last_update_ret','oracle_own_stepper_s2_invoke_last_candidate',
 'oracle_own_stepper_s2_invoke_last_blocker',
 'oracle_loading_bar_enabled','oracle_loading_bar_hook_installed','oracle_loading_bar_update_hits',
 'oracle_loading_bar_current_frame','oracle_loading_bar_max_frame',
 'oracle_loading_bar_progress_permille','oracle_loading_bar_current_terminal',
 'oracle_now_loading','oracle_load_in_progress_b80',
 'oracle_title_05_000_runtime_strip_armed','oracle_title_05_000_runtime_strip_serves',
 'oracle_title_native_menu_visual_suppress_installed','oracle_title_native_menu_visual_suppressed_builds',
 'oracle_stats_panel_enabled','oracle_profile_05_010_runtime_edit_armed',
 'oracle_profile_05_010_runtime_edit_serves','oracle_loading_bg_portrait_redirect_installed',
 'oracle_loading_bg_portrait_redirect_commits','oracle_present_overlay_installed',
 'oracle_native_window_overlay_installed',
]
for k in keys:
    if k in o:
        print(f'{k}={o[k]!r}')
print('\n## compact menu/native keys')
for k in sorted(o):
    if any(s in k for s in ('autoload','continue','menu_window','load_game','native_load','own_stepper_s2','title_native','profile_05_010','loading_bg','loading_bar','now_loading','load_in_progress')):
        v=o[k]
        if isinstance(v,(int,float,str,bool)) or v is None:
            print(f'{k}={v!r}')
PY

# Target-only process snapshot. Avoid pgrep; do not print unrelated windows/apps/processes.
ps -eo pid=,comm=,args= | awk '
  /eldenring\.exe/ || ($2 == "me3" && /-g eldenring/) {
    pid=$1; comm=$2; sub(/^[[:space:]]*[0-9]+[[:space:]]+[^[:space:]]+[[:space:]]+/, "");
    print pid "\t" comm "\t" $0
  }
' >"$PROCESS_SNAPSHOT"
{
  printf '\n## target process snapshot\n'
  if [[ -s "$PROCESS_SNAPSHOT" ]]; then
    cat "$PROCESS_SNAPSHOT"
  else
    printf 'no target eldenring/me3 processes found\n'
  fi
} >>"$SUMMARY"

if command -v tasklist.exe >/dev/null 2>&1; then
  {
    printf '\n## Windows tasklist exact image\n'
    tasklist.exe /FI 'IMAGENAME eq eldenring.exe' 2>/dev/null | tr -d '\r' || true
  } >>"$SUMMARY"
fi

if [[ "$CLOSE" == 1 && -s "$PROCESS_SNAPSHOT" ]]; then
  awk '{print $1}' "$PROCESS_SNAPSHOT" | while read -r pid; do
    [[ "$pid" =~ ^[0-9]+$ ]] || continue
    if kill -0 "$pid" 2>/dev/null; then
      kill "$pid" 2>/dev/null || true
    fi
  done
  wait_args=()
  while read -r pid _; do
    [[ "$pid" =~ ^[0-9]+$ ]] || continue
    if kill -0 "$pid" 2>/dev/null; then
      wait_args+=("--pid=$pid")
    fi
  done <"$PROCESS_SNAPSHOT"
  if ((${#wait_args[@]} > 0)); then
    timeout 2 tail -f "${wait_args[@]}" /dev/null >/dev/null 2>&1 || true
  fi
  awk '{print $1}' "$PROCESS_SNAPSHOT" | while read -r pid; do
    [[ "$pid" =~ ^[0-9]+$ ]] || continue
    if kill -0 "$pid" 2>/dev/null; then
      kill -9 "$pid" 2>/dev/null || true
    fi
  done
  ps -eo pid=,comm=,args= | awk '
    /eldenring\.exe/ || ($2 == "me3" && /-g eldenring/) {
      pid=$1; comm=$2; sub(/^[[:space:]]*[0-9]+[[:space:]]+[^[:space:]]+[[:space:]]+/, "");
      print pid "\t" comm "\t" $0
    }
  ' >"$ARTIFACT_DIR/processes-after-close.tsv"
  {
    printf '\n## close result\n'
    if [[ -s "$ARTIFACT_DIR/processes-after-close.tsv" ]]; then
      printf 'remaining target processes:\n'
      cat "$ARTIFACT_DIR/processes-after-close.tsv"
      exit_code=1
    else
      printf 'all identified target processes closed\n'
      exit_code=0
    fi
  } >>"$SUMMARY"
else
  exit_code=0
fi

cat "$SUMMARY"
exit "${exit_code:-0}"
