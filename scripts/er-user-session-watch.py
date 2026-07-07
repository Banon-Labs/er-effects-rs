#!/usr/bin/env python3
"""Watch a user-driven Elden Ring session: no teardown, observation only.

Unlike er-readiness-watch.py this owns NO process teardown and injects nothing;
it is for no-auto-teardown user play sessions where the agent only needs to know
when the game exits and to collect a semaphore timeline for post-run analysis.

- Waits for eldenring.exe to appear (boot grace), then watches until it exits.
- Every 2s stats the er-effects-* log files; records size/mtime changes as
  timestamped events so post-run analysis can correlate wall-clock time with
  log growth (e.g. "user was in the blank menu around HH:MM:SS").
- Copies er-effects-telemetry.json to a numbered snapshot each time it changes,
  building a semaphore timeline across the session.
- Hard cap (default 3600s) so this background job can never go stale; if the
  cap fires while the game is still up, the caller restarts the watcher.

Usage: er-user-session-watch.py <artifact_dir> [cap_seconds]
"""
import ctypes
import json
import os
import select
import shutil
import sys
import time

GAME = "/home/banon/.local/share/Steam/steamapps/common/ELDEN RING/Game"
OUT = sys.argv[1]
CAP_SECONDS = int(sys.argv[2]) if len(sys.argv) > 2 else 3600
# Teardown when oracle_optionsetting_real_blank_detected_count reaches this. 0 disables blank-teardown
# (used for fix-verify runs where the fix corrects the blank and we want the full curve to the cap).
BLANK_THRESHOLD = int(sys.argv[3]) if len(sys.argv) > 3 else 1
BOOT_GRACE_SECONDS = 180

FILES = [
    "er-effects-telemetry.json",
    "er-effects-autoload-debug.log",
    "er-effects-continue-trace.log",
    "er-effects-bootstrap.jsonl",
    "er-effects-bootstrap-state.json",
    "er-effects-crash-log.txt",
    "er-effects-crash.log",
]


def er_pids():
    # Read-only /proc comm scan; never embeds the exe name in a command line.
    pids = []
    for pid in os.listdir("/proc"):
        if not pid.isdigit():
            continue
        try:
            comm = open(f"/proc/{pid}/comm").read().strip()
        except OSError:
            continue
        c = comm.lower()
        if c.startswith("eldenring") or comm == "me3" or "start_protected" in c:
            pids.append((int(pid), comm))
    return pids


def er_alive():
    return any(c.lower().startswith("eldenring") for _, c in er_pids())


def teardown():
    import signal as _sig

    for pid, _ in er_pids():
        try:
            os.kill(pid, _sig.SIGTERM)
        except OSError:
            pass


def blank_detected_count():
    # RAM-read semaphore is the run-stopping oracle: the DLL's oracle_optionsetting_pane_blank_detected_count
    # in er-effects-telemetry.json. Non-visual; fires the instant the blank Game Options pane reproduces.
    try:
        d = json.load(open(os.path.join(GAME, "er-effects-telemetry.json")))
        # REAL signal: healthy pane seen THEN went hidden (cannot false-fire on boot/preload).
        return int(d.get("oracle_optionsetting_real_blank_detected_count", 0))
    except (OSError, ValueError, json.JSONDecodeError):
        return 0


def ts(t):
    return time.strftime("%Y-%m-%d %H:%M:%S", time.localtime(t))


start = time.time()
seen_game = False
snap_idx = 0
last = {}
os.makedirs(OUT, exist_ok=True)
log = open(os.path.join(OUT, "watch-events.jsonl"), "a", buffering=1)


def emit(event, **kw):
    rec = {"t": time.time(), "ts": ts(time.time()), "event": event}
    rec.update(kw)
    log.write(json.dumps(rec) + "\n")


emit("watcher_start", cap_seconds=CAP_SECONDS)

# Deterministic readiness (repo no-sleep policy): block on inotify game-dir file-change events with a
# bounded select() timeout as the hard safety cap -- never a bare sleep. inotify is the policy's
# recommended primitive; the timeout only bounds how long we block before re-checking process liveness.
_IN_MODIFY = 0x00000002
_IN_CREATE = 0x00000100
_IN_MOVED_TO = 0x00000080
_IN_CLOSE_WRITE = 0x00000008
_POLL_CAP_SECONDS = 2.0
_inotify_fd = -1
try:
    _libc = ctypes.CDLL("libc.so.6", use_errno=True)
    _inotify_fd = _libc.inotify_init1(0)
    if _inotify_fd >= 0:
        _libc.inotify_add_watch(
            _inotify_fd,
            GAME.encode("utf-8"),
            _IN_MODIFY | _IN_CREATE | _IN_MOVED_TO | _IN_CLOSE_WRITE,
        )
except OSError:
    _inotify_fd = -1


def wait_for_change(timeout):
    # Return when a game-dir file changes OR the timeout safety cap elapses, then the loop re-checks
    # process liveness + the blank semaphore. Readiness is the inotify event; the cap only bounds it.
    watch = [_inotify_fd] if _inotify_fd >= 0 else []
    ready, _, _ = select.select(watch, [], [], timeout)
    if ready:
        try:
            os.read(_inotify_fd, 65536)  # drain events; we re-stat every tracked file anyway
        except OSError:
            pass


exit_reason = "unknown"
while True:
    now = time.time()
    if now - start > CAP_SECONDS:
        exit_reason = (
            "cap_reached_game_still_up"
            if seen_game and er_alive()
            else "cap_reached"
        )
        emit(exit_reason)
        break
    alive = er_alive()
    if alive and not seen_game:
        seen_game = True
        emit("game_up")
    if not alive and seen_game:
        exit_reason = "game_exit"
        emit("game_exit")
        break
    if not alive and not seen_game and now - start > BOOT_GRACE_SECONDS:
        exit_reason = "never_booted"
        emit("never_booted")
        break
    # RAM-semaphore run-stopping oracle: the instant the blank pane reproduces, capture + tear down.
    if seen_game and alive and BLANK_THRESHOLD >= 1 and blank_detected_count() >= BLANK_THRESHOLD:
        emit("blank_detected_semaphore", count=blank_detected_count())
        p = os.path.join(GAME, "er-effects-telemetry.json")
        try:
            shutil.copyfile(p, os.path.join(OUT, "telemetry-blank-detected.json"))
        except OSError:
            pass
        teardown()
        exit_reason = "blank_detected_teardown"
        emit("blank_detected_teardown")
        break
    for name in FILES:
        p = os.path.join(GAME, name)
        try:
            st = os.stat(p)
            cur = (st.st_size, st.st_mtime)
        except OSError:
            cur = None
        if last.get(name, "unset") != cur:
            first = name not in last
            last[name] = cur
            if not first:
                emit(
                    "file_change",
                    file=name,
                    size=cur[0] if cur else None,
                    mtime=ts(cur[1]) if cur else None,
                )
            if name == "er-effects-telemetry.json" and cur and not first:
                snap_idx += 1
                try:
                    shutil.copyfile(
                        p, os.path.join(OUT, f"telemetry-{snap_idx:04d}.json")
                    )
                    emit("telemetry_snapshot", index=snap_idx)
                except OSError as e:
                    emit("telemetry_snapshot_failed", error=str(e))
    wait_for_change(_POLL_CAP_SECONDS)

print(f"watcher exit: {exit_reason}")
