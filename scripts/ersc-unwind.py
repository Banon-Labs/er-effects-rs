#!/usr/bin/env python3
"""Read the x64 exception data of the installed Seamless Co-op `ersc.dll`.

Answers "does a `try`/`catch` sit in this frame" without decoding a single instruction.
An MSVC `catch` is not only code: it forces `UNW_FLAG_EHANDLER` into the frame's
`UNWIND_INFO` and a `__CxxFrameHandler3` language-specific record naming every catch
type. So the question "is the throw caught below us" is a table lookup, and it stays a
table lookup even where the code is mutated beyond linear decode.

    uv run --with capstone python3 scripts/ersc-unwind.py --rva 0x28a85b 0x636e75
    uv run --with capstone python3 scripts/ersc-unwind.py --throwinfo 0x201658
    uv run --with capstone python3 scripts/ersc-unwind.py --survey
    uv run --with capstone python3 scripts/ersc-unwind.py --selftest

This module carries two exception tables and they are not the same table. The section
`.pdata` at rva `0x223000` is the linker's, 4903 entries over `.text`, disjoint. The
`IMAGE_DIRECTORY_ENTRY_EXCEPTION` the loader actually reads was repointed by the
protector to rva `0xcfad90`: the same 4903 entries plus 7591 for the `ERSC` section.
`--table old|new|both` selects which is consulted; the loader reads `new`, so that is
the default and the only one whose verdict describes the running process.
"""

from __future__ import annotations

import argparse
import bisect
import os
import struct
import sys

DEFAULT_DLL = os.environ.get(
    "ER_ERSC_DLL",
    "/home/banon/.local/share/Steam/steamapps/common/ELDEN RING/Game/SeamlessCoop/ersc.dll",
)

UNW_FLAG_EHANDLER = 0x1
UNW_FLAG_UHANDLER = 0x2
UNW_FLAG_CHAININFO = 0x4

UWOP = {
    0: "PUSH_NONVOL",
    1: "ALLOC_LARGE",
    2: "ALLOC_SMALL",
    3: "SET_FPREG",
    4: "SAVE_NONVOL",
    5: "SAVE_NONVOL_FAR",
    6: "EPILOG",
    7: "SPARE",
    8: "SAVE_XMM128",
    9: "SAVE_XMM128_FAR",
    10: "PUSH_MACHFRAME",
}
REGS = ["rax", "rcx", "rdx", "rbx", "rsp", "rbp", "rsi", "rdi",
        "r8", "r9", "r10", "r11", "r12", "r13", "r14", "r15"]


class Image:
    def __init__(self, path: str) -> None:
        self.path = path
        d = self.data = open(path, "rb").read()
        pe = struct.unpack_from("<I", d, 0x3C)[0]
        nsec = struct.unpack_from("<H", d, pe + 6)[0]
        optsize = struct.unpack_from("<H", d, pe + 20)[0]
        self.base = struct.unpack_from("<Q", d, pe + 24 + 24)[0]
        sect = pe + 24 + optsize
        self.secs = []
        for i in range(nsec):
            o = sect + i * 40
            name = d[o : o + 8].rstrip(b"\0").decode(errors="replace")
            vs, va, rs, rp = struct.unpack_from("<IIII", d, o + 8)
            self.secs.append((name, va, vs, rp, rs))
        self.dir_rva, self.dir_size = struct.unpack_from("<II", d, pe + 24 + 112 + 3 * 8)
        self.tables = {
            "new": self._read_table(self.dir_rva, self.dir_size),
            "old": self._read_table(*self._section_span(".pdata")),
        }

    def _section_span(self, want):
        for name, va, vs, _rp, _rs in self.secs:
            if name == want:
                return va, vs
        raise KeyError(want)

    def _read_table(self, rva, size):
        o = self.off(rva)
        out = []
        for i in range(size // 12):
            s, e, u = struct.unpack_from("<III", self.data, o + i * 12)
            if s == 0 and e == 0 and u == 0:
                continue
            out.append((s, e, u))
        return out

    def off(self, rva):
        for _n, va, vs, rp, rs in self.secs:
            if va <= rva < va + max(vs, rs):
                return rp + (rva - va)
        return None

    def section_of(self, rva):
        for n, va, vs, rp, rs in self.secs:
            if va <= rva < va + max(vs, rs):
                return n
        return None

    def u32(self, rva):
        return struct.unpack_from("<I", self.data, self.off(rva))[0]

    def cstr(self, rva, limit=256):
        o = self.off(rva)
        end = self.data.find(b"\0", o, o + limit)
        return self.data[o : end if end >= 0 else o + limit].decode("latin1")

    # ---- lookup -------------------------------------------------------------
    def lookup_all(self, rva, table="new"):
        """Every entry whose range contains `rva`, in table order."""
        return [(s, e, u) for s, e, u in self.tables[table] if s <= rva < e]

    def lookup_binary(self, rva, table="new"):
        """What `RtlLookupFunctionEntry`'s binary search returns for `rva`.

        The loader does not scan; it bisects, and it bisects the table exactly as stored.
        A table that is not sorted by `Begin`, or whose ranges overlap, makes the bisect
        land somewhere other than the containing entry -- which is a different failure
        from having no entry, so it is reported separately from `lookup_all`.
        """
        t = self.tables[table]
        lo, hi = 0, len(t) - 1
        while lo <= hi:
            mid = (lo + hi) // 2
            s, e, u = t[mid]
            if rva < s:
                hi = mid - 1
            elif rva >= e:
                lo = mid + 1
            else:
                return (s, e, u)
        return None

    def table_sorted(self, table="new"):
        t = self.tables[table]
        return all(t[i][0] <= t[i + 1][0] for i in range(len(t) - 1))


class UnwindInfo:
    def __init__(self, img: Image, rva: int) -> None:
        d = img.data
        o = img.off(rva)
        self.rva = rva
        self.ok = o is not None
        if not self.ok:
            return
        b0, self.prolog, self.ncodes, b3 = struct.unpack_from("<BBBB", d, o)
        self.version = b0 & 7
        self.flags = b0 >> 3
        self.frame_reg = b3 & 0xF
        self.frame_off = (b3 >> 4) * 16
        self.codes = []
        i = 0
        raw = [struct.unpack_from("<H", d, o + 4 + 2 * k)[0] for k in range(self.ncodes)]
        while i < self.ncodes:
            w = raw[i]
            # `UNWIND_CODE` is `UBYTE CodeOffset; UBYTE UnwindOp:4; UBYTE OpInfo:4`, and
            # MSVC fills bitfields from the low bit, so within the second byte `UnwindOp`
            # is the low nibble and `OpInfo` the high one. Read as a little-endian word
            # that is bits 8..11 and 12..15 -- inverting the two turns ordinary
            # `ALLOC_SMALL` records into fictitious `SAVE_NONVOL` overruns.
            offset = w & 0xFF
            op = (w >> 8) & 0xF
            opinfo = (w >> 12) & 0xF
            slots = 1
            if op in (1,):
                slots = 2 if opinfo == 0 else 3
            elif op in (4, 8):
                slots = 2
            elif op in (5, 9):
                slots = 3
            # `slots` is what the opcode demands; `raw[i:i+slots]` is what the record
            # actually supplies, and a short slice is exactly the overrun that makes the
            # unwinder read past `CountOfCodes` into the handler rva. Keep both.
            self.codes.append((offset, UWOP.get(op, str(op)), opinfo, raw[i : i + slots], slots))
            i += slots
        tail = o + 4 + 2 * ((self.ncodes + 1) & ~1)
        self.handler = None
        self.lang_rva = None
        self.chain = None
        if self.flags & UNW_FLAG_CHAININFO:
            self.chain = struct.unpack_from("<III", d, tail)
        elif self.flags & (UNW_FLAG_EHANDLER | UNW_FLAG_UHANDLER):
            self.handler = struct.unpack_from("<I", d, tail)[0]
            self.lang_rva = tail  # file offset of the language data that follows
            self.lang_off = tail + 4

    def flagstr(self):
        f = []
        if self.flags & UNW_FLAG_EHANDLER:
            f.append("EHANDLER")
        if self.flags & UNW_FLAG_UHANDLER:
            f.append("UHANDLER")
        if self.flags & UNW_FLAG_CHAININFO:
            f.append("CHAININFO")
        return "|".join(f) or "none"


def parse_funcinfo(img: Image, fi_rva: int, indent="      "):
    """Decode `_s_FuncInfo` -- the `__CxxFrameHandler3` language record.

    `nTryBlocks == 0` is the whole answer for a frame: the compiler emitted unwind-only
    cleanup (destructors), never a `catch`, so an exception passes straight through.
    """
    d = img.data
    o = img.off(fi_rva)
    if o is None:
        print(f"{indent}FuncInfo rva {fi_rva:#x} is unmapped")
        return None
    magic, max_state, disp_unwind, n_try, disp_try, n_ip, disp_ip, disp_help, disp_es, ehflags = (
        struct.unpack_from("<10I", d, o)
    )
    print(f"{indent}FuncInfo @{fi_rva:#x}  magic {magic:#x}  maxState {max_state}  "
          f"nTryBlocks {n_try}  nIPMap {n_ip}  ESTypeList {disp_es:#x}  EHFlags {ehflags:#x}")
    if magic not in (0x19930520, 0x19930521, 0x19930522):
        print(f"{indent}  magic is not a FuncInfo -- record is not __CxxFrameHandler3 shape")
        return None
    catches = []
    if n_try and disp_try:
        to = img.off(disp_try)
        for i in range(n_try):
            tl, th, ch, nc, disp_h = struct.unpack_from("<5I", d, to + i * 20)
            print(f"{indent}  try[{i}] states {tl}..{th} catchHigh {ch} nCatches {nc} "
                  f"handlers @{disp_h:#x}")
            ho = img.off(disp_h)
            for j in range(nc):
                adj, disp_type, disp_obj, disp_fn, disp_frame = struct.unpack_from(
                    "<5I", d, ho + j * 20
                )
                if disp_type == 0:
                    tname = "... (catch-all ellipsis)"
                else:
                    tname = img.cstr(disp_type + 16)
                print(f"{indent}    catch[{j}] adjectives {adj:#x} type {disp_type:#x} "
                      f"`{tname}` funclet {disp_fn:#x} ({img.section_of(disp_fn)})")
                catches.append((tname, disp_fn, adj))
    return {"magic": magic, "n_try": n_try, "catches": catches}


def parse_scopetable(img: Image, off: int, indent="      "):
    d = img.data
    (count,) = struct.unpack_from("<I", d, off)
    print(f"{indent}SCOPE_TABLE count {count}")
    out = []
    for i in range(min(count, 64)):
        b, e, h, t = struct.unpack_from("<4I", d, off + 4 + i * 16)
        kind = "EXCEPT (filter)" if h not in (0, 1) else ("FINALLY" if h == 0 else "EXCEPT(1)")
        print(f"{indent}  [{i}] {b:#x}..{e:#x} handler {h:#x} target {t:#x}   {kind}")
        out.append((b, e, h, t))
    return out


def describe(img: Image, rva: int, table: str, handler_names: dict, depth=0):
    pad = "  " * depth
    sec = img.section_of(rva)
    print(f"\n{pad}##### rva {rva:#x} (va {img.base + rva:#x}, section {sec}) [table={table}]")
    allents = img.lookup_all(rva, table)
    bs = img.lookup_binary(rva, table)
    print(f"{pad}  containing entries: {len(allents)}")
    for s, e, u in allents[:12]:
        print(f"{pad}    {s:#x}..{e:#x} (span {e - s:#x}) unwind @{u:#x} ({img.section_of(u)})")
    if len(allents) > 12:
        print(f"{pad}    ... {len(allents) - 12} more")
    print(f"{pad}  RtlLookupFunctionEntry bisect returns: "
          f"{'NONE' if bs is None else f'{bs[0]:#x}..{bs[1]:#x} unwind @{bs[2]:#x}'}")
    seen = set()
    for s, e, u in allents:
        if u in seen:
            continue
        seen.add(u)
        ui = UnwindInfo(img, u)
        if not ui.ok:
            print(f"{pad}    unwind @{u:#x} unmapped")
            continue
        hn = ""
        if ui.handler is not None:
            hn = f"  handler {ui.handler:#x} {handler_names.get(ui.handler, '(unknown)')}"
        print(f"{pad}    unwind @{u:#x} for {s:#x}..{e:#x}: ver {ui.version} flags "
              f"{ui.flagstr()} prolog {ui.prolog:#x} ncodes {ui.ncodes} "
              f"frame {REGS[ui.frame_reg] if ui.frame_reg else '-'}+{ui.frame_off:#x}{hn}")
        if ui.chain:
            print(f"{pad}      CHAININFO -> {ui.chain[0]:#x}..{ui.chain[1]:#x} unwind @{ui.chain[2]:#x}")
        if ui.handler is not None:
            name = handler_names.get(ui.handler, "")
            if "CxxFrameHandler" in name:
                fi = struct.unpack_from("<I", img.data, ui.lang_off)[0]
                parse_funcinfo(img, fi, pad + "      ")
            elif "C_specific" in name:
                parse_scopetable(img, ui.lang_off, pad + "      ")
            else:
                blob = struct.unpack_from("<4I", img.data, ui.lang_off)
                print(f"{pad}      language data head: {[hex(x) for x in blob]}")


def find_handlers(img: Image):
    """Name the language-specific handlers by how their language data actually parses.

    The module is statically linked so neither handler is an import and neither has a
    symbol. A handler whose language record starts with a `_s_FuncInfo` magic is
    `__CxxFrameHandler3`; one whose record is a plausible `SCOPE_TABLE` is
    `__C_specific_handler`. That is evidence from the data, not a guess from the address.
    """
    from collections import Counter

    votes = Counter()
    cxx, spec = Counter(), Counter()
    for s, e, u in img.tables["new"]:
        ui = UnwindInfo(img, u)
        if not ui.ok or ui.handler is None:
            continue
        votes[ui.handler] += 1
        magic = struct.unpack_from("<I", img.data, ui.lang_off)[0]
        fi = img.off(magic) if magic else None
        if fi is not None:
            m = struct.unpack_from("<I", img.data, fi)[0]
            if m in (0x19930520, 0x19930521, 0x19930522):
                cxx[ui.handler] += 1
                continue
        cnt = struct.unpack_from("<I", img.data, ui.lang_off)[0]
        if 0 < cnt < 4096:
            spec[ui.handler] += 1
    names = {}
    for h, n in votes.most_common():
        if cxx[h] > spec[h]:
            names[h] = f"__CxxFrameHandler3 (FuncInfo magic on {cxx[h]}/{n})"
        elif spec[h]:
            names[h] = f"__C_specific_handler (SCOPE_TABLE shape on {spec[h]}/{n})"
        else:
            names[h] = f"(unclassified, {n} uses)"
    return names, votes


def parse_throwinfo(img: Image, rva: int):
    d = img.data
    o = img.off(rva)
    attrs, pfn_unwind, pfn_fwd, disp_catchable = struct.unpack_from("<4I", d, o)
    print(f"\n===== _ThrowInfo @{rva:#x} =====")
    print(f"  attributes {attrs:#x}  pmfnUnwind {pfn_unwind:#x}  pForwardCompat {pfn_fwd:#x} "
          f"CatchableTypeArray @{disp_catchable:#x}")
    ao = img.off(disp_catchable)
    (n,) = struct.unpack_from("<I", d, ao)
    print(f"  {n} catchable types (a catch matches the throw only if it names one of these, "
          f"or is an ellipsis):")
    types = []
    for i in range(n):
        ct = struct.unpack_from("<I", d, ao + 4 + i * 4)[0]
        props, disp_type, thisoff, cdisp, vdisp, size, copy = struct.unpack_from(
            "<7I", d, img.off(ct)
        )
        name = img.cstr(disp_type + 16)
        print(f"    [{i}] `{name}`  (props {props:#x} size {size:#x})")
        types.append(name)
    return types


def survey(img: Image, handler_names):
    """How much of each table carries a `catch` at all."""
    from collections import Counter

    for tname in ("old", "new"):
        t = img.tables[tname]
        c = Counter()
        trys = 0
        ersc_eh = 0
        for s, e, u in t:
            ui = UnwindInfo(img, u)
            if not ui.ok:
                c["unmapped"] += 1
                continue
            c[ui.flagstr()] += 1
            if ui.handler is not None and "CxxFrameHandler" in handler_names.get(ui.handler, ""):
                fi = struct.unpack_from("<I", img.data, ui.lang_off)[0]
                fo = img.off(fi)
                if fo is None:
                    continue
                m, _ms, _du, n_try = struct.unpack_from("<4I", img.data, fo)
                if m in (0x19930520, 0x19930521, 0x19930522) and n_try:
                    trys += 1
                    if img.section_of(s) == "ERSC":
                        ersc_eh += 1
        print(f"\n===== table `{tname}`: {len(t)} entries =====")
        for k, v in c.most_common():
            print(f"  {k:<24} {v}")
        print(f"  entries whose FuncInfo has >=1 try block: {trys}  (of those, begin in ERSC: {ersc_eh})")
        print(f"  sorted by Begin: {img.table_sorted(tname)}")


def malformed_reason(img: Image, u: int):
    """Why this `UNWIND_INFO` cannot be processed, or `None` if it is well formed.

    The x64 record is self-describing, so garbage announces itself. `Version` is 1 or 2
    and nothing else. `UNW_FLAG_CHAININFO` is exclusive with the two handler flags. And
    an unwind code that needs more slots than `CountOfCodes` declares makes the unwinder
    read the handler `rva` as a stack displacement -- the failure that matters here,
    because it happens before the handler is ever returned.
    """
    ui = UnwindInfo(img, u)
    if not ui.ok:
        return "unwind data is unmapped"
    if ui.version not in (1, 2):
        return f"Version is {ui.version} (must be 1 or 2)"
    if ui.flags & UNW_FLAG_CHAININFO and ui.flags & (UNW_FLAG_EHANDLER | UNW_FLAG_UHANDLER):
        return "CHAININFO combined with EHANDLER/UHANDLER"
    used = sum(c[4] for c in ui.codes)
    if used > ui.ncodes:
        return (f"unwind code {ui.codes[-1][1]} needs {used} slots but CountOfCodes is "
                f"{ui.ncodes}; the unwinder reads the handler rva as a stack displacement")
    return None


def overrun_displacement(img: Image, u: int):
    """The `rsp` displacement the overrunning `*_FAR` code makes the unwinder dereference."""
    ui = UnwindInfo(img, u)
    return struct.unpack_from("<I", img.data, img.off(u) + 6)[0]


CATCHABLE_BY_THE_THROW = (
    ".?AVsystem_error@std@@",
    ".?AV_System_error@std@@",
    ".?AVruntime_error@std@@",
    ".?AVexception@std@@",
)
CXX_MAGIC = (0x19930520, 0x19930521, 0x19930522)


def catch_records(img: Image, table="new"):
    """Every distinct unwind record in `table` that declares at least one `catch`."""
    handler = cxx_handler(img)
    out = {}
    for _s, _e, u in img.tables[table]:
        if u in out:
            continue
        ui = UnwindInfo(img, u)
        if not ui.ok or ui.handler != handler:
            continue
        fi = struct.unpack_from("<I", img.data, ui.lang_off)[0]
        fo = img.off(fi)
        if fo is None:
            continue
        magic, _ms, _du, n_try, disp_try = struct.unpack_from("<5I", img.data, fo)
        if magic not in CXX_MAGIC or not n_try:
            continue
        names = []
        to = img.off(disp_try)
        for i in range(n_try):
            _tl, _th, _ch, nc, dh = struct.unpack_from("<5I", img.data, to + i * 20)
            ho = img.off(dh)
            if ho is None:
                continue
            for j in range(nc):
                _adj, dt, _do, dfn, _df = struct.unpack_from("<5I", img.data, ho + j * 20)
                names.append(("..." if dt == 0 else img.cstr(dt + 16), dfn))
        out[u] = names
    return out


def cxx_handler(img: Image):
    names, _votes = find_handlers(img)
    for h, n in names.items():
        if "CxxFrameHandler" in n and "7562" in n or "CxxFrameHandler" in n:
            pass
    best = max(
        (h for h, n in names.items() if "CxxFrameHandler" in n),
        key=lambda h: sum(1 for _s, _e, u in img.tables["new"] if UnwindInfo(img, u).handler == h),
        default=None,
    )
    return best


def selftest(img: Image) -> int:
    """Assert every fact the `caught` / `not caught` verdict rests on.

    Written as checks rather than prose so the verdict cannot rot silently: a Seamless
    build whose protector emits a different table fails here instead of being described
    by a stale paragraph.
    """
    fails = []

    def check(name, cond, detail=""):
        print(f"{'pass' if cond else 'FAIL'}: {name}{('  -- ' + detail) if detail else ''}")
        if not cond:
            fails.append(name)

    old, new = img.tables["old"], img.tables["new"]
    check("the linker's .pdata is a subset of the directory the loader reads",
          set(old) <= set(new), f"{len(old)} of {len(new)}")
    check("the linker's .pdata is sorted and disjoint", img.table_sorted("old") and
          all(old[i][1] <= old[i + 1][0] for i in range(len(old) - 1)))
    check("the loader's directory is NOT sorted, so its bisect is unreliable",
          not img.table_sorted("new"),
          f"{sum(1 for i in range(len(new) - 1) if new[i][0] > new[i + 1][0])} inversions")

    bad_old = {u for _s, _e, u in old if malformed_reason(img, u)}
    bad_new = {u for _s, _e, u in new if malformed_reason(img, u)}
    check("no record the linker emitted is unreadable", not bad_old, f"{len(bad_old)} bad")
    check("exactly one record the loader's directory names is unreadable",
          bad_new == {0x249E0},
          ", ".join(f"{u:#x}: {malformed_reason(img, u)}" for u in sorted(bad_new)))
    check("that unreadable record is .text code, not exception data",
          img.section_of(0x249E0) == ".text" and
          img.data[img.off(0x249E0) : img.off(0x249E0) + 3] == b"\xff\x50\x08",
          "0x249e0 is `call qword ptr [rax + 8]`")

    # The frame directly below our detour.
    ent = img.lookup_binary(0x28A85B)
    check("rva 0x28a85b bisects to the protector's blanket entry 0x240000..0x333fd1",
          ent == (0x240000, 0x333FD1, 0xCFAD70),
          "none" if ent is None else f"{ent[0]:#x}..{ent[1]:#x} unwind {ent[2]:#x}")
    if ent:
        ui = UnwindInfo(img, ent[2])
        check("that record is well formed, so the unwinder will act on it",
              malformed_reason(img, ent[2]) is None,
              f"ver {ui.version} {ui.flagstr()} prolog {ui.prolog:#x} "
              + " ".join(f"{c[1]}({c[2]})@{c[0]:#x}" for c in ui.codes))
        # It says: `push rdi` ending at prolog offset 0x14, `sub rsp,0x50` ending at 0x18.
        # Those bytes are a table of `jmp rel32`, so the frame description is a fiction
        # applied to 0xf3fd1 bytes of mutated code that has no single frame shape.
        o = img.off(ent[0])
        check("its claimed prologue is not the code at its BeginAddress -- the frame "
              "description is a fiction",
              img.data[o + 0x13] != 0x57 or img.data[o + 0x14 : o + 0x18] != b"\x48\x83\xec\x50",
              f"bytes at +0x13: {img.data[o + 0x13:o + 0x18].hex(' ')}, and +0x00 is "
              f"{img.data[o:o + 5].hex(' ')} (`jmp rel32`)")
        check("it declares EHANDLER but carries no language-specific data, so it can "
              "express no catch of any type",
              bool(ui.flags & UNW_FLAG_EHANDLER)
              and img.data[img.off(ent[2]) + 12 : img.off(ent[2]) + 32] == b"\0" * 20,
              f"handler {ui.handler:#x} in {img.section_of(ui.handler)}, 20 zero bytes of "
              f"language data")

    # The other recorded frame.
    ent2 = img.lookup_binary(0x636E75)
    check("rva 0x636e75 bisects to 0x635147..0x636e7a with a real .rdata record",
          ent2 == (0x635147, 0x636E7A, 0x214460))
    if ent2:
        ui = UnwindInfo(img, ent2[2])
        fi = struct.unpack_from("<I", img.data, ui.lang_off)[0]
        magic, _ms, _du, n_try = struct.unpack_from("<4I", img.data, img.off(fi))
        check("that record is __CxxFrameHandler3 with zero try blocks -- cleanup, no catch",
              magic in CXX_MAGIC and n_try == 0, f"magic {magic:#x} nTryBlocks {n_try}")

    # No catch was synthesized for the mutated section.
    cnew, cold = catch_records(img, "new"), catch_records(img, "old")
    check("every catch-bearing unwind record in the loader's directory is one the linker "
          "emitted for .text -- the protector synthesized none",
          set(cnew) == set(cold), f"{len(cnew)} records, {len(cold)} of them from .pdata")
    matching = {u: [n for n, _f in v] for u, v in cnew.items()
                if any(n == "..." or n in CATCHABLE_BY_THE_THROW for n, _f in v)}
    check("catch clauses that would match this throw exist only in .text",
          all(any(img.section_of(s) == ".text" for s, _e, uu in old if uu == u)
              for u in matching), f"{len(matching)} such records")

    names = parse_throwinfo(img, 0x201658)
    check("ThrowInfo 0x201658 is std::system_error", any("system_error" in n for n in names))

    print()
    print(("selftest passed" if not fails else f"selftest FAILED: {fails}"))
    return 0 if not fails else 1


def verdict(img: Image) -> None:
    print("===== does an ERSC frame below the detour catch the std::system_error? =====")
    for pc, label in ((0x28A85B, "the call site into our detour"),
                      (0x636E75, "the other recorded frame")):
        ent = img.lookup_binary(pc)
        if ent is None:
            print(f"  {pc:#x} ({label}): no entry -- the unwinder treats it as a LEAF frame")
            continue
        ui = UnwindInfo(img, ent[2])
        cat = catch_records(img, "new").get(ent[2])
        print(f"  {pc:#x} ({label}) -> {ent[0]:#x}..{ent[1]:#x} unwind {ent[2]:#x} "
              f"[{img.section_of(ent[2])}] flags {ui.flagstr()}")
        print(f"      catch clauses declared: {cat if cat else 'none'}")
    holes = 0
    total = 0
    for pc in range(0x240000, 0xD30000, 0x40):
        total += 1
        if img.lookup_binary(pc) is None:
            holes += 1
    print(f"  {holes}/{total} sampled ERSC addresses have no entry at all "
          f"({100 * holes // total}%), so the unwinder would unwind them as leaf frames.")
    print("  verdict: NOT CAUGHT. Every catch in the module belongs to a record the linker "
          "emitted for .text; the protector synthesized none for the mutated section.")


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("--dll", default=DEFAULT_DLL)
    ap.add_argument("--rva", nargs="*", default=[])
    ap.add_argument("--table", default="new", choices=["new", "old", "both"])
    ap.add_argument("--throwinfo")
    ap.add_argument("--survey", action="store_true")
    ap.add_argument("--handlers", action="store_true")
    ap.add_argument("--selftest", action="store_true")
    ap.add_argument("--verdict", action="store_true")
    a = ap.parse_args()

    img = Image(a.dll)
    names, votes = find_handlers(img)
    if a.handlers or a.survey or a.rva:
        print("===== language-specific handlers in this module =====")
        for h, n in votes.most_common(8):
            print(f"  handler rva {h:#x} ({img.section_of(h)}): {n} frames -- {names[h]}")
    if a.verdict:
        verdict(img)
    if a.selftest:
        return selftest(img)
    if a.throwinfo:
        parse_throwinfo(img, int(a.throwinfo, 0))
    if a.survey:
        survey(img, names)
    tables = ["old", "new"] if a.table == "both" else [a.table]
    for raw in a.rva:
        for t in tables:
            describe(img, int(raw, 0), t, names)
    return 0


if __name__ == "__main__":
    sys.exit(main())
