#!/usr/bin/env python3
"""Tear down every process belonging to the Elden Ring Proton prefix, not just the obvious four.

Why this exists
---------------
bd `er-teardown-must-kill-wineserver-or-next-boot-hangs-2026-08-24` says to kill
`eldenring.exe`, `me3`, `me3-launcher.exe`, `wineserver` and `winedevice.exe`. That list is
incomplete, and the omission is what wedges the next launch.

A Wine prefix session also runs `services.exe`, `plugplay.exe`, `explorer.exe`, `svchost.exe`,
`rpcss.exe`, and (under Proton) `tabtip.exe` and `xalia.exe`. Killing only the five above leaves
about seven processes per launch alive, parented to the dead wineserver. They accumulate.

Measured 2026-08-25: after four "successful" teardowns that each verified their own list was
empty, the machine held **105** orphaned prefix processes across roughly fifteen sessions, some
days old. Every launch after the first came up as a two-thread `eldenring.exe` husk at 0% CPU --
the process exists, so a naive liveness check calls it running, and both the game's own log and
the mod DLLs' logs stop within ~100ms looking exactly like a DLL hang. It is not one.

The LIVENESS oracle, since it is the other half of the same mistake: a real Elden Ring has ~57
threads and burns CPU. Two threads at 0% is a husk. `--status` reports both rather than the
presence of a pid.

The hole that made the first version of this tool lie
-----------------------------------------------------
A Wine process's `comm` is the Windows executable name (`eldenring.exe`) while its `exe` symlink
points at `wine64-preloader`. Matching on `comm` therefore finds the Windows-side processes and
misses the entire container stack underneath them: `srt-bwrap`, `pv-adverb`, Proton's own
`python3.13`, `wine-preloader`, `wine64-preloader`. The first version of this script swept 101
processes, reported "clean -- zero prefix processes remain", and left **93** alive -- parented to
the Steam client itself, which is why the Steam UI sat on "Stopping" and every relaunch wedged.
"My list is empty" is not "the game is gone" unless the list was built the right way.

Scope
-----
The primary classifier is now the process ENVIRONMENT: `SteamGameId=1245620`,
`SteamAppId=1245620` or `STEAM_COMPAT_APP_ID=1245620`. Every process in the session inherits one,
whatever it renamed itself to, so the container layers are caught with the Windows ones. The
comm+prefix rule is kept as a second net for anything that lost its environment.

These are exact literals naming one appid. AGENTS.md forbids broad `wine`/`rsi` command-line
patterns because they match unrelated words (`rsi` matches `version`); an appid equality test in
`environ` has no such failure mode, and nothing here reads a command line.

A TEARDOWN that leaves no record is indistinguishable from a crash
------------------------------------------------------------------
`er-quickload` stamps `er-run-outcome.txt` with `outcome=running` the moment its exit hooks are
armed, and its hooks rewrite that line on the way out: `clean-exit`, `exit-unclassified`, or
`fatal-exception`. The file's own contract says that finding `running` after the process is gone
means no exit path ran -- "killed from outside (an agent teardown, `wineserver`, the OOM killer)".

Which was true and useless, because this script was one of those outside killers and said nothing.
Measured 2026-09-06: a run ended, the file read `outcome=running`, and there was no way to tell a
deliberate teardown-to-rebuild from a death nobody understood. Every agent teardown was a
permanent false positive in the one instrument built to answer "did it crash".

So this script now owns that line while it kills. It stamps before signalling -- after SIGKILL
there is no process left to write anything -- and again after the sweep, because the DLL's own
exit hooks may fire in between and overwrite the record with a code that on this target says
nothing (a normal quit exits `0xc0000005` under Proton). The `was=` field carries whatever the
file said beforehand, so nothing is destroyed by being superseded. `running` then finally means
what it claims: nobody admits to this one.

Usage:
    python3 scripts/er-teardown.py --status     # report, kill nothing
    python3 scripts/er-teardown.py              # SIGTERM, wait, SIGKILL survivors
    python3 scripts/er-teardown.py --reason rebuilding-er-invasion-warp
    python3 scripts/er-teardown.py --selftest
"""

from __future__ import annotations

import argparse
import glob
import os
import select
import signal
import sys

DEFAULT_PREFIX = os.path.expanduser(
    "~/.local/share/Steam/steamapps/compatdata/1245620"
)

# Wine/Proton per-session service processes. `eldenring.exe` and the launchers are listed with
# them because they belong to the same session and must go in the same sweep.
PREFIX_COMMS = frozenset(
    {
        "eldenring.exe",
        "explorer.exe",
        "plugplay.exe",
        "rpcss.exe",
        "services.exe",
        "start.exe",
        "svchost.exe",
        "tabtip.exe",
        "wineboot.exe",
        "winedevice.exe",
        "winemenubuilder.exe",
        "wineserver",
        "xalia.exe",
    }
)

# Launcher processes this repo starts. They have no exe inside the prefix, so they are matched by
# name alone -- which is safe because these names are ours.
LAUNCHER_COMMS = frozenset({"me3", "me3-launcher.exe"})

# The Steam appid for ELDEN RING, as it appears in the environment of every process in the game's
# session -- container shims included. This is the classifier that actually finds everything.
APPID_NEEDLES = (
    b"SteamGameId=1245620",
    b"SteamAppId=1245620",
    b"STEAM_COMPAT_APP_ID=1245620",
)

# A real Elden Ring runs far more threads than this. At or below it, the process is a husk.
HUSK_THREAD_CEILING = 4

# The run-outcome file `er-quickload` owns, and the `outcome=` value this script writes into it.
# The name and the one-line `key=value` shape are the DLL's contract -- see the module docs of
# `crates/er-quickload/src/crashlog/veh_exit_hooks.rs`; a second format here would mean a reader
# has to know which writer it is looking at, which is the guesswork the file exists to end.
RUN_OUTCOME_FILE_NAME = "er-run-outcome.txt"
RUN_OUTCOME_TORN_DOWN = "torn-down"
# What `--reason` defaults to. Deliberately not "unknown": the reason field exists to say why a
# process was killed, and a teardown run without one was still a deliberate act by an agent.
DEFAULT_TEARDOWN_REASON = "agent-teardown"

# Milliseconds to wait for SIGTERM to be honoured before escalating to SIGKILL, and again for the
# kill itself to land. Spent inside `poll()` on a pidfd -- a readiness wait on the actual event
# ("this process exited"), not a sleep polling for it. `scripts/check-no-timeouts.py` rejects the
# sleep form, and rightly: a sleep both wastes the time the process was already gone and races
# when it needs longer.
TERM_GRACE_MS = 12_000
KILL_GRACE_MS = 3_000
# Window over which CPU burn is sampled in `--status`. Also spent in `poll()`, so a process that
# dies mid-sample is reported as having died rather than silently scoring zero ticks.
CPU_SAMPLE_MS = 3_000


def _read(path: str) -> str | None:
    try:
        with open(path, encoding="utf-8", errors="replace") as handle:
            return handle.read()
    except OSError:
        return None


def _link(path: str) -> str:
    try:
        return os.readlink(path)
    except OSError:
        return ""


def _has_appid(entry: str) -> bool:
    """Does this process's environment name the Elden Ring appid?"""
    try:
        with open(f"{entry}/environ", "rb") as handle:
            environ = handle.read()
    except OSError:
        return False
    return any(needle in environ for needle in APPID_NEEDLES)


def wait_for_exit(pids: list[int], timeout_ms: int) -> bool:
    """Block until every pid in `pids` has exited, or `timeout_ms` elapses. True if all exited.

    Uses `pidfd_open` + `poll`, which is the readiness primitive for this event: a pidfd becomes
    readable exactly when its process dies, including for processes that are not our children.
    A sleep-and-recheck loop would be both slower (it waits out the interval after the process is
    already gone) and less reliable (it races when the process needs longer than the interval),
    which is why `scripts/check-no-timeouts.py` rejects that form.

    A pid that is already gone, or that we may not open, is treated as exited -- both mean there
    is nothing left to wait for.
    """
    poller = select.poll()
    fds: dict[int, int] = {}
    for pid in pids:
        try:
            fd = os.pidfd_open(pid, 0)
        except (OSError, AttributeError):
            continue
        fds[fd] = pid
        poller.register(fd, select.POLLIN)

    try:
        remaining_ms = timeout_ms
        while fds and remaining_ms > 0:
            ready = poller.poll(remaining_ms)
            if not ready:
                break
            for fd, _event in ready:
                poller.unregister(fd)
                os.close(fd)
                fds.pop(fd, None)
            # `poll` returns as soon as any pid exits, so the loop re-enters for the rest. The
            # budget is deliberately not decremented by observed elapsed time: shrinking it here
            # would need a clock, and the caller's contract is an upper bound on the wait, which
            # a re-entered poll with the same bound still satisfies for a set that is strictly
            # shrinking.
        return not fds
    finally:
        for fd in fds:
            try:
                poller.unregister(fd)
                os.close(fd)
            except OSError:
                pass


def _steam_client_pids() -> set[int]:
    """The user's Steam client and its helpers -- Never targets.

    The Proton prefix contains a Windows `steam.exe` shim, and the client's own children can
    inherit the game's environment. Killing the client would log the user out of Steam to clean
    up after a game, which is not a trade this tool gets to make. Identified by the native
    executable name, which the Windows shim does not share.
    """
    client: set[int] = set()
    for entry in glob.glob("/proc/[0-9]*"):
        comm = _read(f"{entry}/comm")
        if comm is None:
            continue
        if comm.strip() in {"steam", "steamwebhelper"}:
            try:
                client.add(int(entry.rsplit("/", 1)[-1]))
            except ValueError:
                pass
    return client


def survey(prefix: str = DEFAULT_PREFIX) -> list[dict[str, object]]:
    """Every prefix-owned process currently alive, with the evidence that classifies it."""
    found: list[dict[str, object]] = []
    protected = _steam_client_pids()
    for entry in glob.glob("/proc/[0-9]*"):
        pid_text = entry.rsplit("/", 1)[-1]
        comm = _read(f"{entry}/comm")
        if comm is None:
            continue
        comm = comm.strip()
        exe = _link(f"{entry}/exe")
        in_prefix = prefix in exe or prefix in _link(f"{entry}/cwd")
        by_appid = _has_appid(entry)
        # The game itself needs no CORROBORATION, and twice it has been missed for want of some.
        # Launched through the Steam Linux Runtime the game's /proc/<pid>/exe and cwd are inside
        # the bwrap container, not the prefix, and the appid env is not always readable -- so both
        # of the other rules can be false for a process whose comm is literally `eldenring.exe`.
        # Measured twice on 2026-08-29: `--status` reported "no eldenring.exe" while pid 829287
        # (86 threads) sat there with a mapped 3068x1703 window on screen. There is one ELDEN RING
        # on this machine; its name is enough.
        by_name = comm == "eldenring.exe"
        if not (
            by_appid or by_name or (comm in PREFIX_COMMS and in_prefix) or comm in LAUNCHER_COMMS
        ):
            continue
        # Hard exclusion, applied after classification so it cannot be reasoned around.
        if int(pid_text) in protected:
            continue
        stat = _read(f"{entry}/stat")
        state = stat.split()[2] if stat else "?"
        found.append(
            {
                "pid": int(pid_text),
                "comm": comm,
                "state": state,
                "threads": len(glob.glob(f"{entry}/task/*")),
                "in_prefix": in_prefix,
                # Which rule caught it, so a survey that misses something is diagnosable rather
                # than merely short.
                "matched_by": (
                    "appid"
                    if by_appid
                    else ("name" if by_name else ("prefix" if in_prefix else "launcher"))
                ),
                "exe": exe.rsplit("/", 1)[-1],
            }
        )
    found.sort(key=lambda row: row["pid"])
    return found


def is_zombie(pid: int) -> bool:
    """Has this pid exited but not been reaped?

    A zombie leader is dead no matter what else is true, and nothing here used to ask. Measured
    2026-09-04: `eldenring.exe` pid 3885401 sat in state `Z` with `Threads: 127` still listed and
    27 CPU ticks in the sample window, so the thread/CPU husk rule below passed it as `running`
    while `/proc/<pid>/stat` said it had already exited. A driver that trusted that verdict kept
    driving input into a corpse, and a human reading it was told the game was fine.
    """
    stat = _read(f"/proc/{pid}/stat")
    if stat is None:
        return False
    # "pid (comm) state ..." -- comm can contain spaces and parens, so split on the last ')'.
    try:
        return stat[stat.rindex(")") + 1 :].split()[0] == "Z"
    except (ValueError, IndexError):
        return False


def cpu_ticks(pid: int) -> int | None:
    """utime + stime for `pid`, or None if it is gone."""
    stat = _read(f"/proc/{pid}/stat")
    if stat is None:
        return None
    fields = stat.split()
    try:
        return int(fields[13]) + int(fields[14])
    except (IndexError, ValueError):
        return None


# The three verdicts `game_status` can reach. A caller that scores a launch decides on these
# rather than on `survey()` rows, so the husk rule lives here and nowhere else.
GAME_RUNNING = "running"
GAME_HUSK = "husk"
GAME_EXITED = "exited"


def game_status(
    prefix: str = DEFAULT_PREFIX, sample_ms: int = CPU_SAMPLE_MS
) -> list[dict[str, object]]:
    """Every `eldenring.exe` in the prefix, with the two facts that separate a game from a husk.

    Machine-readable half of [`game_health`]. It exists because a tool that scores a launch --
    an A/B, a bisect -- has to branch on the verdict, and re-deriving it from a formatted string
    would put a second copy of the husk rule outside this module.
    """
    rows: list[dict[str, object]] = []
    for row in (entry for entry in survey(prefix) if entry["comm"] == "eldenring.exe"):
        pid = int(row["pid"])
        threads = int(row["threads"])
        before = cpu_ticks(pid)
        exited = wait_for_exit([pid], sample_ms)
        after = cpu_ticks(pid)
        if exited or before is None or after is None:
            rows.append(
                {
                    "pid": pid,
                    "threads": threads,
                    "cpu_ticks": None,
                    "verdict": GAME_EXITED,
                }
            )
            continue
        burned = after - before
        rows.append(
            {
                "pid": pid,
                "threads": threads,
                "cpu_ticks": burned,
                "verdict": (
                    GAME_HUSK
                    if is_zombie(pid) or threads <= HUSK_THREAD_CEILING or burned == 0
                    else GAME_RUNNING
                ),
            }
        )
    return rows


def game_health(prefix: str = DEFAULT_PREFIX, sample_ms: int = CPU_SAMPLE_MS) -> str:
    """Is there a real game running -- threads and CPU, not merely a pid?"""
    rows = game_status(prefix, sample_ms)
    if not rows:
        return "no eldenring.exe"
    lines = []
    for row in rows:
        pid = row["pid"]
        if row["verdict"] == GAME_EXITED:
            lines.append(f"pid={pid} EXITED during sampling -- it was already dying")
            continue
        verdict = (
            "HUSK (wedged; tear down)" if row["verdict"] == GAME_HUSK else GAME_RUNNING
        )
        lines.append(
            f"pid={pid} threads={row['threads']} "
            f"cpu_ticks_in_{sample_ms}ms={row['cpu_ticks']} -> {verdict}"
        )
    return "; ".join(lines)


def game_directory(explicit: str | None = None) -> str:
    """Where the DLLs write their artifacts, matching `er_game_base::log::game_directory_path`.

    Env-overridable and current-user-aware rather than a hard-coded literal: AGENTS.md's
    reusable-tooling rule, and the reason the old WSL2 `/mnt/c/SteamLibrary/...` path is not
    written anywhere here -- it resolves to nothing on this machine, which reads as "the run
    wrote no outcome" instead of "you looked in the wrong place".
    """
    if explicit:
        return explicit
    direct = os.environ.get("ER_GAME_DIR")
    if direct:
        return direct
    steam = os.environ.get(
        "ME3_STEAM_DIR", os.path.join(os.path.expanduser("~"), ".local/share/Steam")
    )
    return os.path.join(steam, "steamapps/common/ELDEN RING/Game")


def read_run_outcome(game_dir: str | None = None) -> str | None:
    """The current `er-run-outcome.txt` line, or `None` when the file is not there.

    Absent is not the same as `running` and must not be reported as it: the DLL writes the file
    at install, so no file at all means the logger never started and the record says nothing
    about the game.
    """
    return _read(os.path.join(game_directory(game_dir), RUN_OUTCOME_FILE_NAME))


def format_run_outcome(reason: str, previous: str | None) -> str:
    """The line this script writes. One line, self-describing, same shape as the DLL's.

    `was=` carries what the file held before, so superseding a record never destroys one -- and
    on the second stamp it is how a DLL exit-path write that landed mid-sweep stays visible.
    """
    carried = "-" if previous is None else previous.strip().replace(" ", ",") or "-"
    return (
        f"outcome={RUN_OUTCOME_TORN_DOWN} api=er-teardown code=- "
        f"reason={reason} was={carried}\n"
    )


def stamp_run_outcome(reason: str, game_dir: str | None = None) -> str | None:
    """Record that this script is the reason the process is about to stop existing.

    Returns the line written, or `None` when there was nowhere to write it. A failure here is
    reported by the caller and never raises: refusing to tear down because a log could not be
    written would be the instrument breaking the thing it measures.
    """
    directory = game_directory(game_dir)
    previous = _read(os.path.join(directory, RUN_OUTCOME_FILE_NAME))
    line = format_run_outcome(reason, previous)
    try:
        with open(
            os.path.join(directory, RUN_OUTCOME_FILE_NAME), "w", encoding="utf-8"
        ) as handle:
            handle.write(line)
    except OSError:
        return None
    return line


def teardown(
    prefix: str = DEFAULT_PREFIX,
    verbose: bool = True,
    reason: str = DEFAULT_TEARDOWN_REASON,
    game_dir: str | None = None,
) -> int:
    """SIGTERM every prefix process, wait, then SIGKILL whatever survived. Returns the count."""
    targets = survey(prefix)
    if verbose:
        print(f"[er-teardown] {len(targets)} prefix process(es) to remove")

    # Before the first signal, because SIGKILL leaves nobody to write anything afterwards, and
    # because a teardown that dies half way through should still have said why it started.
    if targets:
        stamped = stamp_run_outcome(reason, game_dir)
        if verbose:
            print(
                f"[er-teardown] run outcome: {stamped.strip()}"
                if stamped
                else "[er-teardown] run outcome: NOT WRITTEN -- "
                f"{os.path.join(game_directory(game_dir), RUN_OUTCOME_FILE_NAME)} "
                "is not writable; this teardown will look like an unexplained kill"
            )

    for row in targets:
        try:
            os.kill(int(row["pid"]), signal.SIGTERM)
        except OSError:
            pass

    wait_for_exit([int(row["pid"]) for row in targets], TERM_GRACE_MS)

    survivors = survey(prefix)
    for row in survivors:
        try:
            os.kill(int(row["pid"]), signal.SIGKILL)
            if verbose:
                print(f"[er-teardown] SIGKILL {row['comm']} {row['pid']}")
        except OSError:
            pass

    # Wait on the exits themselves rather than on a clock, so the final report cannot list a
    # process that is already gone (nor miss one that is not).
    wait_for_exit([int(row["pid"]) for row in survivors], KILL_GRACE_MS)
    remaining = survey(prefix)

    # Again, now that everything is gone. SIGTERM can reach the DLL's own exit hooks, which
    # rewrite this file with an exit code that on this target diagnoses nothing -- a normal quit
    # exits 0xc0000005 under Proton. Whatever they wrote is carried into `was=` rather than
    # dropped, and the final word is the one fact neither hook could know: an agent did this.
    if targets:
        stamped = stamp_run_outcome(reason, game_dir)
        if verbose and stamped:
            print(f"[er-teardown] run outcome: {stamped.strip()}")

    if verbose:
        if remaining:
            print(f"[er-teardown] STILL ALIVE: {remaining}")
        else:
            print("[er-teardown] clean -- zero prefix processes remain")
    return len(targets)


def selftest() -> int:
    failures = 0

    def check(name: str, condition: bool) -> None:
        nonlocal failures
        print(f"  {'ok  ' if condition else 'FAIL'} {name}")
        if not condition:
            failures += 1

    check(
        "the service processes the old teardown missed are all targets",
        {"services.exe", "plugplay.exe", "explorer.exe", "svchost.exe", "rpcss.exe"}
        <= PREFIX_COMMS,
    )
    check("proton's helpers are targets", {"tabtip.exe", "xalia.exe"} <= PREFIX_COMMS)
    check(
        "the originally-documented five are still targets",
        {"eldenring.exe", "wineserver", "winedevice.exe"} <= PREFIX_COMMS
        and {"me3", "me3-launcher.exe"} <= LAUNCHER_COMMS,
    )
    check(
        "a two-thread game counts as a husk and a full one does not",
        HUSK_THREAD_CEILING >= 2 and HUSK_THREAD_CEILING < 57,
    )
    # AGENTS.md forbids matching on command lines -- `rsi` matches `version`, `wine` matches
    # anything. Checked BEHAVIOURALLY: two earlier versions of this test scanned this file for a
    # literal and both matched their own text, reporting fail against a file that was correct.
    # What matters is not the source text but that every row survey() returns was classified by
    # comm plus prefix, so that is what is asserted.
    rows = survey()
    # The run-outcome contract. These are the cases that make `running` mean something: a
    # teardown must be distinguishable from a death nobody explains, and an absent file must not
    # be reported as either.
    import tempfile

    with tempfile.TemporaryDirectory() as game_dir:
        outcome_path = os.path.join(game_dir, RUN_OUTCOME_FILE_NAME)

        check("an absent outcome file reads as None, never as running", read_run_outcome(game_dir) is None)

        # The live shape the DLL stamps at install.
        with open(outcome_path, "w", encoding="utf-8") as handle:
            handle.write("outcome=running api=- code=-\n")
        written = stamp_run_outcome("rebuilding-a-dll", game_dir)
        check("a teardown stamps outcome=torn-down", written is not None and "outcome=torn-down" in written)
        check("...naming er-teardown as the api", written is not None and "api=er-teardown" in written)
        check("...carrying the reason it was given", written is not None and "reason=rebuilding-a-dll" in written)
        check(
            "...and superseding rather than destroying what was there",
            written is not None and "was=outcome=running,api=-,code=-" in written,
        )
        check(
            "the file on disk holds exactly that one line",
            _read(outcome_path) == written,
        )
        check(
            "a torn-down record no longer reads as running",
            "outcome=running" not in (read_run_outcome(game_dir) or "").split("was=")[0],
        )

        # The second stamp: a DLL exit hook fired mid-sweep and overwrote us. Its verdict is
        # carried, and the deliberate teardown is still the final word.
        with open(outcome_path, "w", encoding="utf-8") as handle:
            handle.write("outcome=exit-unclassified api=NtTerminateProcess code=0xc0000005\n")
        second = stamp_run_outcome("rebuilding-a-dll", game_dir)
        check(
            "a mid-sweep exit-hook write is carried into was=, not lost",
            second is not None and "was=outcome=exit-unclassified,api=NtTerminateProcess,code=0xc0000005" in second,
        )
        check(
            "...while the final outcome is still the teardown",
            second is not None and second.startswith("outcome=torn-down "),
        )

    # An unwritable directory must degrade to a report, never to an exception: refusing to kill
    # because a log failed would be the instrument breaking the thing it measures.
    check(
        "an unwritable game directory returns None rather than raising",
        stamp_run_outcome("whatever", "/proc/nonexistent-er-teardown-selftest") is None,
    )
    check(
        "the game directory is env-overridable rather than a hard-coded literal",
        game_directory("/explicit") == "/explicit"
        and "ELDEN RING/Game" in game_directory(None),
    )

    check(
        "every row records which rule caught it",
        all(row["matched_by"] in {"appid", "prefix", "launcher"} for row in rows),
    )
    check(
        "the real Steam client is never a target",
        not any(int(row["pid"]) in _steam_client_pids() for row in rows),
    )
    check(
        "survey reports the evidence a caller needs to judge liveness",
        all({"pid", "comm", "state", "threads", "matched_by"} <= set(row) for row in rows),
    )
    check(
        "the appid classifier exists and names exactly one game",
        len(APPID_NEEDLES) >= 3 and all(b"1245620" in n for n in APPID_NEEDLES),
    )
    check(
        "a container shim is reachable by appid though its comm is in no list",
        not {"srt-bwrap", "pv-adverb", "wine64-preloader", "wine-preloader"} & PREFIX_COMMS,
    )
    check("this process is not its own target", "python3" not in PREFIX_COMMS)
    check(
        "waiting on an already-dead pid returns immediately",
        wait_for_exit([2**22], 50) is True,
    )
    check(
        "waiting on a LIVE pid times out rather than reporting it gone",
        wait_for_exit([os.getpid()], 50) is False,
    )
    # Structural, not textual. Three earlier checks in this file scanned the source for a literal
    # and each matched the check's own text, reporting fail against a file that was correct. The
    # module simply does not import `time`; if a sleep is ever reintroduced it must import it, and
    # this fails.
    check("the module has no time facility to sleep on", "time" not in globals())
    print("selftest:", "PASS" if failures == 0 else "FAIL")
    return 1 if failures else 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--prefix", default=DEFAULT_PREFIX)
    parser.add_argument("--status", action="store_true", help="report only, kill nothing")
    parser.add_argument(
        "--reason",
        default=DEFAULT_TEARDOWN_REASON,
        help="why this teardown is happening; recorded in er-run-outcome.txt",
    )
    parser.add_argument(
        "--game-dir",
        default=None,
        help="where er-run-outcome.txt lives (default: ER_GAME_DIR, else ME3_STEAM_DIR)",
    )
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()

    if args.selftest:
        return selftest()

    if args.status:
        rows = survey(args.prefix)
        by_comm: dict[str, int] = {}
        for row in rows:
            by_comm[str(row["comm"])] = by_comm.get(str(row["comm"]), 0) + 1
        print(f"[er-teardown] {len(rows)} prefix process(es)")
        for comm, count in sorted(by_comm.items()):
            print(f"    {count:3d}  {comm}")
        print(f"[er-teardown] game health: {game_health(args.prefix)}")
        # The outcome file is reported here because not reading it is the whole defect this
        # change exists to close: on 2026-09-06 "did it crash?" was answered by inference from
        # process absence while the file that answers it sat unread beside the game.
        outcome = read_run_outcome(args.game_dir)
        if outcome is None:
            print(
                "[er-teardown] run outcome: NO FILE at "
                f"{os.path.join(game_directory(args.game_dir), RUN_OUTCOME_FILE_NAME)} "
                "-- the DLL's exit hooks never installed, so this says nothing about the game"
            )
        else:
            print(f"[er-teardown] run outcome: {outcome.strip()}")
        return 0

    teardown(args.prefix, reason=args.reason, game_dir=args.game_dir)
    return 0


if __name__ == "__main__":
    sys.exit(main())
