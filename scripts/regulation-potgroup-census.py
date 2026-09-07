#!/usr/bin/env python3
"""Offline census of every `EquipParamGoods` row that belongs to a POT group.

`EquipParamGoods.potGroupId` (壺グループID, s8, -1..15) is the field behind the one inventory
limit the build importer cannot see. A consumable in a pot group can be held only up to the
number of *regenerative materials* (Cracked Pot / Ritual Pot / Perfume Bottle) sharing that
group, and both acquisition paths clamp to it silently: `EquipInventoryData::InsertItem`
(1.16.2 @0x14024cfd0) and `UpdateQuantity` (@0x14024d760) each do `if (max < amount) amount = max;`.
So a grant can report success and deliver three of five.

The engine's two predicates, byte-verified on 1.16.2 and 1.17 (identical bytes, so the row
layout survived the patch):

    IsPotConsumable        goodsType(+0x3e) == 0x00 (NORMAL_ITEM) && potGroupId(+0x2e) >= 0
                           1.16.2 @0x140d3a190   1.17 @0x140d3b8e0
    IsRegenerativeMaterial goodsType(+0x3e) == 0x0b (REGENERATIVE_MATERIAL)
                           1.16.2 @0x140d3a1c0   1.17 @0x140d3b910

`EquipInventoryData::UpdatePotsStates` (@0x14024e930) walks the carried inventory and sums the
first kind into `potItemsCount[16]` and the second into `potItemsCapacity[16]`; the headroom
`GetMaxAmountForItem` (@0x14024e570) hands out is the difference.

The distinction matters to any caller that wants to free POT SPACE: depositing a *consumable*
raises the ceiling, depositing the *material* lowers it. This script reports them separately
for exactly that reason.

Decrypt/unpack is the four-stage path validated in `regulation-params.py` (AES-256-CBC ->
DCX/zstd -> BND4 -> PARAM); no Smithbox, no dotnet, no paramdef, no pip.

Env overrides: ER_REGULATION_BIN (full path), ER_GAME_DIR (directory holding regulation.bin).

    python3 scripts/regulation-potgroup-census.py
    python3 scripts/regulation-potgroup-census.py --rows      # every row id, one per line
"""

import argparse
import importlib.util
import os
import sys
from pathlib import Path

#: `EquipParamGoods` row byte offsets, read out of the 1.16.2 Ghidra dump's
#: `_EQUIP_PARAM_GOODS_ST` (row size 176) and re-confirmed against 1.17 by matching the
#: byte-identical predicate functions above.
POT_GROUP_ID_OFFSET = 0x2E
GOODS_TYPE_OFFSET = 0x3E

#: `EquipParamGoodType` values the two predicates compare against.
GOODS_TYPE_NORMAL_ITEM = 0x00
GOODS_TYPE_REGENERATIVE_MATERIAL = 0x0B

#: `EquipInventoryData::potItemsCount` / `potItemsCapacity` are `int[16]`, and
#: `UpdatePotsStates` drops any group id `>= 0x10` on the floor.
POT_GROUP_COUNT = 16

#: The category nibble the engine ORs onto a goods row to make an item id.
GOODS_ITEM_CATEGORY = 0x4000_0000


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


def param_rows(param: bytes) -> list[tuple[int, bytes]]:
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
    parser.add_argument("--rows", action="store_true", help="print every row id and its group")
    args = parser.parse_args()

    path = locate_regulation()
    print(f"regulation: {path} ({path.stat().st_size} bytes)")
    files = RP.bnd4_entries(RP.dcx_unpack(RP.decrypt(str(path))))

    wanted = "equipparamgoods.param"
    match = next(
        (n for n in files if n.replace("\\", "/").rsplit("/", 1)[-1].lower() == wanted),
        None,
    )
    if match is None:
        raise SystemExit("EquipParamGoods.param not found in the regulation BND4")

    rows = param_rows(files[match])
    if not rows:
        raise SystemExit("EquipParamGoods has fewer than two rows -- cannot derive a row size")
    row_size = rows[0][2]
    print(f"EquipParamGoods: {len(rows)} rows, row size {row_size} (0x{row_size:x})")
    # A wrong stride reads a neighbouring field, and the tell is that potGroupId stops looking
    # like an s8 in -1..15. Say so rather than printing a plausible census of nonsense.
    if row_size <= max(POT_GROUP_ID_OFFSET, GOODS_TYPE_OFFSET):
        raise SystemExit(f"row size {row_size} is too small for +0x{GOODS_TYPE_OFFSET:x}")

    consumables: dict[int, list[int]] = {}
    materials: dict[int, list[int]] = {}
    out_of_range = []
    for row_id, row, _ in rows:
        group = int.from_bytes(row[POT_GROUP_ID_OFFSET : POT_GROUP_ID_OFFSET + 1], "little", signed=True)
        goods_type = row[GOODS_TYPE_OFFSET]
        if group < -1 or group >= POT_GROUP_COUNT:
            out_of_range.append((row_id, group))
        if goods_type == GOODS_TYPE_NORMAL_ITEM and 0 <= group < POT_GROUP_COUNT:
            consumables.setdefault(group, []).append(row_id)
        elif goods_type == GOODS_TYPE_REGENERATIVE_MATERIAL:
            materials.setdefault(group, []).append(row_id)

    if out_of_range:
        print(
            f"WARNING: {len(out_of_range)} rows carry a potGroupId outside -1..{POT_GROUP_COUNT - 1} "
            f"(first: {out_of_range[:5]}) -- the offset or the stride is wrong."
        )

    total_consumable = sum(len(v) for v in consumables.values())
    total_material = sum(len(v) for v in materials.values())
    print(
        f"pot-capped consumables (goodsType 0, potGroupId >= 0): {total_consumable} rows "
        f"in {len(consumables)} groups"
    )
    print(
        f"regenerative materials (goodsType 0x0b): {total_material} rows "
        f"in {len(materials)} groups"
    )
    for group in sorted(set(consumables) | set(materials)):
        cs = consumables.get(group, [])
        ms = materials.get(group, [])
        print(
            f"  group {group:>2}: {len(cs):>3} consumables, {len(ms):>2} materials"
            + (f" (materials: {ms})" if ms else "")
        )

    if args.rows:
        print("\n# item_id\trow_id\tpotGroupId\tkind")
        for group in sorted(consumables):
            for row_id in sorted(consumables[group]):
                print(f"0x{GOODS_ITEM_CATEGORY | row_id:08X}\t{row_id}\t{group}\tconsumable")
        for group in sorted(materials):
            for row_id in sorted(materials[group]):
                print(f"0x{GOODS_ITEM_CATEGORY | row_id:08X}\t{row_id}\t{group}\tmaterial")

    return 1 if out_of_range else 0


if __name__ == "__main__":
    sys.exit(main())
