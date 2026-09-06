#!/usr/bin/env bash
# Bounded standalone smoke for the individually-shippable loading-portrait DLL
# (crates/er-loading-portrait). Proves er_loading_portrait.dll loads ALONE
# through me3 (no product er_quickload.dll -- NEVER both in one profile: double
# Present detour / double MinHook), attaches in the live process, and its Present
# compositor path runs, with zero crash-log entries.
#
# Teardown is semaphore-driven from the DLL's own log, which this run redirects
# into $ART_DIR (ER_QUICKLOAD_LOADING_PORTRAIT_PATH / _CRASH_LOG_PATH, 2026-08-31);
# the monitor falls back to $GAME_DIR by existence in case the env does not survive
# me3 -> Proton. PASS bar for a no-load boot: the attach line
# ("loaded module_base="), the Present hook's first hit line, then HOLD_SECONDS
# alive past that hit with a clean crash log. "portrait-frame:" compositor lines
# CANNOT be required here: compose_portrait_stats_rgba returns None (hidden
# frames, no log) until a save load publishes portrait/stats content, and
# standalone has no autoload -- if any appear they are logged as a bonus. The
# canonical runtime cap (.auto/runtime_timeout_cap_seconds) is the idle/stall
# backstop. This is a lifecycle/render-path smoke, NOT the full portrait feature
# proof; that stays with the product profile probes.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GAME_DIR="${GAME_DIR:-$HOME/.local/share/Steam/steamapps/common/ELDEN RING/Game}"
DLL_SRC="${PORTRAIT_DLL:-$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_loading_portrait.dll}"
HOLD_SECONDS="${HOLD_SECONDS:-60}"

# shellcheck source=scripts/me3-launch-lib.sh disable=SC1091
source "$REPO_ROOT/scripts/me3-launch-lib.sh"
# shellcheck source=scripts/steam-running.sh disable=SC1091
source "$REPO_ROOT/scripts/steam-running.sh"

CAP_SECONDS="$(python3 "$REPO_ROOT/scripts/runtime_timeout_cap.py")"
STAMP="$(date +%Y%m%d-%H%M%S)"
ART_DIR="$REPO_ROOT/target/runtime-probe/portrait-dll-standalone-$STAMP"
LOG_NAME="er-loading-portrait.log"
CRASH_LOG_NAME="er-loading-portrait-crash-log.txt"

fatal() { echo "portrait-dll-smoke: $*" >&2; exit 2; }

[[ -f "$DLL_SRC" ]] || fatal "missing DLL: $DLL_SRC (build: cargo xwin build --release --target x86_64-pc-windows-msvc -p er-loading-portrait)"
[[ -d "$GAME_DIR" ]] || fatal "missing GAME_DIR: $GAME_DIR"
steam_running || fatal "Steam is not running -- start Steam first (interactive login)"
me3_preflight || fatal "me3 preflight failed"
me3_require_no_lazyloader "$GAME_DIR" || fatal "leftover LazyLoader proxy in $GAME_DIR"
python3 - <<'PY' || fatal "live game/launcher process present -- refusing a second instance"
import os, sys
for p in os.listdir('/proc'):
    if not p.isdigit():
        continue
    try:
        c = open(f'/proc/{p}/comm').read().strip().lower()
    except OSError:
        continue
    if 'eldenring' in c or 'me3-launcher' in c or 'start_protect' in c:
        print(f'live pid={p} comm={c}', file=sys.stderr)
        sys.exit(1)
PY

mkdir -p "$ART_DIR"
cp -f "$DLL_SRC" "$ART_DIR/er_loading_portrait.dll"
me3_write_profile "$ART_DIR/portrait-dll-standalone.me3" "$ART_DIR/er_loading_portrait.dll"
# NOTHING IS DELETED FROM THE GAME DIRECTORY HERE. This used to be
#     rm -f "$GAME_DIR/er-loading-portrait.log" "$GAME_DIR/er-loading-portrait-crash-log.txt"
# to guarantee the lines the monitor read below belonged to THIS run. It destroyed two runs at
# once: the live file, and -- because `er_game_base::log::begin_fresh_run` unconditionally removes
# a stale `<name>.prev` when the live file is absent -- the generation behind it. Neither of them
# was this run's. The freshness those deletes bought comes from the redirect instead: the two
# files are written into `$ART_DIR` (below), which no other run has ever touched, and the monitor
# resolves by existence with `newer_than` so it can never bind to a leftover in `$GAME_DIR`.

echo "portrait-dll-smoke: launching me3 with ONLY er_loading_portrait.dll (cap ${CAP_SECONDS}s, hold ${HOLD_SECONDS}s past first Present) -> $ART_DIR"
# Launch from GAME_DIR: me3 resolves its launcher payload from CWD-relative rust
# target dirs (bd me3-launch-cwd-must-lack-rust-target-dir), and the DLL writes its
# log into the game process CWD -- both need GAME_DIR, exactly like the probe scripts.
# EVERY per-run artifact goes into THIS run's directory. A GAME_DIR artifact is SINGLE-SLOT:
# the DLL rotates `<name>` to `<name>.prev` on its first write, so two launches lose the run
# before last, and several sessions launch concurrently here. `me3_launch` is a shell FUNCTION,
# so an `env VAR=... me3_launch` prefix would not work -- the redirects are exported instead.
# `ER_RUN_ARTIFACT_DIR` is what a watcher reads to find this run rather than the game directory.
export ER_RUN_ARTIFACT_DIR="$ART_DIR"
export ER_QUICKLOAD_TELEMETRY_PATH="$ART_DIR/er-quickload-telemetry.json"
export ER_QUICKLOAD_AUTOLOAD_DEBUG_PATH="$ART_DIR/er-quickload-autoload-debug.log"
export ER_QUICKLOAD_CRASH_LOG_PATH="$ART_DIR/er-quickload-crash-log.txt"
export ER_QUICKLOAD_TRACE_CONTINUE_PATH="$ART_DIR/er-quickload-continue-trace.log"
export ER_QUICKLOAD_INPUT_TRACE_PATH="$ART_DIR/er-quickload-input-trace.jsonl"
export ER_QUICKLOAD_BOOTSTRAP_PATH="$ART_DIR/er-quickload-bootstrap.jsonl"
export ER_QUICKLOAD_BOOTSTRAP_STATE_PATH="$ART_DIR/er-quickload-bootstrap-state.json"
export ER_QUICKLOAD_PROFILE_PATH="$ART_DIR/er-quickload-profile.jsonl"
export ER_QUICKLOAD_RELOAD_TRACE_PATH="$ART_DIR/er-reload-trace.log"
export ER_QUICKLOAD_INPUT_HARNESS_LOG_PATH="$ART_DIR/er-input-harness.log"
export ER_QUICKLOAD_INPUT_HARNESS_PHASES_PATH="$ART_DIR/er-input-harness-phases.jsonl"
export ER_QUICKLOAD_DIAG_HARNESS_PATH="$ART_DIR/er-diag-harness.log"
export ER_QUICKLOAD_TIMESERIES_PATH="$ART_DIR/er-telemetry-timeseries.jsonl"
export ER_QUICKLOAD_CPU_PROFILE_PATH="$ART_DIR/er-cpu-profile.txt"
export ER_QUICKLOAD_ARMAMENT_ICONS_PATH="$ART_DIR/er-armament-icons.log"
export ER_QUICKLOAD_SAVE_DISABLE_LOG_PATH="$ART_DIR/er-save-disable.log"
export ER_QUICKLOAD_SAVE_DISABLE_TELEMETRY_PATH="$ART_DIR/er-save-disable-telemetry.json"
export ER_QUICKLOAD_LOADING_PORTRAIT_PATH="$ART_DIR/er-loading-portrait.log"
export ER_QUICKLOAD_LOADING_PORTRAIT_CRASH_LOG_PATH="$ART_DIR/er-loading-portrait-crash-log.txt"
export ER_QUICKLOAD_CRASH_LOGGING_LOG_PATH="$ART_DIR/er-crash-log.txt"
export ER_QUICKLOAD_CRASH_LOGGING_LATEST_PATH="$ART_DIR/er-crash-latest.txt"
export ER_QUICKLOAD_CRASH_LOGGING_BREADCRUMB_PATH="$ART_DIR/er-crash-breadcrumb-latest.txt"
export ER_QUICKLOAD_CRASH_LOGGING_MODULES_PATH="$ART_DIR/er-crash-modules.txt"

LAUNCH_EPOCH="$(date +%s)"
cd "$GAME_DIR"
me3_launch "$ART_DIR/portrait-dll-standalone.me3" >"$ART_DIR/me3-launch.out" 2>&1 &
ME3_PID=$!
echo "$ME3_PID" > "$ART_DIR/me3-launch.pid"

# Monitor + teardown + sweep, all in one python child so the bash parent stays the
# launch owner for the game's whole lifetime (me3's wine tree dies with its parent).
python3 - "$ME3_PID" "$LOG_NAME" "$CRASH_LOG_NAME" "$ART_DIR" "$CAP_SECONDS" "$HOLD_SECONDS" \
	"$GAME_DIR" "$REPO_ROOT" "$LAUNCH_EPOCH" <<'PY'
import os, shutil, signal, sys, time

me3_pid = int(sys.argv[1])
log_name, crash_log_name, art_dir = sys.argv[2], sys.argv[3], sys.argv[4]
cap_seconds, hold_seconds = int(sys.argv[5]), int(sys.argv[6])
game_dir, repo_root, launch_epoch = sys.argv[7], sys.argv[8], float(sys.argv[9])

# RESOLVE BY EXISTENCE, AND ONLY FILES NEWER THAN THIS LAUNCH. The redirects above are exported
# into the game's environment, but the DLL only honours them if the env survives me3 -> Proton; if
# it does not, it falls back to the game directory rather than writing nowhere. A monitor that
# knows only one of the two calls a healthy run silent. `newer_than` is the other half: the game
# directory still holds the PREVIOUS run's copy of both files (nothing deletes them any more), and
# a reader that resolves at t=0 would bind to it, count its attach line, and report last week's
# run as this one's.
sys.path.insert(0, os.path.join(repo_root, 'scripts'))
from er_artifact_env import resolve_artifact  # noqa: E402 - repo-local, path set above


def artifact(name):
    return str(resolve_artifact(name, game_dir, prefer=art_dir, newer_than=launch_epoch))


ATTACH_MARK = 'loaded module_base='
PRESENT_MARK = 'loading-bar-present: first Present hit'
FRAME_MARK = 'portrait-frame: present_frame='


def read_text(path):
    try:
        return open(path, encoding='utf-8', errors='replace').read()
    except OSError:
        return ''


def pid_alive(pid):
    try:
        os.kill(pid, 0)
        return True
    except OSError:
        return False


def game_pids():
    hits = []
    for p in os.listdir('/proc'):
        if not p.isdigit():
            continue
        try:
            c = open(f'/proc/{p}/comm').read().strip().lower()
        except OSError:
            continue
        if 'eldenring' in c or 'me3-launcher' in c:
            hits.append(int(p))
    return hits


verdict, reason = 'FAIL', f'cap_{cap_seconds}s_without_semaphores'
attach_seen = False
present_at = None
deadline = time.monotonic() + cap_seconds
while time.monotonic() < deadline:
    # Re-resolved every poll: the redirect does not exist until the DLL's first write, so the
    # answer legitimately changes once during the run.
    crash_log, game_log = artifact(crash_log_name), artifact(log_name)
    crash = read_text(crash_log)
    if crash.count('crash logger installed') > 0 and crash.count('\n') > 1:
        verdict, reason = 'FAIL', 'crash_log_entries'
        break
    log = read_text(game_log)
    if not attach_seen and ATTACH_MARK in log:
        attach_seen = True
        print('portrait-dll-smoke: attach line seen', flush=True)
    if present_at is None and PRESENT_MARK in log:
        present_at = time.monotonic()
        print('portrait-dll-smoke: first Present hit seen', flush=True)
    if attach_seen and present_at is not None and time.monotonic() - present_at >= hold_seconds:
        frames = log.count(FRAME_MARK)
        verdict = 'PASS'
        reason = f'attach+present+{hold_seconds}s_alive_clean' + (
            f'+{frames}_bonus_compositor_frames' if frames else ''
        )
        break
    if not pid_alive(me3_pid) and not game_pids():
        verdict, reason = 'FAIL', 'launcher_and_game_exited_early'
        break
    time.sleep(2)

# Teardown: kill the me3 launch owner, then sweep exact game/launcher leftovers.
if pid_alive(me3_pid):
    os.kill(me3_pid, signal.SIGTERM)
for _ in range(15):
    if not pid_alive(me3_pid) and not game_pids():
        break
    time.sleep(1)
for pid in game_pids():
    try:
        os.kill(pid, signal.SIGKILL)
    except OSError:
        pass

# A COPY ONLY FOR THE FALLBACK CASE. When the redirect took, both files are already in
# `art_dir` and this is a no-op -- copying a file onto itself raises `SameFileError`, and doing it
# under a `try` would hide the case where the source really is elsewhere. When the env did NOT
# survive the launch chain the DLL wrote into the game directory, and this is the only chance to
# get a copy into the run's own directory. Either way NOTHING is deleted: the game-directory file
# stays where it is, because it is the next run's `.prev` and somebody else's evidence.
for name in (log_name, crash_log_name):
    src = artifact(name)
    dst = os.path.join(art_dir, name)
    if os.path.exists(src) and not os.path.samefile(os.path.dirname(src) or '.', art_dir):
        shutil.copy2(src, dst)

print(f'portrait-dll-smoke: verdict={verdict} reason={reason} artifacts={art_dir}', flush=True)
sys.exit(0 if verdict == 'PASS' else 1)
PY
