#!/usr/bin/env python3
"""Fail when the installed game is a version the RVA bundle does not support.

Why this exists
---------------
On 2026-08-29 every product DLL died within a second of loading and it took
eight game launches to learn why.  The cause was one line of data:
`ERGameVersion::from_lang_version` in the sibling `fromsoftware-rs` checkout
accepts exactly two strings, "2.6.2.0" and "2.6.2.1", and the installed game
had become 2.7.0.0.  `eldenring::rva::get()` therefore panicked -- inside a
`LazyLock`, on whatever thread first touched any singleton -- so eight DLLs
each threw a `rust_panic` with an identical stack and no message anywhere a
human would look.

Nothing about that needed a game to discover.  Both facts are readable off the
disk: the version is in the PE resource directory of `eldenring.exe`, and the
supported list is a `match` arm in a file we have checked out.  This script
compares them and says so.

It deliberately refuses to guess.  If it cannot find the game, or cannot find
the sibling checkout, or cannot parse either, it says which one and exits 0 --
a missing game is not a failed gate, it is a machine without the game.  It
exits non-zero only for the case it exists to catch: both sides present,
readable, and disagreeing.
"""

from __future__ import annotations

import argparse
import os
import re
import struct
import sys
from pathlib import Path

# The `match (lang_id, version)` arms in the sibling's rva.rs.  Parsed, never
# assumed: the whole point is to read what the code actually accepts.
VERSION_ARM = re.compile(r'\(\s*LANG_ID_(?:EN|JP)\s*,\s*"([0-9]+(?:\.[0-9]+)*)"\s*\)')

DEFAULT_GAME = Path.home() / ".local/share/Steam/steamapps/common/ELDEN RING/Game/eldenring.exe"
DEFAULT_SIBLING = Path(__file__).resolve().parent.parent.parent / "fromsoftware-rs"
RVA_RS = "crates/eldenring/src/rva.rs"


class Unreadable(Exception):
    """Something we need is absent or unparseable.  Not a gate failure."""


def _u16(b: bytes, o: int) -> int:
    return struct.unpack_from("<H", b, o)[0]


def _u32(b: bytes, o: int) -> int:
    return struct.unpack_from("<I", b, o)[0]


def _sections(data: bytes) -> tuple[list[tuple[str, int, int, int]], int]:
    """Return (name, va, vsize, raw_ptr) per section, plus the resource dir RVA."""
    if data[:2] != b"MZ":
        raise Unreadable("not a PE image (no MZ)")
    pe = _u32(data, 0x3C)
    if data[pe : pe + 4] != b"PE\0\0":
        raise Unreadable("not a PE image (no PE signature)")
    nsec = _u16(data, pe + 6)
    optsz = _u16(data, pe + 20)
    opt = pe + 24
    magic = _u16(data, opt)
    if magic != 0x20B:
        raise Unreadable(f"not a PE32+ image (optional header magic {magic:#06x})")
    # Data directory 2 is the resource table; it starts 0x70 into a PE32+
    # optional header, and each entry is (rva, size).
    res_rva = _u32(data, opt + 0x70 + 2 * 8)
    off = opt + optsz
    out = []
    for i in range(nsec):
        e = data[off + i * 40 : off + (i + 1) * 40]
        name = e[:8].rstrip(b"\0").decode("latin1")
        vsz, va, rsz, rp = struct.unpack_from("<IIII", e, 8)
        out.append((name, va, max(vsz, rsz), rp))
    return out, res_rva


def _to_offset(sections, rva: int) -> int:
    for _name, va, size, rp in sections:
        if va <= rva < va + size:
            return rp + (rva - va)
    raise Unreadable(f"RVA {rva:#x} is outside every section")


def _walk_resources(data: bytes, sections, res_rva: int, base_off: int, depth: int):
    """Yield leaf (rva, size) for every VS_VERSIONINFO (type 16) resource."""
    off = base_off
    named = _u16(data, off + 12)
    ids = _u16(data, off + 14)
    for i in range(named + ids):
        e = off + 16 + i * 8
        name = _u32(data, e)
        entry = _u32(data, e + 4)
        # At depth 0 the id is the resource type; 16 == RT_VERSION.
        if depth == 0 and not (name & 0x80000000) and name != 16:
            continue
        if entry & 0x80000000:
            child = _to_offset(sections, res_rva) + (entry & 0x7FFFFFFF)
            yield from _walk_resources(data, sections, res_rva, child, depth + 1)
        else:
            leaf = _to_offset(sections, res_rva) + entry
            yield _u32(data, leaf), _u32(data, leaf + 4)


def product_version(exe: Path) -> str:
    """Read dwProductVersion out of the PE's VS_FIXEDFILEINFO."""
    try:
        data = exe.read_bytes()
    except OSError as err:
        raise Unreadable(f"cannot read {exe}: {err}") from err
    sections, res_rva = _sections(data)
    if not res_rva:
        raise Unreadable("no resource directory")
    for rva, size in _walk_resources(data, sections, res_rva, _to_offset(sections, res_rva), 0):
        blob_off = _to_offset(sections, rva)
        blob = data[blob_off : blob_off + size]
        sig = blob.find(struct.pack("<I", 0xFEEF04BD))
        if sig < 0:
            continue
        # VS_FIXEDFILEINFO: signature, strucVersion, then file version
        # (most/least) and product version (most/least), each a packed pair.
        prod_ms = _u32(blob, sig + 16)
        prod_ls = _u32(blob, sig + 20)
        return f"{prod_ms >> 16}.{prod_ms & 0xFFFF}.{prod_ls >> 16}.{prod_ls & 0xFFFF}"
    raise Unreadable("no VS_FIXEDFILEINFO in any version resource")


def supported_versions(rva_rs: Path) -> list[str]:
    try:
        text = rva_rs.read_text(encoding="utf-8")
    except OSError as err:
        raise Unreadable(f"cannot read {rva_rs}: {err}") from err
    found = VERSION_ARM.findall(text)
    if not found:
        raise Unreadable(f"no (LANG_ID_*, \"x.y.z.w\") arms in {rva_rs}")
    return found


def selftest() -> int:
    """Prove the two parsers on inputs whose answers are known."""
    failures = []

    # The version-arm parser, against the shape rva.rs actually uses.
    sample = '''
        fn from_lang_version(lang_id: u16, version: &str) -> Option<Self> {
            match (lang_id, version) {
                (LANG_ID_EN, "2.6.2.0") => Some(Self::Ww262),
                (LANG_ID_JP, "2.6.2.1") => Some(Self::Jp2621),
                _ => None,
            }
        }
    '''
    got = VERSION_ARM.findall(sample)
    if got != ["2.6.2.0", "2.6.2.1"]:
        failures.append(f"version-arm parser returned {got!r}, want ['2.6.2.0', '2.6.2.1']")

    # A file with no arms must raise rather than silently pass the gate.
    import tempfile

    with tempfile.TemporaryDirectory() as tmp:
        empty = Path(tmp) / "rva.rs"
        empty.write_text("// nothing here\n", encoding="utf-8")
        try:
            supported_versions(empty)
        except Unreadable:
            pass
        else:
            failures.append("an arm-less rva.rs did not raise Unreadable")

    # The PE reader, against this machine's own game if it is here.  A parser
    # that returns a plausible-looking string for the wrong reason is the
    # failure mode worth catching, so assert the shape, not a fixed value.
    exe = Path(os.environ.get("ER_GAME_EXE", DEFAULT_GAME))
    if exe.is_file():
        try:
            version = product_version(exe)
        except Unreadable as err:
            failures.append(f"PE reader failed on {exe}: {err}")
        else:
            if not re.fullmatch(r"\d+\.\d+\.\d+\.\d+", version):
                failures.append(f"PE reader returned {version!r}, not a four-part version")
            else:
                print(f"selftest: read {version} from {exe}")
    else:
        print(f"selftest: no game at {exe}; PE reader not exercised")

    for line in failures:
        print(f"SELFTEST FAIL: {line}")
    print(f"selftest: {len(failures)} failure(s)")
    return 1 if failures else 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--exe", type=Path, default=Path(os.environ.get("ER_GAME_EXE", DEFAULT_GAME)))
    ap.add_argument(
        "--sibling",
        type=Path,
        default=Path(os.environ.get("FROMSOFTWARE_RS_DIR", DEFAULT_SIBLING)),
        help="the fromsoftware-rs checkout this repo builds against",
    )
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()

    if args.selftest:
        return selftest()

    if not args.exe.is_file():
        print(f"SKIP: no game at {args.exe} (set ER_GAME_EXE to point at eldenring.exe)")
        return 0
    rva_rs = args.sibling / RVA_RS
    if not rva_rs.is_file():
        print(f"SKIP: no sibling checkout at {args.sibling} (set FROMSOFTWARE_RS_DIR)")
        return 0

    try:
        installed = product_version(args.exe)
        supported = supported_versions(rva_rs)
    except Unreadable as err:
        print(f"SKIP: {err}")
        return 0

    if installed in supported:
        print(f"OK: installed game {installed} is in the RVA bundle's supported set {supported}")
        return 0

    print(f"FAIL: the installed game is {installed}; {rva_rs} supports only {supported}.")
    print()
    print("Every DLL that touches a singleton will panic at load, because")
    print("eldenring::rva::get() resolves its LazyLock<RvaBundle> through")
    print("ERGameVersion::detect() and panics on a version it does not know:")
    print()
    print(f'    thread \'<unnamed>\' panicked: Unsupported game version {installed}')
    print()
    print("The panic is thrown inside a LazyLock on whichever thread touches a")
    print("singleton first, so it surfaces as an unattributed rust_panic with no")
    print("message in any log. Regenerate the bundle rather than catching it:")
    print()
    print("    cargo build --release -p binary-mapper --target x86_64-unknown-linux-gnu")
    print("    ./target/x86_64-unknown-linux-gnu/release/binary-mapper map \\")
    print(f"        --profile crates/eldenring/mapper-profile.toml \\")
    print(f"        --exe '{args.exe}' --output rust")
    return 1


if __name__ == "__main__":
    sys.exit(main())
