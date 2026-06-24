#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="${REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
GAME_DIR="${GAME_DIR:-$HOME/.local/share/Steam/steamapps/common/ELDEN RING/Game}"
PROTON="${PROTON:-$HOME/.local/share/Steam/steamapps/common/Proton - Experimental/proton}"
STEAM_COMPAT_DATA_PATH="${STEAM_COMPAT_DATA_PATH:-$HOME/.local/share/Steam/steamapps/compatdata/1245620}"
STEAM_COMPAT_CLIENT_INSTALL_PATH="${STEAM_COMPAT_CLIENT_INSTALL_PATH:-$HOME/.local/share/Steam}"
ARTIFACT_DIR="${ARTIFACT_DIR:-$REPO_ROOT/target/runtime-probe/product-continue-direct-$(date +%Y%m%d-%H%M%S)}"
PID_FILE="${PID_FILE:-$ARTIFACT_DIR/proton-run.pid}"
TELEMETRY_PATH="${TELEMETRY_PATH:-$ARTIFACT_DIR/er-effects-telemetry.json}"
BOOTSTRAP_PATH="${BOOTSTRAP_PATH:-$ARTIFACT_DIR/bootstrap.jsonl}"
BOOTSTRAP_STATE_PATH="${BOOTSTRAP_STATE_PATH:-$ARTIFACT_DIR/bootstrap-state.json}"
# CONSOLIDATED per-run DLL log outputs: keep the crash log + autoload debug log in the SAME
# timestamped artifact dir as telemetry/bootstrap, instead of accumulating across runs in the game
# dir under divergent names. The DLL honors ER_EFFECTS_CRASH_LOG_PATH / ER_EFFECTS_AUTOLOAD_DEBUG_PATH.
CRASH_LOG_PATH="${CRASH_LOG_PATH:-$ARTIFACT_DIR/er-effects-crash-log.txt}"
AUTOLOAD_DEBUG_PATH="${AUTOLOAD_DEBUG_PATH:-$ARTIFACT_DIR/er-effects-autoload-debug.log}"
AUTOLOAD_PATH="${AUTOLOAD_PATH:-$GAME_DIR/er-effects-autoload.txt}"
AUTOLOAD_REQUEST="${AUTOLOAD_REQUEST:-}"
# Single source of truth for the runtime-probe wall-clock cap (seconds). Read from the canonical
# file; fail safe to the historical 60 if it is somehow unreadable.
RUNTIME_TIMEOUT_CAP_SECONDS="$(cat "$REPO_ROOT/.auto/runtime_timeout_cap_seconds" 2>/dev/null || echo 45)"
RUNTIME_TIMEOUT_SECONDS="${RUNTIME_TIMEOUT_SECONDS:-$RUNTIME_TIMEOUT_CAP_SECONDS}"
RUNTIME_EXPECTED_MODE="${RUNTIME_EXPECTED_MODE:-vanilla}"
DRY_RUN=0

# SAVE-SOURCE STAGING (save-override-no-default-fallback-mandatory-env-2026-06-23).
# The DLL refuses to assume the default user save dir: it requires ER_EFFECTS_SAVE_FILE (or an
# explicit telemetry-only run). The GOLD SAVE does NOT live in appdata -- the user holds it and
# supplies it via ER_EFFECTS_GOLD_SAVE. Every load probe stages a COPY of that gold save and points
# the DLL at it (autosaves then land in the copy, never anywhere user-owned). Pure observe/menu-reach
# probes set RUNTIME_TELEMETRY_ONLY=1 instead.
GOLD_SAVE="${ER_EFFECTS_GOLD_SAVE:-}"
RUNTIME_TELEMETRY_ONLY="${RUNTIME_TELEMETRY_ONLY:-0}"
# A real fixed-slot ER0000.sl2 BND4 is ~28MB even with empty slots; reject anything implausibly small.
GOLD_SAVE_MIN_BYTES="${GOLD_SAVE_MIN_BYTES:-1048576}"
# Root of the per-account default save dirs. Their SAVE FILES are wiped before launch AND on teardown
# so the game can never read a default character -- a successful load can ONLY come from our override.
# NEVER back these up: the user holds their own backups (never-backup-user-saves-2026-06-23).
APPDATA_ER_ROOT="${APPDATA_ER_ROOT:-$HOME/.local/share/Steam/steamapps/compatdata/1245620/pfx/drive_c/users/steamuser/AppData/Roaming/EldenRing}"

# Wipe (delete, no backup) every save artifact under the default appdata save dirs. Idempotent.
# Skipped when RUNTIME_SKIP_APPDATA_WIPE=1 (the vanilla save-read TRACE needs a char-present save to
# survive in the real appdata so we can observe how the working case opens ER0000.sl2).
wipe_appdata_saves() {
  [[ "${RUNTIME_SKIP_APPDATA_WIPE:-0}" == "1" ]] && return 0
  [[ -d "$APPDATA_ER_ROOT" ]] || return 0
  find "$APPDATA_ER_ROOT" -maxdepth 2 -type f \
    \( -name '*.sl2' -o -name '*.co2' -o -name '*.bak' \) -delete 2>/dev/null || true
}

usage() {
  cat <<EOF
Usage: $0 [--dry-run] [--autoload-request PATH]

Launches the approved direct/offline eldenring.exe runtime path and runs
.auto/runtime_probe.sh as the bounded readiness watcher. This intentionally has
no Steam/AppID launch path and no protected launcher path.

Required for real execution:
  ER_EFFECTS_AUTHORIZED_DIRECT_RUNTIME=1
  AUTO_ALLOW_MANUAL_RUNTIME_PROBE=1
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --dry-run) DRY_RUN=1; shift ;;
    --autoload-request) AUTOLOAD_REQUEST="$2"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

fatal() { echo "run-product-continue-direct-probe: $*" >&2; exit 2; }
require_file() { [[ -f "$1" ]] || fatal "missing file: $1"; }
require_executable() { [[ -x "$1" ]] || fatal "missing executable: $1"; }

runtime_pids() {
  local proc pid comm cmdline
  for proc in /proc/[0-9]*; do
    pid=${proc##*/}
    [[ -r "$proc/comm" ]] || continue
    comm=$(<"$proc/comm")
    # The exact process name is specific to the game (not a broad pattern like "wine"), and
    # "eldenring.exe" (13 chars) is not /proc/comm-truncated. Match it directly so a
    # wine-reparented process whose cmdline no longer contains the install path is still found
    # and torn down -- the earlier cmdline-substring-only match leaked such processes.
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
  # Steam MUST be running: the offline eldenring.exe Proton launch reuses Steam's environment
  # (wineprefix, CWD, Steam account/save-dir id). With Steam down the game still boots but in a
  # DIFFERENT environment -- the DLL's debug log lands elsewhere and Steam-dependent state degrades,
  # producing a non-representative run (observed 2026-06-21). Fail closed rather than burn a launch.
  pgrep -x steam >/dev/null 2>&1 || fatal "Steam is not running; start Steam first (the offline eldenring.exe launch needs Steam's environment, else the run is degraded)"
  [[ "$RUNTIME_TIMEOUT_SECONDS" =~ ^[0-9]+$ ]] || fatal "RUNTIME_TIMEOUT_SECONDS must be an integer"
  (( RUNTIME_TIMEOUT_SECONDS > 0 && RUNTIME_TIMEOUT_SECONDS <= RUNTIME_TIMEOUT_CAP_SECONDS )) || fatal "RUNTIME_TIMEOUT_SECONDS must be 1..$RUNTIME_TIMEOUT_CAP_SECONDS"
  require_executable "$PROTON"
  require_file "$GAME_DIR/eldenring.exe"
  require_file "$REPO_ROOT/.auto/runtime_probe.sh"
  # Validate the probe harness OFFLINE before spending a launch: py_compile + bash -n the probe
  # scripts and exercise the watcher's early-exit telemetry predicates against None/empty/oracle
  # states. A runtime launch must never be burned to discover a pure-Python harness bug.
  if [[ -f "$REPO_ROOT/scripts/preflight-runtime-watcher.py" ]]; then
    python3 "$REPO_ROOT/scripts/preflight-runtime-watcher.py" \
      || fatal "runtime-harness preflight failed; refusing to launch (fix the watcher/probe scripts first)"
  fi
  [[ -d "$STEAM_COMPAT_DATA_PATH" ]] || fatal "missing compatdata path: $STEAM_COMPAT_DATA_PATH"
  if [[ -n "$(runtime_pids)" ]]; then
    fatal "eldenring.exe is already running; refusing to mix probe ownership"
  fi
  # SAVE-PRESENCE GUARD (fail-closed): unless this is an explicit telemetry-only run, a real gold
  # save MUST exist to stage -- otherwise the DLL would abort at init anyway, and historically a
  # missing/empty save silently degraded the run to the level-9 default with NO signal. Catch it
  # here, before burning a launch.
  if [[ "$RUNTIME_TELEMETRY_ONLY" != "1" ]]; then
    [[ -n "$GOLD_SAVE" ]] || fatal "ER_EFFECTS_GOLD_SAVE is unset -- the gold save is NOT in appdata; supply its absolute path (or set RUNTIME_TELEMETRY_ONLY=1 for an observe-only run)"
    [[ -f "$GOLD_SAVE" ]] || fatal "gold save not found: $GOLD_SAVE"
    local gold_bytes
    gold_bytes=$(stat -c '%s' "$GOLD_SAVE" 2>/dev/null || echo 0)
    (( gold_bytes >= GOLD_SAVE_MIN_BYTES )) || fatal "gold save too small ($gold_bytes bytes < $GOLD_SAVE_MIN_BYTES): $GOLD_SAVE -- not a real save"
  fi
}

write_autoload_request() {
  if [[ -n "$AUTOLOAD_REQUEST" ]]; then
    require_file "$AUTOLOAD_REQUEST"
    cp -f "$AUTOLOAD_REQUEST" "$AUTOLOAD_PATH"
    cp -f "$AUTOLOAD_REQUEST" "$ARTIFACT_DIR/autoload-request.txt"
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
  # Deterministic, bounded wait for graceful exit (no sleep): block on each pid's exit with
  # `tail --pid`, hard-capped by a <=30s timeout. tail returns the instant the pid is gone.
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
  # Teardown/crash/trap EVIDENCE: low-quality screenshot of the ER window BEFORE we kill it
  # (best-effort, privacy fail-closed -- only the validated ER window, else a .txt note; never the
  # desktop). Must run while the game window still exists, so it is the FIRST thing in cleanup.
  if [[ -n "${ARTIFACT_DIR:-}" && -d "${ARTIFACT_DIR:-/nonexistent}" ]]; then
    python3 "$REPO_ROOT/scripts/capture-er-window.py" "$ARTIFACT_DIR/teardown-screenshot.jpg" 2>/dev/null || true
    # NOTE: under gamescope --backend headless there is no host window and no screencopy, so the
    # capture above fail-closes to a .txt note -- expected. We observe via in-process telemetry oracles
    # (title/menu state, privacy-policy gate, continue/char oracles), NOT screenshots.
  fi
  if [[ -s "$PID_FILE" ]]; then
    IFS= read -r pid < "$PID_FILE" || pid=""
    if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
      kill "$pid" 2>/dev/null || true
    fi
  fi
  terminate_runtime_pids
  # Teardown wipe: leave the default appdata save dirs with NO save files, every time.
  wipe_appdata_saves
}
trap cleanup EXIT INT TERM HUP

preflight
ARTIFACT_DIR=$(realpath -m "$ARTIFACT_DIR")
PID_FILE=$(realpath -m "$PID_FILE")
TELEMETRY_PATH=$(realpath -m "$TELEMETRY_PATH")
BOOTSTRAP_PATH=$(realpath -m "$BOOTSTRAP_PATH")
BOOTSTRAP_STATE_PATH=$(realpath -m "$BOOTSTRAP_STATE_PATH")
CRASH_LOG_PATH=$(realpath -m "$CRASH_LOG_PATH")
AUTOLOAD_DEBUG_PATH=$(realpath -m "$AUTOLOAD_DEBUG_PATH")
mkdir -p "$ARTIFACT_DIR"

if (( DRY_RUN )); then
  write_autoload_request
  cat > "$ARTIFACT_DIR/dry-run-summary.json" <<EOF
{"artifact_dir":"$ARTIFACT_DIR","launch":"direct-proton-eldenring-exe","watcher":".auto/runtime_probe.sh","timeout_seconds":$RUNTIME_TIMEOUT_SECONDS,"runtime_expected_mode":"$RUNTIME_EXPECTED_MODE"}
EOF
  echo "dry-run ok: would start .auto/runtime_probe.sh, launch direct eldenring.exe through Proton, wait <=${RUNTIME_TIMEOUT_SECONDS}s, then tear down owned launcher pid and exact eldenring.exe runtime pids"
  exit 0
fi

[[ "${ER_EFFECTS_AUTHORIZED_DIRECT_RUNTIME:-0}" == "1" ]] || fatal "set ER_EFFECTS_AUTHORIZED_DIRECT_RUNTIME=1 for the exact runtime invocation"
[[ "${AUTO_ALLOW_MANUAL_RUNTIME_PROBE:-0}" == "1" ]] || fatal "set AUTO_ALLOW_MANUAL_RUNTIME_PROBE=1 for .auto/runtime_probe.sh"
# Reset stale per-run evidence BEFORE launch so the readiness watcher cannot read a PRIOR run's
# completion and tear the new game down instantly. Observed 2026-06-21: a reused ARTIFACT_DIR left
# an old er-effects-telemetry.json at cold_char_mount_phase=5, so every rerun false-positived
# "cold_char_mount_complete" within ~1s (brief white window) before the new process executed
# anything. Deleting these reproduces first-run-in-a-fresh-dir behavior; the DLL re-creates them
# once it boots, and the watcher already tolerates their absence while waiting for fresh telemetry.
rm -f "$TELEMETRY_PATH" "$BOOTSTRAP_PATH" "$BOOTSTRAP_STATE_PATH" "$CRASH_LOG_PATH" "$AUTOLOAD_DEBUG_PATH"
# Wipe any prior teardown screenshot BEFORE the run (never after -- we keep this run's for inspection)
# so a fail-closed/absent capture this run is OBVIOUS (no file) instead of a STALE image we might
# mis-read as current. (Capture is written at teardown in cleanup().)
rm -f "$ARTIFACT_DIR/teardown-screenshot.jpg" "$ARTIFACT_DIR/teardown-screenshot.png" "$ARTIFACT_DIR/teardown-screenshot.txt"
write_autoload_request

# SAVE SOURCE: the DLL never assumes the default user save dir. Either declare telemetry-only
# (loads nothing) or stage an isolated copy of the gold save and point the DLL at it. Staging a
# COPY (named ER0000.sl2 so the DLL's basename-preserving redirect lands on it) means the game's
# autosaves write to the copy, never the user's real save -- save-safe by construction.
if [[ "$RUNTIME_TELEMETRY_ONLY" == "1" ]]; then
  export ER_EFFECTS_TELEMETRY_ONLY=1
  echo "save-source: TELEMETRY-ONLY (no character load; default save dir not read)"
else
  # Stage into an EldenRing/<steamid>/ subtree: the DLL redirects the whole
  # %APPDATA%\Roaming\EldenRing directory handle (the game decides "save present?" by enumerating it,
  # never opening ER0000.sl2 by path), so the staged tree must mirror that structure with the ACTIVE
  # account's SteamID so the game's <steamid> path resolves into our copy.
  ACTIVE_STEAMID="${ER_EFFECTS_ACTIVE_STEAMID:-76561197986456766}"
  STAGED_ROOT="$ARTIFACT_DIR/save"
  # Stage matching the game's own case (EldenRing/<steamid>/ER0000.sl2, as the vanilla-created file).
  # The DLL redirects the %APPDATA% ROOT via SHGetFolderPathW, so the game builds these exact paths
  # under our tree and opens them natively -- an exact-case match is the safest under Wine.
  STAGED_SAVE_DIR="$STAGED_ROOT/EldenRing/$ACTIVE_STEAMID"
  STAGED_SAVE="$STAGED_SAVE_DIR/ER0000.sl2"
  mkdir -p "$STAGED_SAVE_DIR"
  cp -f "$GOLD_SAVE" "$STAGED_SAVE"
  # A real user's save is WRITABLE; our gold sources are deliberately read-only to protect them, and
  # `cp` inherits that bit. The title-flow "Updating save data" step writes the save (autosave/backup),
  # so a read-only staged copy makes it fail -> "Failed to save game. Save data is corrupted." popup
  # (bd offline-notice-fix-works-revealed-save-update-gate-2026-06-23). Make the ISOLATED staged copy
  # writable so that write lands on the copy (save-safe: the user's gold is never touched).
  chmod u+w "$STAGED_SAVE"
  export ER_EFFECTS_SAVE_FILE="$STAGED_SAVE"
  # Steer the native Continue (most-recent) path to the gold character's slot: the DLL calls the
  # game's set_save_slot(GOLD_SLOT) before firing Continue so continue_load(-1) resolves to it. Unset
  # GOLD_SLOT (or -1) leaves the game's true most-recent selection.
  if [[ -n "${ER_EFFECTS_GOLD_SLOT:-}" && "${ER_EFFECTS_GOLD_SLOT}" != "-1" ]]; then
    export ER_EFFECTS_AUTOLOAD_SLOT="$ER_EFFECTS_GOLD_SLOT"
  fi
  echo "save-source: staged gold save -> $STAGED_SAVE (ER_EFFECTS_SAVE_FILE); slot=${ER_EFFECTS_GOLD_SLOT:-most-recent}; autosaves isolated from $GOLD_SAVE"
fi

# Pre-launch wipe: the default appdata save dirs must start empty so the game cannot read a default
# character -- any character that loads can ONLY have come from our redirect. (Also wiped on teardown.)
wipe_appdata_saves

# TRUE T0 = the closest bash timestamp to eldenring.exe process start. Captured here, immediately
# before the Proton launch is fired, written to launch-epoch.txt AND exported to the watcher as
# ER_PROBE_LAUNCH_EPOCH so every milestone delta (and the world-load fail-fast deadline) is measured
# from the real launch, not from watcher-start. The watcher's spawn-poll tolerates the game process
# already existing, so starting it just after the launch fire does not race.
LAUNCH_EPOCH="$(date +%s.%N)"
printf '%s\n' "$LAUNCH_EPOCH" > "$ARTIFACT_DIR/launch-epoch.txt"

# Elden Ring ALWAYS renders inside a gamescope HEADLESS nested compositor. This is the DEFAULT, not a
# toggle: it is OFFSCREEN (no host-compositor window at all -- never on the user's monitor, never
# steals focus) AND full-speed (gamescope always drives Present into its offscreen buffer, so ER never
# present-blocks like an occluded host window would). We DRIVE AND OBSERVE the run entirely via
# in-process RAM telemetry oracles -- gamescope headless cannot be screenshotted and we do not need it
# to: the oracles are the ground truth (see the title/menu state oracles in telemetry). Requires gamescope.
# RUNTIME_ONSCREEN=1: render to a REAL on-screen window (drop gamescope headless) so a human can WATCH
# the zero-input autoload and TEST the loaded character. The DLL's input block auto-releases in-world
# (IN_WORLD_REACHED), so the user takes control once the character is in the world. Default is the
# offscreen headless render (oracle-observed, never on the user's monitor).
if [[ "${RUNTIME_ONSCREEN:-0}" == "1" ]]; then
  gamescope_prefix=()
  echo "render: ON-SCREEN direct Proton window (RUNTIME_ONSCREEN=1) -- watch + test; input block releases in-world"
else
  command -v gamescope >/dev/null 2>&1 || fatal "gamescope not in PATH (required for the offscreen render)"
  gamescope_prefix=(gamescope --backend headless -W "${GAMESCOPE_W:-1280}" -H "${GAMESCOPE_H:-720}" -r "${GAMESCOPE_FPS:-30}" --)
  echo "render: gamescope headless (offscreen; observed via in-process telemetry oracles, not screenshots)"
fi

# RUNTIME_NO_TEARDOWN=1: run the game in the FOREGROUND of this launcher (which a human runs detached,
# e.g. via the agent's background mode) and do NOT run the readiness watcher. Proton's `run` tears the
# wine tree down if its parent dies, so we must stay as the game's parent for its whole lifetime --
# backgrounding and exiting kills it (observed). The zero-input autoload then runs on the user's
# monitor; the DLL input block releases in-world so the user takes over. Tear down with
# `pkill -x eldenring.exe` (or quit the game). Save-safe: the gold is only read; writes go to the
# isolated staged copy / pre-wiped default dir, never save-files/...).
if [[ "${RUNTIME_NO_TEARDOWN:-0}" == "1" ]]; then
  echo "$$" > "$PID_FILE"
  echo ""
  echo "============================================================================"
  echo " ON-SCREEN WATCH RUN -- NO AUTO-TEARDOWN (RUNTIME_NO_TEARDOWN=1)"
  echo " Booting Elden Ring on your monitor (~30s to title+load). The zero-input"
  echo " autoload opens the menu + Continues the gold character; the input block"
  echo " releases once you are in the world, then you can play."
  echo " Telemetry: $TELEMETRY_PATH"
  echo " Debug log: $AUTOLOAD_DEBUG_PATH"
  echo " TEAR DOWN when done:  pkill -x eldenring.exe"
  echo "============================================================================"
  cd "$GAME_DIR"
  # exec -> this launcher BECOMES the foreground Proton process; it holds the game until quit.
  exec env \
    STEAM_COMPAT_CLIENT_INSTALL_PATH="$STEAM_COMPAT_CLIENT_INSTALL_PATH" \
    STEAM_COMPAT_DATA_PATH="$STEAM_COMPAT_DATA_PATH" \
    ER_EFFECTS_TELEMETRY_PATH="$TELEMETRY_PATH" \
    ER_EFFECTS_BOOTSTRAP_PATH="$BOOTSTRAP_PATH" \
    ER_EFFECTS_BOOTSTRAP_STATE_PATH="$BOOTSTRAP_STATE_PATH" \
    ER_EFFECTS_CRASH_LOG_PATH="$CRASH_LOG_PATH" \
    ER_EFFECTS_AUTOLOAD_DEBUG_PATH="$AUTOLOAD_DEBUG_PATH" \
    "$PROTON" run "$GAME_DIR/eldenring.exe" > "$ARTIFACT_DIR/proton-run.out" 2>&1
fi

(
  cd "$GAME_DIR"
  STEAM_COMPAT_CLIENT_INSTALL_PATH="$STEAM_COMPAT_CLIENT_INSTALL_PATH" \
  STEAM_COMPAT_DATA_PATH="$STEAM_COMPAT_DATA_PATH" \
  ER_EFFECTS_TELEMETRY_PATH="$TELEMETRY_PATH" \
  ER_EFFECTS_BOOTSTRAP_PATH="$BOOTSTRAP_PATH" \
  ER_EFFECTS_BOOTSTRAP_STATE_PATH="$BOOTSTRAP_STATE_PATH" \
  ER_EFFECTS_CRASH_LOG_PATH="$CRASH_LOG_PATH" \
  ER_EFFECTS_AUTOLOAD_DEBUG_PATH="$AUTOLOAD_DEBUG_PATH" \
  "${gamescope_prefix[@]}" "$PROTON" run "$GAME_DIR/eldenring.exe" > "$ARTIFACT_DIR/proton-run.out" 2>&1 & echo $! > "$PID_FILE"
)

# ER renders nested in gamescope, so the steam_app_1245620 Hyprland window the watcher screenshots
# never exists -> always rely on in-process telemetry (not a toggle). And the gamescope boot is
# GPU-contended/slower, so the title/world phase-stall watchdogs would false-positive -> disabled.
(
  cd "$REPO_ROOT"
  ARTIFACT_DIR="$ARTIFACT_DIR" \
  PID_FILE="$PID_FILE" \
  TELEMETRY_PATH="$TELEMETRY_PATH" \
  BOOTSTRAP_PATH="$BOOTSTRAP_PATH" \
  BOOTSTRAP_STATE_PATH="$BOOTSTRAP_STATE_PATH" \
  RUNTIME_TIMEOUT_SECONDS="$RUNTIME_TIMEOUT_SECONDS" \
  RUNTIME_EXPECTED_MODE="$RUNTIME_EXPECTED_MODE" \
  ER_PROBE_LAUNCH_EPOCH="$LAUNCH_EPOCH" \
  AUTO_ALLOW_MANUAL_RUNTIME_PROBE=1 \
  RUNTIME_SKIP_VISUAL_CAPTURE=1 \
  RUNTIME_EXTRA_WATCH_ARGS="${RUNTIME_EXTRA_WATCH_ARGS:-"--no-phase-watchdog --no-world-load-deadline"}" \
  ./.auto/runtime_probe.sh
) > "$ARTIFACT_DIR/runtime-probe.out" 2> "$ARTIFACT_DIR/runtime-probe.err" &
watcher_pid=$!

wait "$watcher_pid"
