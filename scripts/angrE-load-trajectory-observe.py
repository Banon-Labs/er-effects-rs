#!/usr/bin/env python3
"""Observe the boot-autoload trajectory of angrE: telemetry render/available, present-freeze,
mms_step, return-title-chain waits, and log growth. Pure observation, no teardown.

Usage: python3 scripts/angrE-load-trajectory-observe.py [cap_seconds=200]

Which files this reads, and why that is not a constant any more
--------------------------------------------------------------
Every launcher in this repo now redirects the DLL's per-run artifacts out of the game directory
(`ER_QUICKLOAD_TELEMETRY_PATH`, `ER_QUICKLOAD_AUTOLOAD_DEBUG_PATH`, ...), because a game-directory
log is single-SLOT: the DLL rotates `<name>` to `<name>.prev` on its first write, so the next launch
destroys the run before last. So the game directory is now the FALLBACK, not the source of truth --
point this observer at the run's artifact dir with `ER_ARTIFACT_DIR=<dir>` (or set the two
`ER_QUICKLOAD_*_PATH` variables the launcher used), and it reads that run instead of whatever
happens to be lying next to the executable.

`ER_GAME_DIR` / `ME3_STEAM_DIR` override the fallback. The literal that used to sit here,
`/mnt/c/SteamLibrary/...`, belonged to the retired WSL2 setup and resolves to nothing on this
machine -- which reads as "the run wrote no telemetry" rather than "you looked in the wrong place".
"""
import json, os, re, subprocess, sys, threading, time


def _game_dir() -> str:
    explicit = os.environ.get("ER_GAME_DIR")
    if explicit:
        return explicit
    steam = os.environ.get(
        "ME3_STEAM_DIR", os.path.join(os.path.expanduser("~"), ".local/share/Steam")
    )
    return os.path.join(steam, "steamapps/common/ELDEN RING/Game")


def _artifact(name: str, env_var: str) -> str:
    """The run's copy of `name`: the explicit path, else ER_ARTIFACT_DIR, else the game dir."""
    explicit = os.environ.get(env_var)
    if explicit:
        return explicit
    artifact_dir = os.environ.get("ER_ARTIFACT_DIR")
    if artifact_dir:
        return os.path.join(artifact_dir, name)
    return os.path.join(GD, name)


GD = _game_dir()
TEL = _artifact("er-quickload-telemetry.json", "ER_QUICKLOAD_TELEMETRY_PATH")
LOG = _artifact("er-quickload-autoload-debug.log", "ER_QUICKLOAD_AUTOLOAD_DEBUG_PATH")
CAP = int(sys.argv[1]) if len(sys.argv) > 1 else 200



def bounded_poll_wait(seconds: float) -> None:
    """Bounded loop pacing; loop predicates still own readiness/stop decisions."""
    threading.Event().wait(min(max(float(seconds), 0.0), 30.0))

def alive():
    """True while an `eldenring.exe` is running, read straight out of /proc.

    Not `tasklist.exe` (this machine is a native Linux Steam install; that binary does not exist
    here, so the old form returned False forever and the observer stopped on its first poll) and
    not `pgrep`, which this repo's guard blocks and which false-negatives on the Proton stack.
    Mirrors `scripts/er_run_lib.py::find_game_pids`.
    """
    for entry in os.scandir("/proc"):
        if not entry.name.isdigit():
            continue
        try:
            with open(f"/proc/{entry.name}/comm", "rb") as handle:
                if handle.read().strip().lower() == b"eldenring.exe":
                    return True
            with open(f"/proc/{entry.name}/cmdline", "rb") as handle:
                if b"eldenring.exe" in handle.read().lower():
                    return True
        except OSError:
            continue
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
        bounded_poll_wait(4)
    print(f"SUMMARY: render_ready_first_true_at={rr_true_at} present_froze_at={froze_at}", flush=True)


if __name__ == "__main__":
    main()
