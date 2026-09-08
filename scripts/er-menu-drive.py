#!/usr/bin/env python3
"""Drive the in-game menu to a save slot in one call.

Why this exists. Reaching a save slot through the real menus is fourteen key presses. Until now each
one was a separate shell round-trip: write a command file, wait several seconds, read the log, decide
the next press. That is slow, it burns the operator's attention on bookkeeping, and it is fragile --
a press issued while the previous one is still held is silently lost, and the only way to notice is
to read the cursor back afterwards. The harness now queues a whole file and paces it one command per
poll, so the entire route can be handed over at once; this script writes that file and then waits on
the harness's own semaphores rather than on a clock.

What it does not do. It does not decide where to go. The route is explicit, because a script that
guessed at row counts would fail the same way hand-counting did -- silently, and one row off.
"""

from __future__ import annotations

import argparse
import pathlib
import re
import sys
import time

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import er_run_lib

# DirectInput scancodes the game's menus read. `confirm` is E, the binding the menu actually consumes;
# Return (0x1c) reaches the text surfaces instead and is deliberately not used here.
KEYS = {"up": 0xC8, "down": 0xD0, "left": 0xCB, "right": 0xCD, "confirm": 0x12, "tab": 0x2C}

# The route from an in-world pause menu to a save slot, as measured on 1.17 (runs br-20260905-201903
# and -203643). Each entry is (key, holds). The two `confirm`s that open a dialog are held one poll
# longer because the dialog build is what the next step reads back.
# `openmenu` leads the route because the pause menu is not opened by a keypress -- it opens when
# `CSPopupMenu+0x121` is set, which `CSPopupMenu::Update` consumes the next frame. A route that
# assumed the menu was already up silently did nothing at all when it was not: measured on run
# br-20260905-210305-c6a6, eleven presses landed on a closed menu and every grid read back cell 0.
# It is a no-op when the menu is already open (the harness refuses on the guard byte).
TO_FILE_PICKER = [
    ("openmenu", 0),
    ("up", 3),       # wrap the left column from Equipment to System (row 6 of 7)
    ("confirm", 3),  # enter the System pane
    ("tab", 3),      # move to the Quit Game tab
    ("down", 3),
    ("right", 3),
    ("confirm", 4),  # activate Load Character from File -> the file browser opens
]


def queue_path(run_dir: pathlib.Path) -> pathlib.Path:
    return run_dir / "er-harness-cmd.txt"


def next_sequence(path: pathlib.Path) -> int:
    """One past whatever the file already claims, so a re-run is never ignored as a repeat."""
    try:
        first = path.read_text(encoding="utf-8").splitlines()[0].strip()
        return int(first) + 1
    except (OSError, IndexError, ValueError):
        return 2


def write_queue(run_dir: pathlib.Path, steps: list[tuple[str, int]]) -> int:
    path = queue_path(run_dir)
    sequence = next_sequence(path)
    lines = [str(sequence)]
    lines += [
        "openmenu" if name == "openmenu" else f"key 0x{KEYS[name]:x} {holds}"
        for name, holds in steps
    ]
    lines.append("picker")
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")
    return sequence


def await_cursor(run_dir: pathlib.Path, want_bound: int, seconds: int) -> tuple[int, int] | None:
    """Block on the harness log until it reports a picker cursor with `want_bound` rows.

    inotify, not polling: the harness appends the line the instant the press releases, and
    `scripts/check-no-timeouts.py` bans a sleep standing in for an event.
    """
    log = run_dir / "er-input-harness.log"
    watch = er_run_lib.DirectoryWatch(run_dir)
    pattern = re.compile(r"PICKER dialog=0x([0-9a-f]+) grid=0x[0-9a-f]+ cursor=(-?\d+) bound=(-?\d+)")
    seen = 0
    # A deadline, not an iteration count. `watch.wait` returns the instant the directory changes, and
    # during a drive the harness is appending constantly -- so a `for _ in range(seconds)` loop spent
    # its whole budget in milliseconds and reported "never appeared" while the drive was still on its
    # second press. Measured on run br-20260905-204256-755e.
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        try:
            text = log.read_text(encoding="utf-8", errors="replace")
        except OSError:
            text = ""
        matches = pattern.findall(text)
        if len(matches) > seen:
            seen = len(matches)
            _, cursor, bound = matches[-1]
            if int(bound) == want_bound:
                return int(cursor), int(bound)
        watch.wait(1.0)
    return None


def await_log(run_dir: pathlib.Path, needle: str, seconds: int) -> bool:
    """Block until `needle` appears in the harness log, or the deadline passes."""
    log = run_dir / "er-input-harness.log"
    watch = er_run_lib.DirectoryWatch(run_dir)
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        try:
            if needle in log.read_text(encoding="utf-8", errors="replace"):
                return True
        except OSError:
            pass
        watch.wait(1.0)
    return False


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("run_dir", help="artifact directory printed by er-run-branch.py")
    parser.add_argument("--file-row", type=int, required=True,
                        help="row index of the save file in the browser (0-based; the browser lists "
                             "current-path, [..], then folders, then saves)")
    parser.add_argument("--slot", type=int, required=True, help="save slot to load (0-9)")
    parser.add_argument("--wait", type=int, default=90, help="seconds to wait for each dialog")
    parser.add_argument("--stop-before-accept", action="store_true",
                        help="park on the slot and stop, instead of loading it")
    parser.add_argument("--watch", type=int, default=0, metavar="SECONDS",
                        help="wait up to SECONDS for the pause menu to open before driving, so this "
                             "can be launched alongside the game instead of being timed by hand")
    args = parser.parse_args()

    run_dir = pathlib.Path(args.run_dir)
    if not run_dir.is_dir():
        print(f"no such run directory: {run_dir}", file=sys.stderr)
        return 2

    if args.watch:
        # Launched with the game, not after it. The route used to be issued by hand once the operator
        # noticed the menu was up, which made every run a manual timing exercise. The harness's own
        # `open_pause_menu ADVANCED` line is the event to wait on -- it is written the frame the menu
        # opens -- so this blocks on the log rather than on a clock.
        if not await_log(run_dir, "open_pause_menu ADVANCED", args.watch):
            print("the pause menu never opened; read er-input-harness.log", file=sys.stderr)
            return 1
        print("pause menu open; driving", flush=True)

    steps = list(TO_FILE_PICKER)
    steps += [("down", 3)] * args.file_row
    steps.append(("confirm", 4))  # open the slot list
    sequence = write_queue(run_dir, steps)
    print(f"queued #{sequence}: {len(steps)} press(es) -> file browser row {args.file_row}", flush=True)

    reached = await_cursor(run_dir, want_bound=10, seconds=args.wait)
    if reached is None:
        print("the slot list never appeared; read er-input-harness.log", file=sys.stderr)
        return 1
    print(f"slot list up (cursor={reached[0]} bound={reached[1]})", flush=True)

    steps = [("down", 3)] * args.slot
    if not args.stop_before_accept:
        steps.append(("confirm", 4))
    if steps:
        sequence = write_queue(run_dir, steps)
        print(f"queued #{sequence}: {len(steps)} press(es) -> slot {args.slot}"
              f"{'' if args.stop_before_accept else ' + accept'}", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
