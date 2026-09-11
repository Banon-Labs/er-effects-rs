#!/usr/bin/env python3
"""Say everything two planner build documents record about one item, by name.

Answers the question a presence diff cannot: is this item equipped, and where. A build document
distinguishes carried from worn by ``equipSet``/``equipIndex`` being present at all, so "the
importer moved my bow" and "the build never asked for the bow to be held" look identical until
somebody prints both fields.

Usage:

    scripts/find-build-item.py "Bone Bow" <doc.json> [<doc.json> ...]
    scripts/find-build-item.py --selftest
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

#: Every list in a build document that can hold a named item, and the key it lives under.
SECTIONS = (
    ("inventory.slots", lambda doc: doc.get("inventory", {}).get("slots", [])),
    ("talismans.slots", lambda doc: doc.get("talismans", {}).get("slots", [])),
    ("spells.slots", lambda doc: doc.get("spells", {}).get("slots", [])),
    ("items.tools.slots", lambda doc: doc.get("items", {}).get("tools", {}).get("slots", [])),
)


def active_weapon_set(doc: dict) -> int:
    sets = doc.get("sets", {}).get("weapons") or [{}]
    for index, entry in enumerate(sets):
        if entry.get("active"):
            return index
    return 0


def report(doc: dict, label: str, needle: str) -> int:
    active = active_weapon_set(doc)
    found = 0
    print(f"--- {label} (id {doc.get('id')!r}, active weapon set {active})")
    for section, pick in SECTIONS:
        for position, slot in enumerate(pick(doc)):
            name = slot.get("name") or ""
            if needle.lower() not in name.lower():
                continue
            found += 1
            equip_set = slot.get("equipSet")
            equip_index = slot.get("equipIndex")
            in_active = None
            if isinstance(equip_set, list) and active < len(equip_set):
                in_active = equip_set[active]
            elif equip_set is None:
                in_active = equip_index
            worn = "WORN" if in_active is not None else "carried only"
            print(
                f"    {section}[{position}] {name!r} infusion={slot.get('infusion')} "
                f"upgrade={slot.get('upgrade')} weaponArt={slot.get('weaponArt')!r}"
            )
            print(
                f"        equipIndex={equip_index} equipSet={equip_set} "
                f"-> position in active set: {in_active} ({worn})"
            )
    # Ammunition is not a slot list; it is four named keys.
    ammo = doc.get("items", {}).get("ammo") or {}
    for key, value in ammo.items():
        if value and needle.lower() in str(value).lower():
            found += 1
            print(f"    items.ammo.{key} = {value!r}")
    if not found:
        print(f"    {needle!r} does not appear in this document at all")
    return found


def selftest() -> int:
    doc = {
        "id": "x",
        "sets": {"weapons": [{"active": True}]},
        "inventory": {
            "slots": [
                {"name": "Bone Bow", "equipSet": [None]},
                {"name": "Nagakiba", "equipSet": [0]},
            ]
        },
        "items": {"ammo": {"arrow1": "Bone Arrow"}, "tools": {"slots": []}},
    }
    # A row whose active-set entry is null is carried, not worn. That distinction is the whole
    # point of this script, so it is the thing the selftest pins.
    assert report(doc, "selftest", "Bone Bow") == 1
    assert report(doc, "selftest", "Bone Arrow") == 1
    assert report(doc, "selftest", "Rivers of Blood") == 0
    print("selftest ok")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("item", nargs="?", help="item name, matched case-insensitively")
    parser.add_argument("documents", nargs="*", help="build JSON files")
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    if args.selftest:
        return selftest()
    if not args.item or not args.documents:
        parser.error("give an item name and at least one document, or --selftest")
    for path in args.documents:
        report(json.loads(Path(path).read_text(encoding="utf-8")), path, args.item)
    return 0


if __name__ == "__main__":
    sys.exit(main())
