#!/usr/bin/env python3
"""Score one run for the second-load teardown ("the black screen"), and refuse to be fooled.

THE MEASUREMENT is `oracle_world_lost_to_title`: the number of times a GENUINELY loaded world
reverted to the title/new-game map default during the run. It counts a TRANSITION (real map id ->
`0xa010000`), because every boot legitimately sits at that default before a save mounts and a
level-triggered check would fire on all of them.

WHY THIS SCRIPT REFUSES MORE OFTEN THAN IT PASSES
-------------------------------------------------
Two ways a run can read "clean" while proving nothing, both of which happened on 2026-09-04:

1. NOTHING WAS TESTED. A run that never switched characters cannot lose a world, so zero is the
   trivially-true answer. The user hitting the bug through the menu while an agent run showed
   `oracle_world_lost_to_title == 0` is that failure exactly. A run with no switch is INCONCLUSIVE.

2. THE SWITCH BYPASSED THE MENU. `er-quickload-switch-slot.txt` drove
   `switch_slot_arm_programmatic`, which set the switch state directly and never touched
   ProfileSelect. AGENTS.md forbids that as validation -- it "skips the exact user path being
   validated" -- and on 2026-09-05 the user ordered the whole mechanism DELETED for that reason:
   every second and third load this project measured had skipped the menu, so no amount of clean
   telemetry said anything about the flow a player uses.

   The detector for it stays, and its verdict is now FAIL rather than INCONCLUSIVE. Nothing in the
   product can emit that line any more, so counting one means the deleted driver came back --
   which is the failure this file is now the executable guard against. A note in AGENTS.md would
   not have been.

Verdicts: PASS (a MENU switch happened and no world was lost), FAIL (a world was lost, or a
menu-free switch armed at all), INCONCLUSIVE (nothing to score). Exit 0 only for PASS.
"""
import argparse
import json
import os
import re
import sys

TELEMETRY = "er-quickload-telemetry.json"
DEBUG_LOG = "er-quickload-autoload-debug.log"
# The product logs this the moment a ProfileSelect row is activated -- the real user path.
MENU_SWITCH = re.compile(r"ProfileSelect slot activation ARMED")
# ...and this when a MENU-FREE switch armed instead. The driver that logged it was deleted on
# 2026-09-05, so this pattern must never match again; if it does, the bypass is back.
PROGRAMMATIC_SWITCH = re.compile(r"switch-trigger #\d+: PROGRAMMATIC arm")

EXIT_OK = 0
EXIT_FAIL = 1
EXIT_INCONCLUSIVE = 2


def scan_log(path):
    menu = programmatic = 0
    if not os.path.exists(path):
        return menu, programmatic
    with open(path, encoding="utf-8", errors="replace") as handle:
        for line in handle:
            if MENU_SWITCH.search(line):
                menu += 1
            elif PROGRAMMATIC_SWITCH.search(line):
                programmatic += 1
    return menu, programmatic


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("run_dir", help="artifact directory printed by er-run-branch.py")
    args = parser.parse_args()

    telemetry_path = os.path.join(args.run_dir, TELEMETRY)
    try:
        with open(telemetry_path, encoding="utf-8") as handle:
            telemetry = json.load(handle)
    except (OSError, ValueError) as err:
        print(f"INCONCLUSIVE -- no readable telemetry at {telemetry_path}: {err}")
        return EXIT_INCONCLUSIVE

    lost = telemetry.get("oracle_world_lost_to_title")
    if lost is None:
        print(
            "INCONCLUSIVE -- this run's DLL predates `oracle_world_lost_to_title`, so the "
            "semaphore did not exist to fire. Rebuild and re-run; absence of the field is NOT "
            "absence of the defect."
        )
        return EXIT_INCONCLUSIVE

    retired = telemetry.get("oracle_switch_return_title_request_retired", 0)
    menu, programmatic = scan_log(os.path.join(args.run_dir, DEBUG_LOG))

    if lost:
        print(
            f"FAIL -- oracle_world_lost_to_title = {lost}: a loaded world reverted to the title "
            f"map. (menu switches: {menu}, programmatic: {programmatic}, "
            f"return-title requests retired: {retired})"
        )
        return EXIT_FAIL

    if menu == 0 and programmatic == 0:
        print(
            "INCONCLUSIVE -- no character switch happened in this run, so zero worlds lost is "
            "trivially true and says nothing about the defect."
        )
        return EXIT_INCONCLUSIVE

    if programmatic:
        print(
            f"FAIL -- {programmatic} MENU-FREE switch(es) armed. The control-file driver that "
            "could do that was deleted on 2026-09-05 because it skipped ProfileSelect entirely, "
            "so this line existing at all means the bypass is back in the product."
        )
        return EXIT_FAIL

    if menu == 0:
        print(
            "INCONCLUSIVE -- no menu switch in this run, so zero worlds lost says nothing about "
            "the ProfileSelect path the defect was reported on."
        )
        return EXIT_INCONCLUSIVE

    print(
        f"PASS -- {menu} menu switch(es) and no world lost "
        f"(programmatic: {programmatic}, return-title requests retired: {retired})"
    )
    return EXIT_OK


if __name__ == "__main__":
    sys.exit(main())
