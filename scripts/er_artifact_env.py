"""The one place a launcher learns where this run's artifacts go.

WHY THIS IS A MODULE AND NOT FIVE COPIES
----------------------------------------
The bug that started all of this was a launcher that redirected the autoload debug log and the crash
log out of the game directory, and not the continue trace. One line of env, missing from one array,
and the 11:09 run destroyed the 09:07 run's 5.4 MB `er-quickload-continue-trace.log` -- a whole day's
evidence, gone, with no error anywhere. Copying that array into every Python launcher would have set
the same trap five more times: the next artifact gets added to four of them.

A game-directory artifact is SINGLE-SLOT. `er_game_base::log::begin_fresh_run` renames `<name>` to
`<name>.prev` and truncates on the first write of each process -- ONE generation -- so two launches
lose the run before last, and several sessions launch concurrently in this repo, which makes that the
normal case rather than a race. Redirecting the writer at LAUNCH, into a directory unique to the run,
means two runs never share a path; a copy at teardown cannot help, because by then this run has
already clobbered the previous one's file, and a crashed or killed run never reaches the copy at all.

`scripts/er-artifact-redirect-audit.py` reads the knob table out of the Rust sources and fails when
this table is missing one, so a new DLL artifact cannot quietly stay unredirected.
"""

from __future__ import annotations

import os
from pathlib import Path

# env var -> the filename it takes over. The filename matches the DLL's own game-directory default,
# so a run directory reads exactly like the game directory did.
ARTIFACT_ENV: dict[str, str] = {
    # `ER_QUICKLOAD_AUTOLOAD_DEBUG_PATH` also moves the `portrait-capture-slot*.bin` dumps, which
    # `er-loading-portrait-core` writes beside it (~63 MB on a portrait run).
    "ER_QUICKLOAD_AUTOLOAD_DEBUG_PATH": "er-quickload-autoload-debug.log",
    "ER_QUICKLOAD_CRASH_LOG_PATH": "er-quickload-crash-log.txt",
    "ER_QUICKLOAD_TRACE_CONTINUE_PATH": "er-quickload-continue-trace.log",
    "ER_QUICKLOAD_TELEMETRY_PATH": "er-quickload-telemetry.json",
    "ER_QUICKLOAD_INPUT_TRACE_PATH": "er-quickload-input-trace.jsonl",
    "ER_QUICKLOAD_BOOTSTRAP_PATH": "er-quickload-bootstrap.jsonl",
    "ER_QUICKLOAD_BOOTSTRAP_STATE_PATH": "er-quickload-bootstrap-state.json",
    "ER_QUICKLOAD_PROFILE_PATH": "er-quickload-profile.jsonl",
    # THE OTHER SHELLS' ARTIFACTS. These five had no redirect knob at all until 2026-08-31, so no
    # launcher could move them and every launch rotated the previous run's copy to `.prev`. The
    # reload trace is the largest producer in the repo (~655 MB/hour) and was the least movable.
    "ER_QUICKLOAD_RELOAD_TRACE_PATH": "er-reload-trace.log",
    "ER_QUICKLOAD_INPUT_HARNESS_LOG_PATH": "er-input-harness.log",
    "ER_QUICKLOAD_INPUT_HARNESS_PHASES_PATH": "er-input-harness-phases.jsonl",
    "ER_QUICKLOAD_DIAG_HARNESS_PATH": "er-diag-harness.log",
    "ER_QUICKLOAD_TIMESERIES_PATH": "er-telemetry-timeseries.jsonl",
    # Zero generations kept in the game directory (a bare `fs::write`), so the previous run's dump
    # is gone the instant this one writes.
    "ER_QUICKLOAD_CPU_PROFILE_PATH": "er-cpu-profile.txt",
    # The badge shell's log. Both of its launchers used to `rm -f` this out of the game directory
    # before launching, which destroys the live file AND the `.prev` behind it -- two prior runs,
    # neither of them the deleting run's.
    "ER_QUICKLOAD_ARMAMENT_ICONS_PATH": "er-armament-icons.log",
    # The standalone portrait shell's run log and crash log -- the last two artifacts in the repo
    # with no knob at all (they resolved against the process CWD, which no launcher can move), and
    # `run-portrait-dll-standalone-smoke.sh` deleted BOTH from the game directory before launching.
    # One knob each, because that smoke reads them for different verdicts.
    "ER_QUICKLOAD_LOADING_PORTRAIT_PATH": "er-loading-portrait.log",
    "ER_QUICKLOAD_LOADING_PORTRAIT_CRASH_LOG_PATH": "er-loading-portrait-crash-log.txt",
    # The standalone crash-logging shell's four files. It had NO knob until 2026-09-04, which made
    # it the one writer no launcher could move -- and the one whose output is least reproducible.
    # A relaunch that day erased a 26-record log mid-investigation, the only copy of a 22-deep
    # unwind cascade, and the user had to re-provoke the crash to get it back.
    "ER_QUICKLOAD_CRASH_LOGGING_LOG_PATH": "er-crash-log.txt",
    "ER_QUICKLOAD_CRASH_LOGGING_LATEST_PATH": "er-crash-latest.txt",
    "ER_QUICKLOAD_CRASH_LOGGING_BREADCRUMB_PATH": "er-crash-breadcrumb-latest.txt",
    "ER_QUICKLOAD_CRASH_LOGGING_MODULES_PATH": "er-crash-modules.txt",
    # The save-census shell's run log and its telemetry -- the last two artifacts in the repo with
    # no knob. The telemetry is the worse of the pair and the more valuable: it is the RUN-STOPPING
    # ORACLE for a save-suppression proof (`escaped_write_sites` must be empty), and it publishes
    # with a write-tmp-then-rename, so it keeps ZERO previous generations -- the last run's verdict
    # is gone the instant this run installs, with no `.prev` to fall back on. Both are read back by
    # `run-save-census-probe.sh`, which also used to `rm -f` them out of the game directory before
    # launching, taking the `.prev` behind the live file with them.
    "ER_QUICKLOAD_SAVE_DISABLE_LOG_PATH": "er-save-disable.log",
    "ER_QUICKLOAD_SAVE_DISABLE_TELEMETRY_PATH": "er-save-disable-telemetry.json",
}

# The name a watcher reads to find this run's artifacts. A reader still on the fixed game-directory
# path finds nothing for a redirected run and reports a perfectly healthy run as SILENT -- a false
# negative that looks exactly like a failed feature, which is the more dangerous half of this change.
RUN_ARTIFACT_DIR_ENV = "ER_RUN_ARTIFACT_DIR"


def artifact_env(directory: str | os.PathLike[str]) -> dict[str, str]:
    """Every redirect for a run whose artifacts live in `directory`, plus the watcher's pointer."""
    home = Path(directory)
    env = {name: str(home / filename) for name, filename in ARTIFACT_ENV.items()}
    env[RUN_ARTIFACT_DIR_ENV] = str(home)
    return env


def artifact_source_dirs(
    game_dir: str | os.PathLike[str], prefer: str | os.PathLike[str] | None = None
) -> list[Path]:
    """Every directory a READER should look in, in priority order.

    The redirect first, then the game directory -- because the env has to survive `launch.sh` -> me3
    -> Proton, and when it does not the DLL falls back to the game directory rather than writing
    nowhere. A reader that knows only one of the two calls a healthy run silent half the time, and
    "the DLL wrote nothing" is indistinguishable from "the feature is broken" in a report.

    `prefer` is the run directory a caller ALREADY KNOWS, and it beats the environment because it is
    the stronger evidence. A shell launcher passes the redirects to the game with an `env VAR=... me3`
    prefix, which does NOT put them in the launcher's own environment -- so a monitor started as a
    sibling of that launch inherits nothing and would fall back to the game directory while the DLL
    writes somewhere else. Every such monitor is handed `--artifact-dir`; that is what to pass here.
    """
    redirected = os.environ.get(RUN_ARTIFACT_DIR_ENV)
    if not redirected:
        telemetry = os.environ.get("ER_QUICKLOAD_TELEMETRY_PATH")
        redirected = str(Path(telemetry).parent) if telemetry else None
    ordered = [prefer, redirected, str(game_dir)]
    seen: list[Path] = []
    for entry in ordered:
        if entry is None:
            continue
        path = Path(entry)
        if path not in seen:
            seen.append(path)
    return seen


def resolve_artifact_dir(
    game_dir: str | os.PathLike[str], prefer: str | os.PathLike[str] | None = None
) -> Path:
    """The single directory a reader should treat as this run's own."""
    return artifact_source_dirs(game_dir, prefer)[0]


def resolve_artifact(
    name: str,
    game_dir: str | os.PathLike[str],
    prefer: str | os.PathLike[str] | None = None,
    newer_than: float | None = None,
) -> Path:
    """This run's copy of `name`: the redirect if it EXISTS on disk, else the game-directory copy.

    Existence decides, not configuration. The redirect is set from the launcher's side; whether the
    DLL honoured it depends on the environment surviving the whole launch chain, and returning a
    path that is not there would make a healthy run look silent.

    `newer_than` IS THE OTHER HALF OF THAT, and it is not optional for a reader that resolves at
    LAUNCH TIME. The game-directory copy of a redirected artifact is the PREVIOUS run's file, sitting
    on disk with its final contents, and it exists for the whole window between the launcher starting
    the watcher and the DLL writing the redirect for the first time -- a couple of seconds. A reader
    that resolves inside that window binds to the last run's answers and never looks again.

    Measured 2026-08-31, run wedge-writers-20260831-a: the same-character watcher resolved at t=0,
    fell through to the game directory, read `system_quit_continue_confirm_fresh_deser_count = 2` and
    `oracle_player_present = true` from a run that had ENDED, declared "epoch 2: WORLD-STABLE reached"
    at elapsed 0.0s, wrote no switch-control file, and tore down a game that had not finished booting.
    The report said `NEVER ARMED ... Trigger never invoked`, which is true and reads as a product
    failure. The same shape explains a frozen-telemetry verdict: a stale file's mtime never changes,
    so a watcher bound to one reports `TELEMETRY_FROZEN_HUNG` against a perfectly healthy game.

    So pass the run's own start time and RE-RESOLVE until it returns something: a candidate older
    than the run cannot be this run's, and returning the run directory's not-yet-existing path is the
    honest answer -- the caller reads nothing and waits, instead of reading someone else's evidence.
    """
    candidates = artifact_source_dirs(game_dir, prefer)
    for directory in candidates:
        candidate = directory / name
        try:
            if newer_than is not None and candidate.stat().st_mtime < newer_than:
                continue
        except OSError:
            continue
        if candidate.exists():
            return candidate
    return candidates[0] / name
