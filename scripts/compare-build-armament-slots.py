#!/usr/bin/env python3
"""Lay two planner build documents' armament slots side by side, by position.

Why this exists, and why a presence diff was not enough: a round trip through this repo's
importer and exporter shares one planner-index-to-``ChrAsmSlot`` table
(``ARMAMENT_CHR_ASM_SLOTS`` in ``er-build-import-core::equip``). The importer reads it forwards
and the exporter reads it backwards, so a *wrong* table cancels itself out and an import/export
comparison cannot see it. Only a comparison keyed on the planner position -- which is what the
author saw on the page -- can.

The planner blocks its six armament indices three per hand; ``ChrAsmSlot`` interleaves them. The
mapping below is the same one the importer uses, restated here so this script agrees with the
code it is checking rather than with an assumption about it.

Usage:

    scripts/compare-build-armament-slots.py <target.json> <after.json>
    scripts/compare-build-armament-slots.py --fetch 98f687a96d43b1 0f15f3fc264f31
    scripts/compare-build-armament-slots.py --selftest
"""

from __future__ import annotations

import argparse
import json
import sys
import urllib.request
from pathlib import Path

#: ``ChrAsmSlot`` of each planner armament index, in planner order. Mirrors
#: ``ARMAMENT_CHR_ASM_SLOTS`` in ``crates/er-build-import-core/src/equip.rs``.
PLANNER_TO_CHR_ASM = [1, 3, 5, 0, 2, 4]

#: What the game calls each ``ChrAsmSlot`` in the weapon range.
CHR_ASM_NAMES = {
    0: "left hand 1",
    1: "right hand 1",
    2: "left hand 2",
    3: "right hand 2",
    4: "left hand 3",
    5: "right hand 3",
}

#: The planner's own label for each of its six armament indices, which is what the author sees.
PLANNER_NAMES = {
    0: "right 1",
    1: "right 2",
    2: "right 3",
    3: "left 1",
    4: "left 2",
    5: "left 3",
}

API = "https://er-inventory-api.nyasu.business/inventories/{}"


def fetch(share_id: str) -> dict:
    """Pull one stored build by its ``?b=`` id."""
    with urllib.request.urlopen(API.format(share_id), timeout=25) as response:
        return json.loads(response.read().decode("utf-8"))


def active_weapon_set(doc: dict) -> int:
    """Index of the loadout set the author has selected."""
    sets = doc.get("sets", {}).get("weapons") or [{}]
    for index, entry in enumerate(sets):
        if entry.get("active"):
            return index
    return 0


def armaments_by_position(doc: dict) -> dict[int, dict]:
    """Every armament the active set equips, keyed by planner position.

    ``equipIndex`` is a cache of ``equipSet[active]``; the array is the authority, for the reason
    the importer reads it that way. Contested positions fold last-wins, the way the planner's own
    equip-slot component does.
    """
    active = active_weapon_set(doc)
    out: dict[int, dict] = {}
    for slot in doc.get("inventory", {}).get("slots", []):
        equip_set = slot.get("equipSet")
        position = None
        if isinstance(equip_set, list) and active < len(equip_set):
            position = equip_set[active]
        elif equip_set is None:
            position = slot.get("equipIndex")
        if position is None:
            continue
        out[position] = {
            "name": slot.get("name"),
            "infusion": slot.get("infusion"),
            "upgrade": slot.get("upgrade"),
            "weapon_art": slot.get("weaponArt"),
        }
    return out


def describe(entry: dict | None) -> str:
    if entry is None:
        return "(empty)"
    bits = [str(entry.get("name"))]
    if entry.get("infusion"):
        bits.append(str(entry["infusion"]))
    if entry.get("upgrade") is not None:
        bits.append(f"+{entry['upgrade']}")
    if entry.get("weapon_art"):
        bits.append(f"[{entry['weapon_art']}]")
    return " ".join(bits)


def compare(target: dict, after: dict) -> int:
    left = armaments_by_position(target)
    right = armaments_by_position(after)
    print(
        f"target {target.get('id')!r} active weapon set {active_weapon_set(target)}; "
        f"after {after.get('id')!r} active weapon set {active_weapon_set(after)}"
    )
    print()
    header = f"{'planner':<9} {'ChrAsm':<16} {'target':<42} after"
    print(header)
    print("-" * len(header))
    moved = 0
    for position in range(6):
        chr_slot = PLANNER_TO_CHR_ASM[position]
        want = left.get(position)
        got = right.get(position)
        flag = ""
        if describe(want) != describe(got):
            flag = "   <-- DIFFERS"
            moved += 1
        print(
            f"{PLANNER_NAMES[position]:<9} "
            f"{chr_slot} ({CHR_ASM_NAMES[chr_slot]}){'':<{max(0, 16 - len(str(chr_slot)) - len(CHR_ASM_NAMES[chr_slot]) - 3)}} "
            f"{describe(want):<42} {describe(got)}{flag}"
        )
    print()

    # The same six rows keyed by item instead of by position. A presence diff answers this
    # question and not the one above, and the two disagreeing is exactly what a hand swap is.
    by_name_target = {describe(v): k for k, v in left.items()}
    by_name_after = {describe(v): k for k, v in right.items()}
    swaps = 0
    for name, position in sorted(by_name_target.items()):
        elsewhere = by_name_after.get(name)
        if elsewhere is not None and elsewhere != position:
            swaps += 1
            print(
                f"MOVED: {name} is at planner {position} ({PLANNER_NAMES[position]}, ChrAsm "
                f"{PLANNER_TO_CHR_ASM[position]}) in the target and at planner {elsewhere} "
                f"({PLANNER_NAMES[elsewhere]}, ChrAsm {PLANNER_TO_CHR_ASM[elsewhere]}) after"
            )
    if not swaps:
        print("No armament changed planner position between the two documents.")
    missing = sorted(set(by_name_target) - set(by_name_after))
    extra = sorted(set(by_name_after) - set(by_name_target))
    if missing:
        print(f"In the target and not equipped after: {missing}")
    if extra:
        print(f"Equipped after and not in the target: {extra}")
    return moved


def selftest() -> int:
    """The table round-trips, and a hand swap is visible to this script."""
    assert sorted(PLANNER_TO_CHR_ASM) == list(range(6)), PLANNER_TO_CHR_ASM
    assert set(CHR_ASM_NAMES) == set(range(6))
    # The first block is the right hand and the second is the left, which is the claim the
    # importer makes and the one this script is used to check.
    assert [CHR_ASM_NAMES[s] for s in PLANNER_TO_CHR_ASM[:3]] == [
        "right hand 1",
        "right hand 2",
        "right hand 3",
    ]
    assert [CHR_ASM_NAMES[s] for s in PLANNER_TO_CHR_ASM[3:]] == [
        "left hand 1",
        "left hand 2",
        "left hand 3",
    ]

    doc_a = {
        "id": "a",
        "sets": {"weapons": [{"active": True}]},
        "inventory": {
            "slots": [
                {"name": "Bone Bow", "equipSet": [1]},
                {"name": "Nagakiba", "equipSet": [0]},
            ]
        },
    }
    doc_b = json.loads(json.dumps(doc_a))
    doc_b["id"] = "b"
    # Same two items, different positions: a presence diff sees nothing, this must see it.
    doc_b["inventory"]["slots"][0]["equipSet"] = [4]
    assert armaments_by_position(doc_a) != armaments_by_position(doc_b)
    assert armaments_by_position(doc_a)[1]["name"] == "Bone Bow"
    assert armaments_by_position(doc_b)[4]["name"] == "Bone Bow"

    # `equipSet` absent predates loadout sets; the planner's own migration reads it as set 0.
    legacy = {
        "sets": {"weapons": [{"active": True}]},
        "inventory": {"slots": [{"name": "Longbow", "equipIndex": 2}]},
    }
    assert armaments_by_position(legacy)[2]["name"] == "Longbow"

    print("selftest ok")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("documents", nargs="*", help="two build JSON files, target first")
    parser.add_argument(
        "--fetch",
        nargs=2,
        metavar=("TARGET", "AFTER"),
        help="two stored-build ids to pull from the planner API instead of reading files",
    )
    parser.add_argument("--selftest", action="store_true", help="check this script's own table")
    args = parser.parse_args()

    if args.selftest:
        return selftest()
    if args.fetch:
        target, after = (fetch(share_id) for share_id in args.fetch)
    elif len(args.documents) == 2:
        target, after = (json.loads(Path(p).read_text(encoding="utf-8")) for p in args.documents)
    else:
        parser.error("give two documents, or --fetch with two ids, or --selftest")
    compare(target, after)
    return 0


if __name__ == "__main__":
    sys.exit(main())
