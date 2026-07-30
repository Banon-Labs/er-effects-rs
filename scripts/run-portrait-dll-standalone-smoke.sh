#!/usr/bin/env bash
# Bounded standalone smoke for the individually-shippable loading-portrait DLL
# (crates/er-loading-portrait-dll). Proves er_loading_portrait_dll.dll loads ALONE
# through me3 (no product er_effects_rs.dll -- NEVER both in one profile: double
# Present detour / double MinHook), attaches in the live process, and its Present
# compositor path runs, with zero crash-log entries.
#
# Teardown is semaphore-driven from the DLL's own log (it writes into the game
# process CWD = $GAME_DIR). PASS bar for a no-load boot: the attach line
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
DLL_SRC="${PORTRAIT_DLL:-$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_loading_portrait_dll.dll}"
HOLD_SECONDS="${HOLD_SECONDS:-60}"

# shellcheck source=scripts/me3-launch-lib.sh disable=SC1091
source "$REPO_ROOT/scripts/me3-launch-lib.sh"
# shellcheck source=scripts/steam-running.sh disable=SC1091
source "$REPO_ROOT/scripts/steam-running.sh"

CAP_SECONDS="$(python3 "$REPO_ROOT/scripts/runtime_timeout_cap.py")"
STAMP="$(date +%Y%m%d-%H%M%S)"
ART_DIR="$REPO_ROOT/target/runtime-probe/portrait-dll-standalone-$STAMP"
GAME_LOG="$GAME_DIR/er-loading-portrait-dll.log"
GAME_CRASH_LOG="$GAME_DIR/er-loading-portrait-dll-crash-log.txt"

fatal() { echo "portrait-dll-smoke: $*" >&2; exit 2; }

[[ -f "$DLL_SRC" ]] || fatal "missing DLL: $DLL_SRC (build: cargo xwin build --release --target x86_64-pc-windows-msvc -p er-loading-portrait-dll)"
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
cp -f "$DLL_SRC" "$ART_DIR/er_loading_portrait_dll.dll"
me3_write_profile "$ART_DIR/portrait-dll-standalone.me3" "$ART_DIR/er_loading_portrait_dll.dll"
rm -f "$GAME_LOG" "$GAME_CRASH_LOG"

echo "portrait-dll-smoke: launching me3 with ONLY er_loading_portrait_dll.dll (cap ${CAP_SECONDS}s, hold ${HOLD_SECONDS}s past first Present) -> $ART_DIR"
# Launch from GAME_DIR: me3 resolves its launcher payload from CWD-relative rust
# target dirs (bd me3-launch-cwd-must-lack-rust-target-dir), and the DLL writes its
# log into the game process CWD -- both need GAME_DIR, exactly like the probe scripts.
cd "$GAME_DIR"
me3_launch "$ART_DIR/portrait-dll-standalone.me3" >"$ART_DIR/me3-launch.out" 2>&1 &
ME3_PID=$!
echo "$ME3_PID" > "$ART_DIR/me3-launch.pid"

# Monitor + teardown + sweep, all in one python child so the bash parent stays the
# launch owner for the game's whole lifetime (me3's wine tree dies with its parent).
python3 - "$ME3_PID" "$GAME_LOG" "$GAME_CRASH_LOG" "$ART_DIR" "$CAP_SECONDS" "$HOLD_SECONDS" <<'PY'
import os, shutil, signal, sys, time

me3_pid = int(sys.argv[1])
game_log, crash_log, art_dir = sys.argv[2], sys.argv[3], sys.argv[4]
cap_seconds, hold_seconds = int(sys.argv[5]), int(sys.argv[6])

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

for src in (game_log, crash_log):
    if os.path.exists(src):
        shutil.copy2(src, os.path.join(art_dir, os.path.basename(src)))

print(f'portrait-dll-smoke: verdict={verdict} reason={reason} artifacts={art_dir}', flush=True)
sys.exit(0 if verdict == 'PASS' else 1)
PY
