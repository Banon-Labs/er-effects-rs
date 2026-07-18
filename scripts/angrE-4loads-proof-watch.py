#!/usr/bin/env python3
"""Prove save slot angrE loads >=N (default 4) times IN A ROW via the DLL, no crash / no stall.

WHY v2 (the v1 MMS-FINISH oracle was WRONG): `MMS-FINISH` fires once per map-block MoveMap
completion, i.e. MANY times per single character load (a boot autoload emitted 10 within 3.4s),
so counting it declared a false "10 loads". A genuine controllable load is instead the game's own
RAM state: the player character becomes RENDER-READY / AVAILABLE and STAYS that way. This harness
polls the DLL's live telemetry.json (RAM oracles) and counts a load only when the player reaches a
sustained controllable state, with a return-to-title dip required between consecutive loads so one
settled load can never be miscounted as several.

GENUINE-LOAD oracle (per load, rising edge that persists):
  a load counts when oracle_player_render_ready (or player_available) has been True for >= PERSIST_S
  continuous seconds AND oracle_char_name == angrE. The NEXT load is only eligible after the player
  first drops out of the controllable state (render_ready & available both False) -- i.e. a real
  System->Quit / return-title dip. This makes each count a distinct load, not a sample.

CRASH disproof: a new `access-violation` line in the crash log, or the game process exiting, before
N loads -> FAIL (the switch-#4 GX command-queue overflow AV is exactly this).

STALL disproof: game alive, fewer than N loads, and NO new genuine load for STALL_S seconds while
also not currently sustaining a fresh controllable state -> FAIL. Lenient by default so user-paced
System->Quit->Continue think-time does not trip it.

This harness does NOT tear the game down except on a final verdict, and it prints incremental state
so the operator can see each load land. RAM oracles are the run-stopping evidence; the wall clock is
only a backstop. Screenshots are never consulted.

Usage: python3 scripts/angrE-4loads-proof-watch.py [target_loads=4] [cap_seconds=1200] \
           [persist_seconds=12] [stall_seconds=150] [sustain_seconds=20]
Env: ER_EFFECTS_TELEMETRY_PATH, ER_EFFECTS_CRASH_LOG_PATH (defaults under the Windows game dir).
"""
import json
import os
import subprocess
import sys
import time

GAME_DIR = "/mnt/c/SteamLibrary/steamapps/common/ELDEN RING/Game"
TEL = os.environ.get("ER_EFFECTS_TELEMETRY_PATH", os.path.join(GAME_DIR, "er-effects-telemetry.json"))
CRASH_LOG = os.environ.get("ER_EFFECTS_CRASH_LOG_PATH", os.path.join(GAME_DIR, "er-effects-crash-log.txt"))

TARGET = int(sys.argv[1]) if len(sys.argv) > 1 else 4
CAP = int(sys.argv[2]) if len(sys.argv) > 2 else 1200
PERSIST_S = int(sys.argv[3]) if len(sys.argv) > 3 else 12
STALL_S = int(sys.argv[4]) if len(sys.argv) > 4 else 150
SUSTAIN_S = int(sys.argv[5]) if len(sys.argv) > 5 else 20
POLL = 2.0
CHAR = "angrE"


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


def av_count():
    try:
        return open(CRASH_LOG, "rb").read().decode("utf-8", "replace").count("access-violation")
    except OSError:
        return 0


def read_tel():
    """Return (render_ready, available, char_name) from live telemetry, or (None,None,None)."""
    try:
        j = json.loads(open(TEL, "rb").read().decode("utf-8", "replace"))
    except (OSError, ValueError):
        return None, None, None
    return (bool(j.get("oracle_player_render_ready")),
            bool(j.get("player_available")),
            j.get("oracle_char_name"))


def controllable(rr, av, name):
    return (rr or av) and name == CHAR


def main():
    t0 = time.time()
    av_baseline = av_count()
    loads = 0
    load_times = []
    since_ctrl = None        # elapsed when current controllable spell began (None = not controllable)
    counted_this_spell = False
    eligible = True          # a load may be counted only after a return-to-title dip
    last_load_at = 0.0
    game_seen = False
    verdict = None
    last_state = None

    while True:
        el = time.time() - t0
        if el > CAP:
            verdict = f"CAP {CAP}s reached with {loads}/{TARGET} genuine loads (backstop)"
            break
        if alive():
            game_seen = True

        if av_count() > av_baseline:
            verdict = f"CRASH: new access-violation after {loads}/{TARGET} loads (GX-overflow signature suspected)"
            break
        if game_seen and not alive():
            verdict = f"GAME EXITED after {loads}/{TARGET} genuine loads (crash or quit-to-desktop)"
            break

        rr, av, name = read_tel()
        ctrl = controllable(rr, av, name)
        state = (rr, av, name, loads)
        if state != last_state:
            print(f"[{el:6.1f}s] render_ready={rr} available={av} char={name} loads={loads} "
                  f"eligible={eligible} ctrl_spell={'on' if since_ctrl else 'off'}", flush=True)
            last_state = state

        if ctrl:
            if since_ctrl is None:
                since_ctrl = el
                counted_this_spell = False
            # count once this spell has persisted and we're eligible (dip happened since last count)
            if (not counted_this_spell) and eligible and (el - since_ctrl) >= PERSIST_S:
                loads += 1
                load_times.append(round(el, 1))
                last_load_at = el
                counted_this_spell = True
                eligible = False
                print(f"[{el:6.1f}s] *** GENUINE LOAD #{loads} confirmed (angrE controllable, "
                      f"persisted {PERSIST_S}s) ***", flush=True)
        else:
            # dropped out of controllable -> a title/return dip -> next load becomes eligible
            if since_ctrl is not None:
                eligible = True
            since_ctrl = None

        # success: N genuine loads, then require the Nth to stay controllable SUSTAIN_S
        if loads >= TARGET:
            if since_ctrl is not None and (el - load_times[TARGET - 1]) >= SUSTAIN_S and ctrl:
                if alive() and av_count() <= av_baseline:
                    verdict = (f"PROVEN: {loads} genuine angrE loads in a row, no crash, no stall; "
                               f"final load sustained {SUSTAIN_S}s controllable")
                    break
            elif since_ctrl is None and (el - last_load_at) > SUSTAIN_S + 30:
                verdict = f"reached {loads} loads but final load did not stay controllable (dropped out)"
                break

        # stall: alive, under target, no new load for STALL_S, and not currently building one
        if (game_seen and loads < TARGET and el > 90 and (el - last_load_at) >= STALL_S
                and since_ctrl is None):
            verdict = f"STALL: no new genuine load for {STALL_S}s at {loads}/{TARGET} (game alive, not controllable)"
            break

        time.sleep(POLL)

    kill("eldenring.exe")
    kill("me3.exe")
    print("=" * 70, flush=True)
    print("VERDICT:", verdict, flush=True)
    print("elapsed_s:", round(time.time() - t0, 1), flush=True)
    print("target_loads:", TARGET, flush=True)
    print("genuine loads:", loads, "at (s):", load_times, flush=True)
    print("new access-violations:", max(0, av_count() - av_baseline), flush=True)
    sys.exit(0 if (verdict or "").startswith("PROVEN") else 1)


if __name__ == "__main__":
    main()
