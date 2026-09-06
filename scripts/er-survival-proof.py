#!/usr/bin/env python3
"""Watch a live ELDEN RING for a measured window and report whether it survived.

WHAT THIS PROVES, AND WHY F9 IS NOT IN THE NAME
------------------------------------------------
This tool was called `er-f9-loop-proof` because the claim it was built for was "repeatedly F9-load
for N minutes without crashing". The measurement killed that framing: the 0x140010043 fault is
TIME-triggered, not action-triggered. Five recorded faults land at 43.8-56.1s regardless of what
was happening, and one of them arrived with nothing pressed at all. So the press count is not the
independent variable and never was -- the DLL combination and the wall-clock are. A name that puts
F9 at the centre advertises a causal role the evidence does not support, and would have the next
reader tuning a cadence that changes nothing.

So the default is to press NOTHING and watch. `--warp` remains, because the OTHER failure this
build has to survive -- a main-thread hard lock inside me3's own infinite mutex -- has only ever
been seen AFTER a completed map jump. The independent variable there is the WARP, not the key: F9
is merely what `er-hotkey-config` happens to bind it to, and a run's metric is therefore warps
COMPLETED, read from the DLL's own `ARRIVED` line. A press count says how hard the driver leaned on
a key; only the arrival count says the feature ran.

THE VERDICT IS THE PROCESS AND THE RECORDS, NEVER THE SCREEN
------------------------------------------------------------
Stop/continue comes from RAM/process telemetry: the game's own liveness, the crash logger's record
count, a `PANIC in` line, and the watchdog's main-thread-stall HANG report -- a lockup writes no
exception record at all, so the record count alone is blind to it. No screenshot is taken and none
would be trusted; AGENTS.md forbids a visual oracle as the run-stopping signal.

EVERY WAIT BLOCKS ON AN EVENT. Readiness is the game's own `player_present` telemetry, the warp
cadence is the DLL's own `ARRIVED` line, focus is Hyprland's `.socket2.sock`, and the file waits
are inotify. Nothing here sleeps, and the only durations are backstops: the watch window (derived
from the worst recorded time-to-first-fault) and the repo's canonical runtime cap.

F9 IS REACHABLE FROM OS-LEVEL INJECTION when it is used, and that is a fact about this specific key
rather than a general licence. AGENTS.md is explicit that synthesized OS input does NOT reach
native ELDEN RING bindings, which the game reads through DirectInput/XInput. `er_invasion_warp`'s
warp hotkeys are not native bindings: `er-hotkey-config` polls them with `GetAsyncKeyState` (see
`keys.rs`, `VK_F9 == 0x78`). A uinput key event is a real kernel input event, so it reaches the
compositor, then Wine, then exactly the state `GetAsyncKeyState` reads. This driver is therefore
correct for the warp keys and would be WRONG for anything the game itself binds.

WINDOW TARGETING IS FAIL-CLOSED AND NARROW when pressing, and here that is a safety property: a
uinput press is system-wide, so an unfocused game means every press lands in whatever the user
actually has in front of them. Focus is asserted by CLASS and confirmed before each press; nothing
else about the desktop is enumerated or printed -- the privacy rule in AGENTS.md exists because a
window list exposes every unrelated app the user is running.

USAGE
    python3 scripts/er-survival-proof.py --crash-log-dir <dir> --await-player --seconds 600
    python3 scripts/er-survival-proof.py --crash-log-dir <dir> --warp
    python3 scripts/er-survival-proof.py --selftest
"""

from __future__ import annotations

import argparse
import functools
import json
import os
import re
import select
import socket
import subprocess
import sys
import time
from pathlib import Path

import er_run_lib

REPO_ROOT = Path(__file__).resolve().parent.parent
RUN_STATE_ROOT = Path.home() / ".cache" / "er-me3-runs"
CRASH_LOG_NAME = "er-crash-log.txt"

# Linux input event code for F9 (`KEY_F9`), which is what `ydotool` speaks. The chain is
# uinput -> the compositor -> Wine -> `GetAsyncKeyState`, where `er-hotkey-config` reads it as
# `VK_F9 == 0x78`.
#
# WHY NOT `xdotool`. This session is Wayland (`XDG_SESSION_TYPE=wayland`) and the game presents no
# X11 window: searching `--class steam_app_1245620`, `eldenring` and `ELDEN` all returned nothing
# while the game was running. `xdotool` had nothing to target, so the first attempt at this proof
# refused rather than pressing keys into whatever else was focused. `ydotool` injects at
# `/dev/uinput`, below the display server, so it is indifferent to X11 vs Wayland.
KEY_F9 = 67

# Injection at uinput is SYSTEM-WIDE: it goes to whatever holds focus, not to a window handle. That
# makes focusing the game a correctness precondition, not a convenience -- an unfocused game means
# every press lands in somebody else's application. Focus is asserted by CLASS through the
# compositor, and the check reads only whether the active window IS that class; no other window is
# ever named, printed or enumerated (AGENTS.md's privacy rule on window lists).
GAME_HYPR_CLASS = "steam_app_1245620"

# Bounded like every other agent-shell op. The GAME window is bounded separately by `--seconds`,
# which is the thing being measured; these are the little subprocesses around it.
# The share of attempted presses that must actually reach the game for the run to mean anything.
# ONE PRESS PER COMPLETED LOAD, not one press per tick.
#
# A cross-area warp is a full map load and takes roughly 20-30 seconds to land. Pressing on a fixed
# 6-second interval therefore fires four or five times INTO a load that is already running: the
# first 90 seconds of one run produced 8 warps, which is hammering the key rather than exercising
# "repeatedly F9-LOAD". Each press now waits for its own `ARRIVED` line before the next one is
# sent, so the press count and the completed-load count are the same number and the cadence is the
# game's, not a timer's.
ARRIVE_MARKER = "invasion-warp: ARRIVED"
# How long one warp may take to land before the run calls it a stall. Generous against a measured
# ~20-30s so a slow area load is not scored as a failure.
ARRIVE_TIMEOUT_SECONDS = 60.0
ARRIVE_POLL_SECONDS = 1.0

# The share of warps that must actually COMPLETE for a warp run to mean anything. Applied to
# arrivals, not to keystrokes: a run whose presses all landed but whose warps never did has
# exercised a hotkey, not a map jump, and the failure under test lives in the map jump.
MIN_DELIVERED_FRACTION = 0.8

SUBPROCESS_TIMEOUT = 10

# Every print flushes. Python buffers stdout when it is not a tty, and this tool's whole output is
# a progress line followed by a verdict up to three minutes later -- so without this the caller sees
# an empty file and cannot tell a driving run from a hung one.
print = functools.partial(print, flush=True)  # noqa: A001 - deliberate module-local shadow

# When THIS driver started, in wall-clock. Anything on disk older than this belongs to a previous
# run: the telemetry file, the crash log and the hang report all persist between launches.
PROCESS_STARTED = time.time()

MS_SINCE_INSTALL = re.compile(r"(?m)^ms_since_install=(\d+)$")

# The margin over the worst fault ever recorded. A multiplier, not an added constant, so it scales
# if the fault window ever moves.
SURVIVAL_MARGIN = 1.6
# Refuse rather than guess when there is no history to derive from.
MIN_SAMPLES_FOR_DERIVED_WINDOW = 3


def measured_fault_window() -> tuple[float | None, int]:
    """(worst recorded time-to-first-fault in seconds, sample count), read off disk.

    WHY THIS IS COMPUTED AND NOT TYPED. Every duration in this tool used to be a number inherited
    from prose -- "3 minutes" came from a goal statement written before anyone knew the fault was
    time-boxed, and a 62-second boot time was asserted from two 25-second timeouts that could only
    ever bound it from below. Both were wrong by more than an order of magnitude in the direction
    that wastes the user's wall-clock on every arm of a bisect, and both survived being written
    down because a typed constant looks exactly like a measured one.
    So the watch length is DERIVED: read `ms_since_install` out of every crash record on disk, take
    the worst, and add a margin. If the fault window moves, this moves with it, and if there is no
    history it refuses instead of inventing a number.
    """
    # PER FAULT ADDRESS, because a window derived across different bugs is not a window for any of
    # them. The first cut of this pooled every record and returned 721s -- inflated 14x by a single
    # unrelated fault at 0x141ebb799 that landed 450s in, which would have made every arm of the
    # bisect twelve minutes long for no reason. Records are grouped by `exception_address` and the
    # most frequent address wins, that being the fault actually under investigation.
    by_address: dict[str, list[float]] = {}
    for run_dir in RUN_STATE_ROOT.glob("*/"):
        log = run_dir / CRASH_LOG_NAME
        if not log.is_file():
            continue
        text = log.read_text(encoding="utf-8", errors="replace")
        for block in re.split(r"(?m)^reason=", text):
            when = MS_SINCE_INSTALL.search(block)
            where = re.search(r"(?m)^exception_address=(\S+)", block)
            if when and where:
                key = where.group(1).split("{")[0]
                by_address.setdefault(key, []).append(int(when.group(1)) / 1000.0)
    if not by_address:
        return None, 0
    address, values = max(by_address.items(), key=lambda kv: len(kv[1]))
    if len(values) < MIN_SAMPLES_FOR_DERIVED_WINDOW:
        return None, len(values)
    return max(values), len(values)


# WHAT COUNTS AS A FAILURE, AND WHAT IS JUST WINE BEING WINE. The crash logger writes a record for
# every FIRST-CHANCE exception it sees, and a live ELDEN RING under Proton raises those constantly
# while surviving them: one measured 10-minute run with the full mod set logged 23 x 0xc0000026
# (STATUS_INVALID_UNWIND_TARGET) and one 0xc0000005 at `ersc.dll+0x258da` with `rcx=0`, all
# `reason=veh-first-chance-exception`, and the process was still running with 127 threads
# afterwards. Scoring on the raw record count called that a crash and tore down a healthy game.
#
# The distinction the logger already draws is the reason line: `unhandled-exception-fatal` is
# written by the top-level filter and means the process died on it. That is the failure. The
# first-chance count is kept and reported, because a RISE in it is diagnostic even though its
# presence is not a verdict.
FATAL_RECORD = re.compile(r"(?m)^reason=unhandled-exception-fatal$")
RECORD_INDEX = re.compile(r"(?m)^record_index=")
PANIC_LINE = re.compile(r"(?m)^.*PANIC in .*$")


# `er-teardown.py --status` samples CPU over a window before it will call a process alive, so it
# is slower than the compositor calls and gets its own bound. Both are module constants rather than
# a parameter because `scripts/check-no-timeouts.py` resolves the bound statically -- a
# `timeout=timeout` parameter is unbounded as far as the gate can see, and it is right about that:
# nothing stops a caller passing an hour.
TEARDOWN_STATUS_TIMEOUT = 25


def run(args: list[str]) -> subprocess.CompletedProcess:
    return subprocess.run(
        args, capture_output=True, text=True, timeout=SUBPROCESS_TIMEOUT, check=False
    )


def run_status(args: list[str]) -> subprocess.CompletedProcess:
    return subprocess.run(
        args, capture_output=True, text=True, timeout=TEARDOWN_STATUS_TIMEOUT, check=False
    )


# How long to let focus settle after asking for it. A single dispatch-then-read raced the
# compositor and reported failure 17 times out of 20 in the first 180s run, which cost that run its
# presses: only 3 landed and only 2 warps happened, so it proved nothing about repeated F9 loading
# even though the game survived.
#
# The settle is spent BLOCKED ON HYPRLAND'S OWN EVENT STREAM, not on a poll: `.socket2.sock`
# emits `activewindow>>class,title` the instant focus changes, so this wakes on the compositor's
# own statement rather than re-asking it four times a second. The seconds below are a backstop for
# the case where the window never takes focus at all, not the synchronisation.
FOCUS_SETTLE_SECONDS = 2.0

# A wait that has no file or socket event to key on still has to notice a process that DIED, and a
# death emits nothing. This is the re-check backstop for those waits -- an upper bound on how long
# a dead game goes unnoticed, never the thing being waited for.
LIVENESS_RECHECK_SECONDS = 2.0


def hypr_event_socket() -> Path | None:
    """Hyprland's event stream, or None if this is not a Hyprland session."""
    runtime = os.environ.get("XDG_RUNTIME_DIR")
    signature = os.environ.get("HYPRLAND_INSTANCE_SIGNATURE")
    if not runtime or not signature:
        return None
    path = Path(runtime) / "hypr" / signature / ".socket2.sock"
    return path if path.exists() else None


def active_is_game() -> bool:
    """Whether the focused window is the game. Reads only its class, never reports another."""
    result = run(["hyprctl", "activewindow", "-j"])
    if result.returncode != 0:
        return False
    try:
        active = json.loads(result.stdout or "{}")
    except json.JSONDecodeError:
        return False
    # Only the class is compared, and only to our own constant. Nothing about the active window is
    # returned or printed when it is NOT the game -- that window belongs to the user, not to this
    # proof.
    return active.get("class") == GAME_HYPR_CLASS


def focus_game() -> bool:
    """Focus the ELDEN RING window by class and WAIT for the compositor to agree.

    Also switches to the window's workspace: `focuswindow` alone does not follow a window that
    lives on another workspace, and a window the user has left on a different workspace is the
    ordinary case while an agent drives a long run.
    """
    if active_is_game():
        return True
    # THE NEW LUA DISPATCHER API, not the classic string form. This Hyprland parses the argument
    # as Lua, so `hyprctl dispatch focuswindow class:...` fails with a Lua syntax error rather than
    # a dispatcher error -- which reads like "the window is missing" and is not. That error was
    # swallowed for a whole 180s run: focus never took, 17 of 20 presses were refused, and the run
    # returned a PASS it had not earned. There is no `focus` under `hl.dsp.window`; the focus
    # dispatcher is top-level and takes a table (`hl.dsp.focus{...}` with one of direction,
    # monitor, window, urgent_or_last, last).
    # Subscribe BEFORE dispatching. Connecting afterwards races the very event being waited for:
    # a window that takes focus in under a millisecond would emit `activewindow` before the socket
    # existed, and the wait would then sit out its whole backstop having already succeeded.
    stream = hypr_event_socket()
    connection: socket.socket | None = None
    if stream is not None:
        try:
            connection = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            connection.connect(str(stream))
            connection.setblocking(False)
        except OSError:
            connection = None
    try:
        run(
            [
                "hyprctl",
                "dispatch",
                f'hl.dsp.focus{{window="class:{GAME_HYPR_CLASS}"}}',
            ]
        )
        deadline = time.monotonic() + FOCUS_SETTLE_SECONDS
        while True:
            if active_is_game():
                return True
            remaining = deadline - time.monotonic()
            if remaining <= 0 or connection is None:
                return False
            try:
                ready, _, _ = select.select([connection], [], [], remaining)
            except OSError:
                return False
            if ready:
                try:
                    connection.recv(65536)  # drain; the state is re-read above, not parsed here
                except OSError:
                    return False
    finally:
        if connection is not None:
            connection.close()


def game_window_id() -> str | None:
    """Kept as the driver's precondition: a focused game window, or None."""
    return GAME_HYPR_CLASS if focus_game() else None


def load_teardown_module():
    """The repo's own process survey, imported rather than shelled out to.

    `er-teardown.py --status` samples CPU for 3 seconds before it will call a process alive. That
    is the right answer for a one-shot verdict and the wrong one inside a wait loop, where it would
    dominate the cadence being measured. Importing gives the cheap half (`survey`, which reads
    /proc) alongside the expensive half, so the loop can use a pid and the verdict can use CPU.
    """
    import importlib.util

    spec = importlib.util.spec_from_file_location(
        "er_teardown", REPO_ROOT / "scripts" / "er-teardown.py"
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def game_pid() -> int | None:
    """The running game's pid, by the repo's own definition of running -- threads AND CPU."""
    try:
        rows = load_teardown_module().game_status()
    except Exception:
        return None
    for row in rows:
        if row.get("verdict") == "running":
            return int(row["pid"])
    return None


def game_alive() -> bool:
    """Process liveness via the repo's own teardown status, never a raw process-name grep.

    A raw `pgrep` is both guard-denied here and blind to the Proton container stack.
    """
    result = run_status(
        [sys.executable, str(REPO_ROOT / "scripts" / "er-teardown.py"), "--status"]
    )
    return "-> running" in (result.stdout + result.stderr)


def first_chance_records(run_dir: Path) -> int:
    """Every record the logger wrote, fatal or not. Diagnostic only, never a verdict."""
    log = run_dir / CRASH_LOG_NAME
    if not log.is_file():
        return 0
    return len(RECORD_INDEX.findall(log.read_text(encoding="utf-8", errors="replace")))


def crash_evidence(run_dir: Path) -> tuple[int, list[str]]:
    """(FATAL fault records, panic lines seen) for this run -- the run-stopping oracle."""
    records = 0
    log = run_dir / CRASH_LOG_NAME
    if log.is_file():
        records = len(FATAL_RECORD.findall(log.read_text(encoding="utf-8", errors="replace")))
    panics: list[str] = []
    warp_log = er_run_lib.game_dir() / "er-invasion-warp.log"
    if warp_log.is_file():
        panics = PANIC_LINE.findall(warp_log.read_text(encoding="utf-8", errors="replace"))
    return records, panics


# A LOCKUP WRITES NO EXCEPTION RECORD. The crash logger's watchdog reports it separately, into
# this file beside the executable -- so a run that hangs rather than faults is invisible to the
# record count alone, which is exactly the failure this driver was blind to until a hard lock
# produced a 56 KB hang report the driver never looked at.
HANG_REPORT_NAME = "er-crash-hang-latest.txt"


def hang_report_state() -> tuple[bool, int, float]:
    """(exists, size, mtime) for the hang report -- baselined, because a STALE one is not this run's."""
    report = er_run_lib.game_dir() / HANG_REPORT_NAME
    try:
        stat = report.stat()
    except OSError:
        return False, 0, 0.0
    return True, stat.st_size, stat.st_mtime


# THE READINESS SIGNAL FOR "THE CHARACTER IS IN THE WORLD". Watching from the title screen would
# score the boot, not the build: the fault window is measured from DLL install, and a run that
# spends four of its ten minutes on a loading screen is watching a different program.
#
# The field is `player_available`, which `er_quickload` writes from a RAM read each frame and which
# means the player object EXISTS RIGHT NOW. Not `player_seen`, which is sticky once the world has
# ever been reached, and not `player_present`, which does not exist -- a first cut of this waiter
# keyed on that invented name, found it missing from every snapshot, and sat out its entire backstop
# beside a game that had been in the world for minutes.
PLAYER_TELEMETRY_NAME = "er-quickload-telemetry.json"


def player_present(newer_than: float) -> tuple[bool, str | None]:
    """(is the player in the world, which character) from THIS run's telemetry, or (False, None)."""
    telemetry = er_run_lib.game_dir() / PLAYER_TELEMETRY_NAME
    try:
        if telemetry.stat().st_mtime < newer_than:
            return False, None
        data = json.loads(telemetry.read_text(encoding="utf-8", errors="replace"))
    except (OSError, json.JSONDecodeError):
        return False, None
    return bool(data.get("player_available")), data.get("oracle_char_name")


def runtime_cap_seconds() -> float:
    """The repo's canonical idle/stall backstop, read from its single source of truth.

    Used to bound the wait for the character to load. It is a backstop, not the synchronisation --
    the wait ends on the telemetry, and this only stops it waiting forever on a boot that hung.
    """
    result = run([sys.executable, str(REPO_ROOT / "scripts" / "runtime_timeout_cap.py")])
    return float(result.stdout.strip())


def arrivals(game_dir: Path) -> int:
    """How many completed warps this run has logged. The progress oracle, read from the DLL."""
    log = game_dir / "er-invasion-warp.log"
    if not log.is_file():
        return 0
    return log.read_text(encoding="utf-8", errors="replace").count(ARRIVE_MARKER)


def press_f9(_window: str) -> bool:
    """One F9 down/up at uinput. Re-asserts focus first, because a press that lands elsewhere is
    both useless as evidence and rude to whatever is actually focused."""
    if not focus_game():
        return False
    result = run(["ydotool", "key", f"{KEY_F9}:1", f"{KEY_F9}:0"])
    return result.returncode == 0


def drive(
    run_id: str | None,
    seconds: float,
    interval: float,
    crash_log_dir: str | None = None,
    no_input: bool = False,
    await_player: bool = False,
) -> int:
    run_dir = Path(crash_log_dir) if crash_log_dir else RUN_STATE_ROOT / (run_id or "")
    run_id = run_id or str(run_dir)
    if not run_dir.is_dir():
        print(f"er-survival-proof: no such crash-log directory: {run_dir}")
        return 2
    game_dir = er_run_lib.game_dir()
    if await_player:
        started_waiting = time.monotonic()
        cap = runtime_cap_seconds()
        boot_watch = er_run_lib.WatchSet([game_dir])
        try:
            if not boot_watch.available:
                print("er-survival-proof: inotify unavailable; refusing to poll for readiness")
                return 2
            deadline = started_waiting + cap
            character = None
            while True:
                ready, character = player_present(PROCESS_STARTED)
                if ready:
                    break
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    print(
                        f"er-survival-proof: the character never loaded within the repo's "
                        f"{cap:g}s runtime backstop -- nothing to drive"
                    )
                    return 2
                boot_watch.wait(min(LIVENESS_RECHECK_SECONDS, remaining))
        finally:
            boot_watch.close()
        print(
            f"er-survival-proof: character {character!r} is loaded after "
            f"{time.monotonic() - started_waiting:.1f}s -- starting the drive now"
        )

    # FOCUS AFTER THE CHARACTER LOADS, NEVER BEFORE. The window does not exist yet at launch, so
    # asking for it first refuses a run that was only a few seconds early -- measured 2026-09-04,
    # a relaunch was aborted at t+0 with "could not focus an ELDEN RING window" while the game was
    # still booting perfectly well.
    window = GAME_HYPR_CLASS if no_input else game_window_id()
    if window is None:
        print(
            f"er-survival-proof: could not focus an ELDEN RING window of class "
            f"{GAME_HYPR_CLASS!r} -- refusing to inject, because a uinput press goes to whatever "
            f"IS focused, which would be someone else's application"
        )
        return 2

    pid = game_pid()
    if pid is None:
        print("er-survival-proof: the game is not running; nothing to drive")
        return 2

    # EVERY WAIT IN THIS LOOP BLOCKS ON AN EVENT. The two things worth waking for both arrive as
    # writes to a file -- the DLL's `ARRIVED` line, and the crash logger's record -- and they land
    # in two different trees, so both are watched at once. If inotify is unavailable this refuses
    # rather than degrading to a poll: a poll is a sleep with extra steps, `scripts/check-no-
    # timeouts.py` rejects it, and it would silently change what the cadence is measuring.
    watch = er_run_lib.WatchSet([run_dir, game_dir])
    if not watch.available:
        print(
            "er-survival-proof: inotify is unavailable, so there is no readiness primitive to wait "
            "on -- refusing to fall back to polling"
        )
        return 2

    # A hang report from an EARLIER run is not this run's evidence. Baseline it, and treat any
    # change -- appearing, growing, being rewritten -- as this run's failure.
    hang_before = hang_report_state()

    def verdict_now() -> tuple[str | None, int, list[str]]:
        """The run-stopping oracle: the first failure visible right now, or None.

        Ordered cheapest-and-most-specific first. Liveness is last because it is the one that
        needs a syscall per thread, and because a fault record explains a death better than the
        death does.
        """
        records, panics = crash_evidence(run_dir)
        if records:
            return f"{records} FATAL record(s) in {CRASH_LOG_NAME}", records, panics
        if panics:
            return f"a PANIC line in the warp log: {panics[0].strip()}", records, panics
        if hang_report_state() != hang_before:
            return (
                "the crash logger wrote a main-thread-stall HANG report -- the game locked up "
                "rather than faulting, which writes no exception record at all",
                records,
                panics,
            )
        if not er_run_lib.process_alive(pid):
            return (
                "the process is gone -- a HARD KILL: no fault record and no panic line, so it "
                "bypassed both the vectored handler and DllMain detach, which is the signature "
                "of abort/__fastfail",
                records,
                panics,
            )
        return None, records, panics

    def wait_until(predicate, until: float) -> bool:
        """Block on the watched directories until `predicate` holds or `until` passes.

        `WatchSet.wait` is a `select` over inotify fds, so this consumes no CPU while it waits and
        wakes on the write itself. The per-iteration bound exists only so a process that DIED --
        which emits no file event at all -- is still noticed.
        """
        while True:
            if predicate():
                return True
            remaining = until - time.monotonic()
            if remaining <= 0:
                return False
            watch.wait(min(LIVENESS_RECHECK_SECONDS, remaining))

    started = time.monotonic()
    presses = 0
    failed_presses = 0
    arrived = 0
    stalls = 0
    deadline = started + seconds
    how = "watching (no input)" if no_input else "driving warps"
    print(
        f"er-survival-proof: {how} into pid {pid} for {seconds:g}s (run {run_id}), "
        f"oracle={run_dir}"
    )

    try:
        while time.monotonic() < deadline:
            if no_input:
                # Watch-only: the fault under investigation is TIME-triggered, so surviving the
                # window with nothing pressed is the whole claim and a press would only add noise.
                if wait_until(lambda: verdict_now()[0] is not None, deadline):
                    break
                continue

            before = arrivals(game_dir)
            if press_f9(window):
                presses += 1
            else:
                failed_presses += 1
            if verdict_now()[0] is not None:
                break
            # ONE PRESS PER COMPLETED LOAD. A cross-area warp is a full map load; pressing again
            # before it lands is hammering the key, not "repeatedly F9-LOAD". The cadence is the
            # game's own `ARRIVED` line, and `interval` is only a floor under it.
            settle = min(time.monotonic() + ARRIVE_TIMEOUT_SECONDS, deadline)
            landed = wait_until(
                lambda: arrivals(game_dir) > before or verdict_now()[0] is not None, settle
            )
            if verdict_now()[0] is not None:
                break
            if landed:
                arrived += 1
            else:
                stalls += 1
            if interval > 0:
                floor = min(time.monotonic() + interval, deadline)
                wait_until(lambda: verdict_now()[0] is not None, floor)
                if verdict_now()[0] is not None:
                    break
    finally:
        watch.close()

    elapsed = time.monotonic() - started
    reason, records, panics = verdict_now()
    if reason is not None:
        print(
            f"er-survival-proof: FAILED after {elapsed:.1f}s and {presses} press(es) -- {reason}"
        )
        for line in panics[:3]:
            print(f"  {line.strip()}")
        return 1

    alive = er_run_lib.process_alive(pid)
    # SURVIVING IS NOT PASSING. The claim is "repeatedly F9-load for N minutes", so a run whose
    # presses never landed proves only that an idle game does not crash. The first 180s run
    # returned PASS on 3 delivered presses out of 20 -- 2 warps in three minutes -- which is not
    # the thing being claimed. A run must deliver most of its presses to say anything at all.
    # SURVIVING A WARP RUN IS NOT PASSING IT. The claim a warp run makes is "N completed map jumps
    # and no lock", so the gate is arrivals -- warps the DLL itself said it finished. Scoring on
    # presses instead would pass a run whose every keystroke landed and whose every warp stalled,
    # which is the exact run that proves nothing about the failure being chased.
    attempted_warps = arrived + stalls
    delivered_enough = no_input or (
        attempted_warps > 0 and arrived >= (attempted_warps * MIN_DELIVERED_FRACTION)
    )
    ok = alive and records == 0 and not panics and delivered_enough
    print(
        json.dumps(
            {
                "verdict": "PASS" if ok else "FAIL",
                "run_id": run_id,
                "seconds_driven": round(elapsed, 1),
                "f9_presses": presses,
                "failed_presses": failed_presses,
                "fatal_records": records,
                "first_chance_records": first_chance_records(run_dir),
                "panic_lines": len(panics),
                "hang_report": hang_report_state() != hang_before,
                "process_alive": alive,
                "warps_completed": arrived,
                "warps_that_never_landed": stalls,
                "warp_completion_fraction": (
                    round(arrived / attempted_warps, 2) if attempted_warps else 0.0
                ),
                "keys_delivered": presses,
                "verdict_reason": (
                    "survived and delivered its presses"
                    if ok
                    else (
                        "the game survived but too few warps COMPLETED -- this proves nothing "
                        "about the failure that follows a map jump"
                        if alive and records == 0 and not panics
                        else "faulted or died"
                    )
                ),
            },
            indent=1,
        )
    )
    return 0 if ok else 1


def selftest() -> int:
    failures = 0

    def check(condition: bool, label: str) -> None:
        nonlocal failures
        print(f"  {'ok  ' if condition else 'FAIL'} {label}")
        if not condition:
            failures += 1

    check(
        len(RECORD_INDEX.findall("reason=x\nrecord_index=0\nrecord_index=1\n")) == 2,
        "the first-chance counter counts records, not bytes",
    )
    check(
        RECORD_INDEX.findall("build git=abc module=er_crash_logging.dll\n") == [],
        "a crash log holding only its build header counts as ZERO records",
    )
    check(
        len(PANIC_LINE.findall("er-invasion-warp: PANIC in er-invasion-warp at src/x.rs:1:2: boom"))
        == 1,
        "a panic line is detected",
    )
    check(
        PANIC_LINE.findall("panic reporter ARMED for er_invasion_warp") == [],
        "the ARMED line is NOT mistaken for a panic (it is the opposite claim)",
    )
    check(
        subprocess.run(["ydotool", "key", "0:0"], capture_output=True, timeout=5).returncode == 0,
        "ydotool can reach its daemon (uinput path, works under Wayland)",
    )
    check(KEY_F9 == 67, "F9 is KEY_F9 = 67 in Linux input event codes")
    check(
        hypr_event_socket() is not None,
        "Hyprland's event socket is reachable (the focus wait blocks on it, never on a poll)",
    )
    watch = er_run_lib.WatchSet([REPO_ROOT / "scripts"])
    try:
        check(watch.available, "inotify is available (every wait in the drive loop blocks on it)")
        check(
            watch.wait(0.05) is False,
            "a quiet directory times out rather than reporting a phantom event",
        )
    finally:
        watch.close()
    check(
        hang_report_state() == hang_report_state(),
        "the hang-report baseline is stable when nothing writes it",
    )
    check(
        len(FATAL_RECORD.findall("reason=unhandled-exception-fatal\n")) == 1,
        "a fatal record is recognised",
    )
    check(
        FATAL_RECORD.findall("reason=veh-first-chance-exception\n") == [],
        "a SURVIVED first-chance exception is NOT scored as a crash -- the failure this fixes",
    )
    check(
        er_run_lib.process_alive(0) is False,
        "process liveness reads /proc and rejects a nonexistent pid",
    )
    snapshot = er_run_lib.game_dir() / PLAYER_TELEMETRY_NAME
    check(
        (not snapshot.is_file())
        or "player_available"
        in snapshot.read_text(encoding="utf-8", errors="replace"),
        "the readiness field this waits on is a field the DLL actually writes",
    )
    print("selftest: " + ("PASS" if failures == 0 else "FAIL"))
    return 1 if failures else 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--run-id", help="the er-run-branch run whose artifacts hold the oracle")
    parser.add_argument(
        "--crash-log-dir",
        help=(
            "read the oracle from this directory instead of a run directory. Needed for the "
            "AUTOLOAD route: that launches through ~/Elden/launch.sh, which sets none of the "
            "ER_QUICKLOAD_CRASH_LOGGING_*_PATH redirects, so the records land beside the "
            "executable in the game directory rather than in a per-run directory."
        ),
    )
    parser.add_argument(
        "--seconds",
        type=float,
        default=None,
        help=(
            "how long to watch. DEFAULTS TO A MEASURED VALUE: the worst time-to-first-fault in "
            "every crash record on disk, times a margin. Pass a number only to override that "
            "deliberately, and say why."
        ),
    )
    parser.add_argument("--interval", type=float, default=6.0)
    parser.add_argument(
        "--warp",
        action="store_true",
        help=(
            "drive map jumps, one per completed load, and score the run on warps that LANDED. "
            "OFF by default: the 0x140010043 fault is TIME-triggered and needs no input at all. "
            "Use this for the other failure -- the main-thread hard lock, which has only ever "
            "followed a completed warp."
        ),
    )
    parser.add_argument(
        "--await-player",
        action="store_true",
        help=(
            "block until the game's own telemetry says the character is loaded before driving "
            "anything. Bounded by the repo's canonical runtime backstop, never by a typed wait."
        ),
    )
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    if args.selftest:
        return selftest()
    if not args.run_id and not args.crash_log_dir:
        parser.error("one of --run-id or --crash-log-dir is required")
    if args.seconds is None:
        worst, samples = measured_fault_window()
        if worst is None:
            parser.error(
                f"no --seconds given and only {samples} recorded fault(s) on disk to derive one "
                f"from (need {MIN_SAMPLES_FOR_DERIVED_WINDOW}). Refusing to invent a duration."
            )
        args.seconds = round(worst * SURVIVAL_MARGIN, 1)
        print(
            f"er-survival-proof: watching {args.seconds:g}s, derived from {samples} recorded "
            f"fault(s) whose worst time-to-first-fault is {worst:.1f}s, x{SURVIVAL_MARGIN} margin"
        )
    return drive(
        args.run_id,
        args.seconds,
        args.interval,
        args.crash_log_dir,
        not args.warp,
        args.await_player,
    )


if __name__ == "__main__":
    raise SystemExit(main())
