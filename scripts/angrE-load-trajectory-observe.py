#!/usr/bin/env python3
"""Observe the boot-autoload trajectory of angrE: telemetry render/available, present-freeze,
mms_step, return-title-chain waits, and log growth. Pure observation, no teardown.

Usage: python3 scripts/angrE-load-trajectory-observe.py [cap_seconds=200]
Env: ER_EFFECTS_TELEMETRY_PATH, ER_EFFECTS_AUTOLOAD_DEBUG_PATH (default: Windows game dir).
"""
import json, os, re, subprocess, sys, time

GD = "/mnt/c/SteamLibrary/steamapps/common/ELDEN RING/Game"
TEL = os.environ.get("ER_EFFECTS_TELEMETRY_PATH", os.path.join(GD, "er-effects-telemetry.json"))
LOG = os.environ.get("ER_EFFECTS_AUTOLOAD_DEBUG_PATH", os.path.join(GD, "er-effects-autoload-debug.log"))
CAP = int(sys.argv[1]) if len(sys.argv) > 1 else 200


def alive():
    try:
        return "eldenring.exe" in subprocess.run(
            ["tasklist.exe", "/FI", "IMAGENAME eq eldenring.exe", "/NH"],
            capture_output=True, text=True, timeout=10).stdout.lower()
    except Exception:
        return False


def tel():
    try:
        j = json.loads(open(TEL, "rb").read().decode("utf-8", "replace"))
        return (j.get("oracle_player_render_ready"), j.get("player_available"),
                j.get("oracle_player_present"), j.get("oracle_char_name"),
                j.get("oracle_present_hook_hits"))
    except Exception:
        return (None,) * 5


def logtail_state():
    try:
        d = open(LOG, "rb").read()[-8000:].decode("utf-8", "replace")
    except Exception:
        return (None, None)
    mms = None
    for m in re.finditer(r'mms_step=(\d+)\(', d):
        mms = int(m.group(1))
    waits = None
    for m in re.finditer(r'return-title chain WAIT[^\n]*waits=(\d+)', d):
        waits = int(m.group(1))
    return (mms, waits)


def main():
    t0 = time.time()
    last_sz = 0
    last_present = None
    rr_true_at = None
    froze_at = None
    print(f"{'t':>6} {'alive':>5} {'rr':>5} {'avail':>6} {'pres':>5} {'char':>7} "
          f"{'present':>8} {'mms':>4} {'waits':>7} {'logdelta':>9}", flush=True)
    while True:
        el = time.time() - t0
        if el > CAP:
            print("== CAP ==", flush=True)
            break
        a = alive()
        rr, av, pres, name, ph = tel()
        mms, waits = logtail_state()
        try:
            sz = os.path.getsize(LOG)
        except Exception:
            sz = last_sz
        delta = sz - last_sz
        last_sz = sz
        if rr and rr_true_at is None:
            rr_true_at = el
        if ph is not None and ph == last_present and delta == 0 and a and froze_at is None and el > 60:
            froze_at = el
        last_present = ph
        print(f"{el:6.1f} {str(a):>5} {str(rr):>5} {str(av):>6} {str(pres):>5} {str(name):>7} "
              f"{str(ph):>8} {str(mms):>4} {str(waits):>7} {delta:>9}", flush=True)
        if not a and el > 30:
            print("== GAME EXITED ==", flush=True)
            break
        time.sleep(4)
    print(f"SUMMARY: render_ready_first_true_at={rr_true_at} present_froze_at={froze_at}", flush=True)


if __name__ == "__main__":
    main()
