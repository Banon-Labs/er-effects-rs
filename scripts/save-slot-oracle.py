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
# ClayAmore's SL2.bt Slot layout puts PlayerGameData before FaceData, but the
# variable-width Gaitem map and Seamless/vanilla profile differences mean the
# absolute offsets vary in live USER_DATA entries.  The extractor therefore uses
# these deltas only as fast paths, then falls back to scanning backwards from a
# FACE buffer for a plausible PlayerGameData block instead of hard-coding a
# particular character's identity.
FIXTURE_FACE_TO_PLAYER_GAME_DATA_DELTA = FIXTURE_FACE_MAGIC_OFFSET - FIXTURE_PLAYER_GAME_DATA_OFFSET
LIVE_FACE_TO_PLAYER_GAME_DATA_DELTA = 0xA26C
MAX_PLAYER_TO_FACE_SEARCH = 0x20000
# A character's OWN FaceData (the first FACE magic in its USER_DATA00N slot) sits a
# short, version-dependent distance after its PlayerGameData.  Measured deltas
# (150-Banon): slot 0 = 0xa26c, slots 1-9 = 0xa22c.  We window the name-anchored
# PGD scan to this bracket before the slot's leading FACE magic occurrence(s) so
# the scan stays fast (a ~0x600-byte window, not a whole-slot walk) while still
# tolerating the per-version delta variance with margin.
PGD_FACE_DELTA_WINDOW_LOW = 0xA000
PGD_FACE_DELTA_WINDOW_HIGH = 0xA600
PGD_SCAN_LEADING_FACE_COUNT = 4
MAX_FACE_MAGIC_OFFSETS = 64
PLAYER_GAME_DATA_MIN_SIZE = 0x1B0
FACE_DATA_BUFFER_PAYLOAD_SIZE = 276
FACE_DATA_BUFFER_SIZE = len(FACE_MAGIC) + U32_SIZE + U32_SIZE + FACE_DATA_BUFFER_PAYLOAD_SIZE
FIXTURE_MIN_LENGTH = FIXTURE_FACE_MAGIC_OFFSET + FACE_DATA_BUFFER_SIZE

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

# Offsets relative to SL2.bt's FaceData.Magic / fromsoftware-rs FaceDataBuffer.magic.
FACE_BODY_FIELD_OFFSETS = {
    "face_model": 0x0C,
    "hair_model": 0x10,
    "eyebrow_model": 0x18,
    "beard_model": 0x1C,
    "eye_patch_model": 0x20,
    "apparent_age": 0x2C,
    "facial_aesthetic": 0x2D,
    "form_emphasis": 0x2E,
    "head_size": 0xAC,
    "chest_size": 0xAD,
    "abdomen_size": 0xAE,
    "arms_size": 0xAF,
    "legs_size": 0xB0,
    "skin_color_r": 0xB3,
    "skin_color_g": 0xB4,
    "skin_color_b": 0xB5,
}

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
    "face_data_buffer_sha256": "SL2.bt FaceData bytes from Magic through FaceDataBuffer payload; runtime fromsoftware-rs PlayerGameData.face_data.face_data_buffer raw bytes",
    "face_body_fields": "Human-readable subset of FaceDataBuffer bytes for face/body validation; runtime oracle_face_body_fields",
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


def pgd_offset(player_game_data_offset: int, relative_offset: int) -> int:
    return player_game_data_offset + relative_offset


def face_buffer_offset(face_magic_offset: int, relative_offset: int) -> int:
    return face_magic_offset + relative_offset


def decode_face_body_fields(slot_data: bytes, face_magic_offset: int) -> tuple[bytes, dict[str, int | None]]:
    face_data_buffer = slot_data[face_magic_offset:face_magic_offset + FACE_DATA_BUFFER_SIZE]
    face_body_fields = {
        name: read_u8(slot_data, face_buffer_offset(face_magic_offset, offset))
        for name, offset in FACE_BODY_FIELD_OFFSETS.items()
    }
    return face_data_buffer, face_body_fields


def find_face_magic_offsets(slot_data: bytes) -> list[int]:
    offsets: list[int] = []
    search_at = 0
    while True:
        found = slot_data.find(FACE_MAGIC, search_at)
        if found < 0:
            return offsets
        offsets.append(found)
        search_at = found + len(FACE_MAGIC)
        if len(offsets) >= MAX_FACE_MAGIC_OFFSETS:
            return offsets


def name_empty_like(value: str) -> bool:
    stripped = value.strip()
    return stripped == "" or stripped == "_"


def plausible_character_name(name: str) -> bool:
    return bool(
        name
        and "\ufffd" not in name
        and "\uffff" not in name
        and all(char.isprintable() for char in name)
    )


def player_game_data_name(slot_data: bytes, player_game_data_offset: int) -> str:
    return read_fixed_utf16z(
        slot_data,
        pgd_offset(player_game_data_offset, PGD_REL_CHARACTER_NAME),
        PGD_CHARACTER_NAME_UNITS,
    )


def plausible_player_game_data(slot_data: bytes, player_game_data_offset: int, face_magic_offset: int) -> bool:
    if player_game_data_offset < 0 or face_magic_offset + FACE_DATA_BUFFER_SIZE > len(slot_data):
        return False
    if player_game_data_offset + PLAYER_GAME_DATA_MIN_SIZE > len(slot_data):
        return False
    if slot_data[face_magic_offset:face_magic_offset + len(FACE_MAGIC)] != FACE_MAGIC:
        return False
    face_version = read_u32_le(slot_data, face_buffer_offset(face_magic_offset, len(FACE_MAGIC)))
    face_size = read_u32_le(slot_data, face_buffer_offset(face_magic_offset, len(FACE_MAGIC) + U32_SIZE))
    level = read_u32_le(slot_data, pgd_offset(player_game_data_offset, PGD_REL_LEVEL))
    health = read_u32_le(slot_data, pgd_offset(player_game_data_offset, PGD_REL_HEALTH))
    max_health = read_u32_le(slot_data, pgd_offset(player_game_data_offset, PGD_REL_MAX_HEALTH))
    base_max_health = read_u32_le(slot_data, pgd_offset(player_game_data_offset, PGD_REL_BASE_MAX_HEALTH))
    gender = read_u8(slot_data, pgd_offset(player_game_data_offset, PGD_REL_GENDER))
    max_crimson = read_u8(slot_data, pgd_offset(player_game_data_offset, PGD_REL_MAX_CRIMSON_FLASK_COUNT))
    max_cerulean = read_u8(slot_data, pgd_offset(player_game_data_offset, PGD_REL_MAX_CERULEAN_FLASK_COUNT))
    stats = [
        read_u32_le(slot_data, pgd_offset(player_game_data_offset, PGD_REL_VIGOR + index * U32_SIZE))
        for index in range(PGD_STAT_COUNT)
    ]
    if (
        None in [face_version, face_size, level, health, max_health, base_max_health, gender, max_crimson, max_cerulean]
        or any(stat is None for stat in stats)
    ):
        return False
    name = player_game_data_name(slot_data, player_game_data_offset)
    return bool(
        face_version == 4
        and face_size == FACE_DATA_BUFFER_SIZE
        and plausible_character_name(name)
        and 0 < level <= 713
        and 0 < health <= 100_000
        and 0 < max_health <= 100_000
        and 0 < base_max_health <= 100_000
        and health <= max_health
        # SL2.bt PlayerGameData.MaxHealth is the *effective* max HP (base + talisman
        # / buff modifiers), while BaseMaxHealth is the unmodified base from Vigor.
        # An equipped/leveled character therefore has base_max_health <= max_health
        # (NOT the reverse) -- the earlier `max_health <= base_max_health` constraint
        # was inverted and rejected every real high-level/equipped character, forcing
        # the decoder into a backwards FACE scan that latched onto garbage
        # character-creation template remnants in empty slots.  Evidence: 150-Banon
        # slot 0 has health=2343, max_health=2343, base_max_health=1704.
        and base_max_health <= max_health
        and gender in (0, 1)
        and 0 <= max_crimson <= 14
        and 0 <= max_cerulean <= 14
        and all(1 <= int(stat) <= 99 for stat in stats)
    )


def plausible_player_game_data_core(slot_data: bytes, player_game_data_offset: int) -> bool:
    """Validate a PlayerGameData block on its OWN fields, independent of any FACE buffer.

    The FACE-anchored predicate (``plausible_player_game_data``) additionally
    requires a co-located, well-formed FaceDataBuffer (version 4, size 288).  That
    gate is too strong: some real characters (observed: 150-Banon slot 1
    "Dark Moon Bean" L90) store a FaceData whose ``size`` field is not 288 at the
    usual delta, so no FACE buffer validates next to the PGD even though the PGD
    itself is sound.  This core predicate anchors only on the SL2.bt
    PlayerGameData fields, so name+level+stats decode correctly even when the
    face buffer is absent or in an unexpected format.
    """
    if player_game_data_offset < 0:
        return False
    if player_game_data_offset + PLAYER_GAME_DATA_MIN_SIZE > len(slot_data):
        return False
    level = read_u32_le(slot_data, pgd_offset(player_game_data_offset, PGD_REL_LEVEL))
    health = read_u32_le(slot_data, pgd_offset(player_game_data_offset, PGD_REL_HEALTH))
    max_health = read_u32_le(slot_data, pgd_offset(player_game_data_offset, PGD_REL_MAX_HEALTH))
    base_max_health = read_u32_le(slot_data, pgd_offset(player_game_data_offset, PGD_REL_BASE_MAX_HEALTH))
    gender = read_u8(slot_data, pgd_offset(player_game_data_offset, PGD_REL_GENDER))
    max_crimson = read_u8(slot_data, pgd_offset(player_game_data_offset, PGD_REL_MAX_CRIMSON_FLASK_COUNT))
    max_cerulean = read_u8(slot_data, pgd_offset(player_game_data_offset, PGD_REL_MAX_CERULEAN_FLASK_COUNT))
    stats = [
        read_u32_le(slot_data, pgd_offset(player_game_data_offset, PGD_REL_VIGOR + index * U32_SIZE))
        for index in range(PGD_STAT_COUNT)
    ]
    if (
        None in [level, health, max_health, base_max_health, gender, max_crimson, max_cerulean]
        or any(stat is None for stat in stats)
    ):
        return False
    name = player_game_data_name(slot_data, player_game_data_offset)
    return bool(
        plausible_character_name(name)
        and 0 < level <= 713
        and 0 < health <= 100_000
        and 0 < max_health <= 100_000
        and 0 < base_max_health <= 100_000
        and health <= max_health
        # SL2.bt PlayerGameData.MaxHealth is the *effective* max HP (base + talisman
        # / buff modifiers); BaseMaxHealth is the unmodified base from Vigor.  A
        # leveled/equipped character has base_max_health <= max_health (not the
        # reverse).  Evidence: 150-Banon slot 0 health=2343 max_health=2343
        # base_max_health=1704.
        and base_max_health <= max_health
        and gender in (0, 1)
        and 0 <= max_crimson <= 14
        and 0 <= max_cerulean <= 14
        and all(1 <= int(stat) <= 99 for stat in stats)
    )


def scan_player_game_data_offsets(slot_data: bytes) -> list[int]:
    """Locate PlayerGameData blocks using the FACE-independent core validator.

    Every occupied SL2.bt slot stores its FaceData (a ``FACE`` magic) a short,
    version-dependent distance AFTER its PlayerGameData.  We therefore window the
    search to the region preceding each ``FACE`` magic occurrence (the magic bytes
    are found even when the buffer's version/size fields are malformed, as happens
    for some characters' own FaceData -- e.g. 150-Banon slot 1 "Dark Moon Bean",
    whose nearest FACE has size!=288).  Within each window we accept any PGD that
    passes ``plausible_player_game_data_core`` -- which anchors only on the
    CharacterName / PlayerGameData fields, NOT on a well-formed FaceDataBuffer.
    This is both correct (finds PGDs the FACE-coupled scan misses) and bounded
    (a few windows of MAX_PLAYER_TO_FACE_SEARCH instead of a whole-slot byte walk).
    """
    offsets: set[int] = set()
    leading_faces = find_face_magic_offsets(slot_data)[:PGD_SCAN_LEADING_FACE_COUNT]
    for face_magic_offset in leading_faces:
        start = max(0, face_magic_offset - PGD_FACE_DELTA_WINDOW_HIGH)
        stop = max(start, face_magic_offset - PGD_FACE_DELTA_WINDOW_LOW)
        for player_game_data_offset in range(start, stop):
            if plausible_player_game_data_core(slot_data, player_game_data_offset):
                offsets.add(player_game_data_offset)
    return sorted(offsets)


def face_magic_offset_for_pgd(slot_data: bytes, player_game_data_offset: int) -> int:
    """Pick the FACE buffer offset to use for secondary face-data fields.

    Prefer a well-formed FaceDataBuffer (FACE + version 4 + size 288) located at
    one of the known PGD->FACE deltas; otherwise the nearest well-formed FACE
    after the PGD; otherwise the fixed LIVE delta location (face bytes may then be
    partial -- face fields are best-effort and never gate identity decoding).
    """
    def is_well_formed(face_magic_offset: int) -> bool:
        if face_magic_offset < 0 or face_magic_offset + FACE_DATA_BUFFER_SIZE > len(slot_data):
            return False
        if slot_data[face_magic_offset:face_magic_offset + len(FACE_MAGIC)] != FACE_MAGIC:
            return False
        version = read_u32_le(slot_data, face_buffer_offset(face_magic_offset, len(FACE_MAGIC)))
        size = read_u32_le(slot_data, face_buffer_offset(face_magic_offset, len(FACE_MAGIC) + U32_SIZE))
        return version == 4 and size == FACE_DATA_BUFFER_SIZE

    for delta in (LIVE_FACE_TO_PLAYER_GAME_DATA_DELTA, FIXTURE_FACE_TO_PLAYER_GAME_DATA_DELTA):
        candidate = player_game_data_offset + delta
        if is_well_formed(candidate):
            return candidate
    nearest = [off for off in find_face_magic_offsets(slot_data) if off >= player_game_data_offset and is_well_formed(off)]
    if nearest:
        return min(nearest, key=lambda off: off - player_game_data_offset)
    return player_game_data_offset + LIVE_FACE_TO_PLAYER_GAME_DATA_DELTA


def candidate_score(slot_data: bytes, player_game_data_offset: int, face_magic_offset: int) -> tuple[int, int]:
    level = read_u32_le(slot_data, pgd_offset(player_game_data_offset, PGD_REL_LEVEL)) or 0
    stats = [
        read_u32_le(slot_data, pgd_offset(player_game_data_offset, PGD_REL_VIGOR + index * U32_SIZE)) or 0
        for index in range(PGD_STAT_COUNT)
    ]
    name_len = len(player_game_data_name(slot_data, player_game_data_offset))
    return (name_len + len([stat for stat in stats if stat > 0]) + (1 if level > 0 else 0), -abs(face_magic_offset - player_game_data_offset))


def candidate_player_game_data_offsets(slot_data: bytes, face_magic_offset: int) -> list[int]:
    candidates: set[int] = set()
    for delta in (FIXTURE_FACE_TO_PLAYER_GAME_DATA_DELTA, LIVE_FACE_TO_PLAYER_GAME_DATA_DELTA):
        candidate = face_magic_offset - delta
        if plausible_player_game_data(slot_data, candidate, face_magic_offset):
            candidates.add(candidate)
    if candidates:
        return sorted(candidates)

    start = max(0, face_magic_offset - MAX_PLAYER_TO_FACE_SEARCH)
    stop = max(start, face_magic_offset - PLAYER_GAME_DATA_MIN_SIZE)
    for candidate in range(start, stop):
        if plausible_player_game_data(slot_data, candidate, face_magic_offset):
            candidates.add(candidate)
    return sorted(candidates, key=lambda offset: candidate_score(slot_data, offset, face_magic_offset), reverse=True)


def decode_fields_at(
    slot_data: bytes,
    player_game_data_offset: int,
    face_magic_offset: int,
    layout: str,
) -> dict[str, Any]:
    def pgd(relative_offset: int) -> int:
        return pgd_offset(player_game_data_offset, relative_offset)

    stats = [
        read_u32_le(slot_data, pgd(PGD_REL_VIGOR + index * U32_SIZE))
        for index in range(PGD_STAT_COUNT)
    ]
    stats_named = dict(zip(STAT_NAMES, stats, strict=True))
    status_buildup = {
        name: read_u32_le(slot_data, pgd(offset))
        for name, offset in STATUS_BUILDUP_FIELDS
    }
    name = read_fixed_utf16z(
        slot_data,
        pgd(PGD_REL_CHARACTER_NAME),
        PGD_CHARACTER_NAME_UNITS,
    )
    face_data_buffer, face_body_fields = decode_face_body_fields(slot_data, face_magic_offset)
    decoded = {
        "version": read_u32_le(slot_data, FIXTURE_SLOT_VERSION_OFFSET),
        "saved_map_c30": read_u32_le(slot_data, FIXTURE_MAP_ID_OFFSET),
        "saved_map_id_text": map_id_text(slot_data, FIXTURE_MAP_ID_OFFSET),
        "health": read_u32_le(slot_data, pgd(PGD_REL_HEALTH)),
        "max_health": read_u32_le(slot_data, pgd(PGD_REL_MAX_HEALTH)),
        "max_base_health": read_u32_le(slot_data, pgd(PGD_REL_BASE_MAX_HEALTH)),
        "fp": read_u32_le(slot_data, pgd(PGD_REL_FP)),
        "max_fp": read_u32_le(slot_data, pgd(PGD_REL_MAX_FP)),
        "base_max_fp": read_u32_le(slot_data, pgd(PGD_REL_BASE_MAX_FP)),
        "stamina": read_u32_le(slot_data, pgd(PGD_REL_STAMINA)),
        "max_stamina": read_u32_le(slot_data, pgd(PGD_REL_MAX_STAMINA)),
        "base_max_stamina": read_u32_le(slot_data, pgd(PGD_REL_BASE_MAX_STAMINA)),
        "stats": stats,
        "stats_named": stats_named,
        "humanity": read_u32_le(slot_data, pgd(PGD_REL_HUMANITY)),
        "level": read_u32_le(slot_data, pgd(PGD_REL_LEVEL)),
        "runes": read_u32_le(slot_data, pgd(PGD_REL_RUNES)),
        "souls": read_u32_le(slot_data, pgd(PGD_REL_RUNES)),
        "rune_memory": read_u32_le(slot_data, pgd(PGD_REL_RUNE_MEMORY)),
        "soulmemory": read_u32_le(slot_data, pgd(PGD_REL_RUNE_MEMORY)),
        "status_buildup": status_buildup,
        "chr_type": read_u32_le(slot_data, pgd(PGD_REL_CHARACTER_TYPE)),
        "name": name,
        "name_len": len(name),
        "name_empty_like": name_empty_like(name),
        "gender": read_u8(slot_data, pgd(PGD_REL_GENDER)),
        "archetype": read_u8(slot_data, pgd(PGD_REL_ARCHETYPE)),
        "voice_type": read_u8(slot_data, pgd(PGD_REL_VOICE_TYPE)),
        "starting_gift": read_u8(slot_data, pgd(PGD_REL_STARTING_GIFT)),
        "unlocked_talisman_slots": read_u8(slot_data, pgd(PGD_REL_UNLOCKED_TALISMAN_SLOTS)),
        "spirit_ash_level": read_u8(slot_data, pgd(PGD_REL_MATCHMAKING_SPIRIT_ASHES_LEVEL)),
        "max_crimson_flask_count": read_u8(slot_data, pgd(PGD_REL_MAX_CRIMSON_FLASK_COUNT)),
        "max_cerulean_flask_count": read_u8(slot_data, pgd(PGD_REL_MAX_CERULEAN_FLASK_COUNT)),
        "face_data_magic": face_data_buffer[:len(FACE_MAGIC)].decode("ascii", errors="replace"),
        "face_data_version": read_u32_le(face_data_buffer, len(FACE_MAGIC)),
        "face_data_buffer_size": read_u32_le(face_data_buffer, len(FACE_MAGIC) + U32_SIZE),
        "face_data_buffer_sha256": hashlib.sha256(face_data_buffer).hexdigest(),
        "face_data_buffer_hex": face_data_buffer.hex(),
        "face_body_fields": face_body_fields,
    }
    return {
        "layout": layout,
        "decoded_fields": decoded,
        "decoded_field_sources": DECODED_FIELD_SOURCES,
        "layout_offsets": {
            "slot_version": f"0x{FIXTURE_SLOT_VERSION_OFFSET:x}",
            "slot_map_id": f"0x{FIXTURE_MAP_ID_OFFSET:x}",
            "player_game_data": f"0x{player_game_data_offset:x}",
            "face_magic": f"0x{face_magic_offset:x}",
            "face_data_buffer_size": f"0x{FACE_DATA_BUFFER_SIZE:x}",
        },
        "reference": SL2_BT_REFERENCE_URL,
    }


def decode_sl2_bt_fixture_fields(slot_data: bytes) -> dict[str, Any]:
    # Fixture fast-path: only trust the hard-coded fixture offsets when the PGD
    # they point at actually validates -- otherwise a coincidental FACE byte at
    # FIXTURE_FACE_MAGIC_OFFSET would mis-decode a differently-laid-out live save.
    face_at_fixture_offset = slot_data[FIXTURE_FACE_MAGIC_OFFSET:FIXTURE_FACE_MAGIC_OFFSET + len(FACE_MAGIC)] == FACE_MAGIC
    if (
        len(slot_data) >= FIXTURE_MIN_LENGTH
        and face_at_fixture_offset
        and plausible_player_game_data(slot_data, FIXTURE_PLAYER_GAME_DATA_OFFSET, FIXTURE_FACE_MAGIC_OFFSET)
    ):
        return decode_fields_at(
            slot_data,
            FIXTURE_PLAYER_GAME_DATA_OFFSET,
            FIXTURE_FACE_MAGIC_OFFSET,
            "sl2-bt-er-save-file-readers-fixture",
        )

    # Primary live path: anchor on the CharacterName / PlayerGameData fields
    # directly (FACE-independent).  This locates the true PGD for every occupied
    # slot, including characters whose FaceDataBuffer is missing or in an
    # unexpected format next to the PGD (e.g. 150-Banon slot 1 "Dark Moon Bean").
    pgd_candidates = scan_player_game_data_offsets(slot_data)
    if pgd_candidates:
        scored = [
            (
                candidate_score(slot_data, player_game_data_offset, face_magic_offset_for_pgd(slot_data, player_game_data_offset)),
                player_game_data_offset,
            )
            for player_game_data_offset in pgd_candidates
        ]
        _, player_game_data_offset = max(scored, key=lambda item: item[0])
        face_magic_offset = face_magic_offset_for_pgd(slot_data, player_game_data_offset)
        return decode_fields_at(
            slot_data,
            player_game_data_offset,
            face_magic_offset,
            "sl2-bt-live-user-data",
        )

    # Fallback: FACE-anchored scan (kept for saves where the name-anchored core
    # validator finds nothing but a co-located valid FACE buffer exists).
    candidates: list[tuple[tuple[int, int], int, int]] = []
    for face_magic_offset in find_face_magic_offsets(slot_data):
        for player_game_data_offset in candidate_player_game_data_offsets(slot_data, face_magic_offset):
            candidates.append(
                (
                    candidate_score(slot_data, player_game_data_offset, face_magic_offset),
                    player_game_data_offset,
                    face_magic_offset,
                )
            )
    if candidates:
        _, player_game_data_offset, face_magic_offset = max(candidates, key=lambda item: item[0])
        return decode_fields_at(
            slot_data,
            player_game_data_offset,
            face_magic_offset,
            "sl2-bt-live-user-data",
        )
    return {"layout": "unknown", "decoded_fields": {}}


def decode_save_slot(data: bytes, save_path: Path, slot: int) -> dict[str, Any]:
    slot_data, source = extract_slot(data, slot)
    fixture = decode_sl2_bt_fixture_fields(slot_data)
    face_offsets = find_face_magic_offsets(slot_data)
    return {
        "source_path": str(save_path),
        "slot": slot,
        "slot_sha256": hashlib.sha256(slot_data).hexdigest(),
        "slot_size": len(slot_data),
        "source": source,
        "face_magic_offsets": face_offsets,
        **fixture,
    }


def choose_auto_slot(data: bytes, save_path: Path) -> dict[str, Any]:
    entries = bnd4_entries(data)
    if not entries:
        return decode_save_slot(data, save_path, 0)
    for slot in range(SLOT_COUNT):
        result = decode_save_slot(data, save_path, slot)
        fields = result.get("decoded_fields") or {}
        if fields and fields.get("name_empty_like") is False:
            return result
    raise SystemExit("no non-empty-like character slot found in save file")


def parse_slot(value: str) -> int | str:
    if value == "auto":
        return value
    try:
        slot = int(value, 10)
    except ValueError as exc:
        raise argparse.ArgumentTypeError("slot must be 0-9 or auto") from exc
    if slot < 0 or slot >= SLOT_COUNT:
        raise argparse.ArgumentTypeError("slot must be 0-9 or auto")
    return slot


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--save", required=True, type=Path, help="ER0000.sl2/.co2 BND4 or extracted slot file")
    parser.add_argument("--slot", type=parse_slot, default=0, metavar="0-9|auto")
    parser.add_argument("--output", type=Path, help="write JSON here instead of stdout")
    args = parser.parse_args()

    data = args.save.read_bytes()
    result = choose_auto_slot(data, args.save) if args.slot == "auto" else decode_save_slot(data, args.save, args.slot)
    text = json.dumps(result, indent=2, sort_keys=True) + "\n"
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(text, encoding="utf-8")
    else:
        print(text, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
