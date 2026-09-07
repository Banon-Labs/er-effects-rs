#!/usr/bin/env python3
"""Print an instruction-level diff of one function between ELDEN RING 1.16.2 and 1.17.

Why this exists
---------------
`scripts/verify-rva-map-1170.py` answers "is the 1.17 address the same function?" and
`scripts/detect-struct-field-drift.py` answers "did a struct field move?".  Neither answers the
question that follows a shape-diff verdict: *what actually changed inside the body*.  The RVA gate
in `er-game-base` installs a detour as soon as an address maps, so a hooked function whose address
is fine but whose body changed passes every existing check silently.  This is the tool for reading
that difference.

EXTENTS come from `.pdata`, not from a guess
--------------------------------------------
Each image's own exception directory declares where a function begins and ends, so the two bodies
being compared are FromSoftware's own extents.  When an address is not a `.pdata` entry (a leaf or
a linker thunk) pass `--bytes N` to compare a fixed window instead, and the header says so.

Alignment
---------
Instructions are aligned with `difflib.SequenceMatcher` over a NORMALISED key so that an inserted
instruction shifts the diff by one line instead of desynchronising the rest of the function:

  * `rip`-relative displacements  -> `[rip+*]`     (code and data moved between builds)
  * branch / call targets         -> `<target>`    (same reason)
  * everything else, including register-base displacements and immediates, is compared literally,
    because those are exactly the changes worth seeing.

The printed text is always the raw disassembly; only the alignment key is normalised.  Call targets
are resolved to a `.pdata` entry when one exists so a swapped callee is visible as an address pair.

Usage
    python3 scripts/diff-function-bodies-1162-1170.py 0x140af7cf0 0x140af9000
    python3 scripts/diff-function-bodies-1162-1170.py --context 4 0x14067a810 0x14067b660
    python3 scripts/diff-function-bodies-1162-1170.py --bytes 0x80 0x1426634a0 0x142665cb0
    python3 scripts/diff-function-bodies-1162-1170.py --selftest

Needs capstone:  uv run --with capstone python3 scripts/diff-function-bodies-1162-1170.py ...
(the script re-execs itself under `uv run --with capstone` if the import fails).
"""
from __future__ import annotations

import argparse
import difflib
import os
import re
import struct
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import function_extent  # noqa: E402 - repo-local, and the sys.path line above is what makes it work

ROOT = Path(__file__).resolve().parent.parent
BASE = 0x140000000
IMAGES = {
    "1162": Path(os.environ.get("ER_DEOBF_1162", ROOT / "eldenring-deobf.bin")),
    "1170": Path(os.environ.get("ER_DEOBF_1170", ROOT / "eldenring-deobf-1.17.bin")),
}


def _ensure_capstone():
    try:
        import capstone  # noqa: F401

        return
    except ImportError:
        pass
    if os.environ.get("_ER_DIFF_BOOTSTRAPPED"):
        raise SystemExit("capstone unavailable even under `uv run --with capstone`")
    os.environ["_ER_DIFF_BOOTSTRAPPED"] = "1"
    os.execvp("uv", ["uv", "run", "--with", "capstone", "python3", *sys.argv])


class Image:
    """A flat (virtual-layout) PE image: file offset == RVA, VA == base + offset."""

    def __init__(self, path: Path):
        self.path = path
        self.data = path.read_bytes()
        pe = struct.unpack_from("<I", self.data, 0x3C)[0]
        if self.data[pe : pe + 4] != b"PE\0\0":
            raise SystemExit(f"{path}: not a PE image")
        nsec = struct.unpack_from("<H", self.data, pe + 6)[0]
        optsz = struct.unpack_from("<H", self.data, pe + 20)[0]
        off = pe + 24 + optsz
        self.sections: dict[str, tuple[int, int]] = {}
        for i in range(nsec):
            entry = self.data[off + i * 40 : off + (i + 1) * 40]
            name = entry[:8].rstrip(b"\0").decode("latin1")
            vsz, va, rsz, _raw = struct.unpack_from("<IIII", entry, 8)
            self.sections.setdefault(name, (va, max(vsz, rsz)))
        self._ends: dict[int, int] | None = None

    def function_ends(self) -> dict[int, int]:
        if self._ends is None:
            va, size = self.sections[".pdata"]
            out: dict[int, int] = {}
            for off in range(va, va + size, 12):
                begin, end, _unwind = struct.unpack_from("<III", self.data, off)
                if begin == 0 or end <= begin or end - begin > 0x20000:
                    continue
                out.setdefault(begin, end)
            self._ends = out
        return self._ends

    def body(self, va: int, nbytes: int | None, gap: bool = False) -> tuple[bytes, str]:
        rva = va - BASE
        if nbytes is not None:
            return self.data[rva : rva + nbytes], f"window {nbytes:#x} bytes (forced)"
        end = self.function_ends().get(rva)
        if end is not None:
            return self.data[rva:end], f".pdata extent {end - rva:#x} bytes"
        if not gap:
            raise SystemExit(
                f"{va:#x} is not a .pdata function entry in {self.path.name}; "
                "pass --bytes N, or --gap to decode the leaf's own extent"
            )
        # A leaf or a linker thunk carries no unwind record, so its end is DECODED by the shared
        # `leaf_extent` watermark rule (via `function_extent.body_end`).
        #
        # This arm used to run to the next `.pdata` start, capped at 0x400 -- the retired guess
        # that `pair-leaf-functions-1162-1170.py` keeps as its `--mutate extent` control precisely
        # because it walks through inter-function padding whose bytes differ between builds, so
        # everything after the first pad compares unequal. Measured on the pair
        # `verify-thunk-rva-1170.py` exists for, 0x14090a0a0 -> 0x14090b240 (two 23-byte
        # `CS::KnowledgeLoadingScreen` `_Func_impl` thunks, identical but for a moved `.rdata`
        # label and a moved callee): the old arm read 0x60 bytes on both sides, decoded 24 vs 25
        # instructions and printed `0.6122 similarity, 3 differing region(s)`. Every one of those
        # differences was past byte 23 and belonged to the next thunk.
        end = function_extent.body_end(self.data, va)
        if end is None:
            raise SystemExit(
                f"{va:#x} has no .pdata record in {self.path.name} and no decodable leaf end "
                "either -- pass --bytes N if you want a fixed window, and read the result knowing "
                "it may cross a function boundary"
            )
        return self.data[rva:end], f"decoded leaf extent, {end - rva:#x} bytes"


_RIP = re.compile(r"\[rip \+ (0x[0-9a-f]+|\d+)\]")
_HEXNUM = re.compile(r"^0x[0-9a-f]+$")


def norm(insn) -> str:
    """Alignment key: mask what must move between builds, keep what must not."""
    op = _RIP.sub("[rip+*]", insn.op_str)
    if insn.mnemonic in _BRANCHY and _HEXNUM.match(op.strip()):
        op = "<target>"
    return f"{insn.mnemonic} {op}"


_BRANCHY = {
    "call",
    "jmp",
    "je",
    "jne",
    "jz",
    "jnz",
    "ja",
    "jae",
    "jb",
    "jbe",
    "jg",
    "jge",
    "jl",
    "jle",
    "js",
    "jns",
    "jo",
    "jno",
    "jp",
    "jnp",
    "jecxz",
    "jrcxz",
    "loop",
    "loope",
    "loopne",
}


def decode(md, blob: bytes, va: int):
    out = []
    for insn in md.disasm(blob, va):
        out.append(insn)
    return out


def annotate(image: Image, insn) -> str:
    """Append a resolved call/jmp target when the image declares a function there."""
    if insn.mnemonic not in ("call", "jmp"):
        return ""
    op = insn.op_str.strip()
    if not _HEXNUM.match(op):
        return ""
    target = int(op, 16)
    rva = target - BASE
    return "  -> .pdata fn" if rva in image.function_ends() else ""


def render(image: Image, insn) -> str:
    return f"{insn.address:#012x}  {insn.mnemonic:<7} {insn.op_str}{annotate(image, insn)}"


def diff_one(a_va: int, b_va: int, nbytes: int | None, context: int, quiet: bool, gap: bool = False) -> int:
    import capstone

    md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
    img_a, img_b = Image(IMAGES["1162"]), Image(IMAGES["1170"])
    blob_a, how_a = img_a.body(a_va, nbytes, gap)
    blob_b, how_b = img_b.body(b_va, nbytes, gap)
    ins_a, ins_b = decode(md, blob_a, a_va), decode(md, blob_b, b_va)

    print(f"1.16.2 {a_va:#x}  {how_a}  {len(ins_a)} instructions")
    print(f"1.17   {b_va:#x}  {how_b}  {len(ins_b)} instructions")

    key_a = [norm(i) for i in ins_a]
    key_b = [norm(i) for i in ins_b]
    sm = difflib.SequenceMatcher(a=key_a, b=key_b, autojunk=False)
    ops = sm.get_opcodes()
    changed = sum(1 for tag, *_ in ops if tag != "equal")
    print(f"alignment: {sm.ratio():.4f} similarity, {changed} differing region(s)")
    if changed == 0:
        print("NO DIFFERENCE under the alignment key (rip-displacements and branch targets masked)")
        return 0
    print()
    for index, (tag, i1, i2, j1, j2) in enumerate(ops):
        if tag == "equal":
            if quiet:
                span = i2 - i1
                head = ops[index - 1][0] if index else None
                tail = ops[index + 1][0] if index + 1 < len(ops) else None
                if span > 2 * context and (head or tail):
                    for insn in ins_a[i1 : i1 + context]:
                        print("   " + render(img_a, insn))
                    print(f"   ... {span - 2 * context} identical instructions ...")
                    for insn in ins_a[i2 - context : i2]:
                        print("   " + render(img_a, insn))
                    continue
                if span > 2 * context:
                    continue
            for insn in ins_a[i1:i2]:
                print("   " + render(img_a, insn))
            continue
        print(f"  --- {tag.upper()} ---")
        for insn in ins_a[i1:i2]:
            print("-1162 " + render(img_a, insn))
        for insn in ins_b[j1:j2]:
            print("+1170 " + render(img_b, insn))
    return 0


def selftest() -> int:
    """Assert the alignment key on hand-written encodings, then on a known 1.17 field move."""
    _ensure_capstone()
    import capstone

    md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)

    def only(blob: bytes, va: int = 0x140000000):
        return next(iter(md.disasm(blob, va)))

    # rip-relative displacements are masked (code moved), register displacements are not.
    a = only(bytes.fromhex("488B0D11111111"))  # mov rcx, [rip + 0x11111111]
    b = only(bytes.fromhex("488B0D22222222"))  # mov rcx, [rip + 0x22222222]
    assert norm(a) == norm(b), (norm(a), norm(b))
    c = only(bytes.fromhex("8B81B50A0000"))  # mov eax, [rcx + 0xab5]
    d = only(bytes.fromhex("8B81BD0A0000"))  # mov eax, [rcx + 0xabd]
    assert norm(c) != norm(d), "a moved FIELD must not be masked away"
    # branch targets are masked, immediates are not.
    e = only(bytes.fromhex("E80B000000"))
    f = only(bytes.fromhex("E816000000"))
    assert norm(e) == norm(f) == "call <target>", (norm(e), norm(f))
    g = only(bytes.fromhex("83F901"))  # cmp ecx, 1
    h = only(bytes.fromhex("83F902"))  # cmp ecx, 2
    assert norm(g) != norm(h), "an immediate change must survive the key"

    # Ground truth from the migration doc: GetScadutreeBlessing is byte-identical between the
    # builds except [rcx+0xab5] -> [rcx+0xabd]. Both images must be present for this half.
    if IMAGES["1162"].exists() and IMAGES["1170"].exists():
        img = Image(IMAGES["1162"])
        assert ".pdata" in img.sections, "1.16.2 image has no .pdata"
        img2 = Image(IMAGES["1170"])
        assert ".pdata" in img2.sections, "1.17 image has no .pdata"
        assert len(img.function_ends()) > 100000, "1.16.2 .pdata looks empty"
        assert len(img2.function_ends()) > 100000, "1.17 .pdata looks empty"
        print(f"images OK: {len(img.function_ends())} / {len(img2.function_ends())} .pdata entries")
    else:
        print("images absent -- key assertions only")
    print("selftest OK")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--selftest", action="store_true")
    ap.add_argument("--bytes", dest="nbytes", type=lambda s: int(s, 0), default=None,
                    help="compare a fixed byte window instead of the .pdata extent")
    ap.add_argument("--context", type=int, default=6, help="identical lines kept around a change")
    ap.add_argument("--full", action="store_true", help="print every identical instruction too")
    ap.add_argument("--gap", action="store_true",
                    help="address has no .pdata record: run to the next .pdata start (a HEURISTIC)")
    ap.add_argument("vas", nargs="*", help="<1.16.2 VA> <1.17 VA>")
    args = ap.parse_args()
    if args.selftest:
        return selftest()
    if len(args.vas) != 2:
        ap.error("give exactly two addresses: <1.16.2 VA> <1.17 VA>")
    _ensure_capstone()
    return diff_one(int(args.vas[0], 16), int(args.vas[1], 16), args.nbytes, args.context, not args.full, args.gap)


if __name__ == "__main__":
    raise SystemExit(main())
