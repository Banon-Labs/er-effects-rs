#!/usr/bin/env python3
"""Bounded vanilla (-v, no DLL) control run for the Windows-crash A/B.

Launches `~/Elden/launch.sh -v` (me3 offline, no er_quickload.dll), watches the
`eldenring.exe` process lifetime, then tears down ER + me3. Data artifacts go to
target/runtime-probe/ (repo-local /tmp writes for artifacts are allowed; only
source belongs in the repo).

Signal: vanilla ER should boot and sit at the title screen alive for the full window.
  - survived_to_cap -> clean boot, no early crash  => the 0x67141a crash is DLL-specific (ours)
  - exited_early    -> vanilla itself is unstable here (crash not ours / environment)
  - never_started   -> me3/launch failure (inconclusive)
"""
import json, os, signal, subprocess, sys, time, glob
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from er_artifact_env import artifact_env  # noqa: E402

# argv[1] = artifact label (subdir); argv[2:] = launch.sh flags (['-o'] = DLL profile without
# Seamless, ['-v'] = vanilla). Pass the flag explicitly: since 2026-08-24 launch.sh includes
# ersc.dll by default, so no flag now means a Seamless run -- not the DLL-only control arm.
LABEL = sys.argv[1] if len(sys.argv) > 1 else 'boot-ab'
LAUNCH_ARGS = sys.argv[2:]
# Derived, not hard-coded to one developer's home: this script used to carry `/home/banon` literals
# for the repo, the launcher and its cwd, so it only ran for one user on one machine.
REPO_ROOT = Path(__file__).resolve().parent.parent
LAUNCHER = Path(os.environ.get('ER_LAUNCHER', str(Path.home() / 'Elden/launch.sh')))
ART = str(REPO_ROOT / 'target/runtime-probe' / LABEL)
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
# Every per-run artifact into this probe's directory. A game-directory artifact is single-slot --
# `er_game_base::log::begin_fresh_run` keeps one generation -- so two launches lose the run before
# last, and several sessions launch concurrently here. The vanilla arm (`-v`) loads no DLL and
# writes none of these, but the same invocation takes `-o` for the DLL arm, which does.
proc = subprocess.Popen(
    [str(LAUNCHER), *LAUNCH_ARGS],
    stdout=log, stderr=subprocess.STDOUT,
    cwd=str(LAUNCHER.parent), start_new_session=True,
    env={**os.environ, **artifact_env(ART)},
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
