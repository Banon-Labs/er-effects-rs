#!/usr/bin/env python3
"""Offline census of every regulation.bin row the game considers auto-replenishable.

`GetEquipParamReplenishType` (1.16.2 @0x14023de20) is the only eligibility gate the storage-box
refill path uses. It reads exactly two bytes:

    item id high nibble 0x0  -> EquipParamWeapon.autoReplenishType  @ +0x197
    item id high nibble 0x4  -> EquipParamGoods.autoReplenishType   @ +0x6e

anything else -> None(0). This script decrypts regulation.bin with nothing but python3 + a vendored
AES key and reports how many rows return non-zero, because the durable state tracker those rows feed
(`CS::EquipGameData::ItemReplenishStateTracker`) is a DLFixedVector capped at 2048 entries that
DLPanics -- crashes the game -- on overflow.

Env overrides: ER_REGULATION_BIN (full path), ER_GAME_DIR (directory holding regulation.bin).
"""

import os
import struct
import subprocess
import sys
from pathlib import Path

# SoulsFormats RegulationKey.EldenRing.
REGULATION_KEY = (
    "99bffc366a6bc8c6f5827d093602d676c42892a01c207fb024d3af4e493fef99"
)

# GetEquipParamReplenishType, 1.16.2. Byte offset of autoReplenishType within each row.
WEAPON_AUTOREPLENISH_OFFSET = 0x197
GOODS_AUTOREPLENISH_OFFSET = 0x6E

# CS::ItemReplenishStateTracker.entries is ItemReplenishStateEntry[2048]; both InsertSorted
# (0x14023df20) and the append path (0x14023e270) DLPanic when count+1 exceeds this.
TRACKER_CAPACITY = 0x800


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


def decrypt(path: Path) -> bytes:
    """AES-256-CBC. IV is the first 16 bytes of the file, ciphertext is the rest."""
    raw = path.read_bytes()
    iv, body = raw[:16], raw[16:]
    body = body[: len(body) - len(body) % 16]
    plain = subprocess.run(
        [
            "openssl", "enc", "-d", "-aes-256-cbc", "-nopad",
            "-K", REGULATION_KEY,
            "-iv", iv.hex(),
        ],
        input=body,
        capture_output=True,
        check=True,
        timeout=30,
    ).stdout
    if plain[:4] != b"DCX\0":
        raise SystemExit(f"decrypt failed: expected DCX magic, got {plain[:4]!r}")
    return plain


def decompress_dcx(dcx: bytes) -> bytes:
    """Big-endian DCX wrapper around a zstd payload."""
    uncompressed_size = struct.unpack_from(">i", dcx, 0x1C)[0]
    compressed_size = struct.unpack_from(">i", dcx, 0x20)[0]
    data_offset = struct.unpack_from(">i", dcx, 0x14)[0]
    if dcx[0x24:0x28] != b"DCP\0" or dcx[0x28:0x2C] != b"ZSTD":
        raise SystemExit("unexpected DCX format (expected ZSTD)")
    from compression import zstd  # python 3.14 stdlib

    # Slice to exactly compressed_size: the -nopad AES decrypt leaves trailing bytes past the
    # zstd frame, and the stdlib decoder treats those as a second frame ("Unknown frame descriptor").
    out = zstd.decompress(dcx[data_offset : data_offset + compressed_size])
    if len(out) != uncompressed_size:
        raise SystemExit(f"zstd size mismatch: {len(out)} != {uncompressed_size}")
    if out[:4] != b"BND4":
        raise SystemExit(f"expected BND4, got {out[:4]!r}")
    return out


def read_bnd4(bnd: bytes) -> dict[str, bytes]:
    """Every entry in the regulation BND4 is stored uncompressed."""
    file_count = struct.unpack_from("<i", bnd, 0x0C)[0]
    files: dict[str, bytes] = {}
    for i in range(file_count):
        entry = 0x40 + i * 0x24
        size = struct.unpack_from("<q", bnd, entry + 0x10)[0]
        data_offset = struct.unpack_from("<I", bnd, entry + 0x18)[0]
        name_offset = struct.unpack_from("<I", bnd, entry + 0x20)[0]
        end = bnd.index(b"\0\0", name_offset)
        if (end - name_offset) % 2:
            end += 1
        name = bnd[name_offset:end].decode("utf-16-le")
        files[name] = bnd[data_offset : data_offset + size]
    return files


def param_rows(param: bytes) -> list[tuple[int, bytes]]:
    """Row size is not stored; derive it from the gap between consecutive row data offsets."""
    row_count = struct.unpack_from("<H", param, 0x0A)[0]
    # Row index entry, 24 bytes: id<i, pad<i, dataOffset<q, nameOffset<q.
    index = [
        struct.unpack_from("<iiqq", param, 0x40 + i * 24) for i in range(row_count)
    ]
    offsets = sorted(o for _, _, o, _ in index)
    if len(offsets) < 2:
        return []
    row_size = min(b - a for a, b in zip(offsets, offsets[1:]))
    return [(row_id, param[off : off + row_size]) for row_id, _, off, _ in index]


def census(name: str, param: bytes, offset: int) -> dict[int, int]:
    """Count rows by autoReplenishType value."""
    by_type: dict[int, int] = {}
    for _row_id, row in param_rows(param):
        if len(row) <= offset:
            continue
        by_type[row[offset]] = by_type.get(row[offset], 0) + 1
    return by_type


def main() -> int:
    path = locate_regulation()
    print(f"regulation: {path} ({path.stat().st_size} bytes)")
    files = read_bnd4(decompress_dcx(decrypt(path)))

    targets = [
        ("EquipParamWeapon", WEAPON_AUTOREPLENISH_OFFSET),
        ("EquipParamGoods", GOODS_AUTOREPLENISH_OFFSET),
    ]
    eligible_total = 0
    for stem, offset in targets:
        # BND4 names are Windows paths ("N:\\GR\\...\\EquipParamGoods.param"); Path().name does not
        # split on backslash under Linux, so take the basename by hand.
        wanted = f"{stem.lower()}.param"
        match = next(
            (n for n in files if n.replace("\\", "/").rsplit("/", 1)[-1].lower() == wanted),
            None,
        )
        if match is None:
            print(f"{stem}: NOT FOUND in regulation BND4")
            continue
        by_type = census(stem, files[match], offset)
        total = sum(by_type.values())
        eligible = sum(v for k, v in by_type.items() if k != 0)
        eligible_total += eligible
        breakdown = ", ".join(
            f"type {k}={v}" for k, v in sorted(by_type.items()) if k != 0
        )
        print(
            f"{stem}: {total} rows, +0x{offset:x} eligible={eligible}"
            + (f" ({breakdown})" if breakdown else "")
        )

    print(f"\nELIGIBLE ROWS TOTAL: {eligible_total}")
    print(f"TRACKER CAPACITY:    {TRACKER_CAPACITY}")
    if eligible_total > TRACKER_CAPACITY:
        over = eligible_total - TRACKER_CAPACITY
        print(
            f"VERDICT: OVERFLOW by {over}. Marking every eligible row DLPanics the game.\n"
            "         A 'mark everything' hotkey MUST be scoped to a bounded set."
        )
    else:
        print("VERDICT: fits, with %d entries of headroom." % (TRACKER_CAPACITY - eligible_total))
    return 0


if __name__ == "__main__":
    sys.exit(main())
