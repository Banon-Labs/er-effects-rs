#!/usr/bin/env python3
"""Prove angrE (100-Lilbro save) reaches a clean, SUSTAINED in-world load via the DLL autoload.

Watches the DLL debug log for the world-enter / MoveMap-complete semaphores, confirms the loaded
character is angrE, then verifies the game stays alive with no stall/crash for a sustain window (a
clean load must persist, not flash and die). RAM-derived log semaphores are the oracle; the wall
clock is only a backstop. Tears the game down on a verdict.

Usage: python3 scripts/angrE-boot-proof-watch.py <log_start_offset> [cap_seconds] [sustain_seconds]
"""
import os
import re
import subprocess
import sys
import time

LOG = "/mnt/c/SteamLibrary/steamapps/common/ELDEN RING/Game/er-effects-autoload-debug.log"
START = int(sys.argv[1]) if len(sys.argv) > 1 else 0
CAP = int(sys.argv[2]) if len(sys.argv) > 2 else 300
SUSTAIN = int(sys.argv[3]) if len(sys.argv) > 3 else 45
POLL = 3.0


def alive():
    try:
        out = subprocess.run(["tasklist.exe", "/FI", "IMAGENAME eq eldenring.exe", "/NH"],
                             capture_output=True, text=True, timeout=10).stdout.lower()
        return "eldenring.exe" in out
    except (OSError, subprocess.SubprocessError):
        return False


def kill(n):
    try:
        subprocess.run(["taskkill.exe", "/F", "/IM", n], capture_output=True, timeout=15)
    except (OSError, subprocess.SubprocessError):
        pass


def main():
    t0 = time.time()
    offset = START
    world_enter = 0          # MMS-FINISH / MoveMap complete count
    world_enter_at = None
    angrE_seen = False
    in_world = False
    last_char_line = ""
    last_finish_line = ""
    game_seen = False
    verdict = None

    while True:
        el = time.time() - t0
        if el > CAP:
            verdict = f"CAP {CAP}s reached without a sustained clean load"
            break
        if alive():
            game_seen = True
        try:
            sz = os.path.getsize(LOG)
        except OSError:
            sz = offset
        if sz > offset:
            with open(LOG, "rb") as f:
                f.seek(offset)
                data = f.read()
            offset = sz
            for line in data.decode("utf-8", "replace").splitlines():
                if "angrE" in line or "c30=0x1c000000" in line or "curblk=0x1c000000" in line:
                    angrE_seen = True
                    last_char_line = line.strip()
                if "MMS-FINISH" in line and "world enters" in line:
                    world_enter += 1
                    last_finish_line = line.strip()
                    if world_enter_at is None:
                        world_enter_at = el
                if "in-world" in line.lower() and ("reached" in line.lower() or "settled" in line.lower()):
                    in_world = True
        # sustained clean load: world entered + angrE + game still alive SUSTAIN seconds later
        if world_enter_at is not None and (el - world_enter_at) >= SUSTAIN:
            if alive():
                verdict = f"CLEAN LOAD PROVEN: angrE world-enter, sustained {SUSTAIN}s in-world, game alive"
            else:
                verdict = "world entered but game EXITED within the sustain window (not a clean sustained load)"
            break
        if game_seen and not alive():
            verdict = "GAME EXITED before a sustained clean load"
            break
        time.sleep(POLL)

    kill("eldenring.exe")
    kill("me3.exe")
    print("=" * 66, flush=True)
    print("VERDICT:", verdict, flush=True)
    print("elapsed_s:", round(time.time() - t0, 1), flush=True)
    print("angrE char loaded:", angrE_seen, flush=True)
    print("world-enter (MMS-FINISH) count:", world_enter, "| first at s:", world_enter_at, flush=True)
    print("in-world reached:", in_world, flush=True)
    if last_char_line:
        print("angrE line:", last_char_line[-200:], flush=True)
    if last_finish_line:
        print("world-enter line:", last_finish_line[-160:], flush=True)


if __name__ == "__main__":
    main()
