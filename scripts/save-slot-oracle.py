#!/usr/bin/env python3
"""Emit expected save-slot identity JSON for runtime oracle comparisons.

The helper is deliberately conservative: it can extract USER_DATA### entries from
an ER0000.sl2/ER0000.co2 BND4 container and can decode the field offsets that are
currently proven by the ER-Save-File-Readers fixture layout. Unknown full-save
layouts still produce structural metadata rather than guessed identity fields.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import struct
from pathlib import Path
from typing import Any

BND4_MAGIC = b"BND4"
BND4_FILE_COUNT_OFFSET = 0x0C
BND4_ENTRY_TABLE_OFFSET = 0x40
BND4_ENTRY_SIZE = 0x20
BND4_ENTRY_SIZE_OFFSET = 0x08
BND4_ENTRY_DATA_OFFSET = 0x10
BND4_ENTRY_NAME_OFFSET = 0x14
USER_DATA_PREFIX = "USER_DATA"
SLOT_COUNT = 10
U32_SIZE = 4
FACE_MAGIC = b"FACE"

# Offsets in ER-Save-File-Readers' extracted-slot fixture layout. These are not
# assumed for every live BND slot; the script only emits decoded fields when the
# slot body matches the fixture layout closely enough.
FIXTURE_MAP_ID_OFFSET = 0x14
FIXTURE_PLAYER_DATA_OFFSET = 0xA184
FIXTURE_HEALTH_OFFSET = 0xA184
FIXTURE_MAX_HEALTH_OFFSET = 0xA188
FIXTURE_MAX_BASE_HEALTH_OFFSET = 0xA18C
FIXTURE_FP_OFFSET = 0xA190
FIXTURE_MAX_FP_OFFSET = 0xA194
FIXTURE_BASE_MAX_FP_OFFSET = 0xA198
FIXTURE_FACE_MAGIC_OFFSET = 0x13718
FIXTURE_MIN_LENGTH = FIXTURE_BASE_MAX_FP_OFFSET + U32_SIZE


def read_u32_le(data: bytes, offset: int) -> int | None:
    if offset < 0 or offset + U32_SIZE > len(data):
        return None
    return struct.unpack_from("<I", data, offset)[0]


def read_utf16z(data: bytes, offset: int) -> str:
    end = offset
    while end + 1 < len(data):
        if data[end] == 0 and data[end + 1] == 0:
            break
        end += 2
    return data[offset:end].decode("utf-16le", errors="replace")


def bnd4_entries(data: bytes) -> list[dict[str, Any]]:
    if not data.startswith(BND4_MAGIC):
        return []
    file_count = read_u32_le(data, BND4_FILE_COUNT_OFFSET) or 0
    entries: list[dict[str, Any]] = []
    for index in range(file_count):
        entry_offset = BND4_ENTRY_TABLE_OFFSET + index * BND4_ENTRY_SIZE
        if entry_offset + BND4_ENTRY_SIZE > len(data):
            break
        size = struct.unpack_from("<Q", data, entry_offset + BND4_ENTRY_SIZE_OFFSET)[0]
        data_offset = read_u32_le(data, entry_offset + BND4_ENTRY_DATA_OFFSET) or 0
        name_offset = read_u32_le(data, entry_offset + BND4_ENTRY_NAME_OFFSET) or 0
        name = read_utf16z(data, name_offset) if name_offset < len(data) else ""
        entries.append(
            {
                "index": index,
                "name": name,
                "offset": data_offset,
                "size": size,
                "name_offset": name_offset,
            }
        )
    return entries


def extract_slot(data: bytes, slot: int) -> tuple[bytes, dict[str, Any]]:
    entries = bnd4_entries(data)
    if entries:
        wanted = f"{USER_DATA_PREFIX}{slot:03d}"
        for entry in entries:
            if entry["name"] == wanted:
                start = int(entry["offset"])
                end = start + int(entry["size"])
                return data[start:end], {"container": "bnd4", "entry": entry}
        raise SystemExit(f"slot {wanted} not found in BND4 entries")
    return data, {"container": "raw-slot", "entry": None}


def decode_fixture_fields(slot_data: bytes) -> dict[str, Any]:
    face_at_fixture_offset = slot_data[FIXTURE_FACE_MAGIC_OFFSET:FIXTURE_FACE_MAGIC_OFFSET + len(FACE_MAGIC)] == FACE_MAGIC
    if len(slot_data) < FIXTURE_MIN_LENGTH or not face_at_fixture_offset:
        return {"layout": "unknown", "decoded_fields": {}}
    decoded = {
        "saved_map_c30": read_u32_le(slot_data, FIXTURE_MAP_ID_OFFSET),
        "health": read_u32_le(slot_data, FIXTURE_HEALTH_OFFSET),
        "max_health": read_u32_le(slot_data, FIXTURE_MAX_HEALTH_OFFSET),
        "max_base_health": read_u32_le(slot_data, FIXTURE_MAX_BASE_HEALTH_OFFSET),
        "fp": read_u32_le(slot_data, FIXTURE_FP_OFFSET),
        "max_fp": read_u32_le(slot_data, FIXTURE_MAX_FP_OFFSET),
        "base_max_fp": read_u32_le(slot_data, FIXTURE_BASE_MAX_FP_OFFSET),
    }
    return {"layout": "er-save-file-readers-fixture", "decoded_fields": decoded}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--save", required=True, type=Path, help="ER0000.sl2/.co2 BND4 or extracted slot file")
    parser.add_argument("--slot", type=int, default=0, choices=range(SLOT_COUNT), metavar="0-9")
    parser.add_argument("--output", type=Path, help="write JSON here instead of stdout")
    args = parser.parse_args()

    data = args.save.read_bytes()
    slot_data, source = extract_slot(data, args.slot)
    fixture = decode_fixture_fields(slot_data)
    face_offsets = []
    search_at = 0
    while True:
        found = slot_data.find(FACE_MAGIC, search_at)
        if found < 0:
            break
        face_offsets.append(found)
        search_at = found + len(FACE_MAGIC)
        if len(face_offsets) >= SLOT_COUNT:
            break
    result = {
        "source_path": str(args.save),
        "slot": args.slot,
        "slot_sha256": hashlib.sha256(slot_data).hexdigest(),
        "slot_size": len(slot_data),
        "source": source,
        "face_magic_offsets": face_offsets,
        **fixture,
    }
    text = json.dumps(result, indent=2, sort_keys=True) + "\n"
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(text, encoding="utf-8")
    else:
        print(text, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
