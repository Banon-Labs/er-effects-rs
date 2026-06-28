#!/usr/bin/env bash
set -euo pipefail

# GOLDEN mount-trace scout.
#
# Launches the approved direct/offline eldenring.exe runtime path with NO autoload request, so the
# game boots to the title and WAITS for the USER to drive a NATIVE menu load (Title -> Load Game ->
# select save -> confirm). A software INT3 breakpoint armed at MountEblArchive (RVA 0x1efc00, deobf
# VA 0x1401efc00) fires DURING that native load; the DLL's VEH logs every hit's register/stack/caller
# context to the GAME DIR er-effects-crash.log. That caller chain is the evidence we need to replicate
# the m28 EBL mount on the menu-free SetState5 path.
#
# SAVE-SAFE: no SetState5, no own-load, no autoload, no input block. The user loads their own save the
# normal way; we add only a read-only INT3 logger (plus the anti-anti-debug patch the INT3 needs to
# reach our handler). Mirrors run-product-continue-direct-probe.sh's preflight + direct-Proton launch +
# teardown trap, minus the autoload request and the world-stable readiness watcher (the user drives the
# menu by hand, so a fixed bounded wait replaces the early-teardown watcher).
#
# This script does NOT launch the game by itself unless the authorization gates are set; with --dry-run
# it only validates and reports. The orchestrator + user run the real launch.

REPO_ROOT="${REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
GAME_DIR="${GAME_DIR:-$HOME/.local/share/Steam/steamapps/common/ELDEN RING/Game}"
PROTON="${PROTON:-$HOME/.local/share/Steam/steamapps/common/Proton - Experimental/proton}"
STEAM_COMPAT_DATA_PATH="${STEAM_COMPAT_DATA_PATH:-$HOME/.local/share/Steam/steamapps/compatdata/1245620}"
STEAM_COMPAT_CLIENT_INSTALL_PATH="${STEAM_COMPAT_CLIENT_INSTALL_PATH:-$HOME/.local/share/Steam}"
ARTIFACT_DIR="${ARTIFACT_DIR:-$REPO_ROOT/target/runtime-probe/golden-mount-trace-$(date +%Y%m%d-%H%M%S)}"
PID_FILE="${PID_FILE:-$ARTIFACT_DIR/proton-run.pid}"
TELEMETRY_PATH="${TELEMETRY_PATH:-$ARTIFACT_DIR/er-effects-telemetry.json}"
BOOTSTRAP_PATH="${BOOTSTRAP_PATH:-$ARTIFACT_DIR/bootstrap.jsonl}"
BOOTSTRAP_STATE_PATH="${BOOTSTRAP_STATE_PATH:-$ARTIFACT_DIR/bootstrap-state.json}"
DEPLOYED_DLL="${DEPLOYED_DLL:-$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_effects_rs.dll}"

# Game-dir control files this scout writes/manages.
BREAKPOINTS_FILE="$GAME_DIR/er-effects-breakpoints.txt"
CRASH_LOG_ON_FILE="$GAME_DIR/er-effects-crash-log.txt"
CRASH_LOG="$GAME_DIR/er-effects-crash.log"
AUTOLOAD_PATH="$GAME_DIR/er-effects-autoload.txt"
AUTOLOAD_BACKUP="$GAME_DIR/er-effects-autoload.txt.golden-mount-trace.bak"
BLOCK_INPUT_FILE="$GAME_DIR/er-effects-block-input.txt"

# Breakpoint RVA (deobf base 0x140000000): MountEblArchive 0x1401efc00. The DLL's sw-bp VEH now dumps
# a DEEP RAW stack (40 qwords) at each hit, so the user-load mount's full caller chain -- including the
# map-load ORCHESTRATOR our menu-free path skips -- is captured from this single BP (no need to arm
# each frame; in-image return addresses show as 0x140xxxxxxx in the stack=[...] dump).
MOUNT_EBL_ARCHIVE_RVA="${MOUNT_EBL_ARCHIVE_RVA:-1efc00}"

# Single source of truth for the runtime wall-clock cap (seconds). The user needs the FULL window to
# navigate the menu and trigger the load, so default to the cap (120) rather than a shorter probe value.
RUNTIME_TIMEOUT_CAP_SECONDS="$(cat "$REPO_ROOT/.auto/runtime_timeout_cap_seconds" 2>/dev/null || echo 45)"
RUNTIME_TIMEOUT_SECONDS="${RUNTIME_TIMEOUT_SECONDS:-$RUNTIME_TIMEOUT_CAP_SECONDS}"
DRY_RUN=0

usage() {
  cat <<EOF
Usage: $0 [--dry-run]

Prepares and launches the GOLDEN mount-trace scout: a direct/offline eldenring.exe Proton launch with
NO autoload, an INT3 breakpoint at MountEblArchive (RVA 0x$MOUNT_EBL_ARCHIVE_RVA), so the USER can drive
one native menu load and the DLL logs the m28 EBL-mount caller chain to the game-dir er-effects-crash.log.

Required for real execution (recommended: source .envs/golden-mount-trace.env):
  ER_EFFECTS_AUTHORIZED_DIRECT_RUNTIME=1
  AUTO_ALLOW_MANUAL_RUNTIME_PROBE=1
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --dry-run) DRY_RUN=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

fatal() { echo "run-golden-mount-trace: $*" >&2; exit 2; }
require_file() { [[ -f "$1" ]] || fatal "missing file: $1"; }
require_executable() { [[ -x "$1" ]] || fatal "missing executable: $1"; }

runtime_pids() {
  local proc pid comm cmdline
  for proc in /proc/[0-9]*; do
    pid=${proc##*/}
    [[ -r "$proc/comm" ]] || continue
    comm=$(<"$proc/comm")
    if [[ "$comm" == "eldenring.exe" ]]; then
      printf '%s\n' "$pid"
      continue
    fi
    [[ -r "$proc/cmdline" ]] || continue
    cmdline=$(tr '\0' ' ' < "$proc/cmdline" 2>/dev/null || true)
    if [[ "$cmdline" == *"$GAME_DIR/eldenring.exe"* ]]; then
      printf '%s\n' "$pid"
      continue
    fi
    if [[ "$cmdline" == *"ELDEN RING\\Game\\eldenring.exe"* ]]; then
      printf '%s\n' "$pid"
    fi
  done
}

preflight() {
  # Steam MUST be running: the offline launch reuses Steam's environment (wineprefix, CWD, account/
  # save-dir id). With Steam down the game boots in a DIFFERENT environment and the run is degraded.
  pgrep -x steam >/dev/null 2>&1 || fatal "Steam is not running; start Steam first (the offline eldenring.exe launch needs Steam's environment, else the run is degraded)"
  [[ "$RUNTIME_TIMEOUT_SECONDS" =~ ^[0-9]+$ ]] || fatal "RUNTIME_TIMEOUT_SECONDS must be an integer"
  (( RUNTIME_TIMEOUT_SECONDS > 0 && RUNTIME_TIMEOUT_SECONDS <= RUNTIME_TIMEOUT_CAP_SECONDS )) || fatal "RUNTIME_TIMEOUT_SECONDS must be 1..$RUNTIME_TIMEOUT_CAP_SECONDS"
  require_executable "$PROTON"
  require_file "$GAME_DIR/eldenring.exe"
  require_file "$DEPLOYED_DLL"
  [[ -d "$STEAM_COMPAT_DATA_PATH" ]] || fatal "missing compatdata path: $STEAM_COMPAT_DATA_PATH"
  if [[ -n "$(runtime_pids)" ]]; then
    fatal "eldenring.exe is already running; refusing to mix probe ownership"
  fi
}

# Arm the scout's GAME-DIR control files. Idempotent. Backs up (does not delete) the user's existing
# autoload.txt so this run boots to the title with NO menu-free load; restores it on exit.
arm_scout_files() {
  # 1) Breakpoint file: one hex RVA per line -> sw_breakpoints_enabled() true -> anti-anti-debug auto-on.
  #    Default = MountEblArchive; override via BREAKPOINTS_RVAS (space-separated) for a different trace
  #    (e.g. the Continue-confirm LoadGame-build "826510" to capture the native confirm's real ctx args
  #    + caller chain). The deep raw stack dump at each hit captures the full caller chain.
  : > "$BREAKPOINTS_FILE"
  for _rva in ${BREAKPOINTS_RVAS:-$MOUNT_EBL_ARCHIVE_RVA}; do
    printf '%s\n' "$_rva" >> "$BREAKPOINTS_FILE"
  done
  cp -f "$BREAKPOINTS_FILE" "$ARTIFACT_DIR/er-effects-breakpoints.txt"
  # 1b) UI overlay OFF: no extra render hooks/overhead for a clean trace run.
  : > "$GAME_DIR/er-effects-no-overlay.txt"
  # 2) Crash log on (file channel; reliable through Proton). Do NOT truncate er-effects-crash.log --
  #    the new sw-bp lines APPEND to whatever the user already has.
  [[ -f "$CRASH_LOG_ON_FILE" ]] || : > "$CRASH_LOG_ON_FILE"
  # 3) No autoload: the USER drives the native menu. Move the existing autoload request aside so our
  #    own-load/SetState5 path never arms; restore on exit so the user's config survives.
  if [[ -f "$AUTOLOAD_PATH" ]]; then
    mv -f "$AUTOLOAD_PATH" "$AUTOLOAD_BACKUP"
  fi
  # 4) No input block: the user must navigate. Remove any stale block-input gate file.
  rm -f "$BLOCK_INPUT_FILE"
  # Record where the live crash log is for the post-run grep.
  echo "$CRASH_LOG" > "$ARTIFACT_DIR/crash-log-path.txt"
}

restore_scout_files() {
  # Restore the user's autoload.txt that we moved aside (only if we created the backup AND the user
  # did not write a new one in the meantime).
  if [[ -f "$AUTOLOAD_BACKUP" && ! -f "$AUTOLOAD_PATH" ]]; then
    mv -f "$AUTOLOAD_BACKUP" "$AUTOLOAD_PATH"
  fi
}

terminate_runtime_pids() {
  local pid
  local -a pids=()
  mapfile -t pids < <(runtime_pids)
  for pid in "${pids[@]}"; do
    if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
      kill "$pid" 2>/dev/null || true
    fi
  done
  for pid in "${pids[@]}"; do
    [[ -n "$pid" ]] || continue
    timeout 6 tail --pid="$pid" -f /dev/null >/dev/null 2>&1 || true
  done
  mapfile -t pids < <(runtime_pids)
  for pid in "${pids[@]}"; do
    if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
      kill -9 "$pid" 2>/dev/null || true
    fi
  done
}

cleanup() {
  local pid
  if [[ -s "$PID_FILE" ]]; then
    IFS= read -r pid < "$PID_FILE" || pid=""
    if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
      kill "$pid" 2>/dev/null || true
    fi
  fi
  terminate_runtime_pids
  restore_scout_files
}
trap cleanup EXIT

preflight
ARTIFACT_DIR=$(realpath -m "$ARTIFACT_DIR")
PID_FILE=$(realpath -m "$PID_FILE")
mkdir -p "$ARTIFACT_DIR"

if (( DRY_RUN )); then
  cat > "$ARTIFACT_DIR/dry-run-summary.json" <<EOF
{"artifact_dir":"$ARTIFACT_DIR","launch":"direct-proton-eldenring-exe-no-autoload","autoload":"none-user-drives-menu","breakpoint_rva":"0x$MOUNT_EBL_ARCHIVE_RVA","timeout_seconds":$RUNTIME_TIMEOUT_SECONDS,"crash_log":"$CRASH_LOG"}
EOF
  echo "dry-run ok: would arm INT3 at RVA 0x$MOUNT_EBL_ARCHIVE_RVA (MountEblArchive), move aside er-effects-autoload.txt, deploy the DLL + LazyLoader chainload, launch direct eldenring.exe through Proton with NO autoload, wait <=${RUNTIME_TIMEOUT_SECONDS}s for the user to drive a native load, then tear down the owned launcher pid + exact eldenring.exe runtime pids and restore the user's autoload.txt"
  exit 0
fi

[[ "${ER_EFFECTS_AUTHORIZED_DIRECT_RUNTIME:-0}" == "1" ]] || fatal "set ER_EFFECTS_AUTHORIZED_DIRECT_RUNTIME=1 for the exact runtime invocation (source .envs/golden-mount-trace.env)"
[[ "${AUTO_ALLOW_MANUAL_RUNTIME_PROBE:-0}" == "1" ]] || fatal "set AUTO_ALLOW_MANUAL_RUNTIME_PROBE=1 for the manual runtime probe (source .envs/golden-mount-trace.env)"

# Deploy the current DLL + LazyLoader chainload config (mirrors .auto/runtime_probe.sh setup, minus the
# readiness watcher). The deployed DLL already carries the sw-bp + anti-anti-debug facility.
mkdir -p "$GAME_DIR/dllMods"
cp -f "$DEPLOYED_DLL" "$GAME_DIR/er_effects_rs.dll"
rm -f "$GAME_DIR/dllMods/er_effects_rs.dll"
cat > "$GAME_DIR/lazyLoad.ini" <<'EOF'
; LazyLoader by Church Guard
[LAZYLOAD]
dllModFolderName=dllMods
[LOADORDER]
[CHAINLOAD]
dll=er_effects_rs.dll
EOF

arm_scout_files

# Record the live crash log size BEFORE launch so the post-run grep can focus on the new tail.
wc -l < "$CRASH_LOG" > "$ARTIFACT_DIR/crash-log-lines-before.txt" 2>/dev/null || echo 0 > "$ARTIFACT_DIR/crash-log-lines-before.txt"

echo "***** GOLDEN MOUNT-TRACE SCOUT: launching eldenring.exe (NATIVE, user-driven) -- window <=${RUNTIME_TIMEOUT_SECONDS}s *****"
echo "***** USER: at the title, do Continue (or Load Game -> pick your save -> confirm). The INT3 at MountEblArchive logs to: $CRASH_LOG *****"

# TRUE T0 = the closest bash timestamp to eldenring.exe process start, written to launch-epoch.txt so
# golden runs report the same headline metric (world-loaded - bash launch) as the product probe. The
# user drives the native menu by hand here; the DLL's own load-timeline markers (EVENT ... ms=) plus
# the [+Nms] DLL-log prefix carry the in-process offsets, and this file anchors the bash launch epoch.
LAUNCH_EPOCH="$(date +%s.%N)"
printf '%s\n' "$LAUNCH_EPOCH" > "$ARTIFACT_DIR/launch-epoch.txt"
export ER_PROBE_LAUNCH_EPOCH="$LAUNCH_EPOCH"

# Direct/offline Proton launch, no autoload request. Bounded by RUNTIME_TIMEOUT_SECONDS so the run can
# never overrun the cap even if the user walks away; the EXIT trap tears the game + restores state.
(
  cd "$GAME_DIR"
  STEAM_COMPAT_CLIENT_INSTALL_PATH="$STEAM_COMPAT_CLIENT_INSTALL_PATH" \
  STEAM_COMPAT_DATA_PATH="$STEAM_COMPAT_DATA_PATH" \
  ER_EFFECTS_TELEMETRY_PATH="$TELEMETRY_PATH" \
  ER_EFFECTS_BOOTSTRAP_PATH="$BOOTSTRAP_PATH" \
  ER_EFFECTS_BOOTSTRAP_STATE_PATH="$BOOTSTRAP_STATE_PATH" \
  "$PROTON" run "$GAME_DIR/eldenring.exe" > "$ARTIFACT_DIR/proton-run.out" 2>&1 & echo $! > "$PID_FILE"
)

launcher_pid="$(cat "$PID_FILE" 2>/dev/null || echo)"
if [[ -n "$launcher_pid" ]]; then
  # Bounded wait for launcher exit, in literal <=30s segments up to RUNTIME_TIMEOUT_SECONDS. Each
  # `tail --pid` returns the instant the launcher exits; the literal per-segment timeout is the safety
  # cap (the no-timeouts scanner forbids a variable timeout duration). No sleeps.
  golden_waited=0
  while kill -0 "$launcher_pid" 2>/dev/null && (( golden_waited < RUNTIME_TIMEOUT_SECONDS )); do
    timeout 20 tail --pid="$launcher_pid" -f /dev/null >/dev/null 2>&1 || true
    golden_waited=$(( golden_waited + 20 ))
  done
fi

# Capture the new sw-bp tail for convenience (the authoritative copy stays in the game-dir crash log).
grep -a 'sw-bp' "$CRASH_LOG" > "$ARTIFACT_DIR/sw-bp-lines.txt" 2>/dev/null || true
echo "golden mount-trace scout done. Evidence: $CRASH_LOG (sw-bp lines copied to $ARTIFACT_DIR/sw-bp-lines.txt)"
