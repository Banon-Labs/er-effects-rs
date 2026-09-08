#!/usr/bin/env python3
"""Disassemble / read any archived or installed `ersc.dll` build, for Seamless re-pin work.

Reads the DLLs in place (AGENTS.md forbids copying or staging `ersc.dll`). Reuses the PE
reader and pin parser from `locate-ersc-entry-points.py` so there is one implementation of
"which sections hold real code".

Usage
    uv run --with capstone python3 scripts/ersc-disas.py disas 0x1800258d0 0x1800259d0 --build v200
    uv run --with capstone python3 scripts/ersc-disas.py disas 0x1800243e0 --build v199 -n 0x80
    uv run --with capstone python3 scripts/ersc-disas.py find 'f3 0f 1e fa 56 57' --build v200
    uv run --with capstone python3 scripts/ersc-disas.py xref 0x1800258d0 --build v200
    uv run --with capstone python3 scripts/ersc-disas.py strings lobby_key --build v200
    uv run --with capstone python3 scripts/ersc-disas.py align 0x243e0 0x24460
    python3 scripts/ersc-disas.py --selftest

ENV
    ER_ERSC_DLL            the installed (new) ersc.dll
    ER_ERSC_DLL_REFERENCE  the previous build; defaults to `_SeamlessCoop/ersc.dll` beside it
"""

import argparse
import importlib.util
import os
import struct
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BASE = 0x180000000


def _locate_module():
    path = os.path.join(ROOT, "scripts", "locate-ersc-entry-points.py")
    spec = importlib.util.spec_from_file_location("_ersc_locate", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


LOC = _locate_module()
Pe = LOC.Pe


# `--build` names a seamless version, and it is resolved by the version marker inside the
# file -- never by which directory the file sits in.
#
# It used to be positional: `v200` meant "whatever `find_installed` returns" and `v199` meant
# "the `_SeamlessCoop/` sibling". That was correct for exactly as long as v2.0.0 was the
# installed build. The moment v2.0.1 was extracted on 2026-09-02, `--build v200` silently
# began disassembling v2.0.1 -- same file size, so nothing looked wrong -- and every address
# read through it would have been recorded under the wrong build. That is the failure this
# repo's own locator already refuses to make: "Which file answers for which pin set is decided
# by the version marker inside it, never by the directory it happens to sit in."
#
# So this resolves the same way, fails closed when the asked-for build is not on disk, and
# says which builds are available rather than quietly handing back the wrong bytes.
BUILD_MARKERS = {
    "v199": "Seamless Co-op v1.9.9 by Yui",
    "v200": "Seamless Co-op v2.0.0 by Yui",
    "v201": "Seamless Co-op v2.0.1 by Yui",
}


def available_builds():
    """`{build_name: path}` for every version marker found among the candidate files."""
    found = {}
    for path in LOC.ersc_candidates(None, None):
        pe = LOC.load_pe(path)
        if pe is None:
            continue
        for name, marker in BUILD_MARKERS.items():
            if name not in found and LOC.match_marker(pe, marker):
                found[name] = path
    return found


def build_paths():
    """`(installed_path, reference_path)` -- kept for callers that want the install layout."""
    installed = LOC.find_installed(None)
    reference = None
    if installed:
        game = os.path.dirname(os.path.dirname(installed))
        candidate = os.path.join(game, "_SeamlessCoop", "ersc.dll")
        if os.path.isfile(candidate):
            reference = candidate
    if os.environ.get("ER_ERSC_DLL_REFERENCE"):
        reference = os.environ["ER_ERSC_DLL_REFERENCE"]
    return installed, reference


def load(which):
    marker = BUILD_MARKERS.get(which)
    if marker is None:
        raise SystemExit(f"unknown --build {which!r}; known: {', '.join(sorted(BUILD_MARKERS))}")
    found = available_builds()
    path = found.get(which)
    if not path:
        have = ", ".join(f"{k}={v}" for k, v in sorted(found.items())) or "none"
        raise SystemExit(
            f"no ersc.dll carrying {marker!r} was found.\n"
            f"  builds present: {have}\n"
            "  archive a copy at vendor-archive/seamless/ersc-<version>.dll, or point\n"
            "  ER_ERSC_DLL / ER_ERSC_DLL_REFERENCE at it."
        )
    with open(path, "rb") as handle:
        return Pe(handle.read()), path


def _md():
    from capstone import CS_ARCH_X86, CS_MODE_64, Cs

    md = Cs(CS_ARCH_X86, CS_MODE_64)
    md.detail = True
    return md


def disas(pe, va, nbytes=0x120, stop_at_ret=True):
    offset = pe.rva_to_offset(va - BASE)
    if offset is None:
        return []
    out = []
    for insn in _md().disasm(bytes(pe.image[offset : offset + nbytes]), va):
        out.append(insn)
        if stop_at_ret and insn.mnemonic == "ret":
            break
    return out


def print_disas(pe, va, nbytes, stop_at_ret, label=""):
    if label:
        print(f"=== {label} @ 0x{va:x} ===")
    listing = disas(pe, va, nbytes, stop_at_ret)
    if not listing:
        print(f"  0x{va:x}: outside every section, or did not decode")
    for insn in listing:
        print(f"  {insn.address:#x}  {insn.bytes.hex(' '):<24} {insn.mnemonic} {insn.op_str}")


def find_bytes(pe, pattern_text):
    """`??` is a wildcard nibble-pair; everything else is a hex byte."""
    tokens = pattern_text.replace(",", " ").split()
    pattern, mask = bytearray(), bytearray()
    for token in tokens:
        if token in ("??", "?"):
            pattern.append(0)
            mask.append(0)
        else:
            pattern.append(int(token, 16))
            mask.append(1)
    return pe.find(bytes(pattern), bytes(mask))


def xrefs_to(pe, target, direct_only=True):
    """Every reference in real code that reaches `target`.

    Always: `call`/`jmp rel32`, found by direct displacement arithmetic. With `direct_only`
    false, also every RIP-relative operand of any mnemonic -- which is how a data constant
    (an SHA-256 IV, a salt, a string) is reached, and how an option table takes a callback's
    address. That pass disassembles the whole code section, so it costs a few seconds.
    """
    md = _md()
    hits = []
    for start, length, rva in pe.code_ranges():
        data = bytes(pe.image[start : start + length])
        base = BASE + rva
        # Rel32 call (e8) / jmp (e9): decode the displacement directly, cheap and exhaustive.
        for opcode, kind in ((0xE8, "call"), (0xE9, "jmp")):
            at = data.find(bytes([opcode]))
            while at != -1:
                if at + 5 <= len(data):
                    disp = struct.unpack_from("<i", data, at + 1)[0]
                    if base + at + 5 + disp == target:
                        hits.append((base + at, kind))
                at = data.find(bytes([opcode]), at + 1)
    if not direct_only:
        # Disassemble from function starts, never linearly from the top of the section. A linear
        # sweep desynchronises on the first jump table or alignment padding and then decodes
        # garbage -- which silently misses real references rather than inventing fake ones, so
        # the failure looks like "nothing refers to this constant". Measured: a linear sweep of
        # v1.9.9 `.text` found no reference to the SHA-256 IV at 0x1801bab00, which
        # `movaps xmm0,[rip+0x10eaef]` at 0x1800ac00a plainly is.
        for begin, end in pdata_entries(pe):
            offset = pe.rva_to_offset(begin)
            if offset is None:
                continue
            for insn in md.disasm(bytes(pe.image[offset : offset + (end - begin)]), BASE + begin):
                if "rip" not in insn.op_str:
                    continue
                for operand in insn.operands:
                    if operand.type == 3 and operand.mem.base == 41:  # X86_OP_MEM, RIP
                        if insn.address + insn.size + operand.mem.disp == target:
                            hits.append((insn.address, f"{insn.mnemonic} in 0x{begin:x}"))
    return sorted(set(hits))


def rip_targets(pe, va, nbytes=0x300):
    """`(insn_va, mnemonic, target_va)` for every rip-relative reference in a window."""
    out = []
    for insn in disas(pe, va, nbytes, stop_at_ret=False):
        for operand in insn.operands:
            if operand.type == 3 and operand.mem.base == 41:
                out.append((insn.address, f"{insn.mnemonic} {insn.op_str}", insn.address + insn.size + operand.mem.disp))
    return out


def find_string(pe, text, encoding="ascii"):
    """Every VA (any section, not just code) holding `text`."""
    needle = text.encode(encoding if encoding != "utf16" else "utf-16le")
    hits, at = [], pe.image.find(needle)
    while at != -1 and len(hits) < 64:
        for _, vsize, vaddr, rsize, rptr, _ in pe.sections:
            if rptr <= at < rptr + rsize:
                hits.append(BASE + vaddr + (at - rptr))
                break
        at = pe.image.find(needle, at + 1)
    return hits


def pdata_entries(pe):
    """`[(begin_rva, end_rva)]` from `.pdata`, in link order.

    `.pdata` is the authoritative function table for x64 PE: one RUNTIME_FUNCTION per
    non-leaf function, sorted by address. It is emitted by the linker, not by us, which makes
    an entry's index a build-independent fact about where a function sits in link order --
    evidence of a different kind from any byte pattern.
    """
    import struct

    for name, vsize, _, _, rptr, _ in pe.sections:
        if name != ".pdata":
            continue
        out = []
        for index in range(vsize // 12):
            begin, end, _unwind = struct.unpack_from("<III", pe.image, rptr + 12 * index)
            if begin == 0:
                break
            out.append((begin, end))
        return out
    return []


def align_report(old, new, rvas, tolerance=4):
    """Cross-build function mapping by `.pdata` index rather than by bytes.

    For each `rva` in the old build, report its index, then the longest run of consecutive
    functions around it whose sizes still agree at a constant index shift. A long run is the
    strong claim: it says the linker emitted the same functions in the same order with the
    same sizes on both sides, so the function at `old_index + shift` is the same function --
    an argument that survives a rewrite that defeats byte matching.
    """
    old_entries, new_entries = pdata_entries(old), pdata_entries(new)
    old_sizes = [end - begin for begin, end in old_entries]
    new_sizes = [end - begin for begin, end in new_entries]
    print(f"old .pdata: {len(old_entries)} entries    new .pdata: {len(new_entries)} entries\n")
    for rva in rvas:
        index = next((i for i, (begin, _) in enumerate(old_entries) if begin == rva), None)
        if index is None:
            print(f"0x{rva:x}: not a .pdata entry in the old build (leaf function?)")
            continue
        best = None
        for shift in range(-64, 65):
            at = index + shift
            if not 0 <= at < len(new_entries):
                continue
            if abs(old_sizes[index] - new_sizes[at]) > tolerance:
                continue
            low = index
            while low and abs(old_sizes[low - 1] - new_sizes[low - 1 + shift]) <= tolerance:
                low -= 1
            high = index
            while (
                high + 1 < len(old_sizes)
                and high + 1 + shift < len(new_sizes)
                and abs(old_sizes[high + 1] - new_sizes[high + 1 + shift]) <= tolerance
            ):
                high += 1
            if best is None or high - low > best[0]:
                best = (high - low, shift, low, high)
        if best is None:
            print(f"0x{rva:x}: index {index}, no consistent shift found")
            continue
        span, shift, low, high = best
        mapped = new_entries[index + shift][0]
        print(
            f"0x{rva:x}: old index {index} size 0x{old_sizes[index]:x}"
            f"  ->  0x{mapped:x} at new index {index + shift} size"
            f" 0x{new_sizes[index + shift]:x}  (shift {shift:+d})"
        )
        print(
            f"    that shift holds unbroken over old indices {low}..{high}"
            f" ({span + 1} consecutive functions,"
            f" old 0x{old_entries[low][0]:x}..0x{old_entries[high][0]:x})"
        )


def state_stores(pe, field_offset):
    """Every `mov dword ptr [reg+field_offset], imm32` in real code: `[(va, base_reg, imm)]`.

    The session-state field is written with a plain immediate everywhere it matters, so the set
    of immediates stored into it is a complete, static picture of the state enum -- independent
    of which function does the storing. Comparing that set across two builds is how "the enum
    was renumbered" stops being a claim about five option actions and becomes a measurement over
    every writer in the module.

    Matching is by displacement only, so any other struct with a field at the same offset shows
    up too. Read the result as a distribution to compare across builds, never as a list of
    session writers: it is the per-value site counts lining up under a constant shift that
    carries the argument, not any single row.

    Encoding: `C7 /0 disp32 imm32`, modrm mod=10 reg=000. A REX prefix may extend the base
    register; REX.W would make it a qword store of a sign-extended imm32, which is a different
    instruction and is excluded.
    """
    import struct

    names = ["rax", "rcx", "rdx", "rbx", "rsp", "rbp", "rsi", "rdi"]
    out = []
    for start, length, rva in pe.code_ranges():
        data = bytes(pe.image[start : start + length])
        base_va = BASE + rva
        at = data.find(b"\xc7")
        while at != -1:
            head, rex = at, 0
            if at and 0x40 <= data[at - 1] <= 0x4F:
                head, rex = at - 1, data[at - 1]
            modrm_at = at + 1
            # mod=10 (disp32), reg=000 (/0), rm != 100 (that would be a SIB byte).
            if (
                modrm_at + 9 <= len(data)
                and data[modrm_at] & 0xF8 == 0x80
                and data[modrm_at] & 0x07 != 0x04
                and not rex & 0x08  # REX.W -> qword store, a different instruction
            ):
                if struct.unpack_from("<i", data, modrm_at + 1)[0] == field_offset:
                    index = data[modrm_at] & 0x07
                    register = f"r{8 + index}" if rex & 0x01 else names[index]
                    imm = struct.unpack_from("<I", data, modrm_at + 5)[0]
                    out.append((base_va + head, register, imm))
            at = data.find(b"\xc7", at + 1)
    return out


def selftest():
    failures = []
    payload = bytes([0xF3, 0x0F, 0x1E, 0xFA, 0x56, 0x57, 0x48, 0x83, 0xEC, 0x28, 0xC3])
    image, va = LOC.synth_pe(payload, payload)
    pe = Pe(image)
    if find_bytes(pe, "f3 0f 1e fa 56 57") != [va]:
        failures.append("find_bytes did not return exactly the .text hit")
    if find_bytes(pe, "f3 0f 1e fa ?? 57") != [va]:
        failures.append("find_bytes did not honour the ?? wildcard")
    try:
        listing = disas(pe, va, 0x20)
    except ImportError:
        print("selftest: capstone absent; disassembly not exercised")
    else:
        if not listing or listing[0].mnemonic != "endbr64":
            failures.append("disas did not decode the synthetic endbr64")
        if listing[-1].mnemonic != "ret":
            failures.append("disas did not stop at ret")
    for failure in failures:
        print(f"SELFTEST FAIL: {failure}", file=sys.stderr)
    if failures:
        return 1
    print("selftest passed")
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "command",
        nargs="?",
        choices=[
            "disas",
            "find",
            "xref",
            "strings",
            "rip",
            "align",
            "pdata",
            "states",
            "crossmatch",
        ],
    )
    parser.add_argument("argument", nargs="*")
    parser.add_argument("--build", default="v201", choices=sorted(BUILD_MARKERS))
    parser.add_argument("-n", "--nbytes", default="0x120")
    parser.add_argument("--no-stop", action="store_true", help="do not stop the listing at `ret`")
    parser.add_argument("--utf16", action="store_true")
    parser.add_argument(
        "--lea",
        action="store_true",
        help="xref: also report rip-relative `lea` (how an option-table entry takes a callback's"
        " address). Costs a full capstone pass over the code sections.",
    )
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()

    if args.selftest:
        return selftest()
    if not args.command:
        parser.error("a command is required")
    if not args.argument:
        parser.error(f"{args.command} needs at least one argument")

    nbytes = int(args.nbytes, 0)

    if args.command == "crossmatch":
        # Take a byte window out of one build and count where it occurs in both. This is the
        # check a version gate needs before it can trust a pin: the window must occur exactly
        # once in the build it came from (so it identifies a function, not a shape) and not at
        # all in the other build (so the two versions' gates cannot both accept the same DLL).
        source, source_path = load(args.build)
        # "the other build" is the next-newest one actually on disk, not a hard-coded twin.
        others = [name for name in sorted(BUILD_MARKERS) if name != args.build]
        present = available_builds()
        other_name = next(
            (name for name in reversed(others) if name in present), others[-1]
        )
        other, other_path = load(other_name)
        print(f"# from {args.build}: {source_path}\n#   {source.describe()}")
        print(f"# vs   {other_name}: {other_path}\n#   {other.describe()}\n")
        for argument in args.argument:
            va = int(argument, 0)
            window = source.bytes_at_va(va, nbytes)
            if window is None:
                print(f"0x{va:x}: outside every section")
                continue
            here, there = source.find(window), other.find(window)
            verdict = (
                "OK -- unique here, absent there"
                if len(here) == 1 and not there
                else "NOT A GATE -- see counts"
            )
            print(f"0x{va:x} [{nbytes} bytes]  {verdict}")
            print(f"  in {args.build}: " + (", ".join(f"0x{h:x}" for h in here) or "none"))
            print(f"  in {other_name}: " + (", ".join(f"0x{h:x}" for h in there) or "none"))
        return 0

    if args.command == "states":
        pe, path = load(args.build)
        print(f"# {args.build}: {path}\n#   {pe.describe()}")
        for argument in args.argument:
            stores = state_stores(pe, int(argument, 0))
            values = {}
            for va, register, imm in stores:
                values.setdefault(imm, []).append(va)
            print(f"=== stores of an immediate to [reg+{argument}]: {len(stores)} sites ===")
            for imm in sorted(values):
                sites = ", ".join(f"0x{v:x}" for v in values[imm][:6])
                more = "" if len(values[imm]) <= 6 else f" (+{len(values[imm]) - 6} more)"
                print(f"  0x{imm:<8x} x{len(values[imm]):<3} {sites}{more}")
        return 0

    if args.command == "pdata":
        pe, path = load(args.build)
        print(f"# {args.build}: {path}\n#   {pe.describe()}")
        entries = pdata_entries(pe)
        for argument in args.argument:
            value = int(argument, 0)
            if value >= 0x1000 and value < BASE:
                rva = value
            elif value >= BASE:
                rva = value - BASE
            else:
                rva = None
            if rva is None:
                index = value
            else:
                index = next((i for i, (b, _) in enumerate(entries) if b == rva), None)
                if index is None:
                    print(f"{argument}: rva 0x{rva:x} is not a .pdata entry")
                    continue
            low, high = max(0, index - 6), min(len(entries), index + 7)
            print(f"=== {argument} -> index {index} ===")
            for i in range(low, high):
                begin, end = entries[i]
                mark = " <<<" if i == index else ""
                print(f"  [{i:>5}] 0x{begin:<8x}..0x{end:<8x} size 0x{end - begin:<5x}{mark}")
        return 0

    if args.command == "align":
        old, old_path = load("v199")
        new, new_path = load("v200")
        print(f"# old: {old_path}\n#   {old.describe()}")
        print(f"# new: {new_path}\n#   {new.describe()}\n")
        align_report(old, new, [int(a, 0) - BASE if int(a, 0) >= BASE else int(a, 0) for a in args.argument])
        return 0

    pe, path = load(args.build)
    print(f"# {args.build}: {path}\n#   {pe.describe()}")

    # Every command takes a list, so one invocation answers a whole batch of addresses. The
    # shell loop that would otherwise be needed pays uv's startup per address and is refused by
    # this workspace's command guard as unverifiable.
    for argument in args.argument:
        if args.command == "disas":
            print_disas(pe, int(argument, 0), nbytes, not args.no_stop, label=argument)
        elif args.command == "find":
            hits = find_bytes(pe, argument)
            print(f"{argument!r}: {len(hits)} hit(s): " + ", ".join(f"0x{h:x}" for h in hits))
        elif args.command == "xref":
            print(f"=== xrefs to {argument} ===")
            for site, kind in xrefs_to(pe, int(argument, 0), direct_only=not args.lea):
                print(f"  {kind} from 0x{site:x}")
        elif args.command == "rip":
            print(f"=== rip-relative from {argument} ===")
            for site, text, target in rip_targets(pe, int(argument, 0), nbytes):
                print(f"  0x{site:x}  {text:<40} -> 0x{target:x}")
        elif args.command == "strings":
            hits = find_string(pe, argument, "utf16" if args.utf16 else "ascii")
            print(f"{argument!r}: {len(hits)} hit(s): " + ", ".join(f"0x{h:x}" for h in hits))
    return 0


if __name__ == "__main__":
    sys.exit(main())
