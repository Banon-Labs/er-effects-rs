#!/usr/bin/env python3
"""Print a save container's on-disk per-slot occupancy evidence, side by side.

Why this exists. Two readers in this repo answer "which character slots does this
container hold", and on 2026-08-25 they disagreed on the user's own file:

* `er_save_loader::bnd4::active_slots` reads the `USER_DATA010` occupancy bitmap and
  reported one occupant (slot 3) for `~/Downloads/ER0000.co2`;
* `scripts/dump-save-slots.py` decodes each `USER_DATA00N` body and found ten.

The game agrees with the second one, so the System>Quit "Load Character from File"
preview offered one row out of ten. A disagreement like that is invisible until
somebody prints both, which is what this does -- plus the raw bytes the bitmap read
lands on, so a wrong offset is distinguishable from a wrong file.

It is read-only: it opens the container, never writes one.

    python3 scripts/er-save-active-slots.py <ER0000.sl2|co2> [more...]
    python3 scripts/er-save-active-slots.py --selftest
"""

from __future__ import annotations

import argparse
import struct
import sys
from pathlib import Path

ENTRY_MD5_LEN = 0x10
SAVE_SLOT_COUNT = 10
# Mirrors crates/er-save-loader/src/bnd4.rs. Kept as named constants so a drift between
# this diagnostic and the crate is visible as a changed literal, not as a silent skew.
HDR_FILE_COUNT_OFF = 0x0C
HDR_HEADER_SIZE_OFF = 0x10
HDR_FILE_HEADER_SIZE_OFF = 0x20
ENT_SIZE_OFF = 0x08
ENT_DATA_OFFSET_OFF = 0x10
ENT_NAME_OFFSET_OFF = 0x14
USER_DATA010_MENU_SAVE_LOAD_LEN_OFF = 0x150
USER_DATA010_MENU_SAVE_LOAD_DATA_AFTER_LEN_OFF = 0x154


def parse_entries(data: bytes) -> list[tuple[str, int, int]]:
    if data[:4] != b"BND4":
        raise ValueError("not a BND4 container")
    header_size = struct.unpack_from("<q", data, HDR_HEADER_SIZE_OFF)[0]
    stride = struct.unpack_from("<q", data, HDR_FILE_HEADER_SIZE_OFF)[0]
    count = struct.unpack_from("<i", data, HDR_FILE_COUNT_OFF)[0]
    entries: list[tuple[str, int, int]] = []
    for index in range(count):
        head = header_size + index * stride
        size = struct.unpack_from("<q", data, head + ENT_SIZE_OFF)[0]
        offset = struct.unpack_from("<i", data, head + ENT_DATA_OFFSET_OFF)[0]
        name_offset = struct.unpack_from("<i", data, head + ENT_NAME_OFFSET_OFF)[0]
        units = []
        cursor = name_offset
        while True:
            unit = struct.unpack_from("<H", data, cursor)[0]
            if unit == 0:
                break
            units.append(unit)
            cursor += 2
        entries.append(("".join(map(chr, units)), offset, size))
    return entries


def entry_body(data: bytes, name: str) -> bytes | None:
    for entry_name, offset, size in parse_entries(data):
        if entry_name == name:
            return data[offset + ENTRY_MD5_LEN : offset + size]
    return None


def active_slot_bytes(data: bytes) -> tuple[bytes, int, int]:
    """The crate's read, replayed: length prefix at 0x150, bitmap right after the blob."""
    body = entry_body(data, "USER_DATA010")
    if body is None:
        raise ValueError("no USER_DATA010 entry")
    blob_len = struct.unpack_from("<I", body, USER_DATA010_MENU_SAVE_LOAD_LEN_OFF)[0]
    offset = USER_DATA010_MENU_SAVE_LOAD_DATA_AFTER_LEN_OFF + blob_len
    return body[offset : offset + SAVE_SLOT_COUNT], blob_len, offset


def slot_body_is_populated(data: bytes, slot: int) -> bool:
    """A cheap independent occupancy witness: does `USER_DATA00N` carry any nonzero bytes
    in its first 4 KiB? An unused slot body is a zero fill, so this needs no PGD layout
    knowledge and cannot drift with it."""
    body = entry_body(data, f"USER_DATA{slot:03d}")
    if body is None:
        return False
    return any(body[:0x1000])


# --- the two PlayerGameData LOCATORS this repo ships, replayed side by side ------------------
#
# `er_save_loader::stats::located_stat_block` walks the body byte by byte and accepts the first
# offset where eight in-range attributes sum to `level + 79`. `SerializedSaveSlot::player_game_data`
# (er-quickload) instead finds the leading `FACE` magics and searches a fixed window before each
# one. The second can only find a character whose FaceData happens to land inside that window, so
# where they disagree the face-window locator is the one to distrust.
RUNE_LEVEL_BASE = 79
MIN_ATTR = 1
MAX_ATTR = 99
MAX_RUNE_LEVEL = 713
STATS_PGD_STAT_BASE = 0x3C  # stats.rs numbering; the bnd4 family's PGD base is this + 8
STATS_PGD_LEVEL = 0x68
SAVE_FACE_MAGIC = b"FACE"
SAVE_PGD_SCAN_LEADING_FACE_COUNT = 4
SAVE_PGD_FACE_DELTA_WINDOW_LOW = 0xA000
SAVE_PGD_FACE_DELTA_WINDOW_HIGH = 0xA600


def rune_invariant_pgd_offsets(body: bytes) -> list[int]:
    """Every offset satisfying the Rune Level invariant, in body order (stats.rs takes the first)."""
    hits: list[int] = []
    limit = len(body) - STATS_PGD_STAT_BASE
    for base in range(max(limit, 0)):
        total = 0
        ok = True
        for index in range(8):
            at = base + index * 4
            value = int.from_bytes(body[at : at + 4], "little", signed=True)
            if not (MIN_ATTR <= value <= MAX_ATTR):
                ok = False
                break
            total += value
        if not ok:
            continue
        at = base + STATS_PGD_LEVEL - STATS_PGD_STAT_BASE
        level = int.from_bytes(body[at : at + 4], "little", signed=True)
        if level != total - RUNE_LEVEL_BASE or not (MIN_ATTR <= level <= MAX_RUNE_LEVEL):
            continue
        hits.append(base - STATS_PGD_STAT_BASE)
        if len(hits) >= 4:
            break
    return hits


def leading_face_offsets(body: bytes) -> list[int]:
    offsets: list[int] = []
    at = 0
    while len(offsets) < SAVE_PGD_SCAN_LEADING_FACE_COUNT:
        found = body.find(SAVE_FACE_MAGIC, at)
        if found < 0:
            break
        offsets.append(found)
        at = found + 1
    return offsets


def face_window_finds(body: bytes, pgd_offsets: list[int]) -> bool:
    """True when at least one real PGD lies inside a leading face magic's search window --
    i.e. when the er-quickload locator would have found this character at all."""
    faces = leading_face_offsets(body)
    for face in faces:
        low = face - SAVE_PGD_FACE_DELTA_WINDOW_HIGH
        high = face - SAVE_PGD_FACE_DELTA_WINDOW_LOW
        for pgd in pgd_offsets:
            # +8: stats.rs's PGD base sits 8 bytes before the bnd4/er-quickload one.
            if low <= pgd + 8 <= high:
                return True
    return False


# `SerializedPlayerGameData::is_plausible_core` (er-quickload), replayed. Its offsets are the
# bnd4/er-quickload numbering, which is the stats.rs PGD base + 8.
BND4_PGD_FROM_STATS_PGD = 8
PGD_HEALTH = 0x08
PGD_MAX_HEALTH = 0x0C
PGD_BASE_MAX_HEALTH = 0x10
PGD_STAT_BASE = 0x34
PGD_LEVEL = 0x60
PGD_NAME = 0x94
PGD_NAME_BYTES = 0x20
PGD_GENDER = 0xB6
PGD_MAX_CRIMSON = 0xF9
PGD_MAX_CERULEAN = 0xFA
PGD_MIN_SIZE = 0x1B0


def _u32(body: bytes, at: int) -> int | None:
    chunk = body[at : at + 4]
    return int.from_bytes(chunk, "little") if len(chunk) == 4 else None


def plausible_core(body: bytes, pgd: int) -> bool:
    """True when the er-quickload acceptance test passes at this PGD offset."""
    if pgd + PGD_MIN_SIZE > len(body):
        return False
    name = body[pgd + PGD_NAME : pgd + PGD_NAME + PGD_NAME_BYTES]
    if len(name) != PGD_NAME_BYTES:
        return False
    units = []
    for index in range(0, PGD_NAME_BYTES, 2):
        unit = int.from_bytes(name[index : index + 2], "little")
        if unit == 0:
            break
        units.append(unit)
    if not units or all(unit == ord("_") for unit in units):
        return False
    if any(chr(unit).isprintable() is False for unit in units):
        return False
    values = [_u32(body, pgd + off) for off in (PGD_LEVEL, PGD_HEALTH, PGD_MAX_HEALTH, PGD_BASE_MAX_HEALTH)]
    if any(value is None for value in values):
        return False
    level, health, max_health, base_max_health = values
    gender = body[pgd + PGD_GENDER] if pgd + PGD_GENDER < len(body) else 255
    crimson = body[pgd + PGD_MAX_CRIMSON] if pgd + PGD_MAX_CRIMSON < len(body) else 255
    cerulean = body[pgd + PGD_MAX_CERULEAN] if pgd + PGD_MAX_CERULEAN < len(body) else 255
    stats = [_u32(body, pgd + PGD_STAT_BASE + index * 4) for index in range(8)]
    if any(stat is None for stat in stats):
        return False
    return (
        1 <= level <= 713
        and 1 <= health <= 100_000
        and 1 <= max_health <= 100_000
        and 1 <= base_max_health <= 100_000
        and health <= max_health
        and base_max_health <= max_health
        and gender <= 1
        and crimson <= 14
        and cerulean <= 14
        and all(1 <= stat <= 99 for stat in stats)
    )


def report(path: Path, deep: bool) -> None:
    data = path.read_bytes()
    print(f"== {path} ({len(data)} bytes)")
    try:
        bitmap, blob_len, offset = active_slot_bytes(data)
    except Exception as err:  # noqa: BLE001 - a diagnostic reports, it does not raise
        print(f"   USER_DATA010 bitmap read FAILED: {err}")
        bitmap, blob_len, offset = b"", -1, -1
    if offset >= 0:
        print(
            f"   USER_DATA010 blob_len=0x{blob_len:x} bitmap_off=0x{offset:x} "
            f"bytes={list(bitmap)}"
        )
    nonzero = [slot for slot in range(SAVE_SLOT_COUNT) if slot_body_is_populated(data, slot)]
    flagged = [slot for slot, byte in enumerate(bitmap) if byte]
    print(f"   bitmap says occupied : {flagged}")
    print(f"   slot bodies non-empty: {nonzero}")
    if flagged != nonzero:
        print("   *** THE TWO WITNESSES DISAGREE -- the bitmap read is not describing this file")
    if not deep:
        return
    rune_ok: list[int] = []
    face_ok: list[int] = []
    for slot in range(SAVE_SLOT_COUNT):
        body = entry_body(data, f"USER_DATA{slot:03d}")
        if body is None:
            continue
        pgds = rune_invariant_pgd_offsets(body)
        if not pgds:
            continue
        rune_ok.append(slot)
        faces = leading_face_offsets(body)
        found = face_window_finds(body, pgds)
        if found:
            face_ok.append(slot)
        deltas = [f"0x{face - (pgds[0] + BND4_PGD_FROM_STATS_PGD):x}" for face in faces]
        accepted = plausible_core(body, pgds[0] + BND4_PGD_FROM_STATS_PGD)
        print(
            f"   slot {slot}: pgd=0x{pgds[0]:x} leading_FACE={[hex(f) for f in faces]} "
            f"delta_from_pgd={deltas} face_window_locator={'HIT' if found else 'MISS'} "
            f"is_plausible_core@rune_pgd={'PASS' if accepted else 'FAIL'}"
        )
    print(f"   rune-invariant locator finds: {rune_ok}")
    print(f"   FACE-window locator finds   : {face_ok}")
    if rune_ok != face_ok:
        print(
            "   *** THE TWO LOCATORS DISAGREE -- the FACE-window locator drops "
            f"{sorted(set(rune_ok) - set(face_ok))}"
        )


def selftest() -> int:
    """Prove the bitmap reader on a synthesised container rather than on a game file.

    Game-derived bytes are never versioned here, so the fixture is generated: a minimal
    BND4 with one `USER_DATA010` entry whose bitmap marks slots 0 and 3.
    """
    want = [1, 0, 0, 1, 0, 0, 0, 0, 0, 0]
    blob_len = 0x20
    body = bytearray(0x400)
    struct.pack_into("<I", body, USER_DATA010_MENU_SAVE_LOAD_LEN_OFF, blob_len)
    bitmap_at = USER_DATA010_MENU_SAVE_LOAD_DATA_AFTER_LEN_OFF + blob_len
    body[bitmap_at : bitmap_at + SAVE_SLOT_COUNT] = bytes(want)

    header_size = 0x40
    stride = 0x20
    entry_head = header_size
    name_at = header_size + stride
    name = "USER_DATA010".encode("utf-16-le") + b"\0\0"
    data_at = name_at + len(name)
    total = data_at + ENTRY_MD5_LEN + len(body)
    blob = bytearray(total)
    blob[0:4] = b"BND4"
    struct.pack_into("<i", blob, HDR_FILE_COUNT_OFF, 1)
    struct.pack_into("<q", blob, HDR_HEADER_SIZE_OFF, header_size)
    struct.pack_into("<q", blob, HDR_FILE_HEADER_SIZE_OFF, stride)
    struct.pack_into("<q", blob, entry_head + ENT_SIZE_OFF, ENTRY_MD5_LEN + len(body))
    struct.pack_into("<i", blob, entry_head + ENT_DATA_OFFSET_OFF, data_at)
    struct.pack_into("<i", blob, entry_head + ENT_NAME_OFFSET_OFF, name_at)
    blob[name_at : name_at + len(name)] = name
    blob[data_at + ENTRY_MD5_LEN :] = bytes(body)

    got, got_len, got_off = active_slot_bytes(bytes(blob))
    assert got_len == blob_len, f"blob_len {got_len:#x} != {blob_len:#x}"
    assert got_off == bitmap_at, f"bitmap_off {got_off:#x} != {bitmap_at:#x}"
    assert list(got) == want, f"bitmap {list(got)} != {want}"
    print("er-save-active-slots selftest OK")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("saves", nargs="*", type=Path)
    parser.add_argument("--selftest", action="store_true")
    parser.add_argument(
        "--deep",
        action="store_true",
        help="also replay both PlayerGameData locators per slot (slow: a byte scan per body)",
    )
    args = parser.parse_args()
    if args.selftest:
        return selftest()
    if not args.saves:
        parser.error("give at least one save container, or --selftest")
    for path in args.saves:
        report(path, args.deep)
    return 0


if __name__ == "__main__":
    sys.exit(main())
