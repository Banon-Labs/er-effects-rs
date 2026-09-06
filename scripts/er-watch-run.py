#!/usr/bin/env python3
"""Follow one `er-run-branch.py` run and print only the lines that decide something.

WHY THIS EXISTS. A run's evidence is spread across three files that answer different questions --
`er-quickload-autoload-debug.log` (what the product's ending-request machinery did),
`er-input-harness.log` (whether the harness actually drove, or stood down `passive`), and
`er-quickload-telemetry.json` (the RAM oracles: which character, which map, is the player up).
Tailing any one of them alone has repeatedly produced a wrong reading of a run.

IT ALSO ENDS. A boot deadlock (bd er-effects-rs-1742) leaves the process alive, 60+ threads,
~0 CPU, and the DLL log simply stops -- indistinguishable from "still loading" unless something
watches for the SILENCE. `--stall-seconds` of no growth prints `WEDGED` and exits, so a hung run
ends the wait instead of consuming it.

Window placement is reported by CLASS ONLY (`steam_app_1245620`), never by enumerating clients:
AGENTS.md forbids dumping the user's window list.
"""
import argparse
import json
import os
import re
import subprocess
import pathlib
import sys
import time

sys.path.insert(0, str(__import__('pathlib').Path(__file__).resolve().parent))
import er_run_lib

EVENT_PATTERN = re.compile(r"CVAR10 RISE|MMS-CLEANUP: child\(mms\)=0x")
DRIVE_PATTERN = re.compile(r"drive: mode=|mode flag read")
ER_WINDOW_CLASS = "steam_app_1245620"
POLL_SECONDS = 4


def oracle_state(telemetry_path):
    try:
        with open(telemetry_path, encoding="utf-8") as handle:
            data = json.load(handle)
    except (OSError, ValueError):
        return None
    saved_map = data.get("oracle_saved_map_c30")
    return (
        data.get("oracle_system_step_label"),
        data.get("oracle_player_present"),
        hex(saved_map) if isinstance(saved_map, int) else saved_map,
        data.get("oracle_char_name"),
    )


def er_window():
    """The Elden Ring window's placement, or None. Queried by class; no client list is printed."""
    try:
        out = subprocess.run(
            ["hyprctl", "clients", "-j"], capture_output=True, text=True, timeout=5
        ).stdout
        for client in json.loads(out):
            if client.get("class") == ER_WINDOW_CLASS:
                return client["monitor"], client["at"], client["size"]
    except (OSError, ValueError, subprocess.SubprocessError):
        pass
    return None


# Lines after which nothing about this run can change, so the watcher exits instead of burning
# live game minutes. `sq-repro` either arms the switch (the run proceeds and this list must NOT
# match) or states it will not arm -- and the can-move verdict it gates on is terminal by
# construction, forced after exactly one inject-on/inject-off interval.
TERMINAL_DECISIONS = (
    ("NOT arming switch", "sq-repro will not arm the switch for this boot epoch"),
    ("no usable autoload save", "no save will autoload, so the run cannot reach a world"),
)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("run_dir", help="artifact directory printed by er-run-branch.py")
    parser.add_argument("--max-seconds", type=int, default=2400)
    parser.add_argument(
        "--stall-seconds",
        type=int,
        default=90,
        help="declare WEDGED after this long with no growth in the product log",
    )
    args = parser.parse_args()

    run_dir = args.run_dir
    quick_log = os.path.join(run_dir, "er-quickload-autoload-debug.log")
    harness_log = os.path.join(run_dir, "er-input-harness.log")
    telemetry = os.path.join(run_dir, "er-quickload-telemetry.json")
    # One watch on the DIRECTORY (not the individual logs): the DLL rotates `<name>.log` to
    # `<name>.log.prev` at startup, so a watch pinned to an inode goes deaf at exactly the
    # moment the interesting run begins. `er_run_lib` already owns this ctypes block.
    watch = er_run_lib.DirectoryWatch(pathlib.Path(run_dir))

    seen, last_state, stalled, last_size, placed = set(), None, 0, -1, False
    decided = None
    deadline = time.time() + args.max_seconds
    while time.time() < deadline:
        for path, pattern, tag in ((quick_log, EVENT_PATTERN, "EVT"), (harness_log, DRIVE_PATTERN, "DRV")):
            if not os.path.exists(path):
                continue
            try:
                with open(path, encoding="utf-8", errors="replace") as handle:
                    for line in handle:
                        if pattern.search(line) and (tag, line) not in seen:
                            seen.add((tag, line))
                            print(f"{tag}: {line.rstrip()[:320]}", flush=True)
                        if decided is None:
                            for probe, why in TERMINAL_DECISIONS:
                                if probe in line:
                                    decided = (why, line.rstrip()[:320])
            except OSError:
                pass

        if not placed:
            window = er_window()
            if window:
                monitor, at, size = window
                print(f"WINDOW: monitor {monitor} at {at} size {size}", flush=True)
                placed = True

        state = oracle_state(telemetry)
        if state and state != last_state:
            print(f"STATE: {state}", flush=True)
            last_state = state

        if decided is not None:
            why, line = decided
            print(f"DECIDED ({why}): {line}", flush=True)
            print(
                "--- watcher stopping: the run has produced every answer it can. "
                "Leaving the game up past this point burns live minutes on a question "
                "already settled (measured on br-20260905-035144-8a54: decision at "
                "+41.0s, log still running at +90.1s, headed for the 180s cap). ---",
                flush=True,
            )
            return 0

        try:
            size = os.path.getsize(quick_log)
            stalled = stalled + POLL_SECONDS if size == last_size else 0
            last_size = size
            if stalled >= args.stall_seconds:
                print(f"WEDGED: product log silent {stalled}s", flush=True)
                return 0
        except OSError:
            pass
        # inotify, not a sleep: readiness here is an EVENT (the DLL appending to its log), and
        # `scripts/check-no-timeouts.py` bans a sleep standing in for one. `wait` returns the instant
        # the directory changes and otherwise at the bound, which is exactly the stall accounting
        # above -- a quiet POLL_SECONDS is what `stalled` is counting.
        watch.wait(POLL_SECONDS)

    print(f"--- watcher done, {len(seen)} event line(s) ---", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
