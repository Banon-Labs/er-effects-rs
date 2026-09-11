#!/usr/bin/env python3
"""Resolve a FromSoft paramdef field to the byte offset a decompiler shows.

Ghidra names an unrecognised param field `field_0x109` and a bitfield read looks
like `(row->field_0x109 & 1)` or `(row->someName >> 2) & 1`. That is an offset
and a bit index, and the paramdef xml is what turns it back into a name. The
paramdefs live in the sibling `fromsoftware-rs` checkout under
`tools/param-generator/params/<game>/`; this script walks one in declaration
order and prints each field's byte offset, plus the bit index and width for
bitfields.

Packing follows SoulsFormats: fields are laid out end to end with no alignment
padding, and consecutive bitfields share a storage unit of their declared type
until the type changes or the next field does not fit.

    python3 scripts/paramdef-field-offset.py EquipParamWeapon disableMultiDropShare
    python3 scripts/paramdef-field-offset.py EquipParamGoods --offset 0x4a
    python3 scripts/paramdef-field-offset.py --selftest
"""

from __future__ import annotations

import argparse
import os
import re
import sys
import xml.etree.ElementTree as ET

SIZES = {
    "s8": 1,
    "u8": 1,
    "dummy8": 1,
    "s16": 2,
    "u16": 2,
    "s32": 4,
    "u32": 4,
    "b32": 4,
    "f32": 4,
    "angle32": 4,
    "f64": 8,
}

FIELD_DEF = re.compile(
    r"^(?P<type>\w+)\s+(?P<name>[A-Za-z_]\w*)"
    r"(?:\[(?P<count>\d+)\]|\s*:\s*(?P<bits>\d+))?"
    r"\s*(?:=.*)?$"
)

DEFAULT_PARAM_ROOT = os.path.join(
    os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))),
    "fromsoftware-rs",
    "tools",
    "param-generator",
    "params",
)


def _unit_size(type_name: str) -> int:
    return SIZES[type_name]


def _bit_unit(type_name: str) -> str:
    """Storage-unit identity for bitfield packing.

    SoulsFormats coerces `dummy8` to `u8` before deciding whether the next
    bitfield continues the current unit, so `u8 a:1` followed by `dummy8 b:7`
    occupies one byte rather than two. Treating the two names as distinct shifts
    every later field by one byte per transition, which is exactly the kind of
    error this script exists to avoid.
    """
    return "u8" if type_name == "dummy8" else type_name


def layout(path: str):
    """Return [(name, byte_offset, bit_index_or_None, bit_width_or_None, type)]."""
    fields_node = ET.parse(path).getroot().find("Fields")
    if fields_node is None:
        raise SystemExit(f"{path}: no Fields element")

    offset = 0
    unit_type = None
    unit_bits_used = 0
    rows = []

    def close_unit():
        nonlocal offset, unit_type, unit_bits_used
        if unit_type is None:
            return
        width = _unit_size(unit_type) * 8
        units = (unit_bits_used + width - 1) // width
        offset += _unit_size(unit_type) * units
        unit_type = None
        unit_bits_used = 0

    for field in fields_node:
        raw = (field.get("Def") or "").strip()
        match = FIELD_DEF.match(raw)
        if match is None:
            raise SystemExit(f"{path}: cannot parse field def {raw!r}")
        type_name = match.group("type")
        name = match.group("name")
        count = match.group("count")
        bits = match.group("bits")

        if bits is not None:
            bits = int(bits)
            unit = _bit_unit(type_name)
            width = _unit_size(unit) * 8
            if unit_type != unit or unit_bits_used + bits > width:
                close_unit()
                unit_type = unit
            rows.append((name, offset, unit_bits_used, bits, type_name))
            unit_bits_used += bits
            continue

        close_unit()
        n = int(count) if count else 1
        if type_name.startswith("fixstr"):
            element = 2 if type_name == "fixstrW" else 1
            size = element * n
        else:
            size = _unit_size(type_name) * n
        rows.append((name, offset, None, None, type_name))
        offset += size

    close_unit()
    return rows


def resolve_path(param: str, root: str, game: str) -> str:
    if os.path.isfile(param):
        return param
    candidate = os.path.join(root, game, param + ".xml")
    if os.path.isfile(candidate):
        return candidate
    raise SystemExit(f"no paramdef for {param!r} under {os.path.join(root, game)}")


def selftest(root: str) -> int:
    """Re-derive four offsets read out of 1.16.2 decompilation by hand.

    `FUN_14055fc80` at `0x14055fc80` reads one bit per equip category to decide
    whether a dropped item is shared with the session. Ghidra renders each read
    against whichever struct its lookup-result variable was typed as, so the
    offsets below are what the decompiler printed:

        weapon      `row->field_0x109 & 1`                  -> 0x109 bit 0
        protector   `*(byte *)(&row->bowDistRate + 1) >> 2`  -> 0xe2 + 1, bit 2
        accessory   `row->spEffectCategory >> 2`             -> 0x40, bit 2
        gem         `row->shopLv >> 3`                       -> 0x34, bit 3

    All four land on `disableMultiDropShare` in the matching paramdef, at four
    different offsets with four different bit indices, which is what identifies
    the field. The fifth category, goods, reads `row->field_0x4a >> 3` while the
    sibling checkout's `EquipParamGoods.xml` puts `disableMultiDropShare` at
    `0x4b` bit 3 and `isDiscard` at `0x4a` bit 3. One byte of paramdef version
    skew in that one file is the likeliest reading, so it is left out of the
    assertions rather than asserted either way.
    """
    expected = [
        ("EquipParamWeapon", "disableMultiDropShare", 0x109, 0),
        ("EquipParamProtector", "disableMultiDropShare", 0xE3, 2),
        ("EquipParamAccessory", "disableMultiDropShare", 0x40, 2),
        ("EquipParamGem", "disableMultiDropShare", 0x34, 3),
    ]
    failures = 0
    for param, field, want_off, want_bit in expected:
        rows = layout(resolve_path(param, root, "eldenring"))
        hit = [r for r in rows if r[0] == field]
        if not hit:
            print(f"FAIL {param}.{field}: not present")
            failures += 1
            continue
        _, off, bit, width, type_name = hit[0]
        ok = off == want_off and bit == want_bit
        print(
            f"{'ok  ' if ok else 'FAIL'} {param}.{field} "
            f"off={hex(off)} bit={bit} width={width} type={type_name} "
            f"(want off={hex(want_off)} bit={want_bit})"
        )
        failures += 0 if ok else 1
    return failures


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("param", nargs="?", help="paramdef name (EquipParamWeapon) or a path to its xml")
    ap.add_argument("field", nargs="*", help="field names to print; omit to print all")
    ap.add_argument("--offset", help="print the field covering this byte offset instead")
    ap.add_argument("--game", default="eldenring")
    ap.add_argument("--root", default=DEFAULT_PARAM_ROOT, help="paramdef root directory")
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()

    if args.selftest:
        return 1 if selftest(args.root) else 0
    if not args.param:
        ap.error("param is required unless --selftest")

    rows = layout(resolve_path(args.param, args.root, args.game))

    if args.offset is not None:
        want = int(args.offset, 0)
        for name, off, bit, width, type_name in rows:
            if off == want:
                extra = f" bit={bit} width={width}" if width else ""
                print(f"{hex(off)}  {type_name:8} {name}{extra}")
        return 0

    wanted = set(args.field)
    for name, off, bit, width, type_name in rows:
        if wanted and name not in wanted:
            continue
        extra = f" bit={bit} width={width}" if width else ""
        print(f"{hex(off)}  {type_name:8} {name}{extra}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
