#!/usr/bin/env python3
"""Locate `er-invasion-warp`'s pinned Seamless Co-op entry points in an installed `ersc.dll`.

Every ERSC address this repo holds is `module base + RVA`, measured against ONE Seamless build --
the one `ERSC_SUPPORTED_VERSION` records. `ersc.dll` is third-party and the user updates it on
their own schedule, so when a new build ships every pinned RVA moves at once and this tool is how
they are found again.

    uv run --with capstone python3 scripts/locate-ersc-entry-points.py --reference <old ersc.dll>

A match here is a CANDIDATE, never an identification. Two lessons from the last such move, both
paid for the hard way:

  * A masked BODY search can miss a function that did not move, because one inverted branch
    changes an OPCODE byte the mask keeps.
  * A unique PROLOGUE hit is a code SHAPE, not a function -- big-frame MSVC functions in one
    source file look alike from the top, and the last migration's prologue search landed
    confidently on the WRONG function.

Resolve candidates by reading, by `.pdata` index and by function size -- never by picking the
first hit. And note that an address and the field offsets it operates on travel TOGETHER: a
correct new address used with the previous build's offsets reads and writes the wrong fields of a
live multiplayer session, which is the failure this whole module exists to avoid. Re-pinning is a
reverse-engineering job, not a find-and-replace.

USAGE
    uv run --with capstone python3 scripts/locate-ersc-entry-points.py
    uv run --with capstone python3 scripts/locate-ersc-entry-points.py --reference <old ersc.dll>
    python3 scripts/locate-ersc-entry-points.py --selftest

`capstone` is only needed for the `--reference` body mapping, which is the strong evidence; the
prologue search runs without it. There is no system pip -- use uv, as AGENTS.md says.

ENV
    ER_ERSC_DLL            the installed ersc.dll to inspect
    ER_ERSC_DLL_REFERENCE  a copy of the build the pins were measured against
    ME3_STEAM_DIR          Steam root, searched before the two default ones
"""

import argparse
import os
import re
import struct
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
# Single source of truth for the pinned addresses and bytes. Parsed rather than copied: a second
# copy of an RVA in a Python file is how the two drift and the tool starts lying.
BUILD_RS = os.path.join(ROOT, "crates", "er-invasion-warp", "build.rs")
# Where the version markers live. Parsed rather than copied for the same reason as the RVAs: a
# second copy of "which string identifies v2.0.0" is how the tool and the build script drift.
PROLOGUE_BUILD_RS = os.path.join(ROOT, "build-support", "prologue_build.rs")
ERSC_BASE = 0x180000000
RELATIVE_INSTALL = "steamapps/common/ELDEN RING/Game/SeamlessCoop/ersc.dll"
IMAGE_SCN_MEM_EXECUTE = 0x20000000
IMAGE_SCN_MEM_WRITE = 0x80000000
# Same cap, and the same reason, as MAX_CONTENT_MATCHES in build-support/prologue_build.rs.
MAX_MATCHES = 8


# ---------------------------------------------------------------------------------------------
# PE
# ---------------------------------------------------------------------------------------------


class Pe:
    """The little of PE32+ this needs: sections, RVA->offset, and the real code ranges."""

    def __init__(self, image):
        self.image = image
        self.sections = []
        pe = struct.unpack_from("<I", image, 0x3C)[0]
        if image[pe : pe + 4] != b"PE\0\0":
            raise ValueError("not a PE image")
        count = struct.unpack_from("<H", image, pe + 6)[0]
        table = pe + 24 + struct.unpack_from("<H", image, pe + 20)[0]
        for index in range(count):
            entry = table + 40 * index
            name = image[entry : entry + 8].rstrip(b"\0").decode("latin1")
            vsize, vaddr, rsize, rptr = struct.unpack_from("<IIII", image, entry + 8)
            chars = struct.unpack_from("<I", image, entry + 36)[0]
            self.sections.append((name, vsize, vaddr, rsize, rptr, chars))

    def rva_to_offset(self, rva):
        for _, vsize, vaddr, rsize, rptr, _ in self.sections:
            if vaddr <= rva < vaddr + max(vsize, rsize):
                return rptr + (rva - vaddr)
        return None

    def bytes_at_va(self, va, length):
        offset = self.rva_to_offset(va - ERSC_BASE)
        if offset is None:
            return None
        window = self.image[offset : offset + length]
        return window if len(window) == length else None

    def code_ranges(self):
        """`(file_offset, length, rva_of_that_offset)` per section holding real compiler-emitted code.

        Executable AND NOT writable. `ersc.dll` keeps most of itself in an Oreans WinLicense VM
        section (named `ERSC`, `.themida` in older builds) that is 11 MB of ciphertext and
        is marked writable; scanning it manufactures coincidental hits. Selecting by section
        characteristics rather than by the name `.text` keeps the rule true if the next build
        renames things again -- which this one already did once.
        """
        ranges = []
        for _, _, vaddr, rsize, rptr, chars in self.sections:
            if chars & IMAGE_SCN_MEM_EXECUTE and not chars & IMAGE_SCN_MEM_WRITE:
                if rptr + rsize <= len(self.image):
                    ranges.append((rptr, rsize, vaddr))
        return ranges

    def find(self, pattern, mask=None):
        """Every VA in real code where `pattern` matches under `mask` (`None` = exact)."""
        mask = mask if mask is not None else b"\x01" * len(pattern)
        anchor = pattern[0] if mask[0] else None
        hits = []
        for start, length, rva in self.code_ranges():
            at = start
            end = start + length - len(pattern)
            while at <= end and len(hits) < MAX_MATCHES:
                if anchor is None or self.image[at] == anchor:
                    window = self.image[at : at + len(pattern)]
                    if all(not k or window[i] == pattern[i] for i, k in enumerate(mask)):
                        hits.append(ERSC_BASE + rva + (at - start))
                at += 1
        return hits

    def describe(self):
        pe = struct.unpack_from("<I", self.image, 0x3C)[0]
        stamp = struct.unpack_from("<I", self.image, pe + 8)[0]
        version = "unknown"
        for match in re.finditer(rb"S\0e\0a\0m\0l\0e\0s\0s\0", self.image):
            text = self.image[match.start() : match.start() + 120].decode("utf-16le", "replace")
            version = text.split("\x00")[0]
            break
        return f"{len(self.image)} bytes, PE timestamp {stamp}, {version!r}"


# ---------------------------------------------------------------------------------------------
# The pins, read out of build.rs
# ---------------------------------------------------------------------------------------------


def parse_version_markers(source):
    """`{"Ersc200": "Seamless Co-op v2.0.0 by Yui"}` out of `Image::version_marker`."""
    return dict(re.findall(r'Self::(Ersc\w+)\s*=>\s*Some\("([^"]+)"\)', source))


def parse_pins(source):
    """`[(name, image, va, pin_bytes)]` for every Seamless spec in er-invasion-warp's build.rs.

    `image` is the `Image::Ersc*` variant, i.e. WHICH Seamless build the spec describes. There is
    one pin set per build because this repo has to keep working across a Seamless update, so a pin
    that does not hold is only news once you know which build it was measured against.
    """
    consts = {
        key: int(value, 16)
        for key, value in re.findall(r"const\s+(\w+_VA)\s*:\s*u64\s*=\s*(0x[0-9a-fA-F_]+)", source)
    }
    byte_consts = {
        key: parse_byte_array(body)
        for key, body in re.findall(
            r"const\s+(\w+)\s*:\s*&\[u8\]\s*=\s*&\[(.*?)\];", source, re.S
        )
    }
    pins = []
    for block in re.findall(r"PrologueSpec\s*\{(.*?)\n\s*\},", source, re.S):
        image = re.search(r"image:\s*Image::(Ersc\w*)", block)
        if not image:
            continue
        name = re.search(r'name:\s*"(\w+)"', block)
        va = re.search(r"va:\s*(\w+)", block)
        pin = re.search(r"pin:\s*(?:&\[(.*?)\]|(\w+))\s*,?\s*$", block, re.S | re.M)
        if not (name and va and pin):
            continue
        raw = parse_byte_array(pin.group(1)) if pin.group(1) else byte_consts.get(pin.group(2), b"")
        pins.append((name.group(1), image.group(1), consts.get(va.group(1)), raw))
    return pins


def parse_byte_array(body):
    return bytes(int(token, 16) for token in re.findall(r"0x([0-9a-fA-F]{2})", body))


# ---------------------------------------------------------------------------------------------
# Masked body mapping (the strong evidence; needs capstone)
# ---------------------------------------------------------------------------------------------

# Longest first. A long signature is stronger, but it reaches past a short function into whatever
# follows it, so a ladder recovers the ones a 64-byte window overshoots.
SIGNATURE_LADDER = (64, 48, 40, 32, 24)


def masked_pattern(image, offset, want):
    """`(pattern, mask)` from `offset` with every version-fragile operand byte wildcarded.

    Same three classes as `scripts/map-rvas-1162-to-1170.py`: RIP-relative displacements, memory
    displacements on a register base (struct fields move between builds -- v2.0.0 moved the ERSC
    session state from `+0x110` to `+0x150`), and immediates. What survives is opcode shape and
    register allocation, which is what identifies a function across a recompile.
    """
    from capstone import CS_ARCH_X86, CS_MODE_64, Cs

    md = Cs(CS_ARCH_X86, CS_MODE_64)
    md.detail = True
    pattern, mask, consumed = bytearray(), bytearray(), 0
    for insn in md.disasm(bytes(image[offset : offset + want * 3]), 0):
        if consumed >= want:
            break
        raw = bytearray(insn.bytes)
        keep = bytearray(b"\x01" * len(raw))
        encoding = getattr(insn, "encoding", None)
        if encoding is not None:
            for at, size in (
                (encoding.disp_offset, encoding.disp_size),
                (encoding.imm_offset, encoding.imm_size),
            ):
                for index in range(at, min(at + size, len(keep))):
                    keep[index] = 0
        pattern += raw
        mask += keep
        consumed += len(raw)
    return (bytes(pattern), bytes(mask)) if pattern else (None, None)


def map_by_body(reference, installed, va):
    """Map one pinned VA from the reference build onto the installed one. `(hits, note)`."""
    offset = reference.rva_to_offset(va - ERSC_BASE)
    if offset is None:
        return [], "VA is outside every section of the reference"
    for length in SIGNATURE_LADDER:
        pattern, mask = masked_pattern(reference.image, offset, length)
        if pattern is None:
            return [], "reference bytes did not decode"
        hits = installed.find(pattern, mask)
        if hits:
            return hits, f"{len(pattern)}B masked body signature"
    return [], "no match at any signature length"


# ---------------------------------------------------------------------------------------------
# Locating the files
# ---------------------------------------------------------------------------------------------


REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def steam_roots():
    roots = []
    if os.environ.get("ME3_STEAM_DIR"):
        roots.append(os.environ["ME3_STEAM_DIR"])
    home = os.path.expanduser("~")
    roots.append(os.path.join(home, ".local/share/Steam"))
    roots.append(os.path.join(home, ".steam/steam"))
    return roots


def find_installed(explicit):
    """The same resolution order the build script uses, so both agree on which file is 'the' DLL."""
    if explicit:
        return explicit if os.path.isfile(explicit) else None
    if os.environ.get("ER_ERSC_DLL"):
        path = os.environ["ER_ERSC_DLL"]
        return path if os.path.isfile(path) else None
    for root in steam_roots():
        candidate = os.path.join(root, RELATIVE_INSTALL)
        if os.path.isfile(candidate):
            return candidate
    return None


def ersc_candidates(explicit_installed, explicit_reference):
    """Every file that might be a build of `ersc.dll`, in the order they are tried.

    Both install directories, because the Seamless launcher shuffles them -- `SeamlessCoop/` is the
    live one and `_SeamlessCoop/` is where the previous build is left -- and the user may
    downgrade. Which file answers for which pin set is decided by the version marker inside it,
    never by the directory it happens to sit in. Same rule, and the same reason, as
    `Image::locate` in `build-support/prologue_build.rs`.
    """
    candidates = []
    for explicit in (explicit_installed, explicit_reference):
        if explicit:
            candidates.append(explicit)
    for variable in ("ER_ERSC_DLL", "ER_ERSC_DLL_REFERENCE"):
        if os.environ.get(variable):
            candidates.append(os.environ[variable])
    for root in steam_roots():
        game = os.path.join(root, "steamapps/common/ELDEN RING/Game")
        candidates.append(os.path.join(game, "SeamlessCoop", "ersc.dll"))
        candidates.append(os.path.join(game, "_SeamlessCoop", "ersc.dll"))
    # The gitignored reference archive, added 2026-09-02. It is listed LAST on purpose: the
    # install directories still answer first, so nothing about which file is "installed"
    # changes. What it buys is the build the launcher has already overwritten -- the whole
    # point of the archive is that `_SeamlessCoop/` is a courtesy the next update may not
    # repeat, and a pin set with nothing to diff against cannot be re-measured at all.
    # Sorted so the ordering is stable rather than filesystem order.
    archive = os.path.join(REPO_ROOT, "vendor-archive", "seamless")
    if os.path.isdir(archive):
        for name in sorted(os.listdir(archive)):
            if name.startswith("ersc-") and name.endswith(".dll"):
                candidates.append(os.path.join(archive, name))
    seen, ordered = set(), []
    for candidate in candidates:
        real = os.path.realpath(candidate)
        if real not in seen and os.path.isfile(candidate):
            seen.add(real)
            ordered.append(candidate)
    return ordered


def load_pe(path):
    try:
        with open(path, "rb") as handle:
            return Pe(handle.read())
    except (ValueError, struct.error, OSError):
        return None


def match_marker(pe, marker):
    return marker.encode("utf-16le") in pe.image


# ---------------------------------------------------------------------------------------------
# Report
# ---------------------------------------------------------------------------------------------


def report(installed_path, installed, builds, pins, markers):
    """Say, per Seamless build this repo pins, whether its pins still hold and where they went.

    `builds` maps an `Image::Ersc*` variant to `(path, Pe)` for the copy of THAT build found on
    this machine, or to `None`. On a machine that has updated Seamless at least once, both are
    usually present -- the launcher leaves the previous build behind -- so both pin sets can be
    verified in one run.
    """
    print(f"installed:  {installed_path}\n            {installed.describe()}")
    for image in sorted(builds):
        found = builds[image]
        marker = markers.get(image, image)
        if found:
            path, pe = found
            print(f"{image:<11} {path}\n            {pe.describe()}")
        else:
            print(f"{image:<11} no copy of {marker!r} on this machine -- its pins cannot be")
            print("            ground-truthed here, only checked against the installed build")
    print()

    recognised = None
    for image in sorted({image for _, image, _, _ in pins}):
        group = [(name, va, pin) for name, gimage, va, pin in pins if gimage == image]
        own = builds.get(image)
        holds_in_own = own is not None and all(
            pin and own[1].bytes_at_va(va, len(pin)) == pin for _, va, pin in group
        )
        holds_in_installed = all(
            pin and installed.bytes_at_va(va, len(pin)) == pin for _, va, pin in group
        )
        verdict = []
        if holds_in_own:
            verdict.append("all pins hold against its own build (ground truth OK)")
        elif own is not None:
            verdict.append("SOME PINS DO NOT HOLD against its own build -- re-measure")
        if holds_in_installed:
            verdict.append("and this IS the installed build")
            recognised = image
        else:
            verdict.append("not the installed build")
        print(f"=== {image} ({markers.get(image, '?')}): {'; '.join(verdict)} ===")
        if holds_in_installed:
            print("    Nothing to do: the runtime gate will recognise this build and arm.\n")
            continue
        for name, va, pin in group:
            actual = installed.bytes_at_va(va, len(pin))
            if actual is None:
                print(f"  {name}: pinned 0x{va:x} falls outside every section of the installed build")
                continue
            if actual == pin:
                print(f"  {name}: pinned 0x{va:x} still holds in the installed build")
                continue
            print(f"  {name} (pinned 0x{va:x}, {len(pin)} bytes)")
            print(f"    by prologue: {format_hits(installed.find(pin))}")
            if own is not None:
                try:
                    mapped, note = map_by_body(own[1], installed, va)
                except ImportError:
                    print(
                        "    by body: capstone not importable -- re-run as"
                        " `uv run --with capstone python3 scripts/locate-ersc-entry-points.py`"
                        " for the body mapping, which is the strong evidence"
                    )
                else:
                    print(f"    by body ({note}): {format_hits(mapped)}")
        print()

    if recognised:
        print(
            f"The installed build is {markers.get(recognised, recognised)}, which this repo already"
            "\nsupports. No re-pin is needed."
        )
        return
    print(
        "NONE of the pin sets matches the installed build, so the invasion filter will refuse to\n"
        "arm against it -- fail-closed, which is correct but inert. Every address above is a\n"
        "CANDIDATE and nothing more:\n"
        "  * a unique PROLOGUE hit is a code SHAPE. v2.0.0's 19-byte `BuildLobbyKey` prologue\n"
        "    matched exactly one address and it was the wrong function.\n"
        "  * a masked BODY match survives struct-offset drift but not a re-ordered branch. v2.0.0\n"
        "    inverted the invade action's idle guard, so it matched nothing at any length while\n"
        "    sitting untouched 0x1470 further along.\n"
        "Identify each one by something independent -- `.pdata` index and function size, a\n"
        "cross-referenced constant, the callers -- before pinning it. `scripts/ersc-disas.py` has\n"
        "`align`, `pdata`, `xref`, `states` and `crossmatch` for exactly that. And re-check the\n"
        "session field offsets and state codes too: v2.0.0 moved the field group by +0x40 and\n"
        "renumbered the whole state enum by +1."
    )


def format_hits(hits):
    if not hits:
        return "no match"
    text = ", ".join(f"0x{va:x}" for va in hits)
    return text + (" (capped)" if len(hits) >= MAX_MATCHES else "")


# ---------------------------------------------------------------------------------------------
# Selftest -- no game files, so it runs anywhere
# ---------------------------------------------------------------------------------------------


def synth_pe(payload, decoy):
    """A minimal PE32+ with one `.text` (exec, not writable) and one packer-style `VM` section
    (exec AND writable). `payload` goes in both, so a locator that does not exclude writable
    executable sections reports two hits where it must report one."""
    text_raw, vm_raw = 0x400, 0x600
    text_rva, vm_rva = 0x1000, 0x2000
    image = bytearray(0x1000)
    struct.pack_into("<I", image, 0x3C, 0x80)
    pe = 0x80
    image[pe : pe + 4] = b"PE\0\0"
    struct.pack_into("<HHIIIHH", image, pe + 4, 0x8664, 2, 0, 0, 0, 0xF0, 0x2022)
    struct.pack_into("<H", image, pe + 24, 0x20B)
    table = pe + 24 + 0xF0
    for index, (name, rva, ptr, chars) in enumerate(
        [
            (b".text", text_rva, text_raw, IMAGE_SCN_MEM_EXECUTE),
            (b"VM", vm_rva, vm_raw, IMAGE_SCN_MEM_EXECUTE | IMAGE_SCN_MEM_WRITE),
        ]
    ):
        entry = table + 40 * index
        image[entry : entry + 8] = name.ljust(8, b"\0")
        struct.pack_into("<IIII", image, entry + 8, 0x200, rva, 0x200, ptr)
        struct.pack_into("<I", image, entry + 36, chars)
    image[text_raw : text_raw + len(payload)] = payload
    image[vm_raw : vm_raw + len(decoy)] = decoy
    return bytes(image), ERSC_BASE + text_rva


def selftest():
    failures = []

    payload = bytes([0x55, 0x41, 0x57, 0x48, 0x81, 0xEC, 0x88, 0x01, 0x00, 0x00, 0xC3])
    image, payload_va = synth_pe(payload, payload)
    pe = Pe(image)

    hits = pe.find(payload)
    if hits != [payload_va]:
        failures.append(
            f"locator returned {[hex(h) for h in hits]}, expected only 0x{payload_va:x} -- the "
            "writable executable (packer VM) section was not excluded"
        )

    if pe.bytes_at_va(payload_va, len(payload)) != payload:
        failures.append("bytes_at_va did not read back the payload it was given")

    mask = b"\x01" * 3 + b"\x00" * (len(payload) - 3)
    if len(pe.find(payload[:3] + b"\xff" * (len(payload) - 3), mask)) != 1:
        failures.append("masked search did not honour its mask")

    try:
        mapped, note = map_by_body(pe, pe, payload_va)
    except ImportError:
        print(
            "selftest: capstone absent, body mapping not exercised "
            "(run under `uv run --with capstone`)"
        )
    else:
        if mapped != [payload_va]:
            failures.append(f"self-mapping gave {[hex(h) for h in mapped]} ({note}), expected one")

    with open(BUILD_RS, encoding="utf-8") as handle:
        pins = parse_pins(handle.read())
    with open(PROLOGUE_BUILD_RS, encoding="utf-8") as handle:
        markers = parse_version_markers(handle.read())
    # `< 4` rather than `!= N`: a broken regex parses too FEW, and a regex that over-matches into
    # the game-image specs is caught by the address-range check below instead. Pinning the exact
    # count would just break the day someone legitimately adds a fifth ERSC entry point -- or a
    # THIRD Seamless build, which is the whole shape this tool now has to survive.
    if len(pins) < 4:
        failures.append(f"parsed {len(pins)} ERSC pins out of {BUILD_RS}, expected at least 4")
    images = {image for _, image, _, _ in pins}
    if not images:
        failures.append("no pin named an Image::Ersc* variant, so no pin can be matched to a build")
    for image in images:
        if image not in markers:
            failures.append(
                f"{image} has no version marker in {os.path.relpath(PROLOGUE_BUILD_RS, ROOT)}, so "
                "no file can be identified as that build"
            )
    if len(set(markers.values())) != len(markers):
        failures.append(f"two Seamless images share a version marker: {markers}")
    for name, image, va, pin in pins:
        if va is None or not (ERSC_BASE <= va < ERSC_BASE + (1 << 32)):
            failures.append(f"{name}: parsed VA {va!r} is not an ersc.dll address")
        if len(pin) < 8:
            failures.append(f"{name}: parsed pin is {len(pin)} bytes, too short to identify a shape")
        if not name.upper().startswith(image.upper()[:4]):
            continue
    # A pin set per build is only useful if the pins differ between them. Two builds whose invade
    # pins are equal cannot be told apart, which is the failure the runtime gate refuses on.
    by_image = {}
    for name, image, _, pin in pins:
        by_image.setdefault(image, {})[name.split("_", 1)[1]] = pin
    roles = set().union(*(set(group) for group in by_image.values())) if by_image else set()
    for role in roles:
        variants = [group[role] for group in by_image.values() if role in group]
        if role != "SHOW_PROLOGUE" and len(variants) > 1 and len(set(variants)) != len(variants):
            failures.append(
                f"two builds share identical bytes for {role}, so they cannot be told apart"
            )

    for failure in failures:
        print(f"SELFTEST FAIL: {failure}", file=sys.stderr)
    if failures:
        return 1
    print(f"selftest passed ({len(pins)} pins parsed from {os.path.relpath(BUILD_RS, ROOT)})")
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--installed", help="the ersc.dll to inspect (default: the Steam install)")
    parser.add_argument("--reference", help="another ersc.dll to consider as a pinned build")
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()

    if args.selftest:
        return selftest()

    with open(BUILD_RS, encoding="utf-8") as handle:
        pins = parse_pins(handle.read())
    if not pins:
        print(f"no Image::Ersc* prologue specs found in {BUILD_RS}", file=sys.stderr)
        return 2
    with open(PROLOGUE_BUILD_RS, encoding="utf-8") as handle:
        markers = parse_version_markers(handle.read())

    installed_path = find_installed(args.installed)
    if installed_path is None:
        print(
            "no ersc.dll found. Seamless Co-op is optional, so this is not an error -- the\n"
            "invasion filter simply never arms. Set ER_ERSC_DLL or pass --installed to inspect\n"
            "a copy somewhere else.",
            file=sys.stderr,
        )
        return 2
    installed = load_pe(installed_path)
    if installed is None:
        print(f"{installed_path}: not a readable PE image", file=sys.stderr)
        return 2

    # Match each pinned build to a copy of ITSELF by the version string inside the file, never by
    # which directory it sits in -- the launcher shuffles those and the user may downgrade.
    candidates = [(path, load_pe(path)) for path in ersc_candidates(args.installed, args.reference)]
    candidates = [(path, pe) for path, pe in candidates if pe is not None]
    builds = {}
    for image in sorted({image for _, image, _, _ in pins}):
        marker = markers.get(image)
        builds[image] = next(
            ((path, pe) for path, pe in candidates if marker and match_marker(pe, marker)),
            None,
        )
    report(installed_path, installed, builds, pins, markers)
    return 0


if __name__ == "__main__":
    sys.exit(main())
