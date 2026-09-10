#!/usr/bin/env python3
"""Dump Elden Ring's `CharacterTypeProperties` and `MultiplayProperties` tables.

Both tables are static data in the game image, so this reads a flat de-Arxan'd image and
needs no game running and no Ghidra daemon.

* `CharacterTypeProperties` is a 23-entry array of 20-byte records indexed by
  `ChrIns::chrType` (`+0x68`). `CS::CharacterTypeProperties::IsHostilePhantom`
  (1.16.2 `0x1404c7d10`) is `MOVSXD RAX,[RCX]; CMP EAX,0x16; JA default; LEA RCX,[RAX+RAX*4];
  LEA RAX,[table]; MOVZX EAX,[RAX+RCX*4+0xa]`, which is where the base, the stride and the
  `isHostilePhantom` field offset below all come from. `isHostLike` (`+0x7`) and
  `isFriendlyPhantom` (`+0x9`) are the neighbouring flags read by the same pattern from
  `IsHostLike` / `IsFriendlyPhantom`.

* `MultiplayProperties` is a 32-entry array of 64-byte records, walked linearly by
  `GetMultiplayPropertiesByMultiplayRole` (1.16.2 `0x1401db340`) comparing `+0xc multiplayRole`.
  Each row carries the `SummonParamType` the engine matched on and the `CharacterType` it
  derives from it -- the `GameMan::GetSummonParamType -> MultiplayProperties ->
  PlayerGameData::SetChrType` chain in one table.

Joining the two answers the question `er-lockon-filter` exists to ask -- which
`ChrType` values and which `SummonParamType` values mean "this is a hostile phantom" --
out of the game's own data rather than a hand-written list, which is why `--rust` prints
the sets in the shape that crate's constants take.

Usage
    python3 scripts/er-character-type-tables.py
    python3 scripts/er-character-type-tables.py --rust
    ER_DEOBF_BIN=eldenring-deobf-1.17.1.bin python3 scripts/er-character-type-tables.py \
        --character-type-properties 0x... --multiplay-properties 0x...
    python3 scripts/er-character-type-tables.py --selftest
"""

from __future__ import annotations

import argparse
import os
import struct
import sys

DEFAULT_IMAGE = os.environ.get("ER_DEOBF_BIN", "eldenring-deobf.bin")
IMAGE_BASE = 0x140000000

# 1.16.2. Every one is read out of the function named in the module docstring above.
CHARACTER_TYPE_PROPERTIES_VA = 0x143B17C00
CHARACTER_TYPE_PROPERTIES_COUNT = 23
CHARACTER_TYPE_PROPERTIES_STRIDE = 20
IS_HOST_LIKE_OFFSET = 0x7
IS_FRIENDLY_PHANTOM_OFFSET = 0x9
IS_HOSTILE_PHANTOM_OFFSET = 0xA

MULTIPLAY_PROPERTIES_VA = 0x143B11230
MULTIPLAY_PROPERTIES_COUNT = 32
MULTIPLAY_PROPERTIES_STRIDE = 64
SUMMON_PARAM_TYPE_OFFSET = 0x4
CHARACTER_TYPE_OFFSET = 0x8
MULTIPLAY_ROLE_OFFSET = 0xC
FLAGS_OFFSET = 0x34
DEBUG_NAME_OFFSET = 0x38

# `ChrType` names, for the report only. The rule derives from the table, never from a name.
CHR_TYPE_NAMES = {
    0: "Local",
    1: "WhitePhantom",
    2: "Duelist",
    5: "Npc",
    8: "GrayPhantom",
    13: "BattleRoyal",
    15: "BloodyFinger",
    16: "Recusant",
    17: "BluePhantom",
    18: "FesteringBloodyFinger",
    19: "WhitePhantomNpc",
    20: "BloodyFingerNpc",
    21: "RecusantNpc",
}

# The kinds the game spawns rather than kinds another human is. A hostile phantom this crate's
# consumer still leaves lockable, because an invader may legitimately want to lock an npc invader.
NPC_CHR_TYPES = (19, 20, 21, 22)


class Image:
    def __init__(self, path: str, base: int) -> None:
        self.path = path
        self.base = base
        with open(path, "rb") as handle:
            self.data = handle.read()

    def read(self, va: int, size: int) -> bytes:
        offset = va - self.base
        if offset < 0 or offset + size > len(self.data):
            raise SystemExit(f"{va:#x} is outside {self.path}")
        return self.data[offset : offset + size]

    def wide_string(self, va: int, limit: int = 64) -> str:
        raw = self.read(va, limit * 2)
        out = []
        for index in range(0, len(raw), 2):
            unit = struct.unpack_from("<H", raw, index)[0]
            if unit == 0:
                break
            out.append(chr(unit))
        return "".join(out)


def character_type_properties(image: Image, table_va: int) -> list[dict]:
    rows = []
    for chr_type in range(CHARACTER_TYPE_PROPERTIES_COUNT):
        raw = image.read(
            table_va + chr_type * CHARACTER_TYPE_PROPERTIES_STRIDE,
            CHARACTER_TYPE_PROPERTIES_STRIDE,
        )
        rows.append(
            {
                "chr_type": chr_type,
                "name": CHR_TYPE_NAMES.get(chr_type, ""),
                "host_like": bool(raw[IS_HOST_LIKE_OFFSET]),
                "friendly_phantom": bool(raw[IS_FRIENDLY_PHANTOM_OFFSET]),
                "hostile_phantom": bool(raw[IS_HOSTILE_PHANTOM_OFFSET]),
            }
        )
    return rows


def multiplay_properties(image: Image, table_va: int) -> list[dict]:
    rows = []
    for index in range(MULTIPLAY_PROPERTIES_COUNT):
        raw = image.read(
            table_va + index * MULTIPLAY_PROPERTIES_STRIDE, MULTIPLAY_PROPERTIES_STRIDE
        )
        name_pointer = struct.unpack_from("<Q", raw, DEBUG_NAME_OFFSET)[0]
        rows.append(
            {
                "role": raw[MULTIPLAY_ROLE_OFFSET],
                "summon_param_type": struct.unpack_from("<i", raw, SUMMON_PARAM_TYPE_OFFSET)[0],
                "chr_type": struct.unpack_from("<i", raw, CHARACTER_TYPE_OFFSET)[0],
                "flags": struct.unpack_from("<I", raw, FLAGS_OFFSET)[0],
                "debug_name": image.wide_string(name_pointer) if name_pointer else "",
            }
        )
    return rows


def hostile_phantom_chr_types(properties: list[dict]) -> list[int]:
    return [row["chr_type"] for row in properties if row["hostile_phantom"]]


def hostile_phantom_summon_param_types(
    properties: list[dict], roles: list[dict]
) -> list[int]:
    """Every `SummonParamType` whose role resolves to a hostile-phantom `CharacterType`."""
    hostile = set(hostile_phantom_chr_types(properties))
    seen = []
    for row in roles:
        if row["chr_type"] in hostile and row["summon_param_type"] not in seen:
            seen.append(row["summon_param_type"])
    return sorted(seen, reverse=True)


def report(properties: list[dict], roles: list[dict]) -> None:
    print("CharacterTypeProperties")
    print(f"{'chrType':>7}  {'name':<22} hostLike friendly hostile")
    for row in properties:
        print(
            f"{row['chr_type']:>7}  {row['name']:<22} "
            f"{int(row['host_like']):>8} {int(row['friendly_phantom']):>8} "
            f"{int(row['hostile_phantom']):>7}"
        )
    print()
    print("MultiplayProperties")
    print(f"{'role':>4} {'summonParam':>11} {'chrType':>7} {'flags':>7}  debugName")
    for row in roles:
        print(
            f"{row['role']:>4} {row['summon_param_type']:>11} {row['chr_type']:>7} "
            f"{row['flags']:>#7x}  {row['debug_name']}"
        )


def rust(properties: list[dict], roles: list[dict]) -> None:
    hostile = hostile_phantom_chr_types(properties)
    candidates = [chr_type for chr_type in hostile if chr_type not in NPC_CHR_TYPES]
    summon = hostile_phantom_summon_param_types(properties, roles)
    print(f"// hostile phantoms, from CharacterTypeProperties: {hostile}")
    print(f"pub(crate) const HOSTILE_PHANTOM_CHR_TYPES: [i32; {len(candidates)}] = {candidates:};".replace("[", "[", 1))
    print(f"// their SummonParamTypes, via MultiplayProperties")
    print(f"pub(crate) const HOSTILE_PHANTOM_SUMMON_PARAM_TYPES: [i32; {len(summon)}] = {summon};")


def selftest(args: argparse.Namespace) -> int:
    """Assert the two tables still say what this crate's constants were derived from.

    An absent image is a skip, not a failure -- the de-Arxan'd image is a local reverse
    engineering input that is never committed, so a machine without one has nothing to check
    rather than something broken. The same line the game-version gates draw.
    """
    if not os.path.exists(args.image):
        print(f"skip: {args.image} is not present; nothing to read")
        return 0
    image = Image(args.image, args.base)
    properties = character_type_properties(image, args.character_type_properties)
    roles = multiplay_properties(image, args.multiplay_properties)
    failures = []

    hostile = hostile_phantom_chr_types(properties)
    if hostile != [2, 15, 16, 18, 20, 21, 22]:
        failures.append(f"hostile phantom ChrTypes changed: {hostile}")
    if [row["chr_type"] for row in properties if row["host_like"]] != [0, 8]:
        failures.append("isHostLike is no longer exactly Local and GrayPhantom")
    if [row["chr_type"] for row in properties if row["friendly_phantom"]] != [1, 17, 19]:
        failures.append("isFriendlyPhantom changed")

    # The pairing the live census measured under Seamless Co-op: summonParamType -12 is a role
    # whose CharacterType is Duelist, and Duelist is a hostile phantom. If that stops being true
    # the lock-on filter's widened rule has lost its justification.
    anor = [row for row in roles if row["summon_param_type"] == -12]
    if len(anor) != 1 or anor[0]["chr_type"] != 2:
        failures.append(f"summonParamType -12 no longer maps to Duelist: {anor}")
    if -12 not in hostile_phantom_summon_param_types(properties, roles):
        failures.append("-12 is not derived as a hostile-phantom role")

    for failure in failures:
        print(f"FAIL {failure}")
    if failures:
        return 1
    print(f"ok ({args.image}): {len(properties)} character types, {len(roles)} multiplay roles")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("--image", default=DEFAULT_IMAGE, help="flat de-Arxan'd image")
    parser.add_argument("--base", type=lambda v: int(v, 0), default=IMAGE_BASE)
    parser.add_argument(
        "--character-type-properties",
        type=lambda v: int(v, 0),
        default=CHARACTER_TYPE_PROPERTIES_VA,
    )
    parser.add_argument(
        "--multiplay-properties", type=lambda v: int(v, 0), default=MULTIPLAY_PROPERTIES_VA
    )
    parser.add_argument("--rust", action="store_true", help="print the derived constant sets")
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()

    if args.selftest:
        return selftest(args)

    image = Image(args.image, args.base)
    properties = character_type_properties(image, args.character_type_properties)
    roles = multiplay_properties(image, args.multiplay_properties)
    if args.rust:
        rust(properties, roles)
    else:
        report(properties, roles)
    return 0


if __name__ == "__main__":
    sys.exit(main())
