#!/usr/bin/env python3
"""Offline census of ammunition in `EquipParamWeapon`, and of the ceiling the engine enforces.

Why AMMO needs its own census
-----------------------------
Arrows and bolts are **not** `EquipParamGoods`. They are `EquipParamWeapon` rows, which is why
they equip into dedicated `ChrAsmSlot` positions rather than the quickbar -- and it is why
`EquipParamGoods.maxNum` (the number behind every consumable's grant quantity) says nothing about
them. The engine routes them somewhere else entirely:

    CS::EquipInventoryData::GetMaxAmountForItem   1.16.2 @0x14024e570   1.17 @0x14024e570
      category 4 (goods)  -> potGroup headroom, else maxNum(+0x3a), else 99
      category 0/1/2/8    -> tail-jumps to ::GetMaxItemQuantity
                             1.16.2 @0x140674680  1.17 @0x1406754d0  (delta +0xe50)

    ::GetMaxItemQuantity, category 0 (weapon):
        movzbl 0xe6(%rcx),%edx   ; weaponCategory, u8
        cmp    $0xd,%dl          ; 13
        je     take_it
        cmp    $0xe,%dl          ; 14
        jne    return_1
    take_it:
        movzbl 0x235(%rcx),%eax  ; maxArrowQuantity, u8   <-- the whole answer

    Any other weapon row falls through to `mov $0x1,%eax` -- an armament's max quantity is one.

The two OFFSETS, confirmed on both images (this is the silent failure class)
---------------------------------------------------------------------------
`weaponCategory` +0xE6 (u8) and `maxArrowQuantity` +0x235 (u8) are named by the 1.16.2 Ghidra
dump's `_EQUIP_PARAM_WEAPON_ST` (struct size 664) and confirmed on 1.17 by reading the function
that consumes them: the instruction bytes are identical between builds --
`0f b6 91 e6 00 00 00 / 80 fa 0d / 74 09 / 80 fa 0e / 0f 85 .. / 0f b6 81 35 02 00 00` at
1.16.2 `0x140674887` and 1.17 `0x1406756d7`. This script is the third leg: it reads the values
out of the installed regulation and checks they behave like the fields those names claim.

`wepType` (+0x1A6, u16) is read alongside because the exporter already classifies ammunition by
it (81 Arrow / 83 Great Arrow / 85 Bolt / 86 Ballista Bolt). The two classifications must agree;
if they ever disagree, the engine's is the one that decides the ceiling.

Decrypt/unpack is the four-stage path validated in `regulation-params.py` (AES-256-CBC ->
DCX/zstd -> BND4 -> PARAM); no Smithbox, no dotnet, no paramdef, no pip.

Env overrides: ER_REGULATION_BIN (full path), ER_GAME_DIR (directory holding regulation.bin).

    python3 scripts/regulation-ammo-census.py
    python3 scripts/regulation-ammo-census.py --rows     # every ammo row id, one per line
"""

import argparse
import collections
import importlib.util
import os
import sys
from pathlib import Path

#: `_EQUIP_PARAM_WEAPON_ST` byte offsets. See the module docstring for the both-image proof.
WEAPON_CATEGORY_OFFSET = 0xE6
MAX_ARROW_QUANTITY_OFFSET = 0x235
WEP_TYPE_OFFSET = 0x1A6

#: The two `weaponCategory` values `::GetMaxItemQuantity` accepts before it will read
#: `maxArrowQuantity`. Anything else is an armament and its max quantity is 1.
AMMO_WEAPON_CATEGORIES = (13, 14)

#: `wepType` values the exporter already treats as ammunition, for the cross-check.
AMMUNITION_WEP_TYPES = {81: "Arrow", 83: "Great Arrow", 85: "Bolt", 86: "Ballista Bolt"}

#: The category nibble the engine ORs onto a weapon row to make an item id: zero.
WEAPON_ITEM_CATEGORY = 0x0000_0000


def _load_regulation_reader():
    """Import `regulation-params.py` despite the hyphen, so the four stages live in one place."""
    path = Path(__file__).resolve().parent / "regulation-params.py"
    spec = importlib.util.spec_from_file_location("regulation_params", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


RP = _load_regulation_reader()


def locate_regulation() -> Path:
    explicit = os.environ.get("ER_REGULATION_BIN")
    if explicit:
        return Path(explicit)
    game_dir = os.environ.get("ER_GAME_DIR")
    if game_dir:
        return Path(game_dir) / "regulation.bin"
    home = Path.home()
    candidates = [
        home / ".local/share/Steam/steamapps/common/ELDEN RING/Game/regulation.bin",
        home / ".steam/steam/steamapps/common/ELDEN RING/Game/regulation.bin",
    ]
    for candidate in candidates:
        if candidate.is_file():
            return candidate
    raise SystemExit(
        "regulation.bin not found; set ER_REGULATION_BIN or ER_GAME_DIR. Looked in:\n  "
        + "\n  ".join(str(c) for c in candidates)
    )


def param_rows(param: bytes) -> list[tuple[int, bytes, int]]:
    """Row size is not stored; derive it from the gap between consecutive row data offsets."""
    import struct

    row_count = struct.unpack_from("<H", param, 0x0A)[0]
    index = [struct.unpack_from("<iiqq", param, 0x40 + i * 24) for i in range(row_count)]
    offsets = sorted(offset for _, _, offset, _ in index)
    if len(offsets) < 2:
        return []
    row_size = min(b - a for a, b in zip(offsets, offsets[1:]))
    return [(row_id, param[off : off + row_size], row_size) for row_id, _, off, _ in index]


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("--rows", action="store_true", help="print every ammo row id")
    args = parser.parse_args()

    path = locate_regulation()
    print(f"regulation: {path} ({path.stat().st_size} bytes)")
    files = RP.bnd4_entries(RP.dcx_unpack(RP.decrypt(str(path))))

    wanted = "equipparamweapon.param"
    match = next(
        (n for n in files if n.replace("\\", "/").rsplit("/", 1)[-1].lower() == wanted),
        None,
    )
    if match is None:
        raise SystemExit("EquipParamWeapon.param not found in the regulation BND4")

    rows = param_rows(files[match])
    if not rows:
        raise SystemExit("EquipParamWeapon has fewer than two rows -- cannot derive a row size")
    row_size = rows[0][2]
    print(f"EquipParamWeapon: {len(rows)} rows, row size {row_size} (0x{row_size:x})")
    # A wrong stride reads a neighbouring field. Say so rather than printing a plausible census
    # of nonsense.
    if row_size <= MAX_ARROW_QUANTITY_OFFSET:
        raise SystemExit(f"row size {row_size} is too small for +0x{MAX_ARROW_QUANTITY_OFFSET:x}")

    by_category: collections.Counter[int] = collections.Counter()
    ammo: list[tuple[int, int, int, int]] = []  # row_id, weaponCategory, wepType, maxArrowQuantity
    by_wep_type: dict[int, list[int]] = {}
    for row_id, row, _ in rows:
        category = row[WEAPON_CATEGORY_OFFSET]
        wep_type = int.from_bytes(row[WEP_TYPE_OFFSET : WEP_TYPE_OFFSET + 2], "little")
        by_category[category] += 1
        if wep_type in AMMUNITION_WEP_TYPES:
            by_wep_type.setdefault(wep_type, []).append(row_id)
        if category in AMMO_WEAPON_CATEGORIES:
            ammo.append((row_id, category, wep_type, row[MAX_ARROW_QUANTITY_OFFSET]))

    print(f"\nweaponCategory histogram over all {len(rows)} rows:")
    for category, count in sorted(by_category.items()):
        mark = "  <-- ammo" if category in AMMO_WEAPON_CATEGORIES else ""
        print(f"  {category:>3}: {count:>5}{mark}")

    print(f"\nweaponCategory in {AMMO_WEAPON_CATEGORIES}: {len(ammo)} rows")
    quantities: collections.Counter[int] = collections.Counter(q for _, _, _, q in ammo)
    print("  maxArrowQuantity histogram: " + repr(dict(sorted(quantities.items()))))

    print("\ncross-check against the exporter's wepType classification:")
    for wep_type, label in sorted(AMMUNITION_WEP_TYPES.items()):
        ids = by_wep_type.get(wep_type, [])
        print(f"  wepType {wep_type:>3} ({label}): {len(ids)} rows")
    wep_type_ids = {row_id for ids in by_wep_type.values() for row_id in ids}
    category_ids = {row_id for row_id, _, _, _ in ammo}
    only_wep_type = sorted(wep_type_ids - category_ids)
    only_category = sorted(category_ids - wep_type_ids)
    print(f"  wepType says ammo, weaponCategory does not: {len(only_wep_type)} {only_wep_type[:10]}")
    print(f"  weaponCategory says ammo, wepType does not: {len(only_category)} {only_category[:10]}")

    if args.rows:
        print("\n# item_id\trow_id\tweaponCategory\twepType\tmaxArrowQuantity")
        for row_id, category, wep_type, quantity in sorted(ammo):
            print(
                f"0x{WEAPON_ITEM_CATEGORY | row_id:08X}\t{row_id}\t{category}\t{wep_type}\t{quantity}"
            )

    # A disagreement between the engine's gate and the exporter's classification is the one
    # result that would make either side unsafe to rely on, so it is the exit code.
    return 1 if (only_wep_type or only_category) else 0


if __name__ == "__main__":
    sys.exit(main())
