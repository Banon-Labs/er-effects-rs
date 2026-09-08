#!/usr/bin/env python3
"""Ask the game's own message files what a param row is called -- offline.

The exporter names every equipped item by handing its row id to the game's name getter, which is
one exact `MsgRepositoryImp::LookupEntry` into these same FMGs. So "would this id have exported?"
is answerable here, with no game running and nothing to contaminate: an id with no entry is an id
the exporter drops.

    python3 scripts/er-item-name.py WeaponName 16110000 16110200 16110217
    python3 scripts/er-item-name.py --list-fmg

Reads the local extraction corpus (`ER_MSG_CORPUS_ROOT`, or the recursive Witchy extraction under
~/er-extract), never the packed archives -- the shipped `item.msgbnd.dcx` is Oodle/KRAK compressed
and needs a library this repository does not carry.
"""

import argparse
import os
import struct
import sys

DEFAULT_CORPUS = os.environ.get(
    "ER_MSG_CORPUS_ROOT",
    os.path.expanduser(
        "~/er-extract/LOOK_HERE_WITCHY_RECURSIVE_20260713/sharded/msg/engus"
    ),
)

# The DLC message files the game falls back to, in the order `GetWeaponName` and its siblings try
# them: base first, then dlc01, then dlc02.
BND_DIRS = ("item-msgbnd-dcx", "item_dlc01-msgbnd-dcx", "item_dlc02-msgbnd-dcx")


def read_fmg(path):
    """Parse one FMG into {id: text}. Elden Ring ships version 2 with wide (64-bit) offsets."""
    data = open(path, "rb").read()
    big_endian = data[1] != 0
    endian = ">" if big_endian else "<"
    version = data[2]
    if version != 2:
        raise SystemExit(f"{path}: FMG version {version} is not the Elden Ring layout")
    wide = data[8] != 0
    group_count = struct.unpack_from(endian + "i", data, 0x0C)[0]
    string_count = struct.unpack_from(endian + "i", data, 0x10)[0]
    if wide:
        offsets_start = struct.unpack_from(endian + "q", data, 0x18)[0]
        groups_start = 0x28
        group_size = 0x10
        offset_size = 8
        offset_fmt = endian + "q"
    else:
        offsets_start = struct.unpack_from(endian + "i", data, 0x18)[0]
        groups_start = 0x1C
        group_size = 0x0C
        offset_size = 4
        offset_fmt = endian + "i"

    # Each group maps a contiguous id range onto consecutive entries of the string-offset table.
    entries = {}
    for index in range(group_count):
        base = groups_start + index * group_size
        offset_index, first_id, last_id = struct.unpack_from(endian + "iii", data, base)
        for step, row in enumerate(range(first_id, last_id + 1)):
            slot = offset_index + step
            if slot >= string_count:
                continue
            string_offset = struct.unpack_from(
                offset_fmt, data, offsets_start + slot * offset_size
            )[0]
            if string_offset == 0:
                entries[row] = None
                continue
            end = data.index(b"\x00\x00", string_offset)
            while (end - string_offset) % 2:
                end = data.index(b"\x00\x00", end + 1)
            entries[row] = data[string_offset:end].decode("utf-16-le")
    return entries


def load(corpus, stem):
    """Every FMG whose stem matches, across the base and DLC message bundles."""
    tables = []
    for directory in BND_DIRS:
        for suffix in ("", "_dlc01", "_dlc02"):
            path = os.path.join(corpus, directory, f"{stem}{suffix}.fmg")
            if os.path.exists(path):
                tables.append((path, read_fmg(path)))
    return tables


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("fmg", nargs="?", default="WeaponName",
                        help="FMG stem, e.g. WeaponName / ProtectorName / AccessoryName / GoodsName / ArtsName")
    parser.add_argument("ids", nargs="*", type=int)
    parser.add_argument("--corpus", default=DEFAULT_CORPUS)
    parser.add_argument("--list-fmg", action="store_true", help="list the FMGs in the corpus and exit")
    args = parser.parse_args()

    if args.list_fmg:
        for directory in BND_DIRS:
            path = os.path.join(args.corpus, directory)
            if os.path.isdir(path):
                for name in sorted(os.listdir(path)):
                    print(f"{directory}/{name}")
        return 0

    tables = load(args.corpus, args.fmg)
    if not tables:
        print(f"no {args.fmg}*.fmg under {args.corpus}", file=sys.stderr)
        return 2
    print(f"{args.fmg}: {sum(len(t) for _, t in tables)} entries across {len(tables)} file(s)")
    status = 0
    for row in args.ids:
        found = [(os.path.basename(path), table[row]) for path, table in tables if row in table]
        if not found:
            print(f"  {row}: NO ENTRY -- the name getter answers null, the exporter drops the slot")
            status = 1
            continue
        for name, text in found:
            print(f"  {row}: {text!r}   ({name})")
    return status


if __name__ == "__main__":
    sys.exit(main())
