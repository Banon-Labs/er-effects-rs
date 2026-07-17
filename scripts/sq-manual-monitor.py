#!/usr/bin/env python3
"""Non-launching ATTACH monitor for a MANUAL (user-driven) me3 run.

The game is already up (launched by scripts/me3_live_launch.py); the USER drives System->Quit->
Load-Profile with a real controller -- NO self-drive, NO fabricated input, NO markers armed here.
This monitor only WATCHES the DLL debug log and tears down on a terminal RAM semaphore:

  * WORLD RES WAIT stall (THE teardown semaphore): a 0x1c block-load whose phase stays below the
    ready value 0x0a(10) with stable_frames==0 and NO phase progress for STALL_SECONDS. That is the
    exact bug -- when the user hits it they are done, so we tear the game down for offline analysis.
  * WORLD READY: stable_frames >= LOADED_STABLE_FRAMES (the second load reached the same world-ready
    state as the first autoload) -- success, tear down.

Other transient stalls are NOT teardown cases right now; only the two above tear down. Teardown =
kill eldenring.exe + me3.exe (which releases me3_live_launch's hold). Deterministic readiness via
inotify on the game dir (repo no-sleep policy); the wall-clock CAP is only a backstop.

Usage: python3 scripts/sq-manual-monitor.py <log_start_offset> [cap_seconds]
"""
import ctypes
import os
import re
import select
import subprocess
import sys
import time

GAMEDIR = "/mnt/c/SteamLibrary/steamapps/common/ELDEN RING/Game"
LOG = os.path.join(GAMEDIR, "er-effects-autoload-debug.log")

START_OFFSET = int(sys.argv[1]) if len(sys.argv) > 1 else 0
CAP_SECONDS = int(sys.argv[2]) if len(sys.argv) > 2 else 600
POLL = 1.0
STALL_SECONDS = 1.0            # no world-load phase progress this long (armed) => THE stall (tear down fast)
LOADED_STABLE_FRAMES = 300     # world entered and held
READY_PHASE = 10               # loadstate +0x35 == 0x0a

_IN_MODIFY = 0x00000002
_IN_CREATE = 0x00000100
_IN_MOVED_TO = 0x00000080
_IN_CLOSE_WRITE = 0x00000008
_inotify_fd = -1
try:
    _libc = ctypes.CDLL("libc.so.6", use_errno=True)
    _inotify_fd = _libc.inotify_init1(0)
    if _inotify_fd >= 0:
        _libc.inotify_add_watch(
            _inotify_fd, GAMEDIR.encode("utf-8"),
            _IN_MODIFY | _IN_CREATE | _IN_MOVED_TO | _IN_CLOSE_WRITE,
        )
except OSError:
    _inotify_fd = -1


def wait_for_change(timeout):
    watch = [_inotify_fd] if _inotify_fd >= 0 else []
    ready, _, _ = select.select(watch, [], [], timeout)
    if ready:
        try:
            os.read(_inotify_fd, 65536)
        except OSError:
            pass


def game_alive():
    try:
        out = subprocess.run(
            ["tasklist.exe", "/FI", "IMAGENAME eq eldenring.exe", "/NH"],
            capture_output=True, text=True, timeout=10,
        ).stdout.lower()
        return "eldenring.exe" in out
    except (OSError, subprocess.SubprocessError):
        return False


def kill(name):
    try:
        subprocess.run(["taskkill.exe", "/F", "/IM", name], capture_output=True, timeout=15)
    except (OSError, subprocess.SubprocessError):
        pass


def main():
    print(f"attach-monitor: watching {LOG} from offset {START_OFFSET}; teardown on WORLD RES WAIT "
          f"stall (phase<{READY_PHASE}, stable=0, no progress {STALL_SECONDS}s) or LOADED_STABLE "
          f">= {LOADED_STABLE_FRAMES}. Drive the game now.", flush=True)

    t0 = time.time()
    offset = START_OFFSET
    phase_hwm = -1          # high-water of the 0x1c block load phase for the CURRENT load
    stable_max = 0
    armed = False           # a 0x1c WORLD RES WAIT is actively being waited on
    last_progress = time.time()
    last_report = ""
    verdict = None

    while True:
        now = time.time()
        if now - t0 > CAP_SECONDS:
            verdict = f"CAP {CAP_SECONDS}s reached (no stall, no world-ready)"
            break
        if not game_alive() and now - t0 > 20:
            verdict = "GAME CLOSED by user (no teardown needed)"
            break
        try:
            sz = os.path.getsize(LOG)
        except OSError:
            sz = offset
        if sz > offset:
            with open(LOG, "rb") as f:
                f.seek(offset)
                chunk = f.read()
            offset = sz
            for line in chunk.decode("utf-8", "replace").splitlines():
                # WORLDRES-GETTER 0x1c: authoritative per-block phase/status/data for the 0x1c load.
                if "WORLDRES-GETTER" in line and "0x1c" in line:
                    mp = re.search(r"\+0x35\(phase\)=(-?\d+)", line)
                    if mp:
                        ph = int(mp.group(1))
                        # New load starts when phase drops well below the high-water -> reset + re-arm.
                        if phase_hwm >= 0 and ph < phase_hwm - 3:
                            phase_hwm = -1
                            armed = False
                        if ph > phase_hwm:
                            phase_hwm = ph
                            last_progress = now
                        armed = True
                        last_report = line.split("dll:", 1)[-1].strip()[:150]
                # SWITCH-ORACLE carries stable_frames (world entered + held).
                if "SWITCH-ORACLE" in line or "LAST ORACLE" in line:
                    st = re.search(r"stable_frames=(\d+)", line)
                    if st:
                        v = int(st.group(1))
                        if v > stable_max:
                            stable_max = v
                            last_progress = now
        if stable_max >= LOADED_STABLE_FRAMES:
            verdict = f"WORLD READY (LOADED_STABLE stable_frames={stable_max}) -- second load reached world readiness"
            break
        if (armed and stable_max == 0 and 0 <= phase_hwm < READY_PHASE
                and now - last_progress >= STALL_SECONDS):
            verdict = (f"WORLD RES WAIT STALL (teardown semaphore): 0x1c block phase stuck at "
                       f"{phase_hwm} (< {READY_PHASE}), stable=0, no progress for {STALL_SECONDS}s "
                       f"-- last: {last_report}")
            break
        wait_for_change(POLL)

    print(f"VERDICT: {verdict}", flush=True)
    print(f"elapsed_s: {round(time.time() - t0, 1)} phase_hwm={phase_hwm} stable_max={stable_max}",
          flush=True)
    # Tear down on a real terminal semaphore; leave the game alone if the user already closed it.
    if verdict and verdict.startswith(("WORLD RES WAIT STALL", "WORLD READY", "CAP")):
        if game_alive():
            kill("eldenring.exe")
            kill("me3.exe")
            print("torn down (eldenring.exe + me3.exe killed)", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
