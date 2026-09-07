#!/usr/bin/env python3
"""Identify an installed `ersc.dll` and diff it against a reference build.

Seamless Co-op is third-party and the user updates it on their own schedule. Every
pin this repo holds into `ersc.dll` -- an RVA, a struct field offset, a byte
signature, a section name -- is only valid for the build it was measured against, and
nothing in a normal build or launch notices when that build is swapped underneath.
This is the read-only check that notices.

It answers three questions and nothing else:

  * Which build is installed -- size, PE timestamp, and the `Seamless Co-op vX.Y.Z`
    string read out of the image, not guessed from a path;
  * What the section layout is -- because v2.0.0 renamed the WinLicense VM section
    `.themida` -> `ERSC`, so any code that matched that section by name is broken, and
    the durable discriminator is "executable AND writable", which both builds share;
  * Whether a given byte needle is present, and in which section -- so a pinned
    signature can be re-tested against a new build in one command instead of by hand.

The DLL is opened read-only and never copied, moved, or staged: `AGENTS.md` forbids
bundling it, and a diff has no reason to want a copy.

Usage:

    python3 scripts/ersc_identify.py                       # identify the installed build
    python3 scripts/ersc_identify.py --reference           # ...and diff vs the backup build
    python3 scripts/ersc_identify.py --find 80B9B50A000000 --find 80B9BD0A000000

As a library (the underscore in the filename is so this import works):

    import ersc_identify as EI
    EI.ErscImage(EI.installed_path()).version_string()   # -> "Seamless Co-op v2.0.0 by Yui"
    EI.require_version("1.9.9")                          # refuse unless that build is installed

Paths resolve env-first so this works on any machine:

    ERSC_DLL      installed build   (default: Game/SeamlessCoop/ersc.dll under the Steam install)
    ERSC_DLL_REF  reference build   (default: Game/_SeamlessCoop/ersc.dll, the backup the
                                     Seamless launcher leaves behind when it updates)
    ER_GAME_DIR   the `Game` directory both defaults hang off
"""

from __future__ import annotations

import argparse
import os
import re
import struct
import sys

IMAGE_SCN_MEM_EXECUTE = 0x20000000
IMAGE_SCN_MEM_WRITE = 0x80000000

# The version banner ersc builds into its own image, e.g. "Seamless Co-op v2.0.0 by Yui".
#
# It is UTF-16LE and NUL-terminated (measured: v1.9.9 @0x1dcaf4, v2.0.0 @0x1e19dc), which is
# why this searches for the encoded prefix and then reads to the terminator rather than
# regexing the raw bytes. Stripping NULs to fake an ASCII search merges the banner with the
# string pooled directly after it -- the observed result was "v2.0.0 by YuiThis", a version
# report that silently carries three characters of the next sentence.
_VERSION_PREFIX = "Seamless Co-op v"
_VERSION_BODY_RE = re.compile(r"\A\d+\.\d+\.\d+(?: by .{1,32})?\Z")


def game_dir() -> str:
    """The ELDEN RING `Game` directory, env-overridable, no hard-coded user path."""
    env = os.environ.get("ER_GAME_DIR")
    if env:
        return env
    return os.path.join(
        os.path.expanduser("~"),
        ".local/share/Steam/steamapps/common/ELDEN RING/Game",
    )


def installed_path() -> str:
    return os.environ.get("ERSC_DLL") or os.path.join(game_dir(), "SeamlessCoop", "ersc.dll")


def reference_path() -> str:
    """The build the Seamless launcher moved aside when it last updated itself."""
    return os.environ.get("ERSC_DLL_REF") or os.path.join(
        game_dir(), "_SeamlessCoop", "ersc.dll"
    )


def version_in_bytes(data: bytes) -> str | None:
    """The `Seamless Co-op vX.Y.Z by Author` banner in `data`, read to its NUL terminator.

    Split out of `ErscImage` so it can be exercised without a PE -- which is what lets
    `--selftest` mean something on a machine with no game installed.
    """
    prefix = _VERSION_PREFIX.encode("utf-16le")
    start = data.find(prefix)
    if start < 0:
        return None
    # Read UTF-16LE code units to the NUL terminator, bounded so a corrupt image cannot walk
    # the whole file. Stripping NULs to fake an ASCII search instead merges the banner with
    # the string pooled after it -- "v2.0.0 by YuiThis" on both real builds.
    end = start + len(prefix)
    limit = min(end + 128, len(data) - 1)
    while end < limit and data[end : end + 2] != b"\x00\x00":
        end += 2
    text = data[start:end].decode("utf-16le", errors="replace")
    body = text[len(_VERSION_PREFIX) :]
    return text if _VERSION_BODY_RE.match(body) else None


class Section:
    __slots__ = ("name", "rva", "vsize", "raw_off", "raw_size", "chars")

    def __init__(self, name, rva, vsize, raw_off, raw_size, chars):
        self.name = name
        self.rva = rva
        self.vsize = vsize
        self.raw_off = raw_off
        self.raw_size = raw_size
        self.chars = chars

    @property
    def executable(self) -> bool:
        return bool(self.chars & IMAGE_SCN_MEM_EXECUTE)

    @property
    def writable(self) -> bool:
        return bool(self.chars & IMAGE_SCN_MEM_WRITE)

    @property
    def is_vm(self) -> bool:
        """A packer's VM section: executable and writable.

        This is the property to test, never the name. v1.9.9 called it `.themida`
        and v2.0.0 calls it `ERSC`; both are RWX, and the original `.text` is RX in
        both. A name match broke on the rename; this does not.
        """
        return self.executable and self.writable


class ErscImage:
    def __init__(self, path: str):
        self.path = path
        with open(path, "rb") as handle:
            self.data = handle.read()
        d = self.data
        pe = struct.unpack_from("<I", d, 0x3C)[0]
        if d[pe : pe + 4] != b"PE\0\0":
            raise ValueError(f"{path}: not a PE image")
        nsec = struct.unpack_from("<H", d, pe + 6)[0]
        self.timestamp = struct.unpack_from("<I", d, pe + 8)[0]
        optsz = struct.unpack_from("<H", d, pe + 20)[0]
        opt = pe + 24
        self.base = struct.unpack_from("<Q", d, opt + 24)[0]
        self.sections = []
        for i in range(nsec):
            o = pe + 24 + optsz + i * 40
            name = d[o : o + 8].rstrip(b"\0").decode("latin1")
            vsize, rva, raw_size, raw_off = struct.unpack_from("<IIII", d, o + 8)
            chars = struct.unpack_from("<I", d, o + 36)[0]
            self.sections.append(Section(name, rva, vsize, raw_off, raw_size, chars))

    @property
    def size(self) -> int:
        return len(self.data)

    def version_string(self) -> str | None:
        """The `Seamless Co-op vX.Y.Z by Author` banner, read to its NUL terminator."""
        return version_in_bytes(self.data)

    def section_of_offset(self, off: int) -> Section | None:
        for sec in self.sections:
            if sec.raw_off <= off < sec.raw_off + sec.raw_size:
                return sec
        return None

    def rva_of_offset(self, off: int) -> int | None:
        sec = self.section_of_offset(off)
        return None if sec is None else sec.rva + (off - sec.raw_off)

    def find(self, needle: bytes, limit: int = 32) -> list[tuple[int, int, str]]:
        """Every occurrence as (file_offset, rva, section_name)."""
        out = []
        i = self.data.find(needle)
        while i >= 0 and len(out) < limit:
            sec = self.section_of_offset(i)
            out.append((i, self.rva_of_offset(i) or 0, sec.name if sec else "<none>"))
            i = self.data.find(needle, i + 1)
        return out

    def code_sections(self) -> list[Section]:
        """Executable sections that are not the packer's writable VM."""
        return [s for s in self.sections if s.executable and not s.writable]


class WrongBuild(RuntimeError):
    """The installed Seamless Co-op is not the build a caller's pins were measured against."""


def require_version(measured: str, dll: str | None = None) -> str:
    """Return the installed version, or raise [`WrongBuild`] naming the exact delta.

    For tools that hold RVAs, struct offsets, or byte signatures measured against one specific
    `ersc.dll`. Hooking a stale address is silently wrong -- it lands mid-function or on
    unrelated code and reports plausible nonsense -- whereas refusing costs nothing but a rerun,
    so this raises rather than warns. The message names the installed version, the measured one,
    and where to look, because a bare "version mismatch" sends the next person back to the disk
    to work out which two things disagreed.
    """
    path = dll or installed_path()
    if not os.path.exists(path):
        raise WrongBuild(
            f"no ersc.dll at {path}; set ERSC_DLL. Pins measured against v{measured} cannot be "
            f"checked against a build that is not there."
        )
    found = ErscImage(path).version_string()
    if found is None:
        raise WrongBuild(
            f"{path} carries no version banner, so it cannot be shown to be the v{measured} "
            f"these pins were measured against."
        )
    version = found.rsplit(" v", 1)[-1].split(" ")[0]
    if version != measured:
        raise WrongBuild(
            f"installed Seamless Co-op is v{version}, but these pins were measured against "
            f"v{measured}. Every RVA, struct field offset and magic constant below describes "
            f"v{measured} and is unverified for v{version} -- v2.0.0 alone moved show "
            f"0x22d30->0x241a0 and cancel 0x24460->0x258d0, moved the session fields "
            f"+0x110->+0x150 and +0x10c->+0x14c, and changed the cancel constant 0x22->0x23. "
            f"Re-measure with scripts/locate-ersc-entry-points.py before trusting this tool."
        )
    return version


def describe(image: ErscImage, label: str) -> None:
    print(f"=== {label}: {image.path}")
    print(f"  size          {image.size} bytes")
    print(f"  PE timestamp  {image.timestamp}")
    print(f"  image base    {image.base:#x}")
    print(f"  version       {image.version_string() or '<not found in image>'}")
    print("  sections:")
    for sec in image.sections:
        tags = []
        if sec.executable:
            tags.append("X")
        if sec.writable:
            tags.append("W")
        if sec.is_vm:
            tags.append("VM")
        print(
            f"    {sec.name:10s} rva={sec.rva:#010x} vsize={sec.vsize:#010x} "
            f"raw={sec.raw_off:#010x}+{sec.raw_size:#010x} [{','.join(tags) or '-'}]"
        )
    code = image.code_sections()
    print(f"  code (X, not W): {', '.join(s.name for s in code) or '<none>'}")
    vm = [s for s in image.sections if s.is_vm]
    print(f"  packer VM (X+W): {', '.join(s.name for s in vm) or '<none>'}")


def _banner_blob(version_and_author: str) -> bytes:
    """An image fragment shaped the way ersc's own `.rdata` holds the banner.

    The trailing pooled string is the point: it is what a NUL-stripping "ASCII" search welds
    onto the version, and reproducing it here is what keeps that bug from coming back.
    """
    blob = bytearray(b"\x11" * 64)
    blob += f"{_VERSION_PREFIX}{version_and_author}".encode("utf-16le")
    blob += b"\x00\x00"
    blob += "This mod uses a separate".encode("utf-16le")
    return bytes(blob)


def selftest() -> int:
    """Prove the parser, then -- only if a build is installed -- prove it against the real one.

    The synthetic half runs everywhere, so this gate is not vacuous on a machine with no ELDEN
    RING (CI). The real half is what would actually notice a Seamless update on a dev box.
    """
    failures = []

    def check(condition: bool, what: str) -> None:
        if not condition:
            failures.append(what)

    check(
        version_in_bytes(_banner_blob("1.9.9 by Yui")) == "Seamless Co-op v1.9.9 by Yui",
        "a v1.9.9 banner reads back exactly, without the next pooled string welded on",
    )
    check(
        version_in_bytes(_banner_blob("2.0.0 by Yui")) == "Seamless Co-op v2.0.0 by Yui",
        "a v2.0.0 banner reads back exactly",
    )
    check(version_in_bytes(b"\x11" * 4096) is None, "an image with no banner is not identified")
    check(
        version_in_bytes(_banner_blob("beta by Yui")) is None,
        "a non-numeric version is refused rather than guessed at",
    )
    # An unterminated banner must stop rather than walk the buffer, and must not be salvaged.
    unterminated = bytearray(f"{_VERSION_PREFIX}1.9.9".encode("utf-16le"))
    unterminated += b"A" * 512
    check(version_in_bytes(bytes(unterminated)) is None, "an unterminated banner is not salvaged")

    dll = installed_path()
    if os.path.exists(dll):
        image = ErscImage(dll)
        check(image.base == 0x180000000, f"{dll} has the expected image base")
        check(bool(image.code_sections()), f"{dll} has a non-writable executable section")
        check(any(s.is_vm for s in image.sections), f"{dll} has a packer VM section (X+W)")
        check(
            image.version_string() is not None,
            f"{dll} carries a readable version banner",
        )
        print(f"selftest: installed build is {image.version_string() or '<unversioned>'}")
    else:
        print(f"selftest: no ersc.dll at {dll}; synthetic checks only")

    for failure in failures:
        print(f"  FAIL {failure}", file=sys.stderr)
    print(f"selftest: {len(failures)} failure(s)")
    return 1 if failures else 0


def diff(installed: ErscImage, reference: ErscImage) -> None:
    print("=== installed vs reference")
    if installed.size == reference.size and installed.timestamp == reference.timestamp:
        print("  SAME build: size and PE timestamp both match; no re-pin is implied.")
        return
    print(f"  size       {reference.size} -> {installed.size}")
    print(f"  timestamp  {reference.timestamp} -> {installed.timestamp}")
    print(
        f"  version    {reference.version_string() or '?'} -> "
        f"{installed.version_string() or '?'}"
    )
    old = {s.name for s in reference.sections}
    new = {s.name for s in installed.sections}
    if old != new:
        print(f"  sections removed: {', '.join(sorted(old - new)) or '<none>'}")
        print(f"  sections added:   {', '.join(sorted(new - old)) or '<none>'}")
        print(
            "  -> a section-NAME match across this pair is broken. Test "
            "executable-and-writable instead."
        )
    print(
        "  DIFFERENT builds. Every pinned RVA, struct field offset, and byte signature "
        "measured against the reference must be re-measured before it is trusted."
    )


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--dll", default=None, help="installed ersc.dll (default: $ERSC_DLL or the Steam install)")
    ap.add_argument("--ref", default=None, help="reference ersc.dll (default: $ERSC_DLL_REF or the _SeamlessCoop backup)")
    ap.add_argument("--reference", action="store_true", help="also load and diff against the reference build")
    ap.add_argument(
        "--find",
        action="append",
        default=[],
        metavar="HEX",
        help="hex byte needle to locate in each image; repeatable",
    )
    ap.add_argument("--selftest", action="store_true", help="check the installed image parses and self-describes")
    args = ap.parse_args()

    if args.selftest:
        return selftest()

    dll = args.dll or installed_path()
    if not os.path.exists(dll):
        print(f"ersc.dll not found at {dll}; set ERSC_DLL", file=sys.stderr)
        return 2
    installed = ErscImage(dll)

    describe(installed, "installed")

    reference = None
    ref_path = args.ref or reference_path()
    if args.reference or args.ref:
        if not os.path.exists(ref_path):
            print(f"\nreference not found at {ref_path}; skipping the diff", file=sys.stderr)
        else:
            reference = ErscImage(ref_path)
            print()
            describe(reference, "reference")
            print()
            diff(installed, reference)

    for hexstr in args.find:
        needle = bytes.fromhex(hexstr.replace(" ", ""))
        print(f"\n=== needle {needle.hex()}")
        for label, image in (("installed", installed), ("reference", reference)):
            if image is None:
                continue
            hits = image.find(needle)
            if not hits:
                print(f"  {label:10s} ABSENT")
                continue
            shown = ", ".join(f"{sec}@rva {rva:#x}" for _off, rva, sec in hits[:8])
            more = "" if len(hits) <= 8 else f" (+{len(hits) - 8} more)"
            print(f"  {label:10s} {len(hits)} hit(s): {shown}{more}")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
