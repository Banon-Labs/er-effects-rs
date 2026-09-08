#!/usr/bin/env python3
"""Read PARAM row ids straight out of the installed `regulation.bin`, offline.

No Smithbox, no dotnet, no WitchyBND, no pip: `python3` (3.14+, for the stdlib
`compression.zstd`) plus `openssl` are the whole toolchain. The four stages --
AES-256-CBC, DCX/zstd, BND4, PARAM -- are the ones validated against the 1.16.2
install; every one of them self-checks against a magic or a length, so a wrong
key or a format change fails loudly rather than producing plausible garbage.

    python3 scripts/regulation-params.py EquipParamWeapon
    python3 scripts/regulation-params.py --contains 16110217 EquipParamWeapon

Field names need a paramdef this does not have. Row IDS do not, which is what
makes this enough to answer "is this id a row at all" -- the question that
decides whether an id the game handed us can be looked up by name.
"""

import argparse
import os
import struct
import subprocess
import sys
import tempfile

# `compression.zstd` is Python 3.14 and newer only (PEP 784), and this import used to be bare.
#
# The dev box runs 3.14, so it resolved here and every local run was green. GitHub's
# ubuntu-latest ships an older 3.x, where the same line raises
# `ModuleNotFoundError: No module named 'compression'` -- at import time, before any argument
# parsing, so it took down every consumer that merely imports this module for its PARAM
# readers and never decompresses anything. Measured on PR #388, run 33793058851: it reached
# check.sh through check-moveset-table.py -> er-moveset-table-gen.py -> er-param-read.py:16
# -> here, and both the gate and its own `--selftest` died with a raw traceback that reads
# like a real failure rather than a missing interpreter feature.
#
# The worst part was the selftest: a gate whose self-test depends on an interpreter the
# runner does not have cannot detect its own unavailability, so it reads as proof when it is
# silence. Deferring the failure to the one function that actually needs zstd lets an absent
# decompressor be reported the way this file already reports an absent regulation -- see
# `missing_regulation` in diff-regulation-params.py: a printed skip, never a silent exit 0.
try:
    from compression import zstd
except ModuleNotFoundError as _zstd_import_error:  # pragma: no cover - interpreter-dependent
    zstd = None
    ZSTD_UNAVAILABLE = (
        f"this interpreter has no `compression.zstd` ({_zstd_import_error}); it is stdlib "
        f"in Python 3.14+ (PEP 784) and this is "
        f"{sys.version_info.major}.{sys.version_info.minor}"
    )
else:
    ZSTD_UNAVAILABLE = None


class ZstdUnavailable(RuntimeError):
    """Raised instead of decompressing when the interpreter has no zstd.

    A distinct type so a caller can tell "I could not look" apart from "I looked and the
    answer is no" -- the same distinction ER_ALLOW_MISSING_REGULATION draws for the game
    file. Catch this to skip; do not catch it to pass.
    """


def require_zstd():
    """Fail loudly, and only at the point of use, when zstd is missing."""
    if zstd is None:
        raise ZstdUnavailable(ZSTD_UNAVAILABLE)

# SoulsFormats `RegulationKey.EldenRing`.
REGULATION_KEY = "99BFFC366A6BC8C6F5827D093602D676C42892A01C207FB024D3AF4E493FEF99"

DEFAULT_REGULATION = os.path.join(
    os.environ.get(
        "ER_GAME_DIR",
        os.path.expanduser("~/.local/share/Steam/steamapps/common/ELDEN RING/Game"),
    ),
    "regulation.bin",
)


def decrypt(path):
    """Stage 1: AES-256-CBC, IV = the file's first 16 bytes."""
    raw = open(path, "rb").read()
    iv, ciphertext = raw[:16], raw[16:]
    ciphertext = ciphertext[: len(ciphertext) // 16 * 16]
    with tempfile.TemporaryDirectory() as work:
        enc = os.path.join(work, "ct")
        dec = os.path.join(work, "pt")
        open(enc, "wb").write(ciphertext)
        subprocess.run(
            ["openssl", "enc", "-d", "-aes-256-cbc", "-nopad",
             "-K", REGULATION_KEY, "-iv", iv.hex(), "-in", enc, "-out", dec],
            check=True,
            # Two megabytes of AES is instant; a minute of it means openssl is stuck on
            # something that is not this file.
            timeout=30,
        )
        plain = open(dec, "rb").read()
    if plain[:4] != b"DCX\0":
        raise SystemExit(f"decrypt produced {plain[:8]!r}, not a DCX -- wrong key or file")
    return plain


def dcx_unpack(dcx):
    """Stage 2: DCX header (big-endian) wrapping a zstd payload.

    The payload is sliced to its declared compressed size rather than "to the end of the
    file": the stdlib decompressor rejects trailing bytes after the frame, and the DCX
    carries a DCA footer after it.
    """
    uncompressed = struct.unpack_from(">I", dcx, 0x1C)[0]
    compressed = struct.unpack_from(">I", dcx, 0x20)[0]
    data_offset = struct.unpack_from(">I", dcx, 0x14)[0]
    if dcx[0x24:0x28] != b"DCP\0" or dcx[0x28:0x2C] != b"ZSTD":
        raise SystemExit(f"unexpected DCX compression {dcx[0x24:0x2C]!r}")
    require_zstd()
    out = zstd.decompress(dcx[data_offset : data_offset + compressed])
    if len(out) != uncompressed or out[:4] != b"BND4":
        raise SystemExit(f"DCX payload is {len(out)} bytes of {out[:4]!r}, wanted {uncompressed} of BND4")
    return out


def bnd4_entries(bnd):
    """Stage 3: BND4 directory -> {name: bytes}. Every regulation entry is stored uncompressed."""
    count = struct.unpack_from("<i", bnd, 0x0C)[0]
    files = {}
    for index in range(count):
        header = 0x40 + index * 0x24
        size = struct.unpack_from("<q", bnd, header + 0x10)[0]
        data_offset = struct.unpack_from("<I", bnd, header + 0x18)[0]
        name_offset = struct.unpack_from("<I", bnd, header + 0x20)[0]
        end = name_offset
        while bnd[end : end + 2] != b"\x00\x00":
            end += 2
        files[bnd[name_offset:end].decode("utf-16-le")] = bnd[data_offset : data_offset + size]
    return files


def param_row_ids(param):
    """Stage 4: PARAM row index -> ids, in file order."""
    row_count = struct.unpack_from("<H", param, 0x0A)[0]
    return [struct.unpack_from("<i", param, 0x40 + index * 24)[0] for index in range(row_count)]


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("params", nargs="*", default=["EquipParamWeapon"],
                        help="param names, with or without the .param suffix")
    parser.add_argument("--regulation", default=DEFAULT_REGULATION)
    parser.add_argument("--contains", type=int, action="append", default=[],
                        help="report whether this row id exists (repeatable)")
    parser.add_argument("--write-ids", metavar="DIR",
                        help="write <param>.ids, one row id per line, into DIR")
    parser.add_argument("--list", action="store_true", help="list every param in the regulation and exit")
    args = parser.parse_args()

    files = bnd4_entries(dcx_unpack(decrypt(args.regulation)))
    if args.list:
        for name in sorted(files):
            print(name)
        return 0

    status = 0
    for wanted in args.params:
        stem = wanted.removesuffix(".param")
        key = next((name for name in files if name.rsplit("\\", 1)[-1].removesuffix(".param") == stem), None)
        if key is None:
            print(f"{stem}: NOT FOUND among {len(files)} params")
            status = 1
            continue
        ids = param_row_ids(files[key])
        print(f"{stem}: {len(ids)} rows, {min(ids)}..{max(ids)}")
        present = set(ids)
        for row in args.contains:
            print(f"  {row}: {'present' if row in present else 'ABSENT'}")
        if args.write_ids:
            os.makedirs(args.write_ids, exist_ok=True)
            out = os.path.join(args.write_ids, f"{stem}.ids")
            open(out, "w").write("\n".join(str(row) for row in ids) + "\n")
            print(f"  ids -> {out}")
    return status


if __name__ == "__main__":
    sys.exit(main())
