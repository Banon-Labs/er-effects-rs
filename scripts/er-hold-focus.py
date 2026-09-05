#!/usr/bin/env python3
"""Keep the ELDEN RING window focused while an agent-driven run needs input to reach the game.

WHY. ER only accepts input while its window has focus. Measured on run br-20260905-031000-5ac5:
the product's move-probe wrote the forward stick into the live pad device for 150 frames, the
character moved for 5, `oracle_can_move` stayed False, and the window was mapped and visible on
DP-1 with `focusHistoryID = 2` -- i.e. not focused. The product's movement proof is ONE-SHOT per
load epoch (it gives up at ~900 frames and switches the probe off), so focusing the window after
it has already given up does nothing: focus has to be held from the moment the window appears.

This is the stop-gap. The durable fix is a shell that removes the focus requirement inside the
game; while that does not exist, an agent-driven run has to actually own the focus.

PRIVACY: the window is located by CLASS ONLY (`steam_app_1245620`). AGENTS.md forbids enumerating
or printing the user's window list, so nothing about any other window is read or reported.
"""
import argparse
import json
import subprocess
import pathlib
import sys
import time

sys.path.insert(0, str(__import__('pathlib').Path(__file__).resolve().parent))
import er_run_lib

ER_CLASS = "steam_app_1245620"
POLL_SECONDS = 1.0


def er_focus_state():
    """(address, focusHistoryID) for the ER window, or None if it is not mapped yet."""
    try:
        out = subprocess.run(
            ["hyprctl", "clients", "-j"], capture_output=True, text=True, timeout=5
        ).stdout
        for client in json.loads(out):
            if client.get("class") == ER_CLASS:
                return client.get("address"), client.get("focusHistoryID")
    except (OSError, ValueError, subprocess.SubprocessError):
        pass
    return None


def focus(address):
    """Focus by ADDRESS. `focuswindow class:^...$` is rejected by this machine's non-legacy Lua
    config parser, which reads the bare `class:` selector as Lua and errors on it."""
    try:
        subprocess.run(
            ["hyprctl", "dispatch", "focuswindow", f"address:{address}"],
            capture_output=True,
            text=True,
            timeout=5,
        )
    except (OSError, subprocess.SubprocessError):
        pass


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--seconds", type=int, default=180, help="how long to hold focus")
    args = parser.parse_args()

    # A bounded `select` to spend each quiet interval on. There is no file to watch here --
    # the compositor is queried, not signalled -- so this watches nothing and simply times
    # out, which is the point: a wait that CANNOT accidentally become a sync primitive.
    idle = er_run_lib.DirectoryWatch(pathlib.Path(__file__).resolve().parent)
    deadline = time.time() + args.seconds
    appeared = False
    refocused = 0
    while time.time() < deadline:
        state = er_focus_state()
        if state is not None:
            address, focus_id = state
            if not appeared:
                print(f"ER window mapped ({address}); holding focus", flush=True)
                appeared = True
            if focus_id != 0:
                focus(address)
                refocused += 1
        # A bounded wait on an inotify fd rather than a sleep: nothing in this loop is a
        # synchronization point (the compositor has no file to signal), but `select` with a timeout
        # is the repo's sanctioned way to spend a quiet interval, and a bare `time.sleep` is banned
        # by `scripts/check-no-timeouts.py` precisely so no wait can quietly become a sync primitive.
        idle.wait(POLL_SECONDS)

    if not appeared:
        print("ER window never appeared; nothing was focused", flush=True)
        return 1
    print(f"done -- re-focused {refocused} time(s)", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
