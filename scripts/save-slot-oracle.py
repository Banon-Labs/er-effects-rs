#!/usr/bin/env python3
"""Emit expected save-slot identity JSON for runtime oracle comparisons.

The extractor is intentionally narrow and evidence-bound.  It understands
USER_DATA### entries in an ER0000.sl2/ER0000.co2 BND4 container, and it decodes
only the slot layout that is anchored by the local ER-Save-File-Readers fixture
and by ClayAmore's 010 Editor template:

    https://github.com/ClayAmore/EldenRingSaveTemplate/blob/master/SL2.bt

The emitted ``decoded_fields`` names are the static-save side of the same
identity oracle that runtime telemetry reads from ``fromsoftware-rs``'s typed
``CS::PlayerGameData`` / ``CS::GameMan`` layouts in this repo.  Unknown layouts
still produce structural metadata rather than guessed identity fields.
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
U8_SIZE = 1
U16_SIZE = 2
U32_SIZE = 4
FACE_MAGIC = b"FACE"

SL2_BT_REFERENCE_URL = "https://github.com/ClayAmore/EldenRingSaveTemplate/blob/master/SL2.bt"

# Offsets in the extracted-slot fixture layout.  The player-data base is the
# start of SL2.bt's ``PlayerGameData`` struct, whose comment says it mirrors
# ``CS::PlayerGameData+0x8``.  Runtime telemetry therefore reads the matching
# fields from fromsoftware-rs' typed ``PlayerGameData`` at save-relative offset
# + 0x8 (e.g. save Health @ 0xa184 == runtime current_hp @ +0x10).
FIXTURE_SLOT_VERSION_OFFSET = 0x10
FIXTURE_MAP_ID_OFFSET = 0x14
FIXTURE_PLAYER_GAME_DATA_OFFSET = 0xA17C
FIXTURE_FACE_MAGIC_OFFSET = 0x13718
FIXTURE_MIN_LENGTH = FIXTURE_FACE_MAGIC_OFFSET + len(FACE_MAGIC)

PGD_REL_HEALTH = 0x08
PGD_REL_MAX_HEALTH = 0x0C
PGD_REL_BASE_MAX_HEALTH = 0x10
PGD_REL_FP = 0x14
PGD_REL_MAX_FP = 0x18
PGD_REL_BASE_MAX_FP = 0x1C
PGD_REL_STAMINA = 0x24
PGD_REL_MAX_STAMINA = 0x28
PGD_REL_BASE_MAX_STAMINA = 0x2C
PGD_REL_VIGOR = 0x34
PGD_STAT_COUNT = 8
PGD_REL_HUMANITY = 0x54
PGD_REL_LEVEL = 0x60
PGD_REL_RUNES = 0x64
PGD_REL_RUNE_MEMORY = 0x68
PGD_REL_POISON_BUILDUP = 0x70
PGD_REL_ROT_BUILDUP = 0x74
PGD_REL_BLEED_BUILDUP = 0x78
PGD_REL_FROST_BUILDUP = 0x7C
PGD_REL_DEATH_BUILDUP = 0x80
PGD_REL_SLEEP_BUILDUP = 0x84
PGD_REL_MADNESS_BUILDUP = 0x88
PGD_REL_CHARACTER_TYPE = 0x90
PGD_REL_CHARACTER_NAME = 0x94
PGD_CHARACTER_NAME_UNITS = 0x10
PGD_REL_GENDER = 0xB6
PGD_REL_ARCHETYPE = 0xB7
PGD_REL_VOICE_TYPE = 0xBA
PGD_REL_STARTING_GIFT = 0xBB
PGD_REL_UNLOCKED_TALISMAN_SLOTS = 0xBE
PGD_REL_MATCHMAKING_SPIRIT_ASHES_LEVEL = 0xBF
PGD_REL_MAX_CRIMSON_FLASK_COUNT = 0xF9
PGD_REL_MAX_CERULEAN_FLASK_COUNT = 0xFA

STAT_NAMES = [
    "vigor",
    "mind",
    "endurance",
    "strength",
    "dexterity",
    "intelligence",
    "faith",
    "arcane",
]
STATUS_BUILDUP_FIELDS = [
    ("poison_buildup", PGD_REL_POISON_BUILDUP),
    ("rot_buildup", PGD_REL_ROT_BUILDUP),
    ("bleed_buildup", PGD_REL_BLEED_BUILDUP),
    ("frost_buildup", PGD_REL_FROST_BUILDUP),
    ("death_buildup", PGD_REL_DEATH_BUILDUP),
    ("sleep_buildup", PGD_REL_SLEEP_BUILDUP),
    ("madness_buildup", PGD_REL_MADNESS_BUILDUP),
]

DECODED_FIELD_SOURCES: dict[str, str] = {
    "saved_map_c30": "SL2.bt Slot.MapID @ slot+0x14; runtime GameMan+0xc30 oracle_saved_map_c30",
    "health": "SL2.bt PlayerGameData.Health @ player+0x08; runtime PlayerGameData.current_hp",
    "max_health": "SL2.bt PlayerGameData.MaxHealth @ player+0x0c; runtime current_max_hp",
    "max_base_health": "SL2.bt PlayerGameData.BaseMaxHealth @ player+0x10; runtime base_max_hp",
    "fp": "SL2.bt PlayerGameData.FP @ player+0x14; runtime current_fp",
    "max_fp": "SL2.bt PlayerGameData.MaxFP @ player+0x18; runtime current_max_fp",
    "base_max_fp": "SL2.bt PlayerGameData.BaseMaxFP @ player+0x1c; runtime base_max_fp",
    "stamina": "SL2.bt PlayerGameData.SP @ player+0x24; runtime current_stamina",
    "max_stamina": "SL2.bt PlayerGameData.MaxSP @ player+0x28; runtime current_max_stamina",
    "base_max_stamina": "SL2.bt PlayerGameData.BaseMaxSP @ player+0x2c; runtime base_max_stamina",
    "stats": "SL2.bt Vigor..Arcane @ player+0x34..0x50; runtime contiguous PlayerGameData vigor..arcane",
    "level": "SL2.bt PlayerGameData.Level @ player+0x60; runtime PlayerGameData.level",
    "runes": "SL2.bt PlayerGameData.Souls @ player+0x64; runtime PlayerGameData.rune_count",
    "rune_memory": "SL2.bt PlayerGameData.Soulmemory @ player+0x68; runtime PlayerGameData.rune_memory",
    "chr_type": "SL2.bt PlayerGameData.CharacterType @ player+0x90; runtime PlayerGameData.chr_type",
    "name": "SL2.bt PlayerGameData.CharacterName[0x10] @ player+0x94; runtime PlayerGameData.character_name",
    "gender": "SL2.bt PlayerGameData.Gender @ player+0xb6; runtime PlayerGameData.gender",
    "archetype": "SL2.bt PlayerGameData.ArcheType @ player+0xb7; runtime PlayerGameData.archetype",
    "voice_type": "SL2.bt PlayerGameData.VoiceType @ player+0xba; runtime PlayerGameData.voice_type",
    "starting_gift": "SL2.bt PlayerGameData.Gift @ player+0xbb; runtime PlayerGameData.starting_gift",
    "unlocked_talisman_slots": "SL2.bt PlayerGameData.AdditionalTalismanSlotsCount @ player+0xbe; runtime PlayerGameData.unlocked_talisman_slots",
    "spirit_ash_level": "SL2.bt PlayerGameData.SummonSpiritLevel @ player+0xbf; runtime PlayerGameData.matchmaking_spirit_ashes_level",
    "max_crimson_flask_count": "SL2.bt PlayerGameData.MaxCrimsonFlaskCount @ player+0xf9; runtime PlayerGameData.max_hp_flask",
    "max_cerulean_flask_count": "SL2.bt PlayerGameData.MaxCeruleanFlaskCount @ player+0xfa; runtime PlayerGameData.max_fp_flask",
}


def read_u8(data: bytes, offset: int) -> int | None:
    if offset < 0 or offset + U8_SIZE > len(data):
        return None
    return data[offset]


def read_u16_le(data: bytes, offset: int) -> int | None:
    if offset < 0 or offset + U16_SIZE > len(data):
        return None
    return struct.unpack_from("<H", data, offset)[0]


def read_u32_le(data: bytes, offset: int) -> int | None:
    if offset < 0 or offset + U32_SIZE > len(data):
        return None
    return struct.unpack_from("<I", data, offset)[0]


def read_utf16z(data: bytes, offset: int) -> str:
    end = offset
    while end + 1 < len(data):
        if data[end] == 0 and data[end + 1] == 0:
            break
        end += U16_SIZE
    return data[offset:end].decode("utf-16le", errors="replace")


def read_fixed_utf16z(data: bytes, offset: int, units: int) -> str:
    raw = data[offset:offset + units * U16_SIZE]
    values = list(struct.unpack_from(f"<{len(raw) // U16_SIZE}H", raw)) if raw else []
    if 0 in values:
        values = values[: values.index(0)]
    return bytes().join(struct.pack("<H", value) for value in values).decode("utf-16le", errors="replace")


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


def map_id_text(data: bytes, offset: int) -> str | None:
    if offset < 0 or offset + U32_SIZE > len(data):
        return None
    chunk = data[offset:offset + U32_SIZE]
    return f"{chunk[3]:02d} {chunk[2]:02d} {chunk[1]:02d} {chunk[0]:02d}"


def pgd_offset(relative_offset: int) -> int:
    return FIXTURE_PLAYER_GAME_DATA_OFFSET + relative_offset


def decode_sl2_bt_fixture_fields(slot_data: bytes) -> dict[str, Any]:
    face_at_fixture_offset = slot_data[FIXTURE_FACE_MAGIC_OFFSET:FIXTURE_FACE_MAGIC_OFFSET + len(FACE_MAGIC)] == FACE_MAGIC
    if len(slot_data) < FIXTURE_MIN_LENGTH or not face_at_fixture_offset:
        return {"layout": "unknown", "decoded_fields": {}}

    stats = [
        read_u32_le(slot_data, pgd_offset(PGD_REL_VIGOR + index * U32_SIZE))
        for index in range(PGD_STAT_COUNT)
    ]
    stats_named = dict(zip(STAT_NAMES, stats, strict=True))
    status_buildup = {
        name: read_u32_le(slot_data, pgd_offset(offset))
        for name, offset in STATUS_BUILDUP_FIELDS
    }
    name = read_fixed_utf16z(
        slot_data,
        pgd_offset(PGD_REL_CHARACTER_NAME),
        PGD_CHARACTER_NAME_UNITS,
    )
    decoded = {
        "version": read_u32_le(slot_data, FIXTURE_SLOT_VERSION_OFFSET),
        "saved_map_c30": read_u32_le(slot_data, FIXTURE_MAP_ID_OFFSET),
        "saved_map_id_text": map_id_text(slot_data, FIXTURE_MAP_ID_OFFSET),
        "health": read_u32_le(slot_data, pgd_offset(PGD_REL_HEALTH)),
        "max_health": read_u32_le(slot_data, pgd_offset(PGD_REL_MAX_HEALTH)),
        "max_base_health": read_u32_le(slot_data, pgd_offset(PGD_REL_BASE_MAX_HEALTH)),
        "fp": read_u32_le(slot_data, pgd_offset(PGD_REL_FP)),
        "max_fp": read_u32_le(slot_data, pgd_offset(PGD_REL_MAX_FP)),
        "base_max_fp": read_u32_le(slot_data, pgd_offset(PGD_REL_BASE_MAX_FP)),
        "stamina": read_u32_le(slot_data, pgd_offset(PGD_REL_STAMINA)),
        "max_stamina": read_u32_le(slot_data, pgd_offset(PGD_REL_MAX_STAMINA)),
        "base_max_stamina": read_u32_le(slot_data, pgd_offset(PGD_REL_BASE_MAX_STAMINA)),
        "stats": stats,
        "stats_named": stats_named,
        "humanity": read_u32_le(slot_data, pgd_offset(PGD_REL_HUMANITY)),
        "level": read_u32_le(slot_data, pgd_offset(PGD_REL_LEVEL)),
        "runes": read_u32_le(slot_data, pgd_offset(PGD_REL_RUNES)),
        "souls": read_u32_le(slot_data, pgd_offset(PGD_REL_RUNES)),
        "rune_memory": read_u32_le(slot_data, pgd_offset(PGD_REL_RUNE_MEMORY)),
        "soulmemory": read_u32_le(slot_data, pgd_offset(PGD_REL_RUNE_MEMORY)),
        "status_buildup": status_buildup,
        "chr_type": read_u32_le(slot_data, pgd_offset(PGD_REL_CHARACTER_TYPE)),
        "name": name,
        "name_len": len(name),
        "gender": read_u8(slot_data, pgd_offset(PGD_REL_GENDER)),
        "archetype": read_u8(slot_data, pgd_offset(PGD_REL_ARCHETYPE)),
        "voice_type": read_u8(slot_data, pgd_offset(PGD_REL_VOICE_TYPE)),
        "starting_gift": read_u8(slot_data, pgd_offset(PGD_REL_STARTING_GIFT)),
        "unlocked_talisman_slots": read_u8(slot_data, pgd_offset(PGD_REL_UNLOCKED_TALISMAN_SLOTS)),
        "spirit_ash_level": read_u8(slot_data, pgd_offset(PGD_REL_MATCHMAKING_SPIRIT_ASHES_LEVEL)),
        "max_crimson_flask_count": read_u8(slot_data, pgd_offset(PGD_REL_MAX_CRIMSON_FLASK_COUNT)),
        "max_cerulean_flask_count": read_u8(slot_data, pgd_offset(PGD_REL_MAX_CERULEAN_FLASK_COUNT)),
    }
    return {
        "layout": "sl2-bt-er-save-file-readers-fixture",
        "decoded_fields": decoded,
        "decoded_field_sources": DECODED_FIELD_SOURCES,
        "layout_offsets": {
            "slot_version": f"0x{FIXTURE_SLOT_VERSION_OFFSET:x}",
            "slot_map_id": f"0x{FIXTURE_MAP_ID_OFFSET:x}",
            "player_game_data": f"0x{FIXTURE_PLAYER_GAME_DATA_OFFSET:x}",
            "face_magic": f"0x{FIXTURE_FACE_MAGIC_OFFSET:x}",
        },
        "reference": SL2_BT_REFERENCE_URL,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--save", required=True, type=Path, help="ER0000.sl2/.co2 BND4 or extracted slot file")
    parser.add_argument("--slot", type=int, default=0, choices=range(SLOT_COUNT), metavar="0-9")
    parser.add_argument("--output", type=Path, help="write JSON here instead of stdout")
    args = parser.parse_args()

    data = args.save.read_bytes()
    slot_data, source = extract_slot(data, args.slot)
    fixture = decode_sl2_bt_fixture_fields(slot_data)
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
