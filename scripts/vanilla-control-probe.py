#!/usr/bin/env python3
"""Bounded vanilla (-v, no DLL) control run for the Windows-crash A/B.

Launches `~/Elden/launch.sh -v` (me3 offline, NO er_effects_rs.dll), watches the
`eldenring.exe` process lifetime, then tears down ER + me3. Data artifacts go to
target/runtime-probe/ (repo-local /tmp writes for artifacts are allowed; only
source belongs in the repo).

Signal: vanilla ER should boot and SIT at the title screen alive for the full window.
  - survived_to_cap -> clean boot, no early crash  => the 0x67141a crash is DLL-specific (ours)
  - exited_early    -> vanilla itself is unstable here (crash not ours / environment)
  - never_started   -> me3/launch failure (inconclusive)
"""
import json, os, signal, subprocess, sys, time, glob

# argv[1] = artifact label (subdir); argv[2:] = launch.sh flags ([] = DLL profile, ['-v'] = vanilla).
LABEL = sys.argv[1] if len(sys.argv) > 1 else 'boot-ab'
LAUNCH_ARGS = sys.argv[2:]
ART = f"/home/banon/projects/er-effects-rs/target/runtime-probe/{LABEL}"
os.makedirs(ART, exist_ok=True)
GAME_RUNTIME_CAP = 60   # seconds to observe the game process once it appears
LAUNCH_GRACE = 45       # seconds to allow me3 setup + logos before expecting the process


def _pids_by_comm(names):
    out = []
    for p in glob.glob('/proc/[0-9]*'):
        try:
            if open(p + '/comm').read().strip() in names:
                out.append(int(p.split('/')[-1]))
        except Exception:
            pass
    return out


def er_pids():
    return _pids_by_comm({'eldenring.exe'})


def me3_pids():
    return _pids_by_comm({'me3', 'me3-launcher.', 'me3_mod_host', 'launch.sh'})


start = time.monotonic()
log = open(ART + '/vanilla-launch.out', 'w')
proc = subprocess.Popen(
    ['/home/banon/Elden/launch.sh', *LAUNCH_ARGS],
    stdout=log, stderr=subprocess.STDOUT,
    cwd='/home/banon/Elden', start_new_session=True,
)

first_seen = last_seen = None
poll = []
while True:
    t = time.monotonic() - start
    pids = er_pids()
    if pids:
        if first_seen is None:
            first_seen = t
        last_seen = t
    poll.append({'t': round(t, 1), 'er_alive': bool(pids), 'n': len(pids)})
    if first_seen is not None and not pids and (t - last_seen) > 3:
        outcome = 'exited_early'
        break
    if first_seen is not None and (t - first_seen) >= GAME_RUNTIME_CAP:
        outcome = 'survived_to_cap'
        break
    if first_seen is None and t >= LAUNCH_GRACE:
        outcome = 'never_started'
        break
    time.sleep(1.5)


def kill(pids, sig):
    for pid in pids:
        try:
            os.kill(pid, sig)
        except Exception:
            pass


kill(er_pids(), signal.SIGTERM)
kill(me3_pids(), signal.SIGTERM)
time.sleep(2)
kill(er_pids(), signal.SIGKILL)
kill(me3_pids(), signal.SIGKILL)
try:
    proc.terminate()
except Exception:
    pass

result = {
    'outcome': outcome,
    'first_seen_s': first_seen,
    'last_seen_s': last_seen,
    'observed_s': round(time.monotonic() - start, 1),
    'game_runtime_cap_s': GAME_RUNTIME_CAP,
    'er_alive_after_teardown': bool(er_pids()),
    'polls': poll,
}
json.dump(result, open(ART + '/control-result.json', 'w'), indent=2)
print(json.dumps({k: v for k, v in result.items() if k != 'polls'}))
