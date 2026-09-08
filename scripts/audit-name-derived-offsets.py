#!/usr/bin/env python3
"""Census: every hand-written field-offset constant, with the provenance its comment claims.

The class this looks for
------------------------
`CS_SYSTEM_STEP_CURRENT_STATE_OFFSET` was 0x40 for its whole life and the field is at 0x48. It
was never measured -- it was back-solved from a member name in the sibling `fromsoftware-rs`
crate (`unk48` sits after `requested_state`, so "current_state must be 0x40"). That member is
itself misnamed. A wrong-but-readable offset returns a legal value of the right width forever:
no fault, no refusal, and no 1.16.2-vs-1.17 drift for a drift check to see, because it is
equally wrong in both builds.

So this census does not ask "is the value right" -- it cannot know that. It asks where the value
came from, by reading the comment block above each definition and bucketing it:

  Measured   -- cites an address, an instruction, a witness function, an alignment result
  name       -- cites a struct member, an `unkNN`, `offset_of!`, a sibling-crate declaration
  none       -- no provenance at all (weaker than wrong provenance, not stronger)

Run it to re-derive the sweep; it is a report, not a gate. The gate that owns the findings is
`scripts/check-object-field-offsets-1170.py`.

Byte coverage, the second question
----------------------------------
`pair-object-field-drift.py` reports the displacement of each memory operand, which is the right
unit for "did the field move". It is the wrong unit for "is this byte a field at all": a
`mov word [rbx+0xbc8],0` initialises 0xbc8 and 0xbc9, and only 0xbc8 appears in the displacement
set. Absence from that set is therefore not absence of a field -- unless nothing wide enough
covers the byte either. `--cover` answers that: for one function extent and one list of byte
offsets, every access whose `[disp, disp+size)` interval contains the byte.

This is the distinction the `CS_SYSTEM_STEP_CURRENT_STATE_OFFSET` finding turned on. 0x40 was not
merely absent from the displacement set; Nothing in the step-template constructor covered it,
because the slot belongs to a different constructor entirely.

Which rows the number is about, the third question
-------------------------------------------------
Provenance is only half of an actionable number. The first census reported `NONE=654`, and that
population contained `DLL_PROCESS_ATTACH = 1` (the name regex matched the `_AT` inside `ATTACH`),
the whole Windows x64 `CONTEXT` register file, PE header offsets, SPIR-V enum values, and offsets
into buffers this workspace itself defines. None of those can be wrong in the way
`CS_SYSTEM_STEP_CURRENT_STATE_OFFSET` was wrong -- an unmeasured displacement onto a live game
object, returning a legal value of the right width forever.

So each row also gets a kind, and the headline number counts only `GAME`:

  Game          an ELDEN RING object-field offset; the population this census exists to size
  OS-ABI        Windows / MSVC-ABI / PE-COFF layout -- not FromSoftware's, cannot drift with a
                game patch, and verified against the published structure in the kinds table
  machine-code  a byte offset inside an x86-64 instruction encoding
  our-own       an offset into a structure this workspace defines
  format        an offset into a file this workspace parses
  not-an-offset an enum value, a count, a limit -- the name regex over-matched

`GAME` is the DEFAULT: a row is only demoted by the name-shape rule (below) or by an explicit
line in `scripts/offset-census-kinds.tsv` that says why, which is a line a reviewer can argue
with. Widening the regex instead would shrink the number without making it truer and leave no
trace of which rows went.

And one more provenance source
------------------------------
`scripts/check-object-field-offsets-1170.py::PINNED_CONSTANTS` is a table of constants that gate
has already measured against both images and re-measures every run. A bare comment above such a
constant is not missing provenance -- the provenance lives in the gate. Those rows are imported
(not copied) and bucketed `PINNED`.

Usage
    python3 scripts/audit-name-derived-offsets.py [--bucket name|none|measured|pinned]
    python3 scripts/audit-name-derived-offsets.py --kind game     # just the counted population
    python3 scripts/audit-name-derived-offsets.py --show-excluded # what the shape rule dropped
    python3 scripts/audit-name-derived-offsets.py --selftest
    python3 scripts/audit-name-derived-offsets.py --cover 0x140675ea0:6895:0x140676cf0:6895 \
        --base rbx --base rcx --byte 0xbc8 --byte 0xbc9

`--all` is accepted and ignored. It used to mean "also scan the file-format crates"; those rows
are now always scanned and bucketed `FORMAT`, because a row that is silently not scanned is a row
nobody can audit.
"""

from __future__ import annotations

import argparse
import os
import re
import sys
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import function_extent  # noqa: E402 - repo-local, and the sys.path line above is what makes it work

REPO = Path(__file__).resolve().parent.parent
EXCLUDED_DIRS = (".git", "target", "node_modules", ".worktrees", ".claude")

# Crates whose "offsets" are file-format offsets, checked by parsing real files, not by reading
# live game memory. A wrong value there fails a parse; it does not silently return a neighbour.
FILE_FORMAT_CRATES = (
    "crates/er-save-loader/",
    "crates/soulsformats/",
    "crates/er-gfx/",
    "crates/er-build-export/",
    "crates/er-build-import-core/",
    "crates/er-profile-summary-core/",
    "crates/er-param",
    "tools/",
)

KINDS_TSV = Path(__file__).resolve().parent / "offset-census-kinds.tsv"
GATE = Path(__file__).resolve().parent / "check-object-field-offsets-1170.py"

# Name shapes that are really offsets. `_AT` and `OFF` as SUBSTRINGS are what dragged
# `DLL_PROCESS_ATTACH`, `NO_PATCH_ATTEMPTS`, `SESSION_STATE_OFFER_RECEIVED` and `MAX_BACK_OFF_SHIFT`
# into the population, so the test is on underscore-delimited TOKENS:
#
#   * a token that is exactly offset / OFFSETS / OFS, anywhere in the name; or
#   * a last token of off / at / OFS / offset -- trailing, so `SAY_AT_MOST` is not an offset; or
#   * a token field followed by a numeric token, which is how a nameless field is spelled here
#     (`..._RENDER_READY_FIELD_754`), while `TEXT_FIELD_CHARACTER_ID` is not.
OFFSET_TOKENS = ("OFFSET", "OFFSETS", "OFS")
OFFSET_TAIL_TOKENS = ("OFF", "AT", "OFS", "OFFSET", "OFFSETS")
NUMERIC_TOKEN = re.compile(r"^(?:0X)?[0-9A-F]+$")


def offset_shaped(name: str) -> bool:
    """Whether the name claims to be a byte offset. See `OFFSET_TOKENS` for why tokens, not text."""
    tokens = name.split("_")
    if any(t in OFFSET_TOKENS for t in tokens):
        return True
    if tokens[-1] in OFFSET_TAIL_TOKENS:
        return True
    return any(
        t == "FIELD" and i + 1 < len(tokens) and NUMERIC_TOKEN.match(tokens[i + 1])
        for i, t in enumerate(tokens)
    )


def load_kinds():
    """`{name: (kind, reason)}` from the reviewable overrides table."""
    kinds = {}
    for raw in KINDS_TSV.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        parts = raw.split("\t")
        if len(parts) != 3:
            raise SystemExit(f"{KINDS_TSV.name}: not three tab-separated fields: {raw!r}")
        name, kind, reason = (p.strip() for p in parts)
        if kind not in KIND_ORDER:
            raise SystemExit(f"{KINDS_TSV.name}: unknown kind {kind!r} for {name}")
        kinds[name] = (kind, reason)
    return kinds


def load_pinned():
    """The gate's measured-and-frozen constant names, imported so there is one copy of the list."""
    import importlib.util

    spec = importlib.util.spec_from_file_location("_offsets_gate", GATE)
    module = importlib.util.module_from_spec(spec)
    sys.modules["_offsets_gate"] = module
    spec.loader.exec_module(module)
    return {row[0] for row in module.PINNED_CONSTANTS}


KIND_ORDER = ("GAME", "PINNED", "OS-ABI", "MACHINE-CODE", "OUR-OWN", "FORMAT", "NOT-AN-OFFSET")


DEF = re.compile(
    r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:const|static)\s+"
    r"([A-Z][A-Z0-9_]*(?:OFFSET|OFF|OFS|FIELD|_AT)[A-Z0-9_]*)\s*:\s*[A-Za-z0-9_:<>]+\s*=\s*"
    r"(0x[0-9a-fA-F]+|[0-9]+)\s*;"
)

MEASURED_TELL = re.compile(
    r"0x1[0-9a-fA-F]{8}"            # a code/data VA
    r"|FUN_1[0-9a-fA-F]{8}"         # a Ghidra function label
    r"|\bdump\b|\bdeobf\b|\bdecompile"
    r"|\bmov\b|\blea\b|\bcmp\b|\bcall\b|\btest\b|\bjs\b"
    r"|aligned|alignment|witness|ctor|constructor|decod|disassembl|instruction"
    r"|byte-proven|byte-identical|measured|pair-object-field-drift|RTTI|vftable|vtable"
    r"|Ghidra|xref|reads it at|writes it at|store at",
    re.I,
)
NAME_TELL = re.compile(
    r"unk[0-9a-fA-F]{2,3}\b|pad[0-9a-fA-F]{2,3}\b"
    r"|fromsoftware-rs|offset_of!|field nam|struct declar|matches the layout"
    r"|layout in\b|\bdeclares\b|as declared|binding|sibling crate",
    re.I,
)


def rust_files():
    for p in sorted(REPO.rglob("*.rs")):
        rel = p.relative_to(REPO).as_posix()
        # Relative parts, not absolute ones. `REPO` is this script's own checkout, and a
        # `git worktree` checkout lives at `<repo>/.worktrees/<name>` -- so every absolute path
        # under it contains `.worktrees` and the old test excluded the entire corpus. The audit
        # then found zero constants and `--selftest` failed with "kinds table names constants
        # that no longer exist", listing all 200-odd of them: a gate that is vacuous in exactly
        # the place a closure gets verified, reporting it as a mass deletion.
        if any(part in EXCLUDED_DIRS for part in Path(rel).parts):
            continue
        yield rel, p


def comment_block(lines, idx):
    """The contiguous comment/attribute lines immediately above `idx` (0-based)."""
    out = []
    i = idx - 1
    blanks = 0
    while i >= 0:
        s = lines[i].strip()
        if s.startswith("//") or s.startswith("#["):
            out.append(s)
            blanks = 0
            i -= 1
            continue
        # A doc block broken by one blank line is still the same block. Two ends it.
        if not s and blanks == 0 and out:
            blanks = 1
            i -= 1
            continue
        break
    return list(reversed(out))


def classify(block, same_line_comment):
    text = " ".join(block) + " " + same_line_comment
    if not text.strip():
        return "NONE", ""
    if MEASURED_TELL.search(text):
        return "MEASURED", text.strip()[:200]
    if NAME_TELL.search(text):
        return "NAME", text.strip()[:200]
    return "NONE", text.strip()[:200]


IMAGE_BASE = 0x140000000
IMAGE_1162 = REPO / "eldenring-deobf.bin"
IMAGE_1170 = REPO / "eldenring-deobf-1.17.bin"


def _capstone():
    import os

    try:
        import capstone  # noqa: F401
    except ImportError:
        os.execvp("uv", ["uv", "run", "--with", "capstone", "python3"] + sys.argv)
    import capstone

    md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
    md.detail = True
    return capstone, md


# Registers that are never `this` and never a useful alias root. `rip` is position-independent
# addressing, `rsp`/`rbp` are the frame -- a displacement off either is a local, not a field.
NON_OBJECT_BASES = ("rip", "rsp", "rbp", "esp", "ebp")


def _reg_roots(capstone):
    """`{sub-register name: 64-bit root name}` for the general-purpose file.

    Writing `ebx` destroys `rbx`, so alias tracking that compares register names has to fold
    `ebx`/`bx`/`bl` onto `rbx` before it can decide whether an alias survived an instruction. The
    families are spelled out here rather than guessed from the name, but every member is checked
    against capstone's own `X86_REG_*` table -- a typo would otherwise create a register that
    simply never matches, which fails silently in the safe-looking direction (aliases that are
    never invalidated).
    """
    known = {n[len("X86_REG_") :].lower() for n in dir(capstone.x86) if n.startswith("X86_REG_")}
    families = {}
    for r in ("ax", "bx", "cx", "dx"):
        families["r" + r] = ("r" + r, "e" + r, r, r[0] + "l", r[0] + "h")
    for r in ("si", "di", "bp", "sp"):
        families["r" + r] = ("r" + r, "e" + r, r, r + "l")
    for n in range(8, 16):
        families[f"r{n}"] = (f"r{n}", f"r{n}d", f"r{n}w", f"r{n}b")
    roots = {}
    for root, members in families.items():
        for m in members:
            if m not in known:
                raise SystemExit(
                    f"_reg_roots: {m!r} is not in capstone's X86_REG_ table -- the family table "
                    "is wrong, and a register that never matches would leave aliases alive"
                )
            roots[m] = root
    return roots


def covering_accesses(capstone, md, blob, va, end, bases, bytes_wanted):
    """Every access whose `[disp, disp + operand_size)` interval contains each wanted byte.

    The blind spot this follows a `lea` to close
    --------------------------------------------
    A `this`-relative displacement census sees only what is written through `this`. Where the
    compiler hands a whole embedded sub-object to a register in one `lea` and writes the interior
    through that register, every interior field is invisible:

        lea   rbx, [rsi+0xb98]     ; the DLDateTime at GameMan+0xb98
        mov   qword [rbx], r14     ; -> 0xb98   -- visible, disp is off `this`
        and   qword [rbx+8], r12   ; -> 0xba0   -- Invisible, disp is off rbx

    0xba0 was named `LOAD_HANDLE` from the shape of whatever value sat there, and 0xdf0 was named
    a "resident device" pointer the same way, because neither ever appeared in a displacement set
    for anything to contradict. Struct-in-struct layouts are everywhere in this codebase, so the
    silence is systematic rather than incidental.

    So `alias` maps a 64-bit register root to `(root base, displacement from it)`, established by
    a `lea` off a tracked base (or off another alias, which chains), and destroyed the moment
    anything else writes that register. An access through an alias is reported at its effective
    displacement, with the `lea` that established it printed alongside so the attribution can be
    audited rather than taken on trust.

    `end` is an absolute file offset from `function_extent`, not a byte budget: past the
    function's last byte this decode would be reading the de-Arxan'd images' leftover bytes and
    inventing accesses out of them.
    """
    roots = _reg_roots(capstone)
    rva = va - IMAGE_BASE
    hits = {b: [] for b in bytes_wanted}
    # {root register: (root base name, displacement from it, text of the establishing lea)}
    alias = {}

    for insn in md.disasm(blob[rva:end], va):
        is_lea = insn.mnemonic == "lea"
        text = f"{insn.mnemonic} {insn.op_str}"

        # (a) record, before anything this instruction writes can invalidate the map.
        #     `mov rbx,[rbx+8]` both reads through an alias and destroys it, which is why
        #     recording has to happen before invalidation.
        #
        #     A `lea` is recorded too, but tagged `address-of`. It is not a read or a write of
        #     that memory, so it must not be read as one -- but it is still a witness that a
        #     field is there, and usually the strongest one available for an embedded object:
        #     `lea rcx,[rbx+0x2b0]` is the PlayerGameData constructor taking the address of the
        #     `equipment` sub-object to construct it in place, which is exactly what the gate's
        #     `equipment` row at 0x2b0 rests on. Dropping `lea` entirely (the first version of
        #     this change) silently took the only covering access away from PGD 0x2b0 and 0x960
        #     and reported a coverage drop as if it were a correction.
        #
        #     A `lea`'s operand size is meaningless -- capstone reports the addressed type, not a
        #     transfer width -- so an address-of witness covers exactly its own byte.
        for op in insn.operands:
            if op.type != capstone.x86.X86_OP_MEM or op.mem.base == 0:
                continue
            name = insn.reg_name(op.mem.base)
            root = roots.get(name, name)
            chain = None
            if (not bases or root in bases or name in bases) and name not in NON_OBJECT_BASES:
                disp = op.mem.disp
            elif root in alias:
                base_name, base_disp, lea_text = alias[root]
                disp = base_disp + op.mem.disp
                chain = (base_name, base_disp, op.mem.disp, lea_text)
            else:
                continue
            width = 1 if is_lea else op.size
            for b in bytes_wanted:
                if disp <= b < disp + width:
                    hits[b].append((insn.address, text, disp, width, chain, is_lea))

        # (b/c) invalidate every register this instruction writes, then re-establish the alias a
        #       `lea` creates. The directive's order is (b) then (c); doing it the other way round
        #       is the same rule -- a `lea`'s destination is written, so (c) would otherwise undo
        #       (b) on the very instruction that establishes the alias.
        try:
            _read, written = insn.regs_access()
        except capstone.CsError:  # pragma: no cover - detail is on, but do not decide on a guess
            alias.clear()
            continue
        for reg in written:
            root = roots.get(insn.reg_name(reg))
            if root is not None:
                alias.pop(root, None)

        if not is_lea or len(insn.operands) != 2:
            continue
        dest, memop = insn.operands
        if dest.type != capstone.x86.X86_OP_REG or memop.type != capstone.x86.X86_OP_MEM:
            continue
        dest_root = roots.get(insn.reg_name(dest.reg))
        # An indexed lea (`lea rbx,[rsi+rax*4]`) has no single constant displacement from the
        # base, so it establishes nothing rather than a wrong something.
        if dest_root is None or memop.mem.base == 0 or memop.mem.index != 0:
            continue
        base_name = insn.reg_name(memop.mem.base)
        base_root = roots.get(base_name, base_name)
        if (not bases or base_root in bases or base_name in bases) and (
            base_name not in NON_OBJECT_BASES
        ):
            alias[dest_root] = (base_name, memop.mem.disp, text)
        elif base_root in alias:
            parent_base, parent_disp, _parent_text = alias[base_root]
            alias[dest_root] = (parent_base, parent_disp + memop.mem.disp, text)

    return hits


def cover_one(capstone, md, blob, va, length, bases, bytes_wanted):
    """`covering_accesses` over one function, bounded by its extent rather than by `length`.

    `length` is only a cap on top of the extent. `body_slice_end` returning None is a refusal, not
    an invitation to substitute the byte count -- an unknown extent is exactly the case where a
    forward decode invents instructions.
    """
    end = function_extent.body_slice_end(blob, va, cap=length)
    if end is None:
        raise SystemExit(
            f"{va:#x}: function_extent cannot resolve the extent, so there is no trustworthy "
            "window to decode. Refusing rather than falling back to the byte count."
        )
    return covering_accesses(capstone, md, blob, va, end, bases, bytes_wanted), end


def run_cover(spec, bases, bytes_wanted):
    capstone, md = _capstone()
    va16, len16, va17, len17 = spec
    for label, path, va, length in (
        ("1.16.2", IMAGE_1162, va16, len16),
        ("1.17  ", IMAGE_1170, va17, len17),
    ):
        blob = path.read_bytes()
        hits, end = cover_one(capstone, md, blob, va, length, bases, bytes_wanted)
        print(f"--- {label}  {va:#x} +{end - (va - IMAGE_BASE)}")
        for b in bytes_wanted:
            if not hits[b]:
                print(f"    {b:#x}: NO COVERING ACCESS")
                continue
            for addr, text, disp, size, chain, is_addr_of in hits[b]:
                what = "address-of" if is_addr_of else f"size={size}"
                if chain is None:
                    print(f"    {b:#x}: {addr:#x}  {text}   [disp={disp:#x} {what}]")
                else:
                    base_name, base_disp, own_disp, lea_text = chain
                    print(
                        f"    {b:#x}: {addr:#x}  {text}   [via {lea_text}  "
                        f"disp={base_disp:#x}+{own_disp:#x}={disp:#x} {what}]"
                    )
    return 0


def collect():
    """Every offset-shaped definition in the tree, with its provenance bucket and its kind.

    Precedence, strongest provenance first: a comment that cites a measurement, then the gate's
    pin table, then the kind overrides, then the name-shape rule. A row that survives all of that
    is an unprovenanced game-object offset, which is the number this tool exists to report.
    """
    overrides = load_kinds()
    pinned = load_pinned()

    rows = []
    for rel, path in rust_files():
        lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
        for i, line in enumerate(lines):
            m = DEF.match(line)
            if not m:
                continue
            name, value = m.group(1), m.group(2)
            tail = line.split("//", 1)[1] if "//" in line.split("=", 1)[-1] else ""
            bucket, why = classify(comment_block(lines, i), tail)

            kind, reason = overrides.get(name, (None, ""))
            if kind is None:
                if any(rel.startswith(c) for c in FILE_FORMAT_CRATES):
                    kind, reason = "FORMAT", "defined in a file-format crate"
                elif not offset_shaped(name):
                    kind, reason = "NOT-AN-OFFSET", "the name does not claim to be a byte offset"
                else:
                    kind, reason = "GAME", ""

            if bucket != "MEASURED" and kind == "GAME" and name in pinned:
                bucket = "PINNED"
                why = "measured by scripts/check-object-field-offsets-1170.py::PINNED_CONSTANTS"
            rows.append((bucket, kind, rel, i + 1, name, value, why))
    return rows, overrides


# The known-answer test for `lea`-following, kept as a pair of real offsets in the GameMan
# constructor rather than a synthetic byte string, because the class this closes is about what a
# real compiler does with a real embedded sub-object.
#
#   0xba0  the upper half of the DLDateTime at 0xb98, written `and %r12,0x8(%rbx)` after
#          `lea 0xb98(%rsi),%rbx`. It was called a LOAD_HANDLE until it was measured.
#   0xdf0  the length of the DLString inside the FD4FilePathBase at 0xdd0, three `lea`s deep
#          (rsi -> 0xdd0 -> 0xdd8 -> 0xde0), so it also proves the alias chains. It was called a
#          "resident device" pointer until it was measured.
#
# Both must be invisible to a `this`-relative reading and visible once the `lea` is followed; the
# first half is what makes this a regression test rather than a tautology.
LEA_KNOWN_ANSWER = (0xBA0, 0xDF0)


def selftest_lea_following(out=sys.stdout):
    """Prove the alias walk finds the two fields a `this`-relative census cannot see.

    The constructor's VA and extent come from `check-object-field-offsets-1170.py::GAME_MAN_CTOR`,
    imported rather than retyped: two copies of an address are two things to keep in step, and
    this file already imports that module for `PINNED_CONSTANTS`.
    """
    import importlib.util

    spec = importlib.util.spec_from_file_location("_offsets_gate_ctors", GATE)
    module = importlib.util.module_from_spec(spec)
    sys.modules["_offsets_gate_ctors"] = module
    spec.loader.exec_module(module)
    ctor = module.GAME_MAN_CTOR

    missing = [p.name for p in (IMAGE_1162, IMAGE_1170) if not p.exists()]
    if missing:
        print(f"skip: lea-following known-answer test needs {', '.join(missing)}", file=out)
        return 0

    capstone, md = _capstone()
    failures = []
    for label, path, va, length in (
        ("1.16.2", IMAGE_1162, ctor["va16"], ctor["len16"]),
        ("1.17", IMAGE_1170, ctor["va17"], ctor["len17"]),
    ):
        blob = path.read_bytes()
        end = function_extent.body_slice_end(blob, va, cap=length)
        if end is None:
            failures.append(f"{label}: no resolvable extent for the GameMan ctor at {va:#x}")
            continue
        bases = tuple(ctor["bases"])
        followed = covering_accesses(capstone, md, blob, va, end, bases, LEA_KNOWN_ANSWER)
        for want in LEA_KNOWN_ANSWER:
            direct = [h for h in followed[want] if h[4] is None]
            if direct:
                failures.append(
                    f"{label}: {want:#x} is reported as a DIRECT `this`-relative access "
                    f"({direct[0][1]}) -- the test no longer proves the blind spot exists"
                )
            if not followed[want]:
                failures.append(
                    f"{label}: {want:#x} has NO covering access even with `lea`-following"
                )
        if not any(
            h[1].startswith("and qword ptr [rbx + 8]") and not h[5] for h in followed[0xBA0]
        ):
            failures.append(
                f"{label}: 0xba0 is covered, but not by the expected "
                "`and qword ptr [rbx + 8], r12` off `lea rbx,[rsi+0xb98]`"
            )

    if failures:
        print("FAIL: lea-following known-answer test:", file=out)
        for f in failures:
            print(f"    {f}", file=out)
        return 1
    print(
        f"ok: lea-following exposes {', '.join(hex(b) for b in LEA_KNOWN_ANSWER)} in both images, "
        "neither of which any `this`-relative access covers",
        file=out,
    )
    return 0


def selftest(rows, overrides):
    """The table must describe rows that exist, and the shape rule must reject what it claims."""
    defined = {r[4] for r in rows}
    ghosts = sorted(set(overrides) - defined)
    if ghosts:
        print("FAIL: kinds table names constants that no longer exist:")
        for g in ghosts:
            print(f"    {g}")
        return 1

    # Values the OS-ABI reasons assert, so an excused row is a verified row rather than a shrug.
    documented = {}
    for n, (k, t) in overrides.items():
        if k != "OS-ABI" or "=" not in t:
            continue
        tail = re.match(r"\s*(0x[0-9a-fA-F]+|\d+)\b", t.rsplit("=", 1)[1])
        if tail:
            documented[n] = int(tail.group(1), 0)
    bad = []
    for _, kind, rel, ln, name, value, _why in rows:
        if name in documented and int(value, 0) != documented[name]:
            bad.append(f"    {rel}:{ln} {name} = {value}, published layout says "
                       f"{documented[name]:#x}")
    if bad:
        print("FAIL: a constant disagrees with the published structure its kinds row cites:")
        print("\n".join(bad))
        return 1

    # The four families that dragged non-offsets into the first census. If the shape rule ever
    # starts accepting these again the headline number silently re-inflates.
    for name in ("DLL_PROCESS_ATTACH", "NO_PATCH_ATTEMPTS", "FIRST_PATCH_ATTEMPT",
                 "SESSION_STATE_OFFER_RECEIVED", "MAX_BACK_OFF_SHIFT", "SAY_AT_MOST",
                 "TEXT_FIELD_CHARACTER_ID", "REFRESH_MAX_ATTEMPTS"):
        if offset_shaped(name):
            print(f"FAIL: the name-shape rule accepts {name}, which is not an offset")
            return 1
    # ...and the two shapes it must not reject.
    for name in ("TITLE_CUSTOM_COVER_PROFILE_RENDER_READY_FIELD_754", "CTX_RIP_OFF",
                 "GAME_MAN_FIELD_B73_OFFSET", "PGD_AT"):
        if not offset_shaped(name):
            print(f"FAIL: the name-shape rule rejects {name}, which is an offset")
            return 1

    print(f"ok: {len(overrides)} kinds rows all resolve, "
          f"{len(documented)} OS-ABI values match their published layout, shape rule holds")
    return selftest_lea_following()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--all", action="store_true", help="accepted and ignored (see the docstring)")
    ap.add_argument("--bucket", action="append", default=None)
    ap.add_argument("--kind", action="append", default=None)
    ap.add_argument("--show-excluded", action="store_true",
                    help="list the rows the name-shape rule dropped, so none is invisible")
    ap.add_argument("--selftest", action="store_true")
    ap.add_argument("--cover", help="VA162:LEN:VA170:LEN -- report byte coverage, not provenance")
    ap.add_argument("--base", action="append", default=[])
    ap.add_argument("--byte", action="append", default=[])
    args = ap.parse_args()

    if args.cover:
        spec = [int(x, 0) for x in args.cover.split(":")]
        return run_cover(spec, tuple(args.base), [int(b, 0) for b in args.byte])

    rows, overrides = collect()
    if args.selftest:
        return selftest(rows, overrides)

    if args.show_excluded:
        for bucket, kind, rel, ln, name, value, _why in sorted(rows, key=lambda r: (r[2], r[3])):
            if kind == "NOT-AN-OFFSET":
                print(f"{rel}:{ln} {name} = {value}")
        return 0

    want_bucket = set(args.bucket) if args.bucket else None
    want_kind = set(args.kind) if args.kind else None
    buckets, kinds, game_unprovenanced = {}, {}, 0
    for bucket, kind, rel, ln, name, value, why in sorted(rows):
        buckets[bucket] = buckets.get(bucket, 0) + 1
        kinds[kind] = kinds.get(kind, 0) + 1
        if kind == "GAME" and bucket == "NONE":
            game_unprovenanced += 1
        if want_bucket and bucket not in want_bucket:
            continue
        if want_kind and kind not in want_kind:
            continue
        print(f"{bucket:8s} {kind:13s} {rel}:{ln} {name} = {value}")
        if bucket not in ("MEASURED", "PINNED") and why:
            print(f"         | {why}")

    print(f"\nTOTAL {len(rows)}   " + "  ".join(f"{k}={v}" for k, v in sorted(buckets.items())))
    print("KIND  " + "  ".join(f"{k}={kinds.get(k, 0)}" for k in KIND_ORDER if k != "PINNED"))
    print(f"\nUNPROVENANCED GAME-OBJECT OFFSETS: {game_unprovenanced}")
    print("  (kind=GAME and no provenance -- not in a comment, not in the gate's pin table)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
