#!/usr/bin/env python3
"""Compare the order two planner build documents list one category's items in.

The reorder pass (``er-build-import-runtime::reorder``) re-acquires the build's items so the
character's inventory sorts the way the build's page reads. Whether it worked is a question about
order, and every other comparison in this repo is a question about presence -- so a pass that
reports ``160/160 re-acquired`` can sit above an inventory in a completely different order and
nothing says so.

This prints the two orders side by side and reports the first place they diverge, plus each item's
index in both, so "my bow moved" has an answer that is not a guess.

Usage:

    scripts/compare-build-inventory-order.py <target.json> <after.json> [--section inventory]
    scripts/compare-build-inventory-order.py --selftest
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

SECTIONS = {
    "inventory": lambda doc: doc.get("inventory", {}).get("slots", []),
    "talismans": lambda doc: doc.get("talismans", {}).get("slots", []),
    "spells": lambda doc: doc.get("spells", {}).get("slots", []),
    "tools": lambda doc: doc.get("items", {}).get("tools", {}).get("slots", []),
}


def names(doc: dict, section: str) -> list[str]:
    return [slot.get("name") or "(unnamed)" for slot in SECTIONS[section](doc)]


def compare(target: dict, after: dict, section: str, focus: str | None) -> None:
    left = names(target, section)
    right = names(after, section)
    print(
        f"{section}: target {target.get('id')!r} lists {len(left)}, "
        f"after {after.get('id')!r} lists {len(right)}"
    )

    shared = [name for name in left if name in right]
    after_shared = [name for name in right if name in left]
    if shared == after_shared:
        print("The items both documents carry are in the SAME relative order.")
    else:
        print("The items both documents carry are in a DIFFERENT relative order.")
        for position, (a, b) in enumerate(zip(shared, after_shared)):
            if a != b:
                print(
                    f"    first divergence at shared position {position}: "
                    f"target has {a!r}, after has {b!r}"
                )
                break

    if focus:
        for label, listing in (("target", left), ("after", right)):
            where = [i for i, name in enumerate(listing) if focus.lower() in name.lower()]
            print(f"    {focus!r} in {label}: index(es) {where}")
        for label, listing in (("target", shared), ("after", after_shared)):
            where = [i for i, name in enumerate(listing) if focus.lower() in name.lower()]
            print(f"    {focus!r} among the items BOTH carry, in {label}: index(es) {where}")

    only_target = [name for name in left if name not in right]
    only_after = [name for name in right if name not in left]
    if only_target:
        print(f"    in the target only ({len(only_target)}): {only_target[:12]}")
    if only_after:
        print(f"    in the after document only ({len(only_after)}): {only_after[:12]}")


def selftest() -> int:
    def doc(ident, names_):
        return {
            "id": ident,
            "inventory": {"slots": [{"name": n} for n in names_]},
        }

    same = doc("a", ["X", "Y", "Bone Bow"])
    # Extra items around it must not count as a reorder: only the shared items' order matters.
    padded = doc("b", ["Q", "X", "R", "Y", "Bone Bow"])
    assert names(same, "inventory") != names(padded, "inventory")
    shared_a = [n for n in names(same, "inventory") if n in names(padded, "inventory")]
    shared_b = [n for n in names(padded, "inventory") if n in names(same, "inventory")]
    assert shared_a == shared_b, (shared_a, shared_b)

    swapped = doc("c", ["Bone Bow", "Y", "X"])
    shared_c = [n for n in names(swapped, "inventory") if n in names(same, "inventory")]
    assert shared_a != shared_c
    print("selftest ok")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("documents", nargs="*")
    parser.add_argument("--section", default="inventory", choices=sorted(SECTIONS))
    parser.add_argument("--focus", help="report this item's index in both listings")
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    if args.selftest:
        return selftest()
    if len(args.documents) != 2:
        parser.error("give two documents, or --selftest")
    target, after = (
        json.loads(Path(p).read_text(encoding="utf-8")) for p in args.documents
    )
    compare(target, after, args.section, args.focus)
    return 0


if __name__ == "__main__":
    sys.exit(main())
