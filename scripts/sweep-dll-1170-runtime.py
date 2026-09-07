#!/usr/bin/env python3
"""Launch each cdylib alone on ELDEN RING 1.17 and record what it does, one DLL per run.

Why one DLL at a time
---------------------
On 2026-08-29 the whole mod set was failing and it was being discussed as "the mods break 1.17".
It is not one failure. Nine single-DLL launches found three different ones, and the crates that
boot cleanly outnumber the ones that do not:

    er_quickload          0xc0000005 execute at a CS::CSFadeImp object, +1.7s
    er_loading_portrait   vanishes ~4s after its first Present, leaving no crash record
    er_armament_icons     0xc000001d ILLEGAL_INSTRUCTION at game+0x32ee2b5

Read the signature, not the LIVENESS. Scoring rows live/dead would have convicted er-gfx (shared
by two dying shells -- but they die differently) and the Present hook (both Present-hooking shells
died -- until er_loading_bar, which hooks Present and boots, killed that theory). A bisect on
liveness alone gets the wrong crate; the crash-log signature is what discriminates.

What it records
    boots        the thread-group leader is still alive at the end of the watch window
    dies:<sig>   the leader went `Z` or the process vanished; `<sig>` is the first
                 access-violation / exception line its own crash log carried, if any
    no-launch    the game never came up at all (a launcher or environment problem, not a DLL one)

Results merge into `docs/recon/dll-1170-runtime-results.json`, which
`scripts/audit-1170-readiness.py` prints as its `runtime` column. That file is the only source of
runtime truth: a DLL with zero ungated addresses is not thereby working, and nothing infers one
from the other.

Do not rebuild while this runs
------------------------------
Measured 2026-08-29, by doing it: a sweep was started, DLLs were rebuilt 10 minutes into it, and
the run silently became a mix -- the first eight DLLs tested one build and the rest another. A
sweep that cannot say which build it measured is not evidence, and 18 verdicts had to be thrown
away. So the sweep now fingerprints every DLL at the start and ABORTS the moment one changes
underneath it, rather than producing a result that looks fine and is not.

Every run gets its own artifact directory
-----------------------------------------
er-artifact-redirect: this launches the game once per cdylib through `~/Elden/launch.sh`, and the
audit's shell-name detector cannot see that -- the DLL under test is resolved from cargo metadata
at runtime, so no `er_quickload.dll` literal appears anywhere in this file. This marker opts the
sweep in explicitly rather than leaving it invisible, which reads exactly like a clean tree.

It matters more here than anywhere else in the repo: `--all` is twenty-plus launches back to back.
A game-directory artifact is single-slot (`er_game_base::log::begin_fresh_run` keeps one `.prev`),
so an unredirected sweep destroys not only every other session's evidence but its own -- by the
time it finishes, the only crash log left describes the last DLL it tested. Each run therefore gets
`target/runtime-probe/sweep-1170/<crate>-<stamp>/`, and `crash_signature` reads that rather than
the game directory.

SAFETY / house rules
  * launches only the approved `~/Elden/launch.sh` path -- never Steam, never the EAC launcher;
  * one game at a time, torn down between runs via `scripts/er-teardown.py`;
  * profiles are written to `~/Elden/sweep-<dll>.me3` and name only the one DLL under test;
  * nothing in the game directory is ever deleted -- it is somebody else's run.

Usage
    python3 scripts/sweep-dll-1170-runtime.py --list
    python3 scripts/sweep-dll-1170-runtime.py --dll er-net-effects
    python3 scripts/sweep-dll-1170-runtime.py --all --watch-seconds 40
    python3 scripts/sweep-dll-1170-runtime.py --selftest
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
# One table, not a copy per launcher. `scripts/er_artifact_env.py` is the single place a launcher
# learns where this run's artifacts go; the audit requires it to cover every knob the DLLs honour,
# so a new artifact cannot quietly stay unredirected in here.
from er_artifact_env import (  # noqa: E402 - path set above
    ARTIFACT_ENV,
    artifact_env,
    artifact_source_dirs,
)

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RESULTS = os.path.join(REPO, "docs", "recon", "dll-1170-runtime-results.json")
DLL_DIR = os.path.join(REPO, "target", "x86_64-pc-windows-msvc", "release")
PROFILE_DIR = os.path.join(os.path.expanduser("~"), "Elden")
LAUNCHER = os.path.join(PROFILE_DIR, "launch.sh")
TEARDOWN = os.path.join(REPO, "scripts", "er-teardown.py")
GAME_DIR = os.path.join(
    os.path.expanduser("~"),
    ".local/share/Steam/steamapps/common/ELDEN RING/Game",
)
# Long enough to clear every failure this sweep has seen: the earliest is +1.7s and the latest
# ~+38s. A DLL still alive after this is reported as booting, not as proven healthy.
DEFAULT_WATCH_SECONDS = 45.0
# How long to wait for the game process to appear before calling it a launcher problem.
LAUNCH_TIMEOUT_SECONDS = 25.0
POLL_SECONDS = 2.0

PROFILE_TEMPLATE = """# GENERATED by scripts/sweep-dll-1170-runtime.py -- one DLL, to see what IT does on 1.17.
# Regenerated on every sweep; edit the script, not this file.
profileVersion = "v1"
start_online = false

[[supports]]
game = "eldenring"

[[natives]]
path = '{dll}'
"""


# Crate name is not the DLL name for every shell: `er-ags-stub` builds `amd_ags_x64.dll`, because
# it has to be named what the game looks for. Assuming `crate.replace("-", "_")` reported it as
# `not-built` and silently skipped it. The `[lib] name` in cargo metadata is the authority.
_LIB_NAMES: dict[str, str] = {}


def dll_path(crate: str) -> str:
    if not _LIB_NAMES:
        try:
            meta = json.loads(
                subprocess.run(
                    ["cargo", "metadata", "--no-deps", "--format-version", "1"],
                    capture_output=True, text=True, cwd=REPO, check=True, timeout=30,
                ).stdout
            )
            for package in meta["packages"]:
                for target in package["targets"]:
                    if "cdylib" in target["kind"]:
                        _LIB_NAMES[package["name"]] = target["name"]
        except (OSError, ValueError, subprocess.SubprocessError):
            pass
    name = _LIB_NAMES.get(crate, crate.replace("-", "_"))
    return os.path.join(DLL_DIR, name + ".dll")


def game_pid() -> int | None:
    """The running `eldenring.exe`, by comm -- deliberately name-only.

    The prefix and appid rules in `er-teardown.py` both go false under the Steam Linux Runtime,
    whose bwrap container puts the exe and cwd somewhere neither matches.
    """
    for entry in os.listdir("/proc"):
        if not entry.isdigit():
            continue
        try:
            with open(f"/proc/{entry}/comm", encoding="utf-8") as handle:
                if handle.read().strip() == "eldenring.exe":
                    return int(entry)
        except OSError:
            continue
    return None


def leader_state(pid: int) -> str | None:
    try:
        with open(f"/proc/{pid}/stat", encoding="utf-8") as handle:
            return handle.read().split()[2]
    except (OSError, IndexError):
        return None


def teardown() -> None:
    subprocess.run(
        ["python3", TEARDOWN], capture_output=True, text=True, cwd=REPO, timeout=30, check=False
    )


def run_dir(crate: str, started: float) -> str:
    """This run's own artifact directory -- unique per (crate, launch), so nothing is shared."""
    stamp = time.strftime("%Y%m%d-%H%M%S", time.localtime(started))
    return os.path.join(REPO, "target", "runtime-probe", "sweep-1170", f"{crate}-{stamp}")


def crash_signature(crate: str, since: float, artifact_dir: str) -> str:
    """The first fault line from any crash log this DLL touched during the run.

    Both directories, redirect first. The redirects are put in the launcher's real environment
    below, so they normally take -- but they still have to survive `launch.sh` -> me3 -> Proton,
    and when they do not the DLL falls back to writing beside `eldenring.exe`. A reader that knows
    only one of the two reports `no crash record` for a DLL that left a perfectly good one, which
    in this script's output is indistinguishable from a clean death.

    `since` stays load-bearing in either directory: the game directory is now never cleared (that
    delete took two other runs' generations with it), so last week's crash log is still sitting
    there with its final contents.
    """
    best = ""
    for directory in artifact_source_dirs(GAME_DIR, prefer=artifact_dir):
        try:
            names = os.listdir(directory)
        except OSError:
            continue
        for name in sorted(names):
            if "crash" not in name or not name.endswith((".txt", ".log")):
                continue
            path = os.path.join(directory, name)
            try:
                if os.path.getmtime(path) < since:
                    continue
                with open(path, encoding="utf-8", errors="replace") as handle:
                    for line in handle:
                        if "access-violation" in line or "exception code" in line:
                            match = re.search(r"(access-violation|exception code)[^;]{0,110}", line)
                            if match:
                                return f"{name}: {match.group(0).strip()}"
            except OSError:
                continue
    return best


def run_one(crate: str, watch_seconds: float) -> str:
    path = dll_path(crate)
    if not os.path.exists(path):
        return "not-built"
    profile = os.path.join(PROFILE_DIR, f"sweep-{crate}.me3")
    with open(profile, "w", encoding="utf-8") as handle:
        handle.write(PROFILE_TEMPLATE.format(dll=path))

    teardown()
    started = time.time()
    artifact_dir = run_dir(crate, started)
    os.makedirs(artifact_dir, exist_ok=True)
    # Redirected at launch, not copied at teardown: by teardown this run has already clobbered the
    # previous one's file, and a DLL that dies -- which is the whole point of this sweep -- never
    # reaches a copy step at all. These go into the launcher's real environment (not an
    # `env VAR=... cmd` prefix), so `crash_signature` above inherits them too.
    environment = dict(os.environ, ME3_PROFILE=profile, **artifact_env(artifact_dir))
    subprocess.Popen(
        ["setsid", "nohup", "bash", LAUNCHER, "-s"],
        env=environment,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        cwd=os.path.expanduser("~"),
        start_new_session=True,
    )

    pid = None
    while time.time() - started < LAUNCH_TIMEOUT_SECONDS and pid is None:
        time.sleep(POLL_SECONDS)  # the poll interval of a wait, not a synchronisation
        pid = game_pid()
    if pid is None:
        teardown()
        return "no-launch"

    verdict = "boots"
    deadline = time.time() + watch_seconds
    while time.time() < deadline:
        time.sleep(POLL_SECONDS)
        state = leader_state(pid)
        if state is None or state == "Z":
            verdict = "dies"
            break
    signature = crash_signature(crate, started, artifact_dir)
    teardown()
    stamp = time.strftime("%Y-%m-%d")
    if verdict == "dies":
        return f"dies: {signature or 'no crash record'} ({stamp})"
    return f"boots, leader alive at {watch_seconds:g}s ({stamp})"


def fingerprint(crates: list[str]) -> dict[str, tuple[int, int]]:
    """`{crate: (size, mtime_ns)}` for the built DLLs, cheap enough to re-check between runs."""
    stamps = {}
    for crate in crates:
        try:
            stat = os.stat(dll_path(crate))
        except OSError:
            continue
        stamps[crate] = (stat.st_size, stat.st_mtime_ns)
    return stamps


def load_results() -> dict:
    try:
        with open(RESULTS, encoding="utf-8") as handle:
            return json.load(handle)
    except (OSError, ValueError):
        return {}


def save_results(results: dict) -> None:
    with open(RESULTS, "w", encoding="utf-8") as handle:
        json.dump(results, handle, indent=2)
        handle.write("\n")


def cdylibs() -> list[str]:
    meta = json.loads(
        subprocess.run(
            ["cargo", "metadata", "--no-deps", "--format-version", "1"],
            capture_output=True, text=True, cwd=REPO, check=True, timeout=30,
        ).stdout
    )
    return sorted(
        p["name"] for p in meta["packages"] if any("cdylib" in t["kind"] for t in p["targets"])
    )


def selftest() -> int:
    failures = []
    if not os.path.exists(LAUNCHER):
        print(f"selftest SKIP: no launcher at {LAUNCHER}")
        return 0
    if game_pid() is not None and leader_state(game_pid()) is None:
        failures.append("leader_state returned None for a pid game_pid() just found")
    if "steam" in PROFILE_TEMPLATE.lower():
        failures.append("the generated profile must not reference Steam")
    built = [c for c in cdylibs() if os.path.exists(dll_path(c))]
    if not built:
        failures.append("no built DLLs found -- run scripts/er-build-dlls.sh --all first")
    # The one shell whose DLL name is not its crate name; if this regresses, so does the mapping.
    if "er-ags-stub" in cdylibs() and not dll_path("er-ags-stub").endswith("amd_ags_x64.dll"):
        failures.append("er-ags-stub did not resolve to amd_ags_x64.dll -- lib names are not being read")
    if DEFAULT_WATCH_SECONDS < 40:
        failures.append("watch window is shorter than the latest failure this sweep has seen")
    stamps = fingerprint(cdylibs())
    if stamps and fingerprint(cdylibs()) != stamps:
        failures.append("fingerprint is not stable across two calls on an unchanged tree")

    # The redirect, and the reader that has to follow it. A sweep whose runs all wrote to the game
    # directory would end holding one crash log -- the last DLL's -- and would have destroyed
    # everyone else's along the way. These two checks are what keep that from coming back.
    import tempfile

    env = artifact_env(run_dir("er-selftest", time.time()))
    missing = sorted(set(ARTIFACT_ENV) - set(env))
    if missing:
        failures.append(f"artifact_env does not carry {', '.join(missing)}")
    if not all(str(value).startswith(os.path.join(REPO, "target")) for value in env.values()):
        failures.append("a redirect points outside this repo's target/ run directory")
    with tempfile.TemporaryDirectory() as root:
        game = os.path.join(root, "Game")
        run = os.path.join(root, "run")
        os.makedirs(game)
        os.makedirs(run)
        since = time.time()
        stale = os.path.join(game, "er-quickload-crash-log.txt")
        with open(stale, "w", encoding="utf-8") as handle:
            handle.write("access-violation exception_addr=game+0xDEAD PREVIOUS RUN\n")
        os.utime(stale, (since - 3600, since - 3600))
        fresh = os.path.join(run, "er-loading-portrait-crash-log.txt")
        with open(fresh, "w", encoding="utf-8") as handle:
            handle.write("access-violation exception_addr=game+0xBEEF THIS RUN\n")
        saved, globals()["GAME_DIR"] = GAME_DIR, game
        try:
            signature = crash_signature("er-selftest", since, run)
        finally:
            globals()["GAME_DIR"] = saved
        if "0xBEEF" not in signature:
            failures.append(f"crash_signature did not read the run directory (got {signature!r})")
        if "0xDEAD" in signature:
            failures.append("crash_signature bound to a crash log older than the launch")

    for line in failures:
        print(f"selftest FAIL {line}")
    print(f"selftest: {len(failures)} failure(s); {len(built)} built cdylib(s)")
    return 1 if failures else 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dll", action="append", help="crate name; repeatable")
    parser.add_argument("--all", action="store_true")
    parser.add_argument("--skip-measured", action="store_true", help="only DLLs with no result yet")
    parser.add_argument("--watch-seconds", type=float, default=DEFAULT_WATCH_SECONDS)
    parser.add_argument("--list", action="store_true")
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    if args.selftest:
        return selftest()

    results = load_results()
    everything = cdylibs()
    if args.list:
        for crate in everything:
            built = "built" if os.path.exists(dll_path(crate)) else "NOT BUILT"
            print(f"{crate:26s} {built:9s} {results.get(crate, '-')}")
        return 0

    targets = args.dll or (everything if args.all else [])
    if not targets:
        parser.error("give --dll NAME, or --all")
    if args.skip_measured:
        targets = [t for t in targets if t not in results or "pre-gating" in str(results.get(t))]

    baseline = fingerprint(everything)
    for crate in targets:
        drifted = [
            name
            for name, stamp in fingerprint(everything).items()
            if baseline.get(name) not in (None, stamp)
        ]
        if drifted:
            print(
                "[sweep] ABORT -- these DLLs changed on disk mid-sweep: "
                + ", ".join(sorted(drifted))
                + ". Verdicts already recorded describe the OLD build; the rest would describe a "
                "different one, and a mix is not evidence. Let the tree settle, then re-run.",
                flush=True,
            )
            return 1
        print(f"[sweep] {crate} ...", flush=True)
        verdict = run_one(crate, args.watch_seconds)
        print(f"[sweep] {crate}: {verdict}", flush=True)
        results[crate] = verdict
        save_results(results)  # after every DLL, so an interrupted sweep keeps what it measured
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
