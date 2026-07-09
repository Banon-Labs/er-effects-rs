#!/usr/bin/env bash
# shellcheck disable=SC2329 # cleanup + its helpers run via the EXIT trap; shellcheck 0.11 drops the
# trap reference from its reachability pass when the script ends with an explicit top-level `exit`.
set -euo pipefail

# me3 PRODUCTION SMOKETEST: launch Elden Ring through me3 (garyttierney's mod loader) with
# er_effects_rs.dll delivered as an me3 [[natives]] profile entry -- NO LazyLoader involved --
# and verify our settings stick:
#   * env settings   (*_PATH) must propagate me3 -> compat tool -> game
#   * flag files     (er-effects-autoload.txt etc., resolved from the exe dir) must be honored
#   * the DLL itself must attach + run its game task when loaded by the me3 mod host
#
# me3 launches Game/eldenring.exe directly via the Steam compat tool (waitforexitandrun verb);
# it never touches the protected/EAC launcher and uses no Steam AppID/URL launch form, so it is
# in the same approved direct/offline launch class as run-product-continue-direct-probe.sh.
#
# LazyLoader was removed as a delivery mechanism (2026-07-04); the staging below is a DEFENSIVE
# transition guard: if a leftover proxy (dinput8.dll + lazyLoad.ini) is still in GAME_DIR it is
# staged away for the run and restored on teardown, because an active proxy would DOUBLE-LOAD the
# DLL (me3 native + chainload = two modules, two DllMains, double hooks). This also makes
# DLL-attach attribution exact: with no proxy, a bootstrap event can only come from me3.

REPO_ROOT="${REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
GAME_DIR="${GAME_DIR:-$HOME/.local/share/Steam/steamapps/common/ELDEN RING/Game}"
# Shared me3 launch helpers (ME3_BIN/ME3_STEAM_DIR/ME3_WINDOWS_BIN_DIR/ME3_LOG_DIR defaults,
# compat-tool preflight, profile writer).
# shellcheck source=scripts/me3-launch-lib.sh
# shellcheck disable=SC1091
source "$REPO_ROOT/scripts/me3-launch-lib.sh"
STEAM_COMPAT_DATA_PATH="${STEAM_COMPAT_DATA_PATH:-$HOME/.local/share/Steam/steamapps/compatdata/1245620}"
ARTIFACT_DIR="${ARTIFACT_DIR:-$REPO_ROOT/target/runtime-probe/me3-product-smoke-$(date +%Y%m%d-%H%M%S)}"
PID_FILE="${PID_FILE:-$ARTIFACT_DIR/me3-launch.pid}"
TELEMETRY_PATH="${TELEMETRY_PATH:-$ARTIFACT_DIR/er-effects-telemetry.json}"
BOOTSTRAP_PATH="${BOOTSTRAP_PATH:-$ARTIFACT_DIR/bootstrap.jsonl}"
BOOTSTRAP_STATE_PATH="${BOOTSTRAP_STATE_PATH:-$ARTIFACT_DIR/bootstrap-state.json}"
CRASH_LOG_PATH="${CRASH_LOG_PATH:-$ARTIFACT_DIR/er-effects-crash-log.txt}"
AUTOLOAD_DEBUG_PATH="${AUTOLOAD_DEBUG_PATH:-$ARTIFACT_DIR/er-effects-autoload-debug.log}"
AUTOLOAD_PATH="${AUTOLOAD_PATH:-$GAME_DIR/er-effects-autoload.txt}"
AUTOLOAD_REQUEST="${AUTOLOAD_REQUEST:-}"
BUILT_DLL="${BUILT_DLL:-$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_effects_rs.dll}"
# Single source of truth for the runtime-probe wall-clock cap (seconds); fail safe to the 45s hard truth.
RUNTIME_TIMEOUT_CAP_SECONDS="$(cat "$REPO_ROOT/.auto/runtime_timeout_cap_seconds" 2>/dev/null || echo 45)"
RUNTIME_TIMEOUT_SECONDS="${RUNTIME_TIMEOUT_SECONDS:-$RUNTIME_TIMEOUT_CAP_SECONDS}"
RUNTIME_EXPECTED_MODE="${RUNTIME_EXPECTED_MODE:-vanilla}"
DRY_RUN=0

# DEPRECATED SAVE-SOURCE STAGING: this script's historical non-telemetry mode writes an
# er-effects.toml save_file and stages an isolated gold save. That is no longer release/autoload
# validation; it is a save-redirect-internals probe only. Normal release validation must use the
# user/product launcher path: ~/Elden/launch.sh.
GOLD_SAVE="${ER_EFFECTS_GOLD_SAVE:-$REPO_ROOT/save-files/150-Banon/ER0000.sl2}"
RUNTIME_TELEMETRY_ONLY="${RUNTIME_TELEMETRY_ONLY:-0}"
GOLD_SAVE_MIN_BYTES="${GOLD_SAVE_MIN_BYTES:-1048576}"
ALLOW_DEPRECATED_STAGED_SAVE_PROBE="${ER_EFFECTS_ALLOW_DEPRECATED_STAGED_SAVE_PROBE:-0}"
APPDATA_ER_ROOT="${APPDATA_ER_ROOT:-$STEAM_COMPAT_DATA_PATH/pfx/drive_c/users/steamuser/AppData/Roaming/EldenRing}"

# LazyLoader neutralization: stage the proxy + ini away for the me3 run, restore on teardown.
LAZYLOADER_PROXY="$GAME_DIR/dinput8.dll"
LAZYLOADER_INI="$GAME_DIR/lazyLoad.ini"
ME3_STAGED_SUFFIX=".me3-smoke-staged"

usage() {
  cat <<EOF
Usage: $0 [--dry-run] [--autoload-request PATH]

Deprecated for release/autoload validation: this script's non-telemetry path stages a save_file
and therefore does NOT match the user/product launcher. Use ~/Elden/launch.sh for release/autoload
validation. This script remains only for telemetry or explicit save-redirect internals with
ER_EFFECTS_ALLOW_DEPRECATED_STAGED_SAVE_PROBE=1.
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

fatal() { echo "run-me3-product-smoke: $*" >&2; exit 2; }
require_file() { [[ -f "$1" ]] || fatal "missing file: $1"; }
require_executable() { [[ -x "$1" ]] || fatal "missing executable: $1"; }

wipe_appdata_saves() {
  [[ "${RUNTIME_SKIP_APPDATA_WIPE:-0}" == "1" ]] && return 0
  [[ -d "$APPDATA_ER_ROOT" ]] || return 0
  find "$APPDATA_ER_ROOT" -maxdepth 2 -type f \
    \( -name '*.sl2' -o -name '*.co2' -o -name '*.bak' \) -delete 2>/dev/null || true
}

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
      continue
    fi
    # me3's own Windows-side launcher: part of this run's tree, torn down with it. Match only a
    # REAL launcher invocation (path-anchored), never prose mentions of the name inside an agent
    # shell wrapper's cmdline (a bare substring match self-matched the harness shell that carried
    # this very script's text).
    if [[ "$cmdline" == *"windows-bin/me3-launcher.exe"* || "$cmdline" == *"windows-bin\\me3-launcher.exe"* ]]; then
      printf '%s\n' "$pid"
    fi
  done
}

describe_runtime_pids() {
  local pid
  while IFS= read -r pid; do
    [[ -n "$pid" ]] || continue
    printf '  pid=%s comm=%s cmd=%.140s\n' "$pid" "$(cat "/proc/$pid/comm" 2>/dev/null || echo '?')" \
      "$(tr '\0' ' ' < "/proc/$pid/cmdline" 2>/dev/null || echo '?')"
  done
}

stage_lazyloader_away() {
  local f
  for f in "$LAZYLOADER_PROXY" "$LAZYLOADER_INI"; do
    if [[ -f "$f" && -f "$f$ME3_STAGED_SUFFIX" ]]; then
      fatal "both $f and $f$ME3_STAGED_SUFFIX exist -- resolve the leftover staging manually first"
    fi
    if [[ -f "$f" ]]; then
      mv -f "$f" "$f$ME3_STAGED_SUFFIX"
      echo "lazyloader: staged away $f -> $f$ME3_STAGED_SUFFIX"
    fi
  done
}

restore_lazyloader() {
  local f
  for f in "$LAZYLOADER_PROXY" "$LAZYLOADER_INI"; do
    if [[ -f "$f$ME3_STAGED_SUFFIX" && ! -f "$f" ]]; then
      mv -f "$f$ME3_STAGED_SUFFIX" "$f"
    fi
  done
}

# The autoload flag file is a PRODUCTION SETTING under test. Unlike the dev probe, back up whatever
# request the workspace currently has staged and restore it on teardown so the smoke leaves the
# game dir exactly as found.
FLAG_BACKUP_DIR=""
AUTOLOAD_HAD_ORIGINAL=0
stage_autoload_request() {
  FLAG_BACKUP_DIR="$ARTIFACT_DIR/flag-backup"
  mkdir -p "$FLAG_BACKUP_DIR"
  if [[ -f "$AUTOLOAD_PATH" ]]; then
    AUTOLOAD_HAD_ORIGINAL=1
    cp -f "$AUTOLOAD_PATH" "$FLAG_BACKUP_DIR/er-effects-autoload.txt"
  fi
  if [[ -n "$AUTOLOAD_REQUEST" ]]; then
    require_file "$AUTOLOAD_REQUEST"
    cp -f "$AUTOLOAD_REQUEST" "$AUTOLOAD_PATH"
  else
    printf 'slot=0\n' > "$AUTOLOAD_PATH"
  fi
  cp -f "$AUTOLOAD_PATH" "$ARTIFACT_DIR/autoload-request.txt"
}

restore_autoload_request() {
  [[ -n "$FLAG_BACKUP_DIR" ]] || return 0
  if [[ "$AUTOLOAD_HAD_ORIGINAL" == "1" ]]; then
    cp -f "$FLAG_BACKUP_DIR/er-effects-autoload.txt" "$AUTOLOAD_PATH"
  else
    rm -f "$AUTOLOAD_PATH"
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

collect_me3_logs() {
  # Copy any me3 log written since launch into the artifact dir (evidence of the native load).
  [[ -d "$ME3_LOG_DIR" && -f "$ARTIFACT_DIR/launch-epoch.txt" ]] || return 0
  find "$ME3_LOG_DIR" -maxdepth 2 -type f -newer "$ARTIFACT_DIR/launch-epoch.txt" \
    -exec cp -f {} "$ARTIFACT_DIR/" \; 2>/dev/null || true
}

cleanup() {
  local pid
  collect_me3_logs
  if [[ "${RUNTIME_NO_TEARDOWN:-0}" == "1" ]]; then
    {
      echo "RUNTIME_NO_TEARDOWN=1"
      echo "leaving launcher/game processes alive and preserving staged save/autoload/lazyloader state"
      echo "pid_file=$(cat "$PID_FILE" 2>/dev/null || true)"
      echo "runtime_pids=$(runtime_pids | tr '\n' ' ')"
    } > "$ARTIFACT_DIR/no-teardown.txt"
    echo "cleanup: RUNTIME_NO_TEARDOWN=1; left runtime processes and staged state untouched" >&2
    return 0
  fi
  if [[ -s "$PID_FILE" ]]; then
    IFS= read -r pid < "$PID_FILE" || pid=""
    if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
      kill "$pid" 2>/dev/null || true
    fi
  fi
  terminate_runtime_pids
  wipe_appdata_saves
  restore_autoload_request
  restore_lazyloader
}
trap cleanup EXIT INT TERM HUP

preflight() {
  pgrep -x steam >/dev/null 2>&1 || fatal "Steam is not running; start Steam first (me3 reuses Steam's environment/prefix, else the run is degraded)"
  [[ "$RUNTIME_TIMEOUT_SECONDS" =~ ^[0-9]+$ ]] || fatal "RUNTIME_TIMEOUT_SECONDS must be an integer"
  (( RUNTIME_TIMEOUT_SECONDS > 0 && RUNTIME_TIMEOUT_SECONDS <= RUNTIME_TIMEOUT_CAP_SECONDS )) || fatal "RUNTIME_TIMEOUT_SECONDS must be 1..$RUNTIME_TIMEOUT_CAP_SECONDS"
  me3_preflight || fatal "me3 preflight failed (see guidance above)"
  require_file "$GAME_DIR/eldenring.exe"
  require_file "$REPO_ROOT/.auto/runtime_probe.sh"
  if [[ -f "$REPO_ROOT/scripts/preflight-runtime-watcher.py" ]]; then
    python3 "$REPO_ROOT/scripts/preflight-runtime-watcher.py" \
      || fatal "runtime-harness preflight failed; refusing to launch (fix the watcher/probe scripts first)"
  fi
  [[ -d "$STEAM_COMPAT_DATA_PATH" ]] || fatal "missing compatdata path: $STEAM_COMPAT_DATA_PATH"
  local existing
  existing="$(runtime_pids)"
  if [[ -n "$existing" ]]; then
    printf '%s\n' "$existing" | describe_runtime_pids >&2
    fatal "eldenring.exe (or an me3 launcher) is already running; refusing to mix probe ownership"
  fi
  [[ -f "$BUILT_DLL" ]] || fatal "built DLL not found: $BUILT_DLL -- run 'cargo xwin build --release --target x86_64-pc-windows-msvc' first"
  if [[ "$RUNTIME_TELEMETRY_ONLY" != "1" && "$ALLOW_DEPRECATED_STAGED_SAVE_PROBE" != "1" ]]; then
    fatal "deprecated staged-save/er-effects.toml save_file smoke is disabled for release/autoload validation; use ~/Elden/launch.sh. Set ER_EFFECTS_ALLOW_DEPRECATED_STAGED_SAVE_PROBE=1 only for save-redirect internals."
  fi
  if [[ "$RUNTIME_TELEMETRY_ONLY" != "1" ]]; then
    [[ -f "$GOLD_SAVE" ]] || fatal "gold save not found: $GOLD_SAVE (set ER_EFFECTS_GOLD_SAVE or RUNTIME_TELEMETRY_ONLY=1)"
    local gold_bytes
    gold_bytes=$(stat -c '%s' "$GOLD_SAVE" 2>/dev/null || echo 0)
    (( gold_bytes >= GOLD_SAVE_MIN_BYTES )) || fatal "gold save too small ($gold_bytes bytes < $GOLD_SAVE_MIN_BYTES): $GOLD_SAVE -- not a real save"
  fi
}

preflight
ARTIFACT_DIR=$(realpath -m "$ARTIFACT_DIR")
PID_FILE=$(realpath -m "$PID_FILE")
TELEMETRY_PATH=$(realpath -m "$TELEMETRY_PATH")
BOOTSTRAP_PATH=$(realpath -m "$BOOTSTRAP_PATH")
BOOTSTRAP_STATE_PATH=$(realpath -m "$BOOTSTRAP_STATE_PATH")
CRASH_LOG_PATH=$(realpath -m "$CRASH_LOG_PATH")
AUTOLOAD_DEBUG_PATH=$(realpath -m "$AUTOLOAD_DEBUG_PATH")
mkdir -p "$ARTIFACT_DIR"

# me3 profile: the production-representative delivery -- a ModProfile with our DLL as a native.
# The DLL is copied into the artifact dir so the profile references an immutable per-run payload
# (and the inert GAME_DIR/er_effects_rs.dll chainload copy is never touched or loaded).
SMOKE_DLL="$ARTIFACT_DIR/er_effects_rs.dll"
PROFILE_FILE="$ARTIFACT_DIR/er-effects-me3-smoke.me3"
RUNTIME_CONFIG_FILE="$ARTIFACT_DIR/er-effects.toml"

if (( DRY_RUN )); then
  cat > "$ARTIFACT_DIR/dry-run-summary.json" <<EOF
{"artifact_dir":"$ARTIFACT_DIR","launch":"me3-native-eldenring-exe","watcher":".auto/runtime_probe.sh","timeout_seconds":$RUNTIME_TIMEOUT_SECONDS,"runtime_expected_mode":"$RUNTIME_EXPECTED_MODE"}
EOF
  echo "dry-run ok: would stage LazyLoader away, write me3 profile ($PROFILE_FILE), launch 'me3 launch -g eldenring' with the DLL as a native, wait <=${RUNTIME_TIMEOUT_SECONDS}s under .auto/runtime_probe.sh (RUNTIME_LOADER=me3), score a settings-stick verdict, then restore LazyLoader + flags"
  exit 0
fi

# Reset stale per-run evidence BEFORE launch so the readiness watcher cannot read a PRIOR run's
# completion and tear the new game down instantly.
rm -f "$TELEMETRY_PATH" "$BOOTSTRAP_PATH" "$BOOTSTRAP_STATE_PATH" "$CRASH_LOG_PATH" "$AUTOLOAD_DEBUG_PATH"
rm -f "$ARTIFACT_DIR/loading-screen-portrait-screenshot.jpg" "$ARTIFACT_DIR/loading-screen-portrait-screenshot.png" "$ARTIFACT_DIR/loading-screen-portrait-screenshot.txt"

cp -f "$BUILT_DLL" "$SMOKE_DLL"
me3_write_profile "$PROFILE_FILE" "$SMOKE_DLL"
echo "me3-profile: wrote $PROFILE_FILE (native: $SMOKE_DLL)"

stage_lazyloader_away
stage_autoload_request

# SAVE SOURCE: DEPRECATED staged-save internals path -- isolated writable copy of the gold save,
# pointed at via er-effects.toml save_file; not a release/autoload validation path.
if [[ "$RUNTIME_TELEMETRY_ONLY" == "1" ]]; then
  export ER_EFFECTS_TELEMETRY_ONLY=1
  echo "save-source: TELEMETRY-ONLY (no character load; default save dir not read)"
else
  ACTIVE_STEAMID="${ER_EFFECTS_ACTIVE_STEAMID:-76561197986456766}"
  CONFIG_SLOT="${ER_EFFECTS_GOLD_SLOT:-0}"
  if [[ "$CONFIG_SLOT" == "-1" ]]; then
    CONFIG_SLOT=0
  fi
  if [[ "${RUNTIME_USE_LOOSE_SAVE_CONFIG:-0}" == "1" ]]; then
    LOOSE_SAVE_DIR="$ARTIFACT_DIR/loose-save"
    STAGED_ROOT="$LOOSE_SAVE_DIR/er-effects-save-redirect-stage"
    CONFIG_SAVE="$LOOSE_SAVE_DIR/ER0000.sl2"
    mkdir -p "$LOOSE_SAVE_DIR"
    cp -f "$GOLD_SAVE" "$CONFIG_SAVE"
    chmod u+w "$CONFIG_SAVE"
    echo "save-source: loose configured save -> $CONFIG_SAVE (no EldenRing/SteamID path; no pre-copied discovery tree); slot=$CONFIG_SLOT; source=$GOLD_SAVE"
  else
    STAGED_ROOT="$ARTIFACT_DIR/save"
    STAGED_SAVE_DIR="$STAGED_ROOT/EldenRing/$ACTIVE_STEAMID"
    CONFIG_SAVE="$STAGED_SAVE_DIR/ER0000.sl2"
    mkdir -p "$STAGED_SAVE_DIR"
    cp -f "$GOLD_SAVE" "$CONFIG_SAVE"
    chmod u+w "$CONFIG_SAVE"
    echo "save-source: staged gold save -> $CONFIG_SAVE (er-effects.toml next to DLL); slot=$CONFIG_SLOT; autosaves isolated from $GOLD_SAVE"
  fi
  python3 - "$RUNTIME_CONFIG_FILE" "$CONFIG_SAVE" "$CONFIG_SLOT" <<'PY'
from pathlib import Path
import json
import sys
config = Path(sys.argv[1])
save = sys.argv[2]
slot = int(sys.argv[3])
config.write_text(
    '# Required DLL-adjacent er-effects config. Env vars may override these values.\n'
    f'save_file = {json.dumps(save)}\n'
    f'slot = {slot}\n',
    encoding='utf-8',
)
PY

  DEFAULT_GRAPHICS_CONFIG="$APPDATA_ER_ROOT/GraphicsConfig.xml"
  GRAPHICS_CONFIG_SOURCE="${ER_EFFECTS_GRAPHICS_CONFIG_SOURCE:-${ER_EFFECTS_GOLD_GRAPHICS_CONFIG:-$DEFAULT_GRAPHICS_CONFIG}}"
  if [[ -f "$GRAPHICS_CONFIG_SOURCE" ]]; then
    STAGED_GRAPHICS_CONFIG="$STAGED_ROOT/eldenring/graphicsconfig.xml"
    mkdir -p "$STAGED_ROOT/eldenring"
    cp -f "$GRAPHICS_CONFIG_SOURCE" "$STAGED_GRAPHICS_CONFIG"
    chmod u+w "$STAGED_GRAPHICS_CONFIG"
    echo "graphics-config: staged -> $STAGED_GRAPHICS_CONFIG (source $GRAPHICS_CONFIG_SOURCE)"
  else
    echo "graphics-config: WARN no config at $GRAPHICS_CONFIG_SOURCE -- game will regenerate defaults"
  fi
fi

wipe_appdata_saves

LAUNCH_EPOCH="$(date +%s.%N)"
printf '%s\n' "$LAUNCH_EPOCH" > "$ARTIFACT_DIR/launch-epoch.txt"
ACTIVE_STEAMID_ENV="${ACTIVE_STEAMID:-}"
if [[ "${RUNTIME_EXPORT_ACTIVE_STEAMID:-1}" != "1" ]]; then
  ACTIVE_STEAMID_ENV=""
fi

# Launch through me3. The CLI stays alive as the launch owner (analog of the direct probe's Proton
# parent); killing it + the exact eldenring.exe/me3-launcher.exe pids is the teardown. All
# ER_EFFECTS_* env must survive me3 -> compat tool -> game; the verdict below PROVES whether it did.
(
  cd "$GAME_DIR"
  ER_EFFECTS_TELEMETRY_PATH="$TELEMETRY_PATH" \
  ER_EFFECTS_BOOTSTRAP_PATH="$BOOTSTRAP_PATH" \
  ER_EFFECTS_BOOTSTRAP_STATE_PATH="$BOOTSTRAP_STATE_PATH" \
  ER_EFFECTS_CRASH_LOG_PATH="$CRASH_LOG_PATH" \
  ER_EFFECTS_AUTOLOAD_DEBUG_PATH="$AUTOLOAD_DEBUG_PATH" \
  ER_EFFECTS_ACTIVE_STEAMID="$ACTIVE_STEAMID_ENV" \
  "$ME3_BIN" --steam-dir "$ME3_STEAM_DIR" launch -g eldenring -p "$PROFILE_FILE" \
    > "$ARTIFACT_DIR/me3-launch.out" 2>&1 & echo $! > "$PID_FILE"
)

DEFAULT_RUNTIME_EXTRA_WATCH_ARGS="--no-phase-watchdog --no-world-load-deadline"
watcher_status=0
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
  RUNTIME_SKIP_VISUAL_CAPTURE=1 \
  RUNTIME_EXTRA_WATCH_ARGS="${RUNTIME_EXTRA_WATCH_ARGS:-$DEFAULT_RUNTIME_EXTRA_WATCH_ARGS}" \
  ./.auto/runtime_probe.sh
) > "$ARTIFACT_DIR/runtime-probe.out" 2> "$ARTIFACT_DIR/runtime-probe.err" || watcher_status=$?

collect_me3_logs

# SETTINGS-STICK VERDICT. RAM/in-process telemetry artifacts are the oracles, never screenshots:
#   dll_attach        bootstrap.jsonl has dllmain_attach -> me3 native load worked (proxy staged away,
#                     so no other loader could have produced it)
#   env_stick         the autoload debug log (whose very PATH comes from env) exists and records the
#                     save-override decision -> ER_EFFECTS_* env propagated through me3
#   game_task         the recurring game task registered -> DLL is live on CSTask, not just attached
#   watcher_pass      .auto/runtime_probe.sh readiness watcher exit (world-stable target)
#   crash_free        the crash log recorded no fault
verdict_status=0
if python3 - "$ARTIFACT_DIR" "$BOOTSTRAP_PATH" "$AUTOLOAD_DEBUG_PATH" "$TELEMETRY_PATH" "$CRASH_LOG_PATH" "$watcher_status" <<'PY'
import json
import sys
from pathlib import Path

artifact_dir, bootstrap, debug_log, telemetry, crash_log, watcher_status = sys.argv[1:7]
watcher_status = int(watcher_status)

def read(path):
    try:
        return Path(path).read_text(encoding="utf-8", errors="replace")
    except OSError:
        return ""

boot = read(bootstrap)
debug = read(debug_log)
crash = read(crash_log)

verdict = {
    "dll_attach": '"stage": "dllmain_attach"' in boot or '"stage":"dllmain_attach"' in boot,
    "env_stick": "save-override:" in debug,
    "save_override_enforced": "save-override: ENFORCED" in debug,
    "game_task": "game_task_recurring_registered" in boot,
    "watcher_pass": watcher_status == 0,
    "watcher_status": watcher_status,
    "crash_free": "ACCESS_VIOLATION" not in crash and "unhandled" not in crash.lower(),
    "telemetry_written": Path(telemetry).is_file(),
}
me3_logs = sorted(p.name for p in Path(artifact_dir).glob("*.log") if p.name != Path(debug_log).name)
verdict["me3_log_files"] = me3_logs
core_ok = verdict["dll_attach"] and verdict["env_stick"] and verdict["telemetry_written"]
verdict["settings_stick"] = core_ok
Path(artifact_dir, "me3-smoke-verdict.json").write_text(json.dumps(verdict, indent=2, sort_keys=True) + "\n", encoding="utf-8")
print("me3-smoke-verdict:", json.dumps(verdict, sort_keys=True))
sys.exit(0 if core_ok and verdict["watcher_pass"] else 3)
PY
then
  verdict_status=0
else
  verdict_status=$?
fi

echo "artifacts: $ARTIFACT_DIR"
exit "$verdict_status"
