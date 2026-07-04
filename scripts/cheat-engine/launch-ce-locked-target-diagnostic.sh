#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
DEFAULT_CT_PATH="$SCRIPT_DIR/locked_target_weapon_level_match_diagnostic.CT"
CT_PATH=${CT_PATH:-$DEFAULT_CT_PATH}
CE_EXE=${CE_EXE:-}
WINE_BIN=${WINE_BIN:-}
PROTON_BIN=${PROTON_BIN:-}
ELDEN_RING_PID=${ELDEN_RING_PID:-}
WAIT=0
TIMEOUT_SECONDS=120
DRY_RUN=0

usage() {
  cat <<'EOF'
Usage: launch-ce-locked-target-diagnostic.sh [options]

Launch Cheat Engine in the same Proton/Wine prefix as an already-running Elden Ring
process, opening the locked-target weapon-level diagnostic CT.

This script does NOT launch Elden Ring, attach CE, or toggle the table entry.
Manual final steps remain:
  1. In Cheat Engine, attach to eldenring.exe.
  2. Enable "ER locked-target weapon level match - DIAGNOSTIC ONLY".

Options:
  --wait                 Wait for eldenring.exe instead of failing immediately.
  --timeout SECONDS      Wait timeout for --wait (default: 120).
  --ct PATH              CT file to open (default: sibling diagnostic CT file).
  --ce-exe PATH          Cheat Engine Windows .exe path. Overrides auto-detection.
  --proton PATH          Proton script/binary to use. Overrides auto-detection.
  --wine PATH            Wine binary to use if Proton cannot be resolved.
  --pid PID              Use a specific running eldenring.exe process.
  --dry-run              Print the launch plan, but do not start Cheat Engine.
  -h, --help             Show this help.

Environment overrides:
  CT_PATH, CE_EXE, PROTON_BIN, WINE_BIN, ELDEN_RING_PID

EOF
}

log() { printf '[ce-er-diagnostic] %s\n' "$*"; }
fail() { printf '[ce-er-diagnostic] ERROR: %s\n' "$*" >&2; exit 1; }

quote_cmd() {
  local out='' arg
  for arg in "$@"; do
    printf -v arg '%q' "$arg"
    out+="${out:+ }$arg"
  done
  printf '%s\n' "$out"
}

parse_args() {
  while (($#)); do
    case "$1" in
      --wait) WAIT=1; shift ;;
      --timeout) [[ $# -ge 2 ]] || fail '--timeout requires a value'; TIMEOUT_SECONDS=$2; shift 2 ;;
      --ct) [[ $# -ge 2 ]] || fail '--ct requires a path'; CT_PATH=$2; shift 2 ;;
      --ce-exe) [[ $# -ge 2 ]] || fail '--ce-exe requires a path'; CE_EXE=$2; shift 2 ;;
      --proton) [[ $# -ge 2 ]] || fail '--proton requires a path'; PROTON_BIN=$2; shift 2 ;;
      --wine) [[ $# -ge 2 ]] || fail '--wine requires a path'; WINE_BIN=$2; shift 2 ;;
      --pid) [[ $# -ge 2 ]] || fail '--pid requires a value'; ELDEN_RING_PID=$2; shift 2 ;;
      --dry-run) DRY_RUN=1; shift ;;
      -h|--help) usage; exit 0 ;;
      *) fail "unknown argument: $1" ;;
    esac
  done
}

validate_positive_int() {
  local name=$1 value=$2
  [[ $value =~ ^[0-9]+$ ]] || fail "$name must be a non-negative integer: $value"
}

pid_comm() {
  local pid=$1
  [[ -r "/proc/$pid/comm" ]] || return 1
  tr -d '\n' < "/proc/$pid/comm"
}

is_er_pid() {
  local pid=$1 comm
  [[ -d "/proc/$pid" ]] || return 1
  comm=$(pid_comm "$pid" 2>/dev/null || true)
  [[ $comm == 'eldenring.exe' ]]
}

find_er_pids() {
  local p comm
  for p in /proc/[0-9]*; do
    p=${p##*/}
    comm=$(pid_comm "$p" 2>/dev/null || true)
    [[ $comm == 'eldenring.exe' ]] && printf '%s\n' "$p"
  done
}

select_er_pid_once() {
  if [[ -n $ELDEN_RING_PID ]]; then
    validate_positive_int ELDEN_RING_PID "$ELDEN_RING_PID"
    is_er_pid "$ELDEN_RING_PID" || fail "PID $ELDEN_RING_PID is not an exact eldenring.exe process"
    printf '%s\n' "$ELDEN_RING_PID"
    return 0
  fi

  mapfile -t pids < <(find_er_pids)
  case ${#pids[@]} in
    0) return 1 ;;
    1) printf '%s\n' "${pids[0]}" ;;
    *) fail "multiple eldenring.exe processes found: ${pids[*]}; rerun with --pid PID" ;;
  esac
}

select_er_pid() {
  local deadline pid
  if ((WAIT == 0)); then
    pid=$(select_er_pid_once) || fail 'eldenring.exe is not running; start Elden Ring first or pass --wait'
    printf '%s\n' "$pid"
    return 0
  fi

  validate_positive_int TIMEOUT_SECONDS "$TIMEOUT_SECONDS"
  deadline=$((SECONDS + TIMEOUT_SECONDS))
  while ((SECONDS <= deadline)); do
    if pid=$(select_er_pid_once 2>/dev/null); then
      printf '%s\n' "$pid"
      return 0
    fi
    sleep 1
  done
  fail "timed out waiting ${TIMEOUT_SECONDS}s for exact eldenring.exe process"
}

proc_env_value() {
  local pid=$1 key=$2
  [[ -r "/proc/$pid/environ" ]] || return 0
  python3 - "$pid" "$key" <<'PY'
import os, sys
pid, key = sys.argv[1], sys.argv[2]
try:
    data = open(f'/proc/{pid}/environ', 'rb').read().split(b'\0')
except OSError:
    sys.exit(0)
prefix = key.encode() + b'='
for item in data:
    if item.startswith(prefix):
        print(item[len(prefix):].decode('utf-8', 'surrogateescape'))
        break
PY
}

first_glob() {
  local pattern match
  shopt -s nullglob
  for match in $pattern; do
    [[ -e $match ]] && { printf '%s\n' "$match"; shopt -u nullglob; return 0; }
  done
  shopt -u nullglob
  return 1
}

find_ce_exe() {
  if [[ -n $CE_EXE ]]; then
    printf '%s\n' "$CE_EXE"
    return 0
  fi

  local patterns=(
    "$HOME/.local/share/Steam/steamapps/compatdata/1245620/pfx/drive_c/Program Files/Cheat Engine*/Cheat Engine.exe"
    "$HOME/.local/share/Steam/steamapps/compatdata/1245620/pfx/drive_c/Program Files (x86)/Cheat Engine*/Cheat Engine.exe"
    "$HOME/.steam/steam/steamapps/compatdata/1245620/pfx/drive_c/Program Files/Cheat Engine*/Cheat Engine.exe"
    "$HOME/.steam/steam/steamapps/compatdata/1245620/pfx/drive_c/Program Files (x86)/Cheat Engine*/Cheat Engine.exe"
    "$HOME/.local/share/Steam/steamapps/compatdata/*/pfx/drive_c/Program Files/Cheat Engine*/Cheat Engine.exe"
    "$HOME/.local/share/Steam/steamapps/compatdata/*/pfx/drive_c/Program Files (x86)/Cheat Engine*/Cheat Engine.exe"
    "$HOME/.steam/steam/steamapps/compatdata/*/pfx/drive_c/Program Files/Cheat Engine*/Cheat Engine.exe"
    "$HOME/.steam/steam/steamapps/compatdata/*/pfx/drive_c/Program Files (x86)/Cheat Engine*/Cheat Engine.exe"
    "$HOME/Downloads/Cheat Engine*/Cheat Engine.exe"
    "$HOME/Games/Cheat Engine*/Cheat Engine.exe"
  )

  local pattern found
  for pattern in "${patterns[@]}"; do
    if found=$(first_glob "$pattern"); then
      printf '%s\n' "$found"
      return 0
    fi
  done

  fail 'could not find Cheat Engine.exe; rerun with --ce-exe PATH or CE_EXE=/path/to/Cheat Engine.exe'
}

resolve_proton() {
  local pid=$1 compat_tool_paths proton candidate
  if [[ -n $PROTON_BIN ]]; then
    printf '%s\n' "$PROTON_BIN"
    return 0
  fi

  compat_tool_paths=$(proc_env_value "$pid" STEAM_COMPAT_TOOL_PATHS || true)
  IFS=':' read -r -a tool_paths <<< "$compat_tool_paths"
  for candidate in "${tool_paths[@]}"; do
    [[ -x "$candidate/proton" ]] && { printf '%s\n' "$candidate/proton"; return 0; }
  done

  local patterns=(
    "$HOME/.local/share/Steam/steamapps/common/Proton*/proton"
    "$HOME/.steam/steam/steamapps/common/Proton*/proton"
  )
  for candidate in "${patterns[@]}"; do
    if proton=$(first_glob "$candidate"); then
      printf '%s\n' "$proton"
      return 0
    fi
  done
  return 1
}

resolve_wine() {
  local pid=$1 loader
  if [[ -n $WINE_BIN ]]; then
    printf '%s\n' "$WINE_BIN"
    return 0
  fi
  loader=$(proc_env_value "$pid" WINELOADER || true)
  if [[ -n $loader && -x $loader ]]; then
    printf '%s\n' "$loader"
    return 0
  fi
  if command -v wine64 >/dev/null 2>&1; then command -v wine64; return 0; fi
  if command -v wine >/dev/null 2>&1; then command -v wine; return 0; fi
  return 1
}

main() {
  parse_args "$@"
  [[ -f $CT_PATH ]] || fail "CT file not found: $CT_PATH"

  local pid world_prefix steam_compat_data_path steam_client_install steam_game_id ce_exe proton wine log_path
  pid=$(select_er_pid)
  log "found eldenring.exe PID $pid"

  steam_compat_data_path=$(proc_env_value "$pid" STEAM_COMPAT_DATA_PATH || true)
  steam_client_install=$(proc_env_value "$pid" STEAM_COMPAT_CLIENT_INSTALL_PATH || true)
  steam_game_id=$(proc_env_value "$pid" SteamGameId || true)
  world_prefix=$(proc_env_value "$pid" WINEPREFIX || true)
  if [[ -z $world_prefix && -n $steam_compat_data_path ]]; then
    world_prefix="$steam_compat_data_path/pfx"
  fi
  [[ -n $world_prefix ]] || fail 'could not resolve WINEPREFIX/STEAM_COMPAT_DATA_PATH from Elden Ring process'
  [[ -d $world_prefix ]] || fail "resolved Wine prefix does not exist: $world_prefix"

  ce_exe=$(find_ce_exe)
  log "CT: $CT_PATH"
  log "Cheat Engine: $ce_exe"
  log "Wine prefix: $world_prefix"
  [[ -n $steam_game_id ]] && log "SteamGameId from target: $steam_game_id"

  export WINEPREFIX="$world_prefix"
  if [[ -n $steam_compat_data_path ]]; then export STEAM_COMPAT_DATA_PATH="$steam_compat_data_path"; fi
  if [[ -n $steam_client_install ]]; then export STEAM_COMPAT_CLIENT_INSTALL_PATH="$steam_client_install"; fi
  export SteamAppId="${steam_game_id:-1245620}"
  export SteamGameId="${steam_game_id:-1245620}"

  local cmd=()
  if proton=$(resolve_proton "$pid"); then
    cmd=("$proton" run "$ce_exe" "$CT_PATH")
    log "launcher: Proton ($proton)"
  elif wine=$(resolve_wine "$pid"); then
    cmd=("$wine" "$ce_exe" "$CT_PATH")
    log "launcher: Wine ($wine)"
  else
    fail 'could not resolve Proton or wine launcher; rerun with --proton PATH or --wine PATH'
  fi

  log "command: $(quote_cmd "${cmd[@]}")"
  log 'manual steps after CE opens: attach to eldenring.exe, then enable the diagnostic record.'

  if ((DRY_RUN)); then
    log 'dry-run requested; not launching Cheat Engine.'
    return 0
  fi

  log_path="$SCRIPT_DIR/cheat-engine-launch.$(date +%Y%m%d-%H%M%S).log"
  nohup "${cmd[@]}" >"$log_path" 2>&1 &
  log "started Cheat Engine launcher PID $!"
  log "launcher log: $log_path"
}

main "$@"
