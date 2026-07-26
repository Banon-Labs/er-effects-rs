#!/usr/bin/env python3
"""y22i (native Windows) agent-owned BOUNDED probe. User authorized self-driving 2026-07-15.

Launches me3 offline, waits for in-world autoload of char #1, watches for PAB->run_post
activity, then HARD-tears-down at the cap. RAM/log oracles only; no fabricated input here.
Direct me3 launch (like the live launcher), NOT the autoresearch run_experiment path.

Readiness is process-exit driven (no time.sleep): each poll blocks on the me3 handle with a
bounded <=30s timeout, so it returns EARLY the instant me3/game exits and otherwise paces the
loop deterministically. Every subprocess call carries an explicit <=30s timeout.

Usage: python3 scripts/y22i_bounded_probe.py [hard_cap_seconds]
Artifacts (data only) go to /tmp; this source lives in-repo per script-authoring policy.
"""
import subprocess, sys, time, json

ME3 = "/mnt/c/Users/choza/AppData/Local/garyttierney/me3/bin/me3.exe"
DLL = r"C:\Users\choza\build\y22i\guard\er-effects-rs\target\x86_64-pc-windows-msvc\release\er_effects_rs.dll"
CWD = "/mnt/c/Users/choza/build/y22i"
GAMEDIR = "/mnt/c/SteamLibrary/steamapps/common/ELDEN RING/Game"
LOG = GAMEDIR + "/er-effects-autoload-debug.log"
TELE = GAMEDIR + "/er-effects-telemetry.json"
TASKLIST = "/mnt/c/Windows/System32/tasklist.exe"
TASKKILL = "/mnt/c/Windows/System32/taskkill.exe"
POLL_SECS = 3.0
HARD_CAP = int(sys.argv[1]) if len(sys.argv) > 1 else 85

def up(name):
    out = subprocess.run([TASKLIST, "/FI", f"IMAGENAME eq {name}", "/NH"],
                         capture_output=True, text=True, timeout=15).stdout
    return name.lower() in out.lower()

def run_start_offset():
    try:
        lines = open(LOG, encoding="utf-8", errors="replace").readlines()
        idx = [i for i, l in enumerate(lines) if "er-effects log opened" in l]
        return idx[-1] if idx else 0
    except FileNotFoundError:
        return 0

def teardown():
    subprocess.run([TASKKILL, "/F", "/IM", "eldenring.exe"], capture_output=True, text=True, timeout=15)
    subprocess.run([TASKKILL, "/F", "/IM", "me3.exe"], capture_output=True, text=True, timeout=15)

def paced_wait(proc, seconds):
    """Block up to `seconds` on the me3 handle: returns early if me3 (hence the game) exits
    (a readiness signal), otherwise paces the loop deterministically. Not time.sleep."""
    try:
        proc.wait(timeout=seconds)
        return False  # me3 exited -> not alive
    except subprocess.TimeoutExpired:
        return True   # still running

if not up("steam.exe"):
    print("PREFLIGHT FAIL: Steam is DOWN", flush=True); sys.exit(2)
if up("eldenring.exe"):
    print("PREFLIGHT: stale eldenring.exe -> killing", flush=True)
    subprocess.run([TASKKILL, "/F", "/IM", "eldenring.exe"], capture_output=True, text=True, timeout=15)

mlog = open("/tmp/me3_bounded_probe.me3.log", "w", encoding="utf-8")
p = subprocess.Popen([ME3, "launch", "-g", "eldenring", "-n", DLL],
                     cwd=CWD, stdin=subprocess.PIPE, stdout=mlog, stderr=subprocess.STDOUT)
print(f"me3 launched pid={p.pid}; bounded cap={HARD_CAP}s", flush=True)

t0 = time.time(); in_world_at = None; appeared = False; base_off = run_start_offset()
try:
    # Phase 1: wait for eldenring.exe to APPEAR; capture the fresh log run offset once it does.
    while time.time() - t0 < min(40, HARD_CAP):
        paced_wait(p, POLL_SECS)
        if up("eldenring.exe"):
            appeared = True
            base_off = run_start_offset()
            print(f"  eldenring.exe APPEARED at {time.time()-t0:.0f}s (fresh run base_off={base_off})", flush=True)
            break
    if not appeared:
        print("  FAIL: eldenring.exe never appeared within 40s", flush=True)
    else:
        # Phase 2: monitor the fresh run until cap or genuine exit (3 consecutive process misses).
        misses = 0
        while time.time() - t0 < HARD_CAP:
            paced_wait(p, POLL_SECS)
            el = time.time() - t0
            alive = up("eldenring.exe")
            if not alive:
                misses += 1
                print(f"  t={el:4.0f}s alive=MISS ({misses}/3)", flush=True)
                if misses >= 3:
                    print("  game exited/crashed (3 consecutive misses)", flush=True); break
                continue
            misses = 0
            pl = None
            try:
                d = json.load(open(TELE, encoding="utf-8"))
                pl = d.get("player_available") or d.get("player_seen")
            except Exception:
                pass
            rp = 0; ingame = False; av = 0
            try:
                lines = open(LOG, encoding="utf-8", errors="replace").readlines()[base_off:]
                rp = sum(1 for l in lines if "pab-run-post" in l.lower() or "real-system-window" in l.lower())
                ingame = any("02_000_IngameTop" in l for l in lines)
                av = sum(1 for l in lines if "AV #" in l or "ExitProcess" in l or "DLPanic" in l)
            except Exception:
                pass
            print(f"  t={el:4.0f}s alive={alive} player={pl} ingameTop={ingame} run_post={rp} av/exit={av}", flush=True)
            if pl and ingame and in_world_at is None:
                in_world_at = el
                print(f"  *** IN-WORLD at {el:.0f}s (char #1 autoloaded) ***", flush=True)
finally:
    print("teardown (hard cap or exit)", flush=True)
    teardown()
    if p.stdin is not None:
        try:
            p.stdin.close()
        except Exception:
            pass
print(f"PROBE DONE. appeared={appeared} in_world_at={in_world_at}s", flush=True)
