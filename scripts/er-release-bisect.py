#!/usr/bin/env python3
"""Score one `er_quickload.dll` by launching it alone and asking whether the game really lived.

Why this exists
---------------
`scripts/er-run-branch.py` verifies a DLL's provenance against the current working tree and
refuses anything that does not match. That is right for testing your own branch and useless for
testing a DLL GitHub built from a commit you are not standing on. This tool takes the opposite
contract: the artifact is authoritative, the tree is irrelevant, and the only question asked is
whether that build survives its boot window. Walk `main-*` pre-releases newest-first and the
answer bisects "which build started dying".

Each candidate runs alone -- a profile with exactly one `[[natives]]` entry, no Seamless, no
co-loaded shells -- so a death cannot be blamed on a neighbour. That is also why this does not go
through `er-dll-closure.py`/`er-gen-me3-profile.py`: their job is to assemble a compatible set
from a source tree, and here there is neither a set nor a tree.

What "SURVIVED" means, and why it is not "THE PROCESS EXISTS"
------------------------------------------------------------
bd `er-liveness-oracle-is-thread-count-not-process-existence-2026-08-25`: after a wedged launch
`eldenring.exe` stays in /proc for minutes as a two-thread husk at 0% CPU. Every scan of
`/proc/*/comm` calls that alive, so an A/B built on process existence measures nothing -- both
arms are husks. The verdict here therefore comes from `er-teardown.game_status()`: thread count
and CPU burn, with the husk rule owned by that module and not restated here.

The DLL's own log is read only as corroboration, never as the verdict. Same memory: a log that
was not rotated is the previous run's log, and the me3 line count of a healthy run and a wedged
one were both 32. This deletes the log before launching, so "no log" means "never reached the
DLL's install()", and it checks the mtime against the launch instant before believing a word of
it.

Usage:
    python3 scripts/er-release-bisect.py                       # newest main-* releases, in order
    python3 scripts/er-release-bisect.py --tag main-abc1234
    python3 scripts/er-release-bisect.py --dll path/to/er_quickload.dll --label tree
    python3 scripts/er-release-bisect.py --selftest
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import re
import subprocess
import sys
import time
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parent
REPO_ROOT = SCRIPTS.parent
sys.path.insert(0, str(SCRIPTS))

import er_run_lib  # noqa: E402
from er_artifact_env import artifact_env  # noqa: E402
from runtime_timeout_cap import runtime_timeout_cap_seconds  # noqa: E402


def _load_teardown():
    """Import `er-teardown.py`, whose filename is not an identifier.

    Imported rather than reimplemented on purpose. Its docstring records the measurement: a
    survey built on `comm` matching swept 101 processes, reported itself clean, and left 93
    alive -- so a private copy of "is the game gone" here would be wrong in exactly the way that
    wedges the next launch and scores it as a failure of the next DLL.
    """
    path = SCRIPTS / "er-teardown.py"
    spec = importlib.util.spec_from_file_location("er_teardown", path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


er_teardown = _load_teardown()

LAUNCHER = Path.home() / "Elden" / "launch.sh"
WORK = REPO_ROOT / "target" / "release-bisect"
AUTOLOAD_LOG_NAME = "er-quickload-autoload-debug.log"
RELEASE_TAG_PREFIX = "main-"
PRODUCT_DLL_NAME = "er_quickload.dll"

# Agent-shell subprocess ceiling (scripts/check-no-timeouts.py, MAX_TIMEOUT_SECONDS = 30). These
# are `gh` calls, not the game, so they get the tight limit and fail fast.
GH_TIMEOUT_SECONDS = 28

# One inotify slice. The blocking primitive is an event on the game directory; the PREDICATE is
# the survey below. Slices are re-armed against a wall-clock deadline rather than counted,
# because during boot that directory sees dozens of writes a second and a slice count would burn
# the whole budget in milliseconds (measured in er-run-branch.py, same pattern).
BOOT_SLICE_SECONDS = 4.0
# How long to wait for `eldenring.exe` to appear at all. Game-runtime budget, so it is bounded by
# the canonical cap rather than by the 30s agent-shell rule.
BOOT_BUDGET_SECONDS = 120
# How long the game must stay genuinely running to pass.
ALIVE_SECONDS_DEFAULT = 45

MILLISECONDS = 1000

VERDICT_ALIVE = "ALIVE"
VERDICT_HUSK = "HUSK"
VERDICT_DIED = "DIED"
VERDICT_NO_BOOT = "NO_BOOT"

EXIT_OK = 0
EXIT_ERROR = 1
EXIT_NOT_CLEARED = 3


def clamped_alive_seconds(requested: int) -> int:
    """Hold the game-runtime wait under the canonical cap, whatever the caller asked for."""
    return max(1, min(requested, int(runtime_timeout_cap_seconds())))


def attempt_dir(label: str) -> Path:
    """This attempt's own artifact directory. One per scored DLL, so nothing is shared."""
    safe = re.sub(r"[^A-Za-z0-9._-]+", "-", label).strip("-") or "attempt"
    directory = WORK / "runs" / f"{time.strftime('%Y%m%d-%H%M%S')}-{safe}"
    directory.mkdir(parents=True, exist_ok=True)
    return directory


def autoload_log_path(art: Path | None = None) -> Path:
    """Where this attempt's autoload debug log lives.

    It used to be the game directory's shared copy, which `score` then `unlink`ed before launching.
    That deleted somebody else's run -- several sessions launch concurrently here -- and it deleted
    two generations at once, because `er_game_base::log::begin_fresh_run` removes `<name>.prev`
    unconditionally when the live file is absent. With a per-attempt directory there is nothing
    shared to clear and nothing of anyone else's to lose.
    """
    return (art if art is not None else er_run_lib.game_dir()) / AUTOLOAD_LOG_NAME


def write_single_dll_profile(dll: Path, destination: Path) -> Path:
    """An me3 profile loading exactly one native, so a death has exactly one suspect."""
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_text(
        'profileVersion = "v1"\n'
        "start_online = false\n\n"
        "[[supports]]\n"
        'game = "eldenring"\n\n'
        "[[natives]]\n"
        f"path = '{dll}'\n",
        encoding="utf-8",
    )
    return destination


def release_tags(limit: int) -> list[str]:
    """`main-*` pre-release tags, newest first, as `gh` reports them."""
    result = subprocess.run(
        ["gh", "release", "list", "--limit", str(limit), "--json", "tagName,createdAt"],
        capture_output=True,
        text=True,
        cwd=str(REPO_ROOT),
        timeout=GH_TIMEOUT_SECONDS,
        check=False,
    )
    rows = json.loads(result.stdout or "[]")
    return [row["tagName"] for row in rows if row["tagName"].startswith(RELEASE_TAG_PREFIX)]


def fetch_release_dll(tag: str) -> Path:
    """Download `tag`'s DLL once, into a per-tag directory so it can be identified later."""
    destination = WORK / tag
    dll = destination / PRODUCT_DLL_NAME
    if dll.is_file():
        return dll
    destination.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        [
            "gh",
            "release",
            "download",
            tag,
            "--pattern",
            PRODUCT_DLL_NAME,
            "--dir",
            str(destination),
            "--clobber",
        ],
        check=True,
        cwd=str(REPO_ROOT),
        timeout=GH_TIMEOUT_SECONDS,
    )
    return dll


def sha256_of(path: Path) -> str:
    import hashlib

    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def wait_for_game_pid(budget_seconds: float) -> int | None:
    """Block until `eldenring.exe` exists, or the budget runs out.

    The wait itself is an inotify wait on the game directory -- every launch writes there long
    before the window appears -- and the game's presence is re-checked whenever a slice ends. No
    sleeps: `scripts/check-no-timeouts.py` rejects them, and rightly, since a sleep both wastes
    the time the process was already there and races when boot needs longer.
    """
    deadline = time.monotonic() + budget_seconds
    with er_run_lib.DirectoryWatch(er_run_lib.game_dir()) as watch:
        while True:
            games = [row for row in er_teardown.survey() if row["comm"] == "eldenring.exe"]
            if games:
                return int(games[0]["pid"])
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                return None
            if not watch.available:
                # No inotify: one bounded pidfd wait on nothing is still a real wait, and the
                # deadline above still bounds the loop.
                er_teardown.wait_for_exit([], int(min(BOOT_SLICE_SECONDS, remaining) * MILLISECONDS))
                return None
            watch.wait(min(BOOT_SLICE_SECONDS, remaining))


def read_log_evidence(log: Path, launched_at: float) -> dict[str, object]:
    """The DLL's own last timestamp and tail -- corroboration only, and only if it is this run's.

    A log whose mtime predates the launch is the previous run's file surviving a process that
    died before `install()` rotated it, which reads exactly like a fresh run.
    """
    if not log.is_file():
        return {"log_fresh": False, "last_log_ms": None, "tail": [], "log_note": "no log written"}
    if log.stat().st_mtime < launched_at:
        return {
            "log_fresh": False,
            "last_log_ms": None,
            "tail": [],
            "log_note": "log predates the launch -- it is the PREVIOUS run's file",
        }
    lines = log.read_text(encoding="utf-8", errors="replace").splitlines()
    last_ms = None
    for line in reversed(lines):
        if line.startswith("[+") and "ms]" in line:
            try:
                last_ms = int(line[2 : line.index("ms]")])
            except ValueError:
                last_ms = None
            break
    return {"log_fresh": True, "last_log_ms": last_ms, "tail": lines[-6:], "log_note": ""}


def classify(game_pid: int | None, exited: bool, status_rows: list[dict[str, object]]) -> str:
    """Turn what was observed into one verdict. Pure, so the selftest can drive every branch."""
    if game_pid is None:
        return VERDICT_NO_BOOT
    if exited:
        return VERDICT_DIED
    for row in status_rows:
        if int(row["pid"]) != game_pid:
            continue
        if row["verdict"] == er_teardown.GAME_RUNNING:
            return VERDICT_ALIVE
        if row["verdict"] == er_teardown.GAME_HUSK:
            return VERDICT_HUSK
        return VERDICT_DIED
    return VERDICT_DIED


def score(dll: Path, label: str, alive_seconds: int) -> dict[str, object]:
    """Launch `dll` alone and report what the game actually did. Tears down before and after."""
    if not LAUNCHER.is_file():
        raise RuntimeError(f"launcher not found: {LAUNCHER}")
    if not dll.is_file():
        raise RuntimeError(f"no such DLL: {dll}")

    er_teardown.teardown(verbose=False)
    if er_teardown.survey():
        raise RuntimeError(
            "prefix processes survived teardown; a leftover session wedges the next boot and "
            "would score this build as a false failure"
        )

    art = attempt_dir(label)
    profile = write_single_dll_profile(dll, art / "bisect.me3")
    log = autoload_log_path(art)

    # Wall clock, not monotonic: this is compared against a file mtime.
    launched_at = time.time()
    started = time.monotonic()
    subprocess.Popen(
        # `-o`: offline/solo. launch.sh includes ersc.dll by default (2026-08-24) and a
        # co-loaded Seamless would give any death a second suspect.
        ["bash", str(LAUNCHER), "-o"],
        # Every per-run artifact into this attempt's directory. A game-directory artifact is
        # single-slot (`begin_fresh_run` keeps one generation), so a bisect that scores twenty
        # builds used to leave evidence for the last two and destroy the other eighteen -- and it
        # took concurrent sessions' logs with it. `read_log_evidence` below reads the redirected
        # copy, so the reader moved with the writer.
        env={**os.environ, "ME3_PROFILE": str(profile), **artifact_env(art)},
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        stdin=subprocess.DEVNULL,
        start_new_session=True,
        cwd=str(REPO_ROOT),
    )

    game_pid = wait_for_game_pid(BOOT_BUDGET_SECONDS)
    exited = False
    status_rows: list[dict[str, object]] = []
    if game_pid is not None:
        exited = er_teardown.wait_for_exit([game_pid], alive_seconds * MILLISECONDS)
        if not exited:
            status_rows = er_teardown.game_status()

    record: dict[str, object] = {
        "label": label,
        "dll": str(dll),
        "sha256": sha256_of(dll)[:16],
        "verdict": classify(game_pid, exited, status_rows),
        "game_pid": game_pid,
        "seconds_observed": round(time.monotonic() - started, 1),
        "threads": next((row["threads"] for row in status_rows), None),
        "cpu_ticks": next((row["cpu_ticks"] for row in status_rows), None),
    }
    record.update(read_log_evidence(log, launched_at))
    er_teardown.teardown(verbose=False)
    return record


def report(record: dict[str, object]) -> None:
    print(
        f"    {record['sha256']}  {record['verdict']}"
        f"  threads={record['threads']} cpu_ticks={record['cpu_ticks']}"
        f"  observed={record['seconds_observed']}s  last_log={record['last_log_ms']}ms"
    )
    if record["log_note"]:
        print(f"      ! {record['log_note']}")
    for line in list(record["tail"])[-2:]:
        print(f"      | {str(line)[:150]}")


def selftest() -> int:
    import tempfile

    failures = 0

    def check(condition: bool, label: str) -> None:
        nonlocal failures
        print(f"  {'ok  ' if condition else 'FAIL'}   {label}")
        failures += 0 if condition else 1

    with tempfile.TemporaryDirectory() as temp:
        root = Path(temp)
        profile = write_single_dll_profile(root / "x.dll", root / "p.me3")
        body = profile.read_text(encoding="utf-8")
        check(body.count("[[natives]]") == 1, "the profile loads exactly ONE native")
        check("x.dll" in body, "the profile names the DLL under test")
        check("ersc.dll" not in body, "no Seamless DLL: a death must have one suspect")

        log = root / AUTOLOAD_LOG_NAME
        log.write_text("[+1234ms] hello\n", encoding="utf-8")
        stale = read_log_evidence(log, log.stat().st_mtime + 10)
        check(not stale["log_fresh"], "a log older than the launch is refused as the previous run's")
        check(stale["last_log_ms"] is None, "a refused log yields no timestamp to be misread")
        fresh = read_log_evidence(log, log.stat().st_mtime - 10)
        check(fresh["log_fresh"] and fresh["last_log_ms"] == 1234, "a fresh log yields its last [+Nms]")
        check(
            read_log_evidence(root / "absent.log", 0.0)["log_note"] == "no log written",
            "an absent log is reported as never reaching the DLL's install()",
        )

    husk = [{"pid": 7, "threads": 2, "cpu_ticks": 0, "verdict": er_teardown.GAME_HUSK}]
    running = [{"pid": 7, "threads": 57, "cpu_ticks": 900, "verdict": er_teardown.GAME_RUNNING}]
    check(classify(7, False, running) == VERDICT_ALIVE, "57 threads burning CPU is ALIVE")
    check(
        classify(7, False, husk) == VERDICT_HUSK,
        "a two-thread husk at 0% CPU is NOT a pass -- the trap this tool exists to avoid",
    )
    check(classify(7, True, []) == VERDICT_DIED, "a process that exited in the window is DIED")
    check(classify(None, False, []) == VERDICT_NO_BOOT, "a game that never appeared is NO_BOOT")
    check(
        classify(7, False, []) == VERDICT_DIED,
        "a pid that vanished between the wait and the health read is DIED, not ALIVE",
    )

    check(
        clamped_alive_seconds(10_000) == int(runtime_timeout_cap_seconds()),
        "the game-runtime wait is clamped to the canonical cap",
    )
    check(clamped_alive_seconds(45) == 45, "a wait under the cap is left alone")

    for name in ("survey", "teardown", "wait_for_exit", "game_status", "GAME_HUSK"):
        check(hasattr(er_teardown, name), f"er-teardown.py still provides {name}")
    check(hasattr(er_run_lib, "DirectoryWatch"), "er_run_lib still provides DirectoryWatch")
    check(hasattr(er_run_lib, "game_dir"), "er_run_lib still owns the game directory path")

    print("selftest: " + ("PASS" if failures == 0 else f"FAIL ({failures})"))
    return EXIT_OK if failures == 0 else EXIT_ERROR


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("--limit", type=int, default=10, help="how many releases to consider")
    parser.add_argument("--max-runs", type=int, default=4, help="stop after this many launches")
    parser.add_argument(
        "--alive-seconds",
        type=int,
        default=ALIVE_SECONDS_DEFAULT,
        help="how long the game must stay genuinely running to PASS",
    )
    parser.add_argument("--tag", action="append", help="test only these release tags, in order")
    parser.add_argument("--dll", help="score a local DLL instead of a release artifact")
    parser.add_argument("--label", default="local", help="name for the --dll run in the output")
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()

    if args.selftest:
        return selftest()

    alive_seconds = clamped_alive_seconds(args.alive_seconds)
    WORK.mkdir(parents=True, exist_ok=True)
    results: list[dict[str, object]] = []

    if args.dll:
        print(f"==> {args.label}")
        record = score(Path(args.dll).resolve(), args.label, alive_seconds)
        report(record)
        results.append(record)
    else:
        tags = args.tag or release_tags(args.limit)
        if not tags:
            print(f"no {RELEASE_TAG_PREFIX}* releases found", file=sys.stderr)
            return EXIT_ERROR
        runs = tags[: args.max_runs]
        print(
            f"scoring {len(runs)} of {len(tags)} release(s); "
            f"{VERDICT_ALIVE} = still genuinely running after {alive_seconds}s\n"
        )
        for tag in runs:
            print(f"==> {tag}")
            record = score(fetch_release_dll(tag), tag, alive_seconds)
            report(record)
            results.append(record)
            print()
            if record["verdict"] == VERDICT_ALIVE:
                print(f"FIRST GOOD BUILD: {tag}")
                break
        if len(runs) < len(tags):
            print(f"NOT scored ({len(tags) - len(runs)} left by --max-runs): {', '.join(tags[len(runs):])}")

    out = WORK / "bisect-results.json"
    out.write_text(json.dumps(results, indent=2), encoding="utf-8")
    print(f"results: {out}")
    return EXIT_OK


if __name__ == "__main__":
    try:
        sys.exit(main())
    except RuntimeError as error:
        print(f"er-release-bisect: {error}", file=sys.stderr)
        sys.exit(EXIT_NOT_CLEARED)
