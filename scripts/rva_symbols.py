#!/usr/bin/env python3
"""Which symbol declares this address? Every spelling this tree uses, or an honest "I cannot tell".

Why this exists
---------------
Two gates were caught on 2026-08-30 giving a confidently wrong answer to the same question, for
the same reason.

  `check-stale-rva-calls.py` searched for a SCREAMING_SNAKE constant added to a module base. It
  reported `1 known ungated site(s), none new` while roughly twenty real sites stood, because
  `er-title-flow` spells most of its addresses as enum variants
  (`ProfileLoadMenuRva::ProfileLoadSelectSaveSlot as usize`) and other crates reach one through its
  module (`jp::GAME_MAN_GLOBAL_RVA`).

  `check-1170-translation-collisions.py` searched for the literal `const NAME: usize = 0x<addr>;`
  and, finding none, printed "row B is claimed by no feature: deleting it removes this collision at
  zero cost." For 0xb0d400 that advice was wrong and destructive. The declaration is
  `MenuJobWait = 0x00b0d400` inside `#[repr(u32)] pub enum MenuTraceRva`, reached through
  `pub const TITLE_MENU_JOB_WAIT_RVA: usize = MenuTraceRva::MenuJobWait as usize;`. The hex literal
  is in the tree; the shape `const NAME: usize = 0x..;` is not. Its three live use sites are on the
  autoload path (`native_title_job.rs`, and twice in `title_load_step_hooks.rs`; a fourth mention is
  the string label passed beside one of them). Following the gate's own advice would have deleted a
  working feature's address.

Both failures are one failure: Searching for a spelling finds SPELLINGS, not addresses. So this
module resolves values. It reads every declaration form in `crates/`, evaluates each to a number,
and answers "which symbols equal this address" from the resolved values rather than from a regex
over the source text.

The part that matters more than the matching
--------------------------------------------
"I found no reference" and "there is no reference" are different facts and must not print the same
sentence. A resolver has a residue -- declarations whose right-hand side it could not evaluate --
and while that residue is non-empty, any one of them could be the address. So `claims()` returns
`proven_unclaimed` as a separate field from `found_nothing`, and it is True only when

  * the walk read files (a scan that read nothing must never look like a clean scan);
  * No declaration in the integer-valued universe resolved to the address;
  * No bare hex literal of the address occurs in code (a `rva: 0xaec480` table field claims an
    address with no constant name at all -- er-reload-trace shipped exactly that); and
  * the residue that could hold this address is empty.

A caller may only advise deletion when `proven_unclaimed` is True. Anything else is
"not proven", and the difference is a deleted feature.

The universe, stated so the proof means something
-------------------------------------------------
An address constant in Rust is an integer-typed `const`/`static`, an integer-repr enum's
discriminant, an element of an integer array/slice/tuple, or an integer `Range` band. Three classes
are then subtracted, each for a reason about what the thing is rather than what it is called --
which is the discipline the failures above violated:

  * a non-integer type. A `&str` cannot hold an address, so an unevaluated one does not weaken the
    proof;
  * a type too narrow for the address being asked about. A `u8` cannot be 0x7ad710, so an
    unevaluated `&[u8; 64]` byte string is not a declaration that might be it. This is
    query-relative: the residue is smaller for a large address than for a small one;
  * a value composed only of Rust type layout -- `offset_of!`, `size_of`, `align_of` and integer
    literals, recursively. Those are lengths and intra-struct offsets, not addresses in
    eldenring.exe's image. A constant that adds a layout quantity to something else is not in this
    class and stays in the residue.

Measured on this tree, 2026-08-30: 5124 address-capable declarations, 285 unevaluated, 219 of them
layout-derived, 66 genuinely unread, and 4 wide enough to hold a `.text` RVA. Those four are what
stands between the three baselined collisions and a proof that nothing claims them.

    python3 scripts/rva_symbols.py 0xb0d400        # who declares this address
    python3 scripts/rva_symbols.py 0x140b0d400     # VA or RVA, both accepted
    python3 scripts/rva_symbols.py --residue       # what the resolver could not evaluate
    python3 scripts/rva_symbols.py --residue 0x7ad710   # ...only the ones wide enough to be it
    python3 scripts/rva_symbols.py --selftest
"""

import argparse
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CRATES = os.path.join(ROOT, "crates")
IMAGE_BASE = 0x140000000

# The sibling `fromsoftware-rs` checkout this repo's Cargo.toml path-deps actually point at
# (`../../../fromsoftware-rs/crates/eldenring`, `.../shared`, verified against
# `crates/er-death-persist/Cargo.toml` and `crates/er-loading-portrait-core/Cargo.toml`). Read
# only for struct/enum/bitfield layout and macro-sourced associated-const values -- never as a
# source of address declarations. Files here are never added to `files_read`, `decls`, `text` or
# the address universe: this repo's own `crates/` tree is the only thing "who claims this
# address" is ever asked about, and widening that scope would make the universe/residue counts
# describe a tree nobody asked about. Scoped to `eldenring` + `shared` because those are the only
# two fromsoftware-rs crates any `crates/*/Cargo.toml` path-depends on (checked 2026-08-30); a repo
# missing the sibling checkout (or a future removal of these deps) degrades gracefully -- every
# reader below is a no-op over an empty/absent tree, not a crash.
FROMSOFT_TYPE_ROOTS = [
    os.path.normpath(os.path.join(ROOT, "..", "fromsoftware-rs", "crates", "eldenring")),
    os.path.normpath(os.path.join(ROOT, "..", "fromsoftware-rs", "crates", "shared")),
]


# --------------------------------------------------------------------------------------------
# Source text
# --------------------------------------------------------------------------------------------


def code_only(text):
    """`text` with every Rust comment and string body blanked to spaces, offsets preserved.

    Moved here from `check-stale-rva-calls.py` on 2026-08-30 so both gates share one dialect
    instead of growing a third. That gate's baseline was contaminated by not doing this at all:
    two of its three rows were prose -- a `//` paragraph in `er-game-base/src/game_build.rs`
    explaining that "a stale address is equally reachable as a CALL (`transmute(base + RVA)`)",
    and a `///` doc comment in `er-invasion-warp-core/src/lib.rs` saying every call "used to be a
    bare `transmute(base + SOME_RVA)`". Both were recorded as findings. That is worse than a plain
    false positive: a ratchet whose baseline holds non-findings stays green while real sites are
    added beside them, and the next agent to shrink the baseline "fixes" a sentence.

    It matters in the other direction here. All three collision addresses this module was written
    for appear in `crates/` only inside doc comments that describe the collision. Counting those
    as claims would make every address look live and the gate's advice useless; failing to blank
    them would be the mirror image of the bug it exists to fix.

    String bodies are blanked for the same reason comments are, and it is the same failure one
    step along: `"// transmute(base + QUOTED_RVA)"` is a quoted example, not a call. Nothing
    either gate looks for can legitimately live inside a string literal, so blanking the body
    cannot hide a finding, while leaving it readable manufactures one.

    The two are parsed together rather than in separate passes, because each can quote the other:
    a `//` inside a string does not open a comment, and a `"` inside a comment does not open a
    string. Getting that backwards would blank live code. Block comments nest, as they do in Rust;
    raw strings honour their hash count; and a `'` is a char literal only when it closes,
    otherwise it is a lifetime and is stepped over.
    """
    out = list(text)
    i, n = 0, len(text)
    while i < n:
        c = text[i]
        if c == "/" and i + 1 < n and text[i + 1] == "/":
            while i < n and text[i] != "\n":
                out[i] = " "
                i += 1
            continue
        if c == "/" and i + 1 < n and text[i + 1] == "*":
            depth = 0
            while i < n:
                if text.startswith("/*", i):
                    depth += 1
                    out[i] = out[i + 1] = " "
                    i += 2
                    continue
                if text.startswith("*/", i):
                    depth -= 1
                    out[i] = out[i + 1] = " "
                    i += 2
                    if depth == 0:
                        break
                    continue
                if text[i] != "\n":
                    out[i] = " "
                i += 1
            continue
        if c == "r" and i + 1 < n and text[i + 1] in '#"':
            j = i + 1
            hashes = 0
            while j < n and text[j] == "#":
                hashes += 1
                j += 1
            if j < n and text[j] == '"':
                close = '"' + "#" * hashes
                end = text.find(close, j + 1)
                stop = n if end < 0 else end
                for k in range(j + 1, stop):
                    if text[k] != "\n":
                        out[k] = " "
                i = n if end < 0 else end + len(close)
                continue
        if c == '"':
            i += 1
            while i < n:
                if text[i] == "\\":
                    out[i] = " "
                    if i + 1 < n and text[i + 1] != "\n":
                        out[i + 1] = " "
                    i += 2
                    continue
                if text[i] == '"':
                    i += 1
                    break
                if text[i] != "\n":
                    out[i] = " "
                i += 1
            continue
        if c == "'":
            if i + 2 < n and text[i + 1] == "\\":
                end = text.find("'", i + 2)
                i = n if end < 0 else end + 1
                continue
            if i + 2 < n and text[i + 2] == "'":
                i += 3
                continue
            i += 1  # a lifetime, not a literal
            continue
        i += 1
    return "".join(out)


def rust_sources(root=None):
    """Every `.rs` file under `crates/`, as absolute paths, `target/` excluded."""
    base = CRATES if root is None else root
    out = []
    for dirpath, dirnames, filenames in os.walk(base):
        dirnames[:] = [d for d in dirnames if d != "target"]
        out.extend(os.path.join(dirpath, name) for name in filenames if name.endswith(".rs"))
    return sorted(out)


# --------------------------------------------------------------------------------------------
# Declaration forms
# --------------------------------------------------------------------------------------------

# An integer-valued type. The residue that blocks a deletion recommendation is counted over
# declarations of these types only, so what counts is written down rather than assumed: a `&str`
# or a struct literal cannot be an address, and letting one sit in the residue would suppress the
# advice forever for no reason. `Range` is in because a band constant
# (`const LEGACY_CONFIRM_CALLER_BAND: Range<usize> = 0x7a3000..0x7a4000;`) claims every address
# inside it, and arrays/slices are in because a table of addresses claims each element.
INT_SCALAR = r"(?:usize|isize|u8|u16|u32|u64|u128|i8|i16|i32|i64|i128)"
INT_TYPE = re.compile(
    r"^\s*(?:&'?\w*\s*)?(?:\[\s*)?"
    r"(?:(?:core|std)::(?:primitive|ops)::)?"
    r"(?:Range(?:Inclusive)?\s*<\s*)?"
    r"(?:\(\s*)?"
    + INT_SCALAR
    + r"\b"
)

# A declaration is found by its head and then scanned, not matched whole. A single regex cannot do
# it: `const TABLE: [usize; 2] = [..];` puts a `;` inside the type, and `const _: () = assert!(A ==
# B);` puts an `=` inside the expression, so any `[^=;]` fence either drops array-typed address
# tables or stops at the wrong character. Both forms carry addresses in this tree, so both are
# scanned with bracket depth instead.
DECLARATION_HEAD = re.compile(
    r"(?:^|[\s;{}()])(?:pub(?:\s*\([^)]*\))?\s+)?(const|static)\s+(?:mut\s+)?"
    r"([A-Za-z_][A-Za-z0-9_]*)\s*:"
)


def _scan_declaration(text, start):
    """From just after `NAME:`, return `(type, expr, end)` or None. Bracket-aware."""
    depth, i, n = 0, start, len(text)
    type_end = None
    while i < n:
        c = text[i]
        if c in "([{":
            depth += 1
        elif c in ")]}":
            if depth == 0:
                return None
            depth -= 1
        elif depth == 0 and c == "=" and text[i - 1] not in "=!<>" and text[i + 1 : i + 2] != "=":
            type_end = i
            break
        elif depth == 0 and c == ";":
            return None  # a declaration with no initialiser (a trait const, an `extern` static)
        i += 1
    if type_end is None:
        return None
    depth, i = 0, type_end + 1
    while i < n:
        c = text[i]
        if c in "([{":
            depth += 1
        elif c in ")]}":
            depth -= 1
        elif depth == 0 and c == ";":
            return text[start:type_end].strip(), text[type_end + 1 : i].strip(), i
        i += 1
    return None
ENUM_HEAD = re.compile(
    r"(?:#\[\s*repr\s*\(\s*([A-Za-z0-9_]+)\s*\)\s*\]\s*)?"
    r"(?:pub(?:\s*\([^)]*\))?\s+)?enum\s+([A-Za-z_][A-Za-z0-9_]*)\s*(?:<[^>{]*>\s*)?\{"
)
# `use a::b::OLD as NEW;` and `use a::{B as C, D as E};`. An alias does not declare a value, but it
# is another name for one, and a gate that reports "no symbol claims this" while an alias points at
# a claiming symbol is repeating the miss in a different key.
USE_LINE = re.compile(r"(?:^|[\s;{}])(?:pub(?:\s*\([^)]*\))?\s+)?use\s+([^;]+);", re.S)
# An identifier, and not the tail of a numeric literal. Without the lookbehind, `0x007a7b60`
# yields the "identifier" `x007a7b60`, which resolves to nothing, and every hex constant in the
# tree becomes unresolvable -- the residue swallows the universe and no address can ever be proven
# unclaimed. A `.` is in the lookbehind so a method call is left as punctuation the arithmetic
# filter then rejects, rather than being mistaken for a name.
IDENTIFIER = re.compile(
    r"(?<![0-9A-Za-z_.])[A-Za-z_][A-Za-z0-9_]*(?:\s*::\s*[A-Za-z_][A-Za-z0-9_]*)*"
)
HEX_LITERAL = re.compile(r"\b0[xX][0-9a-fA-F_]+\b")
DEC_LITERAL = re.compile(r"(?<![\w.])[0-9][0-9_]*(?![\w.])")

BUILTIN = {
    "true": 1,
    "false": 0,
    "i8::MIN": -(1 << 7),
    "i16::MIN": -(1 << 15),
    "i32::MIN": -(1 << 31),
    "i64::MIN": -(1 << 63),
    "isize::MIN": -(1 << 63),
    "i32::MAX": (1 << 31) - 1,
    "usize::BITS": 64,
    "u64::BITS": 64,
    "u32::BITS": 32,
    "u16::BITS": 16,
    "u8::BITS": 8,
    "usize::MAX": (1 << 64) - 1,
    "u64::MAX": (1 << 64) - 1,
    "u32::MAX": (1 << 32) - 1,
    "u16::MAX": (1 << 16) - 1,
    "u8::MAX": 0xFF,
    "i64::MAX": (1 << 63) - 1,
    "usize::MIN": 0,
    "u32::MIN": 0,
    "u64::MIN": 0,
}

ARITHMETIC_ONLY = re.compile(r"^[0-9a-fA-FxXoObB_+\-*/%()<>|&^~! ]*$")

# How wide is the slot? The residue is what blocks a "nothing claims this" proof, so shrinking it
# soundly matters more than shrinking it. This is the one sound way to shrink it: a `u8` cannot
# hold 0x7ad710 no matter what expression fills it, so an unevaluated `&[u8; 64]` byte string is
# not a declaration that "might be the address" -- it is one that PROVABLY is not.
#
# It is the same move `check-stale-rva-calls.py` makes when it excludes `PE_DOS_LFANEW_OFFSET` by
# value rather than by name: an exclusion has to rest on what the thing is. Excluding byte arrays
# because they "look like strings" would be the name-based reasoning this whole module exists to
# stop.
SCALAR_WIDTH = re.compile(r"\b(" + INT_SCALAR + r")\b")
WIDTH = {
    "u8": 8, "i8": 7, "u16": 16, "i16": 15, "u32": 32, "i32": 31,
    "u64": 64, "i64": 63, "u128": 128, "i128": 127, "usize": 64, "isize": 63,
}

# (size, align) in bytes, x86_64-pc-windows-msvc -- the one target this repo cross-compiles the
# game DLLs for (`cargo xwin build --release --target x86_64-pc-windows-msvc`), so `usize`/`isize`
# are pinned at 8 rather than left target-generic. Used only to compute `offset_of!`/`size_of!` /
# `align_of!` calls that reach a real Rust type -- never to guess a value, so an unlisted type
# (a generic, a smart pointer, a `#[repr(Rust)]` struct with no field list we trust) simply misses
# this table and the caller falls back to "unresolved", the same as before this table existed.
PRIMITIVE_SIZE_ALIGN = {
    "u8": (1, 1), "i8": (1, 1), "bool": (1, 1),
    "u16": (2, 2), "i16": (2, 2),
    "u32": (4, 4), "i32": (4, 4), "f32": (4, 4), "char": (4, 4),
    "u64": (8, 8), "i64": (8, 8), "f64": (8, 8),
    "usize": (8, 8), "isize": (8, 8),
    "u128": (16, 16), "i128": (16, 16),
}


def _can_hold(type_text, value):
    """Could a declaration of this type hold `value`? Unknown type -> yes, and stays residue."""
    found = SCALAR_WIDTH.search(type_text or "")
    if not found:
        return True
    bits = WIDTH.get(found.group(1))
    return True if bits is None else value < (1 << bits)


# Rust type layout is not a game address. `offset_of!`, `size_of` and `align_of` compute the layout
# of our own types: a byte offset inside a struct, or a length. Neither is an address in
# eldenring.exe's image, so a declaration built only from them and integer literals names no game
# address whatever number it happens to evaluate to, and keeping it in the residue would suppress
# every proof forever for no reason. Measured 2026-08-30: 219 of this tree's 285 unevaluated
# address-capable declarations are exactly that (`PROFILE_SUMMARY_LEVEL_OFFSET`,
# `PGD_NAME_9C_OFFSET`, the `read_character.rs` field table), leaving 66 that are genuinely
# unread -- and 4 once the width test below has ruled out the ones too narrow to hold a game RVA.
#
# The exclusion is RECURSIVE and it is by what the expression is, not by what it is called -- the
# same discipline as excluding a PE-header field by value. A constant that adds a layout quantity
# to something else (`SOME_RVA + size_of::<T>()`) is not layout-derived: the other operand could be
# a game address, so it stays in the residue and continues to block the proof.
LAYOUT_CALL = re.compile(
    r"(?:[A-Za-z_]\w*\s*::\s*)*(?:offset_of|size_of|align_of|size_of_val)\s*(?:!|::\s*<[^<>]*>)?\s*\("
)


def _strip_layout_calls(expr):
    """`(expr with every layout intrinsic replaced by 0, how many were replaced)`."""
    removed = 0
    while True:
        found = LAYOUT_CALL.search(expr)
        if not found:
            return expr, removed
        depth, i = 0, found.end() - 1
        while i < len(expr):
            if expr[i] == "(":
                depth += 1
            elif expr[i] == ")":
                depth -= 1
                if depth == 0:
                    break
            i += 1
        expr = expr[: found.start()] + "0" + expr[i + 1 :]
        removed += 1


# --------------------------------------------------------------------------------------------
# Rust type layout -- Evaluated, not just recognised. `_strip_layout_calls` above answers "is this
# expression built only from layout intrinsics" (for the residue exclusion); the functions below
# answer "what NUMBER does this layout intrinsic actually compute", by reading the real
# `#[repr(C)]` struct/enum/bitfield-wrapper definitions this tree and its `fromsoftware-rs` sibling
# declare. This is what turns `CHR_ASM_UNKD4_OFFSET`/`CHR_ASM_UNKD8_OFFSET` from "excluded by
# pattern" into "resolved to 0xd4 / 0xd8" -- the actual documented values in
# `chr_asm_layout.rs`, independently confirmed there against the ctor disassembly
# (`movq $-1,0xd4(%rsi)`). Every step below fails closed: an unrecognised type, an ambiguous name,
# or a struct without `#[repr(C...)]` returns None and the caller is left unresolved exactly as it
# was before this existed -- nothing here can manufacture a wrong value, only a missing one.
REPR_C = re.compile(r"repr\s*\(\s*C\b")
# A permissive sibling of `ENUM_HEAD` (used for address DISCRIMINANTS) for layout purposes only:
# `ChrAsmArmStyle` carries `#[repr(u32)]` followed by a `#[derive(..)]` before `enum`, which
# `ENUM_HEAD`'s single-optional-attribute shape does not admit. Kept separate rather than loosening
# `ENUM_HEAD` itself, so this change cannot alter which enum variants the address-claim scanner
# sees -- it only widens what this module can compute a size for.
ENUM_TYPE_HEAD = re.compile(
    r"#\s*\[\s*repr\s*\(\s*([A-Za-z0-9_]+)\s*\)\s*\]\s*"
    r"(?:#\s*\[[^\[\]]*\]\s*)*"
    r"(?:pub(?:\s*\([^)]*\))?\s+)?enum\s+([A-Za-z_][A-Za-z0-9_]*)\s*(?:<[^>{]*>\s*)?\{"
)
STRUCT_HEAD = re.compile(
    r"((?:#\s*\[[^\[\]]*\]\s*)+)(?:pub(?:\s*\([^)]*\))?\s+)?struct\s+([A-Za-z_]\w*)\b"
)
FIELD_RE = re.compile(
    r"^(?:#\s*\[[^\[\]]*\]\s*)*(?:pub(?:\s*\([^)]*\))?\s+)?([A-Za-z_]\w*)\s*:\s*(.+)$", re.S
)
# `bitfield! { ... pub struct GaitemHandle(u32); ... }` -- a primitive-wrapping newtype whose size
# is its inner primitive's size. Only the struct line inside the macro body is matched; the bit
# accessor methods around it are irrelevant to layout.
BITFIELD_MACRO = re.compile(r"\bbitfield!\s*\{")
BITFIELD_STRUCT_LINE = re.compile(r"\bstruct\s+([A-Za-z_]\w*)\s*\(\s*([A-Za-z_][\w:]*)\s*\)\s*;")
# `solo_params!( (SpEffectParam, SP_EFFECT_PARAM_ST, 15), .. )` in fromsoftware-rs's
# `solo_param_repository.rs` macro-generates `impl SoloParam for SpEffectParam { const INDEX: u32
# = 15; .. }` -- a param-table row index, not an address, and the macro invocation is the only
# place its value is spelled out (the `impl` body itself only ever contains the macro's `$Index`
# placeholder). Matched generically by shape, not hard-coded to `SpEffectParam` specifically, so
# any other row added to the same macro resolves the same way.
SOLO_PARAMS_CALL = re.compile(r"\bsolo_params!\s*\(")
SOLO_PARAM_TUPLE = re.compile(
    r"^\(\s*([A-Za-z_]\w*)\s*,\s*([A-Za-z_][\w:<>]*)\s*,\s*(.+?)\s*\)$", re.S
)
# `core::mem::offset_of!(ChrAsm, equipment_param_ids)`, `core::mem::size_of::<i32>()`,
# `align_of::<T>()`. These substitute a number into the expression text before the ordinary
# arithmetic evaluator runs, so downstream arithmetic (`OFFSET + COUNT * size_of::<i32>()`) needs
# no special-casing at all once the intrinsic itself is a plain digit string.
OFFSET_OF_RE = re.compile(
    r"(?:[A-Za-z_]\w*\s*::\s*)*offset_of\s*!\s*\(\s*([A-Za-z_][\w:]*)\s*,\s*([A-Za-z_][\w.]*)\s*\)"
)
SIZE_OF_TURBOFISH = re.compile(
    r"(?:[A-Za-z_]\w*\s*::\s*)*size_of\s*::\s*<\s*([^<>]+?)\s*>\s*\(\s*\)"
)
ALIGN_OF_TURBOFISH = re.compile(
    r"(?:[A-Za-z_]\w*\s*::\s*)*align_of\s*::\s*<\s*([^<>]+?)\s*>\s*\(\s*\)"
)


def _scan_balanced(text, open_index, open_char, close_char):
    """Index of the `close_char` matching the `open_char` at `open_index`, or None."""
    depth, i, n = 0, open_index, len(text)
    while i < n:
        if text[i] == open_char:
            depth += 1
        elif text[i] == close_char:
            depth -= 1
            if depth == 0:
                return i
        i += 1
    return None


def _skip_to_struct_open(text, start):
    """`(index, char)` of the first top-level `{`/`(`/`;` at/after `start`, `<...>` skipped.

    A generic struct's `<T>` parameter list can itself contain `{`/`(`; without tracking angle
    depth a bound like `struct Foo<T: Fn() -> X>` would stop on the closure-arg `(` instead of the
    real body opener.
    """
    i, n, angle = start, len(text), 0
    while i < n:
        c = text[i]
        if c == "<":
            angle += 1
        elif c == ">":
            angle = max(0, angle - 1)
        elif angle == 0 and c in "{(;":
            return i, c
        i += 1
    return None, None


def _round_up(value, align):
    if align <= 1:
        return value
    return (value + align - 1) // align * align


class Decl:
    """One declaration, with everything a failure message needs to be actionable."""

    __slots__ = (
        "path", "line", "symbol", "owner", "form", "type_text", "expr", "value", "band",
    )

    def __init__(self, path, line, symbol, owner, form, type_text, expr):
        self.path, self.line, self.symbol, self.owner = path, line, symbol, owner
        self.form, self.type_text, self.expr = form, type_text, expr
        self.value = None  # set[int] of every integer the expression yields
        self.band = None  # list[(lo, hi)] for range constants

    @property
    def qualified(self):
        return f"{self.owner}::{self.symbol}" if self.owner else self.symbol

    def where(self, root=ROOT):
        rel = os.path.relpath(self.path, root) if self.path.startswith(root) else self.path
        return f"{rel}:{self.line}"

    def __repr__(self):
        return f"<Decl {self.qualified} {self.form} {self.where()}>"


class Literal:
    """A bare hex literal in code -- an address claimed with no constant name at all."""

    __slots__ = ("path", "line", "text", "value")

    def __init__(self, path, line, text, value):
        self.path, self.line, self.text, self.value = path, line, text, value

    def where(self, root=ROOT):
        rel = os.path.relpath(self.path, root) if self.path.startswith(root) else self.path
        return f"{rel}:{self.line}"


class Claims:
    """The answer to "who claims this address", and how much of it is proven."""

    __slots__ = (
        "address",
        "declarations",
        "aliases",
        "literals",
        "uses",
        "residue",
        "files_read",
        "universe",
    )

    def __init__(self, address):
        self.address = address
        self.declarations = []
        self.aliases = []
        self.literals = []
        self.uses = {}
        self.residue = []
        self.files_read = 0
        self.universe = 0

    @property
    def found_nothing(self):
        return not self.declarations and not self.literals

    @property
    def proven_unclaimed(self):
        """True only when nothing claims it and the resolver could evaluate everything.

        The two halves are separate on purpose. `found_nothing` is what a regex can tell you;
        this is what a resolver can prove, and only this may be used to advise a deletion.
        """
        return self.files_read > 0 and self.found_nothing and not self.residue


def _element_count(expr):
    """How many elements a literal `[..]` / `&[..]` table has, or None if it is not one.

    The elements need not be numbers: `const ALL_SEAMS: &[MapSeam] = &[MapSeam { .. }, ..];`
    yields no values at all, but its length is still an ordinary compile-time integer, and leaving
    `ALL_SEAMS.len()` unresolved put a log-line limit into the residue that blocks every proof.
    """
    expr = expr.strip()
    while expr.startswith("&"):
        expr = re.sub(r"^&\s*(?:mut\s+|'\w+\s+)?", "", expr).strip()
    if not (expr.startswith("[") and expr.endswith("]")):
        return None
    inner = expr[1:-1]
    repeat = _split_top(inner, [";"])
    if len(repeat) == 2:
        return None  # `[0u8; 32]` is a repeat expression, not a list
    return sum(1 for element in _split_top(inner, [","]) if element.strip())


def _split_top(text, separators):
    """Split on `separators` that are not inside (), [], <> or {}."""
    parts, depth, angle, start = [], 0, 0, 0
    i = 0
    while i < len(text):
        c = text[i]
        if c in "([{":
            depth += 1
        elif c in ")]}":
            depth -= 1
        elif c == "<":
            angle += 1
        elif c == ">":
            angle = max(0, angle - 1)
        elif depth == 0 and angle == 0:
            for sep in separators:
                if text.startswith(sep, i):
                    parts.append(text[start:i])
                    i += len(sep)
                    start = i
                    break
            else:
                i += 1
                continue
            continue
        i += 1
    parts.append(text[start:])
    return parts


class Index:
    """Every address-valued declaration in `crates/`, resolved to numbers where possible."""

    def __init__(self):
        self.decls = []
        self.by_simple = {}
        self.by_qualified = {}
        self.aliases = {}  # alias name -> target path
        self.literals = []
        self.files_read = 0
        self.text = {}  # path -> comment/string-stripped source
        # Type layout, not address declarations -- see the module docstring above `REPR_C`. Keyed
        # by simple type name; a list because the same name can be declared more than once (only
        # `len(..) == 1` is trusted, same "ambiguous is unresolved" rule as everywhere else here).
        self.structs = {}  # name -> [(path, [(field_name, type_text), ..]), ..]
        self.enum_sizes = {}  # name -> [size_bytes, ..]  (fieldless #[repr(uN)] enums only)
        self.bitfield_inner = {}  # name -> [inner_type_text, ..]
        self.external_consts = {}  # "Type::CONST" -> {value, ..} (fromsoftware-rs macro consts)
        self._layout_cache = {}  # struct name -> resolved layout dict, or None if unresolvable

    # -- building ----------------------------------------------------------------------------

    @classmethod
    def build(cls, sources=None, root=None):
        index = cls()
        for path in rust_sources(root) if sources is None else sources:
            try:
                raw = open(path, encoding="utf-8", errors="replace").read()
            except OSError:
                continue
            index.files_read += 1
            text = code_only(raw)
            index.text[path] = text
            index._read_declarations(path, text)
            index._read_enums(path, text)
            index._read_uses(path, text)
            index._read_literals(path, text)
            index._read_type_defs(path, text)
        index._read_external()
        index._resolve_all()
        return index

    def _read_external(self):
        """`fromsoftware-rs` sibling: type layout + macro-sourced consts, value source only.

        Deliberately does not touch `files_read`, `decls`, `text`, `by_simple`/`by_qualified`, or
        `literals` -- those define "who in crates/ claims this address", and this tree is not
        `crates/`. See the `FROMSOFT_TYPE_ROOTS` comment for why this scope is safe to widen (it
        can only add resolving power, never remove an address from the claims universe).
        """
        for root_dir in FROMSOFT_TYPE_ROOTS:
            if not os.path.isdir(root_dir):
                continue
            for path in rust_sources(root_dir):
                try:
                    raw = open(path, encoding="utf-8", errors="replace").read()
                except OSError:
                    continue
                text = code_only(raw)
                self._read_type_defs(path, text)
                self._read_solo_params(path, text)

    def _add(self, decl):
        self.decls.append(decl)
        self.by_simple.setdefault(decl.symbol, []).append(decl)
        if decl.owner:
            self.by_qualified.setdefault(decl.qualified, []).append(decl)

    def _read_declarations(self, path, text):
        for match in DECLARATION_HEAD.finditer(text):
            scanned = _scan_declaration(text, match.end())
            if scanned is None:
                continue
            type_text, expr, _ = scanned
            # `match.start(1)`, not `match.start()`: the head pattern eats one leading character
            # so it can anchor on a boundary, and counting newlines to it reports the previous
            # line whenever that character is the newline itself.
            line = text.count("\n", 0, match.start(1)) + 1
            self._add(Decl(path, line, match.group(2), None, match.group(1), type_text, expr))

    def _read_enums(self, path, text):
        for head in ENUM_HEAD.finditer(text):
            repr_type, name = head.group(1), head.group(2)
            body, close = self._enum_body(text, head.end() - 1)
            if body is None:
                continue
            previous = None
            for member in _split_top(body, [","]):
                member = member.strip()
                if not member or member.startswith("#"):
                    continue
                if "(" in member.split("=")[0] or "{" in member.split("=")[0]:
                    previous = None  # a payload variant has no discriminant
                    continue
                variant, _, expr = member.partition("=")
                variant = variant.strip()
                if not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", variant):
                    continue
                offset = text.find(variant, head.end())
                line = text.count("\n", 0, offset) + 1 if offset >= 0 else 1
                decl = Decl(
                    path,
                    line,
                    variant,
                    name,
                    "enum-variant",
                    repr_type or "isize",
                    expr.strip() if expr.strip() else f"__IMPLICIT__{previous}",
                )
                self._add(decl)
                previous = decl.qualified

    @staticmethod
    def _enum_body(text, open_brace):
        depth = 0
        for i in range(open_brace, len(text)):
            if text[i] == "{":
                depth += 1
            elif text[i] == "}":
                depth -= 1
                if depth == 0:
                    return text[open_brace + 1 : i], open_brace
        return None, open_brace

    def _read_uses(self, path, text):
        for match in USE_LINE.finditer(text):
            body = match.group(1)
            prefix, _, braced = body.partition("{")
            if braced:
                prefix = prefix.strip()
                for item in _split_top(braced.rsplit("}", 1)[0], [","]):
                    self._alias(prefix + item.strip())
            else:
                self._alias(body)

    def _alias(self, item):
        item = " ".join(item.split())
        found = re.match(r"^([A-Za-z_][A-Za-z0-9_:\s]*?)\s+as\s+([A-Za-z_][A-Za-z0-9_]*)$", item)
        if not found:
            return
        target = found.group(1).replace(" ", "")
        self.aliases.setdefault(found.group(2), target)

    def _read_literals(self, path, text):
        for match in HEX_LITERAL.finditer(text):
            token = match.group(0)
            try:
                value = int(token.replace("_", ""), 16)
            except ValueError:
                continue
            line = text.count("\n", 0, match.start()) + 1
            self.literals.append(Literal(path, line, token, value))

    # -- type layout (structs/enums/bitfield wrappers), value source only ---------------------

    def _read_type_defs(self, path, text):
        self._read_structs(path, text)
        self._read_enum_sizes(path, text)
        self._read_bitfields(path, text)

    def _read_structs(self, path, text):
        """Named-field `#[repr(C)]` structs -> ordered `(field_name, type_text)` lists.

        A struct with no `#[repr(C)]` in its attribute cluster is skipped outright: Rust's default
        repr has unspecified field order, so guessing a layout for one would not be evaluation, it
        would be fabrication wearing the same shape.
        """
        for match in STRUCT_HEAD.finditer(text):
            attrs, name = match.group(1), match.group(2)
            if not REPR_C.search(attrs):
                continue
            open_index, kind = _skip_to_struct_open(text, match.end(2))
            if open_index is None or kind != "{":
                continue  # tuple/unit structs carry no named field to offset_of! against
            close_index = _scan_balanced(text, open_index, "{", "}")
            if close_index is None:
                continue
            fields = []
            for entry in _split_top(text[open_index + 1 : close_index], [","]):
                entry = entry.strip()
                if not entry:
                    continue
                found = FIELD_RE.match(entry)
                if found:
                    fields.append((found.group(1), found.group(2).strip()))
            if fields:
                self.structs.setdefault(name, []).append((path, fields))

    def _read_enum_sizes(self, path, text):
        """Fieldless `#[repr(uN)]` enums -> their discriminant's byte size (== its align)."""
        for head in ENUM_TYPE_HEAD.finditer(text):
            repr_type, name = head.group(1), head.group(2)
            body, _ = self._enum_body(text, head.end() - 1)
            if body is None:
                continue
            fieldless = True
            for member in _split_top(body, [","]):
                member = member.strip()
                if not member or member.startswith("#"):
                    continue
                head_part = member.split("=", 1)[0]
                if "(" in head_part or "{" in head_part:
                    fieldless = False
                    break
            if not fieldless:
                continue
            size_align = PRIMITIVE_SIZE_ALIGN.get(repr_type.strip().lower())
            if size_align is not None:
                self.enum_sizes.setdefault(name, []).append(size_align[0])

    def _read_bitfields(self, path, text):
        """`bitfield! { pub struct Name(u32); .. }` -> `Name`'s inner primitive type text."""
        for match in BITFIELD_MACRO.finditer(text):
            close_index = _scan_balanced(text, match.end() - 1, "{", "}")
            if close_index is None:
                continue
            body = text[match.end() : close_index]
            for found in BITFIELD_STRUCT_LINE.finditer(body):
                self.bitfield_inner.setdefault(found.group(1), []).append(found.group(2))

    def _read_solo_params(self, path, text):
        """`solo_params!( (Type, StructType, Index), .. )` -> `Type::INDEX = Index`.

        The macro's own body only ever contains `const INDEX: u32 = $Index;` -- the placeholder,
        never a number -- so the invocation's tuple list is the only place the real value is
        written down at all.
        """
        for match in SOLO_PARAMS_CALL.finditer(text):
            close_index = _scan_balanced(text, match.end() - 1, "(", ")")
            if close_index is None:
                continue
            body = text[match.end() : close_index]
            for entry in _split_top(body, [","]):
                entry = entry.strip()
                if not entry:
                    continue
                found = SOLO_PARAM_TUPLE.match(entry)
                if not found:
                    continue
                type_name, index_expr = found.group(1), found.group(3)
                try:
                    value = int(index_expr.strip(), 0)
                except ValueError:
                    continue
                self.external_consts.setdefault(f"{type_name}::INDEX", set()).add(value)

    # -- type layout evaluation ----------------------------------------------------------------

    def _type_size_align(self, type_text, seen=None):
        """`(size, align)` in bytes for a Rust type text, or None if it cannot be computed.

        Fails closed at every branch: a type this cannot classify (a generic, a smart pointer, an
        ambiguous or undeclared struct/enum/bitfield name) returns None rather than a guess.
        """
        seen = set() if seen is None else seen
        t = " ".join(type_text.split())
        if t.startswith("&"):
            return (8, 8)  # a reference is pointer-sized regardless of what it points at
        if t.startswith("*const ") or t.startswith("*mut "):
            return (8, 8)
        array = re.fullmatch(r"\[\s*(.+?)\s*;\s*(.+?)\s*\]", t)
        if array:
            element = self._type_size_align(array.group(1), seen)
            if element is None:
                return None
            try:
                count = int(array.group(2).strip(), 0)
            except ValueError:
                return None
            return (element[0] * count, element[1])
        last = t.split("::")[-1].split("<", 1)[0].strip()
        if not last:
            return None
        primitive = PRIMITIVE_SIZE_ALIGN.get(last)
        if primitive is not None:
            return primitive
        if last in seen:
            return None  # recursion guard; unreachable for valid by-value Rust layouts
        nested = seen | {last}
        layout = self._struct_layout(last, nested)
        if layout is not None:
            return (layout["size"], layout["align"])
        enum_size = self._enum_size(last)
        if enum_size is not None:
            return (enum_size, enum_size)
        inner = self._bitfield_inner_type(last)
        if inner is not None:
            return self._type_size_align(inner, nested)
        return None

    def _struct_layout(self, name, seen=None):
        """Sequential `#[repr(C)]` layout for `name`: `{size, align, offsets, field_types}`."""
        if name in self._layout_cache:
            return self._layout_cache[name]
        candidates = self.structs.get(name, [])
        if len(candidates) != 1:
            self._layout_cache[name] = None  # unknown, or declared >1 way -- ambiguous
            return None
        _, fields = candidates[0]
        seen = set() if seen is None else seen
        offset, max_align, offsets, field_types = 0, 1, {}, {}
        for field_name, field_type in fields:
            size_align = self._type_size_align(field_type, seen)
            if size_align is None:
                self._layout_cache[name] = None
                return None
            size, align = size_align
            offset = _round_up(offset, align)
            offsets[field_name] = offset
            field_types[field_name] = field_type
            offset += size
            max_align = max(max_align, align)
        result = {
            "size": _round_up(offset, max_align),
            "align": max_align,
            "offsets": offsets,
            "field_types": field_types,
        }
        self._layout_cache[name] = result
        return result

    def _enum_size(self, name):
        sizes = set(self.enum_sizes.get(name, []))
        return next(iter(sizes)) if len(sizes) == 1 else None

    def _bitfield_inner_type(self, name):
        inner = set(self.bitfield_inner.get(name, []))
        return next(iter(inner)) if len(inner) == 1 else None

    def _offset_of(self, struct_name, field_path):
        """Byte offset of `field_path` (dot-separated for nested fields) within `struct_name`."""
        current = self._struct_layout(struct_name.split("::")[-1])
        if current is None:
            return None
        total = 0
        fields = field_path.split(".")
        for i, field in enumerate(fields):
            if field not in current["offsets"]:
                return None
            total += current["offsets"][field]
            if i == len(fields) - 1:
                return total
            next_name = current["field_types"][field].split("::")[-1].split("<", 1)[0].strip()
            current = self._struct_layout(next_name)
            if current is None:
                return None
        return total

    # -- evaluation --------------------------------------------------------------------------

    def _resolve_all(self):
        for decl in self.decls:
            self.value_of(decl)

    def in_universe(self, decl):
        """Could this declaration hold an address at all? The residue is counted over these."""
        if decl.form == "enum-variant":
            return bool(re.fullmatch(INT_SCALAR, decl.type_text or ""))
        return bool(INT_TYPE.match(decl.type_text or ""))

    def value_of(self, decl, seen=None):
        """`decl`'s value(s), or None. `seen` is the ancestor path, not a visited set.

        It has to be popped on the way out. Left as a visited set it also excludes SIBLINGS: one
        traversal that happens to touch `QuitRow::SaveGame` and then, later in the same traversal,
        needs it again gets None -- and the second answer was cached, so an ordinary implicit enum
        discriminant was recorded as unresolvable and sat in the residue blocking every proof.
        Successes are cached on the declaration; failures are not cached at all, because a failure
        caused by the cycle guard is a fact about one traversal, not about the declaration.
        """
        if decl.value is not None or decl.band is not None:
            return decl.value
        seen = set() if seen is None else seen
        key = id(decl)
        if key in seen:
            return None
        seen.add(key)
        try:
            result = self._evaluate(decl.expr, seen, decl.path)
        finally:
            seen.discard(key)
        if result is None:
            return None
        decl.value, decl.band = result
        return decl.value

    def _evaluate(self, expr, seen, scope=None):
        """`(set_of_ints, list_of_(lo, hi))`, or None when the expression is not a number."""
        expr = expr.strip()
        if not expr:
            return None
        if expr.startswith("__IMPLICIT__"):
            previous = expr[len("__IMPLICIT__") :]
            if previous == "None":
                return ({0}, [])
            earlier = self._lookup(previous, seen, scope)
            return (
                ({min(earlier) + 1}, [])
                if earlier and len(earlier) == 1
                else None
            )
        # `&[..]`, `&FOO`
        while expr.startswith("&"):
            expr = expr[1:].strip()
            expr = re.sub(r"^(?:mut|'\w+)\s+", "", expr).strip()
        # A bracketed or parenthesised list: an array, slice or tuple of addresses. Each element
        # is a claim, so every one is evaluated and the union returned.
        if (expr.startswith("[") and expr.endswith("]")) or (
            expr.startswith("(") and expr.endswith(")") and _split_top(expr[1:-1], [","])[1:]
        ):
            inner = expr[1:-1]
            # `[0u8; 32]` is a repeat, not a list of addresses -- the count is not an address.
            repeat = _split_top(inner, [";"])
            if expr.startswith("[") and len(repeat) == 2:
                inner = repeat[0]
            values, bands, ok = set(), [], True
            for element in _split_top(inner, [","]):
                if not element.strip():
                    continue
                part = self._evaluate(element, seen, scope)
                if part is None:
                    ok = False
                    continue
                values |= part[0]
                bands += part[1]
            return (values, bands) if ok or values else None
        # A range band: every address inside it is claimed.
        for sep, inclusive in (("..=", True), ("..", False)):
            halves = _split_top(expr, [sep])
            if len(halves) == 2 and halves[0].strip() and halves[1].strip():
                low = self._scalar(halves[0], seen, scope)
                high = self._scalar(halves[1], seen, scope)
                if low is None or high is None:
                    return None
                return (set(), [(low, high if inclusive else high - 1)])
        scalar = self._scalar(expr, seen, scope)
        return None if scalar is None else ({scalar}, [])

    LEN_CALL = re.compile(r"(?<![\w.])((?:[A-Za-z_]\w*::)*[A-Za-z_]\w*)\s*\.\s*len\s*\(\s*\)")

    def _scalar(self, expr, seen, scope=None):
        expr = expr.strip()
        # `core::mem::offset_of!(ChrAsm, equipment_param_ids)`, `size_of::<i32>()`,
        # `align_of::<T>()` -- substituted with the actual computed number (via `_offset_of` /
        # `_type_size_align`, which read real `#[repr(C)]` layouts) before anything else runs, so
        # downstream arithmetic needs no special-casing at all: `offset + count *
        # size_of::<i32>()` becomes an ordinary digit expression once the intrinsic is a digit.
        # Each loop `break`s (never substitutes a partial answer) the moment one occurrence can't
        # be computed, so a call this cannot resolve is left verbatim and falls through to the
        # ordinary "not an arithmetic expression" rejection below, exactly as before this existed.
        while True:
            found = OFFSET_OF_RE.search(expr)
            if not found:
                break
            value = self._offset_of(found.group(1), found.group(2))
            if value is None:
                break
            expr = expr[: found.start()] + str(value) + expr[found.end() :]
        while True:
            found = SIZE_OF_TURBOFISH.search(expr)
            if not found:
                break
            size_align = self._type_size_align(found.group(1))
            if size_align is None:
                break
            expr = expr[: found.start()] + str(size_align[0]) + expr[found.end() :]
        while True:
            found = ALIGN_OF_TURBOFISH.search(expr)
            if not found:
                break
            size_align = self._type_size_align(found.group(1))
            if size_align is None:
                break
            expr = expr[: found.start()] + str(size_align[1]) + expr[found.end() :]
        # `const STARTING_CLASS_COUNT: usize = STARTING_CLASSES.len();` -- the length of a literal
        # table is known at parse time, and leaving it unresolved put a plain row count in the
        # residue that blocks every unclaimed proof.
        while True:
            found = self.LEN_CALL.search(expr)
            if not found:
                break
            table = self._table(found.group(1), scope)
            count = None if table is None else _element_count(table.expr)
            if count is None:
                return None
            expr = expr[: found.start()] + str(count) + expr[found.end() :]
        # `X as usize`, `X as u32`, and the `::<..>` turbofish are noise for a value.
        expr = re.sub(r"\s+as\s+[A-Za-z_][A-Za-z0-9_:]*", " ", expr)
        # A typed literal suffix, `0x10usize` and `0x8007_007e_u32` alike. The underscore form is
        # the one that matters: without it the `_u32` survives, the arithmetic filter rejects the
        # `u`, and an ordinary HRESULT constant becomes unresolvable residue.
        expr = re.sub(r"(?<=[0-9a-fA-F])_?(?:u|i)(?:8|16|32|64|128|size)\b", "", expr)
        substituted, offset = [], 0
        for match in IDENTIFIER.finditer(expr):
            name = match.group(0).replace(" ", "")
            if re.fullmatch(r"0[xXbBoO][0-9a-fA-F_]*", name):
                continue
            resolved = self._lookup(name, seen, scope)
            if resolved is None or len(resolved) != 1:
                return None
            substituted.append((match.start(), match.end(), str(next(iter(resolved)))))
        for start, end, replacement in reversed(substituted):
            expr = expr[:start] + replacement + expr[end:]
        # A multi-line expression is one expression. `const CURSOR_SLOT_MASK: usize = (1 <<
        # slot::CURSOR_UP)\n | (1 << slot::CURSOR_DOWN) ...` resolved every name and then failed
        # the arithmetic filter on the NEWLINES, which is a formatting accident recorded as
        # "unknown value" and, downstream, as a reason no address can be proven unclaimed.
        expr = " ".join(expr.split())
        if not ARITHMETIC_ONLY.fullmatch(expr):
            return None
        expr = re.sub(r"(?<![/])/(?![/])", "//", expr)
        # Rust spells bitwise not `!`; Python spells it `~`. `!X` is how this tree writes a
        # sentinel (`const OWN_STEPPER_SLOT_NONE: i32 = !OWN_STEPPER_SLOT_ZERO;`).
        expr = expr.replace("!", "~")
        try:
            value = eval(expr, {"__builtins__": {}}, {})  # noqa: S307 - digits and operators only
        except Exception:
            return None
        return value if isinstance(value, int) else None

    def _table(self, name, scope):
        """The declaration a `NAME.len()` refers to, preferring the file that wrote it."""
        last = name.split("::")[-1]
        pool = self.by_simple.get(last, [])
        local = [decl for decl in pool if decl.path == scope]
        chosen = local or pool
        return chosen[0] if len(chosen) == 1 else None

    def _lookup(self, path, seen, scope=None):
        """A name at a use site -> its value(s). Aliases, module paths and enum variants alike.

        `scope` is the file the name was written in, and its declarations are consulted first.
        Without that, a short name that several crates declare independently -- `BASE` is declared
        three times with three meanings -- reads as ambiguous and drags every constant built on it
        into the residue, which is a wrong answer (it is not ambiguous at the site that wrote it)
        that also suppresses every proof.
        """
        path = path.strip()
        if path in BUILTIN:
            return {BUILTIN[path]}
        segments = [s for s in path.split("::") if s]
        if not segments:
            return None
        # A declaration already being evaluated is not a candidate for its own value. This tree
        # re-exports a crate-wide address under the same simple name in a hundred places
        # (`const GAME_MAN_SINGLETON_RVA: usize = er_game_base::rva::GAME_MAN_SINGLETON_RVA;`), so
        # without this a file-local preference resolves the name to the constant currently being
        # resolved, hits the cycle guard, and reports a perfectly ordinary re-export as residue.
        def candidates(pool):
            return [decl for decl in pool if id(decl) not in seen]

        last = segments[-1]
        if len(segments) >= 2:
            qualified = "::".join(segments[-2:])
            pool = candidates(self.by_qualified.get(qualified, []))
            local = [decl for decl in pool if decl.path == scope]
            if local:
                return self._first_value(local, seen)
            if pool:
                return self._first_value(pool, seen)
        pool = candidates(self.by_simple.get(last, []))
        local = [decl for decl in pool if decl.path == scope]
        if local:
            return self._first_value(local, seen)
        if last in self.aliases and self.aliases[last] != path:
            through = self._lookup(self.aliases[last], seen, scope)
            if through is not None:
                return through
        if pool:
            return self._first_value(pool, seen)
        # No declaration in `crates/` names this -- the last resort is a fromsoftware-rs macro
        # const (`SpEffectParam::INDEX`, from `solo_params!`; see `_read_solo_params`). Checked
        # last, after every local possibility, for the same reason `scope`-local decls win above:
        # this repo's own declarations are the authority on its own names.
        if len(segments) >= 2:
            qualified = "::".join(segments[-2:])
            values = self.external_consts.get(qualified)
            if values is not None and len(values) == 1:
                return set(values)
        return None

    def _first_value(self, decls, seen):
        values = set()
        for decl in decls:
            got = self.value_of(decl, seen)
            if got is not None:
                values |= got
        # A name declared twice with different values is ambiguous, and ambiguous is unresolved:
        # picking one would be a guess wearing a number.
        return values if len(values) == 1 else None

    # -- querying ----------------------------------------------------------------------------

    def layout_derived(self, decl, seen=None):
        """Is this declaration built only from Rust type layout and integer literals?

        `seen` is the ancestor path and is popped on the way out, for the same reason it is in
        `value_of`: as a visited set it also rejects siblings, so a constant that adds two layout
        offsets together decided the second one was unknown purely because the first had already
        been looked at.
        """
        seen = set() if seen is None else seen
        if id(decl) in seen:
            return False
        seen.add(id(decl))
        try:
            return self._layout_derived(decl, seen)
        finally:
            seen.discard(id(decl))

    def _layout_derived(self, decl, seen):
        decl_path = decl.path
        # A `{ use a::b; <layout expr> }` block: the imports are not terms of the value.
        stripped, removed = _strip_layout_calls(re.sub(r"\buse\s+[^;]*;", " ", decl.expr))
        names = [
            match.group(0).replace(" ", "")
            for match in IDENTIFIER.finditer(stripped)
            if match.group(0).replace(" ", "") not in BUILTIN
        ]
        # A literal is not layout-derived, and neither is an empty expression. An expression that
        # is nothing but another layout constant is -- `const ITEM_FUNCTOR_A8: usize =
        # MENU_ITEM_FUNCTOR_A8_OFFSET;` re-exports a struct offset and is no more an address than
        # the offset_of! it forwards.
        if not removed and not names:
            return False
        for name in names:
            pool = self.by_qualified.get(
                "::".join(name.split("::")[-2:]), []
            ) or self.by_simple.get(name.split("::")[-1], [])
            # Same-file first, and never the declaration currently being judged: this tree
            # re-exports a struct offset under its own name (`const GAME_MAN_FLAG_BC4_OFFSET:
            # usize = crate::GAME_MAN_FLAG_BC4_OFFSET;`), and matching that against itself makes a
            # plain forwarding constant look like something unknown.
            pool = [decl for decl in pool if id(decl) not in seen]
            local = [decl for decl in pool if decl.path == decl_path]
            targets = local or pool
            if not targets or not all(self.layout_derived(t, seen) for t in targets):
                return False
        return True

    def residue(self):
        """Declarations in the address-capable universe that could not be evaluated.

        This is the reason a caller may not say "nothing claims this address". Each entry is a
        declaration that might be the address and could not be read.
        """
        return [
            decl
            for decl in self.decls
            if self.in_universe(decl)
            and decl.value is None
            and decl.band is None
            and not self.layout_derived(decl)
        ]

    def universe_size(self):
        return sum(1 for decl in self.decls if self.in_universe(decl))

    def uses_of(self, symbol):
        """Every line in `crates/` code that mentions `symbol`, minus its own declarations."""
        declared = {(decl.path, decl.line) for decl in self.by_simple.get(symbol, [])}
        token = re.compile(r"\b" + re.escape(symbol) + r"\b")
        out = []
        for path, text in self.text.items():
            for match in token.finditer(text):
                line = text.count("\n", 0, match.start()) + 1
                if (path, line) in declared:
                    continue
                out.append((path, line))
        return out

    def claims(self, address):
        """Everything that claims `address`, and whether "nothing does" is proven.

        Both the RVA and the `0x140000000 +` VA spelling are looked for, because the tree writes
        an address either way and a resolver that only knew one would repeat the class of miss
        this module exists to close.
        """
        rva = address - IMAGE_BASE if address >= IMAGE_BASE else address
        wanted = {rva, rva + IMAGE_BASE}
        result = Claims(rva)
        result.files_read = self.files_read
        result.universe = self.universe_size()
        for decl in self.decls:
            hit = (decl.value or set()) & wanted
            in_band = any(
                low <= value <= high for (low, high) in (decl.band or []) for value in wanted
            )
            if hit or in_band:
                result.declarations.append(decl)
        claimed_names = {decl.symbol for decl in result.declarations}
        for alias, target in sorted(self.aliases.items()):
            if target.split("::")[-1] in claimed_names:
                result.aliases.append((alias, target))
        declared_at = {(decl.path, decl.line) for decl in result.declarations}
        for literal in self.literals:
            if literal.value in wanted and (literal.path, literal.line) not in declared_at:
                result.literals.append(literal)
        for symbol in sorted(claimed_names):
            result.uses[symbol] = self.uses_of(symbol)
        # The residue is computed for this address, not in the abstract: a declaration whose type
        # cannot represent the address is not a declaration that might be it.
        result.residue = [
            decl
            for decl in self.residue()
            if any(_can_hold(decl.type_text, value) for value in wanted)
        ]
        return result


_CACHE = {}


def index(root=None):
    """The index for a tree, built once per process."""
    key = os.path.abspath(root or CRATES)
    if key not in _CACHE:
        _CACHE[key] = Index.build(root=key)
    return _CACHE[key]


def describe_claims(result, out=sys.stdout, indent="  "):
    """Print who claims the address, and say plainly which of the three answers this is."""
    pad = indent
    if result.declarations:
        print(f"{pad}0x{result.address:x} IS DECLARED, so the address carries a feature:", file=out)
        for decl in result.declarations:
            uses = result.uses.get(decl.symbol, [])
            print(
                f"{pad}  {decl.where()}  {decl.qualified}"
                f"  ({decl.form}, {len(uses)} use site(s) elsewhere)",
                file=out,
            )
        for alias, target in result.aliases:
            print(f"{pad}  ...also reachable as `{alias}` (use {target} as {alias})", file=out)
    if result.literals:
        print(
            f"{pad}0x{result.address:x} appears as a BARE LITERAL in code "
            f"({len(result.literals)} site(s)) -- an address claimed with no constant name:",
            file=out,
        )
        for literal in result.literals[:6]:
            print(f"{pad}  {literal.where()}  {literal.text}", file=out)
    if result.declarations or result.literals:
        return "CLAIMED"
    if result.proven_unclaimed:
        print(
            f"{pad}NOTHING in crates/ claims 0x{result.address:x}. PROVEN: all "
            f"{result.universe} address-capable declarations were evaluated and none is this "
            f"address,\n{pad}and no bare literal of it occurs in code.",
            file=out,
        )
        return "PROVEN-UNCLAIMED"
    print(
        f"{pad}NOT PROVEN. No declaration or literal for 0x{result.address:x} was FOUND, but "
        f"{len(result.residue)} of {result.universe} address-capable\n"
        f"{pad}declarations could not be evaluated, so one of them may be it. This is "
        f"\"I found no reference\", NOT\n{pad}\"there is no reference\" -- do not delete anything "
        f"on this evidence. Run:\n{pad}  python3 scripts/rva_symbols.py --residue "
        f"0x{result.address:x}",
        file=out,
    )
    if result.files_read == 0:
        print(
            f"{pad}...and the walk read ZERO files, so nothing above was searched at all.",
            file=out,
        )
    return "NOT-PROVEN"


# --------------------------------------------------------------------------------------------
# Selftest
# --------------------------------------------------------------------------------------------

# The MATCHER this module replaces, frozen as a literal. `check-1170-translation-collisions.py`
# built its question as `rf"const [A-Z0-9_]+: *usize *= *0x{rva:x}\b"` and, finding no match, told
# the reader to delete the row. Kept here so the controls below can prove each widening is
# load-bearing: a control the old pattern also catches would pass on the broken gate and prove
# nothing.
#
# Spelled out, not composed. A frozen control assembled from the live pieces is not frozen -- it
# widens whenever they widen, and "the old matcher misses this" silently becomes "the new matcher
# misses this", which is the opposite claim. That is exactly how `check-stale-rva-calls.py`'s
# controls nearly stopped proving anything.
def legacy_names_the_address(rva, text):
    """The pre-2026-08-30 test, verbatim: a hex literal in a `const NAME: usize = ...;`."""
    return re.findall(rf"const [A-Z0-9_]+: *usize *= *0x{rva:x}\b", text, re.I)


ENUM_ONLY_SOURCE = """
#[repr(u32)]
pub enum MenuTraceRva {
    TaskEnqueue = 0x007a7b60,
    MenuJobWait = 0x00b0d400,
}
pub const TITLE_MENU_JOB_WAIT_RVA: usize = MenuTraceRva::MenuJobWait as usize;
"""


def selftest():
    import tempfile

    failures = []

    def check(name, got, want):
        if got != want:
            failures.append(f"{name}: got {got!r}, want {want!r}")

    def tree(files):
        scratch = tempfile.mkdtemp()
        for name, body in files.items():
            path = os.path.join(scratch, name)
            os.makedirs(os.path.dirname(path), exist_ok=True)
            open(path, "w", encoding="utf-8").write(body)
        return Index.build(root=scratch)

    # The control this module exists for. An address declared only as an enum discriminant. The
    # old matcher cannot see it, and on that silence the gate recommended deleting the row.
    check(
        "the OLD matcher misses an enum-discriminant address (control is non-vacuous)",
        legacy_names_the_address(0xB0D400, ENUM_ONLY_SOURCE),
        [],
    )
    enum_index = tree({"crates/a/src/lib.rs": ENUM_ONLY_SOURCE})
    found = enum_index.claims(0xB0D400)
    check(
        "an enum discriminant IS found",
        sorted(d.qualified for d in found.declarations),
        ["MenuTraceRva::MenuJobWait", "TITLE_MENU_JOB_WAIT_RVA"],
    )
    check("...so the address is not unclaimed", found.proven_unclaimed, False)
    check("...and it is not merely 'found nothing'", found.found_nothing, False)

    # The other declaration forms, each one a spelling that defeated a matcher in this tree.
    forms = tree(
        {
            "crates/a/src/lib.rs": (
                "pub const PLAIN_RVA: usize = 0x111000;\n"
                "pub const VA_FORM_RVA: usize = 0x140222000;\n"
                "const DERIVED_RVA: usize = PLAIN_RVA + 0x10;\n"
                "pub const TABLE: [usize; 2] = [0x333000, 0x444000];\n"
                "pub const BAND: core::ops::Range<usize> = 0x555000..0x556000;\n"
                "#[repr(usize)] pub enum E { A = 0x666000, B, }\n"
                "static STATIC_RVA: u32 = 0x777000;\n"
            ),
            "crates/b/src/lib.rs": (
                "use crate::a::PLAIN_RVA as RENAMED;\n"
                "pub const VIA_ALIAS_RVA: usize = RENAMED;\n"
                "const THROUGH_MODULE: usize = a::VA_FORM_RVA;\n"
                "fn f() { let spec = HookSpec { rva: 0x888000 }; }\n"
            ),
        }
    )
    for label, address, want in (
        ("a plain literal const", 0x111000, True),
        ("an address written as a VA", 0x222000, True),
        ("a const derived by arithmetic from another", 0x111010, True),
        ("an element of a const array", 0x444000, True),
        ("an address inside a Range band", 0x555800, True),
        ("an enum discriminant", 0x666000, True),
        ("an IMPLICIT enum discriminant (previous + 1)", 0x666001, True),
        ("a static, not a const", 0x777000, True),
        ("a bare literal in a table field, with no constant name", 0x888000, True),
        ("an address nothing declares", 0x999000, False),
    ):
        got = forms.claims(address)
        check(label, bool(got.declarations or got.literals), want)

    check(
        "an alias is reported as another name for a claimed address",
        [a for a, _ in forms.claims(0x111000).aliases],
        ["RENAMED"],
    )
    check(
        "the VA spelling and the RVA spelling are the same address",
        bool(forms.claims(0x140222000).declarations),
        True,
    )

    # Proof, not silence. A tree the resolver fully understands may report an address unclaimed.
    clean = tree({"crates/a/src/lib.rs": "pub const ONLY_RVA: usize = 0x111000;\n"})
    check("a fully-resolved tree can PROVE an address unclaimed", clean.claims(0x999000).proven_unclaimed, True)

    # ...and one it does not fully understand may not, even though it found nothing. This is the
    # whole point: the two answers must not print the same sentence.
    murky = tree(
        {
            "crates/a/src/lib.rs": (
                "pub const ONLY_RVA: usize = 0x111000;\n"
                "pub const OPAQUE_RVA: usize = some_fn(SOMETHING_ELSE);\n"
            )
        }
    )
    unknown = murky.claims(0x999000)
    check("an unresolvable declaration is residue", len(unknown.residue), 1)
    check("...so the address is NOT proven unclaimed", unknown.proven_unclaimed, False)
    check("...even though nothing was found", unknown.found_nothing, True)

    # A non-address type does not pollute the residue: a `&str` cannot be an address, and letting
    # one block the proof forever would make the safe answer useless rather than safe.
    strings = tree(
        {
            "crates/a/src/lib.rs": (
                "pub const ONLY_RVA: usize = 0x111000;\n"
                'pub const NAME: &str = concat!("a", "b");\n'
            )
        }
    )
    check("a &str const is outside the address universe", strings.claims(0x999000).proven_unclaimed, True)

    # An empty walk is not a clean walk. A scan that read nothing must never look like proof.
    empty = tree({})
    check("a walk that read no files cannot prove anything", empty.claims(0x111000).proven_unclaimed, False)

    # Prose is not a claim. All three collision addresses appear in crates/ only inside doc
    # comments that describe the collision; counting those would make every address look live.
    prose = tree(
        {
            "crates/a/src/lib.rs": (
                "pub const ONLY_RVA: usize = 0x111000;\n"
                "/// the collision shape (`0x6156c0`, `0x7ad710`) bit us once\n"
                'const NOTE: &str = "0x7ad710";\n'
            )
        }
    )
    check("a doc comment does not claim an address", prose.claims(0x7AD710).proven_unclaimed, True)

    # Ambiguity is unresolved, not a guess -- but only real ambiguity. A name declared in the
    # same file as the use resolves there; a name that only two other files declare, differently,
    # does not, and the constant built on it stays residue rather than taking a number on faith.
    ambiguous = tree(
        {
            "crates/a/src/lib.rs": "pub const DUP: usize = 0x1000;\n",
            "crates/b/src/lib.rs": "pub const DUP: usize = 0x2000;\n",
            "crates/c/src/lib.rs": "const X: usize = DUP + 1;\n",
        }
    )
    check(
        "a name two other files declare differently does not resolve",
        [d.symbol for d in ambiguous.claims(0x999000).residue],
        ["X"],
    )
    scoped = tree(
        {
            "crates/a/src/lib.rs": "pub const DUP: usize = 0x1000;\nconst X: usize = DUP + 1;\n",
            "crates/b/src/lib.rs": "pub const DUP: usize = 0x2000;\n",
        }
    )
    check(
        "...while the file that declares it resolves its own",
        bool(scoped.claims(0x1001).declarations),
        True,
    )

    # The real tree. A resolver that only ever runs against its own fixtures is a fixture.
    live = index()
    check("the real walk reads a tree", live.files_read > 200, True)
    check("the real universe is populated", live.universe_size() > 500, True)
    real = live.claims(0xB0D400)
    check(
        "0xb0d400 is claimed in the real tree (the case the old matcher called unclaimed)",
        sorted({d.qualified for d in real.declarations}),
        ["MenuTraceRva::MenuJobWait", "TITLE_MENU_JOB_WAIT_RVA"],
    )
    check("...and it has live use sites", sum(len(u) for u in real.uses.values()) > 0, True)

    # The four that blocked `proven_unclaimed` for every `.text` address, 2026-08-30. Frozen
    # expected values, hand-derived independently of this resolver (never by calling it and
    # asserting the result equals itself, which would let a widened matcher grade its own
    # homework):
    #   * SP_EFFECT_PARAM_INDEX -- `solo_params!` in fromsoftware-rs's `solo_param_repository.rs`
    #     lists `(SpEffectParam, SP_EFFECT_PARAM_ST, 15)`: row 15, read by source inspection.
    #   * CHR_ASM_UNKD4_OFFSET / CHR_ASM_UNKD8_OFFSET -- `chr_asm_layout.rs`'s own doc comments
    #     name +0xd4/+0xd8 from the ctor's `movq $-1,0xd4(%rsi)` (deobf 0x1403be208), independent
    #     disassembly evidence that predates this fix and does not depend on it.
    #   * CHR_ASM_ENTRY_COUNT -- the same file's `CHR_ASM_EQUIPMENT_ENTRY_COUNT` is independently
    #     pinned to 22 by the ctor's `mov $0x16,%ecx` (deobf 0x1403be213), and 0x16 == 22.
    frozen_values = {
        "SP_EFFECT_PARAM_INDEX": 15,
        "CHR_ASM_UNKD4_OFFSET": 0xD4,
        "CHR_ASM_UNKD8_OFFSET": 0xD8,
        "CHR_ASM_ENTRY_COUNT": 22,
    }
    for name, expected in frozen_values.items():
        decls = live.by_simple.get(name, [])
        check(f"{name} is declared exactly once in the real tree", len(decls), 1)
        if len(decls) == 1:
            check(f"{name} EVALUATES to its real (frozen) value", decls[0].value, {expected})

    # Proof, not silence -- On the real tree. Before this fix, `proven_unclaimed` was False for
    # every `.text`-scale address (all four sat in every query's residue, wide enough to hold
    # anything). An address nothing declares must now prove unclaimed...
    check(
        "an undeclared .text-scale address is now PROVEN unclaimed on the real tree",
        live.claims(0x6F6F6F6F).proven_unclaimed,
        True,
    )
    # ...while a real address this fix's own new machinery resolved (CHR_ASM_UNKD4_OFFSET's own
    # computed value, reached only through offset_of!/size_of! evaluation -- a non-literal path)
    # correctly still reports claimed, not unclaimed. The fix must not make its own targets look
    # free.
    unkd4_claims = live.claims(0xD4)
    check(
        "...while an address claimed through the NEW offset_of!-evaluation path stays CLAIMED",
        any(d.qualified == "CHR_ASM_UNKD4_OFFSET" for d in unkd4_claims.declarations),
        True,
    )
    check("...so THAT address is not proven unclaimed", unkd4_claims.proven_unclaimed, False)

    # The mechanism, on a fixture -- so this control does not depend on fromsoftware-rs's current
    # content, which can drift independently of this test. Mirrors the shape that blocked the real
    # CHR_ASM offsets: offset_of! through a nested #[repr(C)] struct, an array of a bitfield-
    # wrapped primitive newtype, and a fieldless #[repr(u32)] enum field. Expected offsets are
    # hand-computed C layout (sequential fields, each aligned to its own alignment), not obtained
    # by calling the resolver and comparing it to itself:
    #   unk0:i32 @0 (+4) -> style:u32(enum) @4, slots:[u32;3] @8(+12)=Equip size 16 -> equip @4
    #   (+16=20) -> handles:[Handle(bitfield u32);5] @20(+20=40) -> ids:[i32;5] @40(+20=60) ->
    #   unkd4:u32 @60. So IDS_OFFSET=40 (0x28), UNKD4_OFFSET=40+5*4=60 (0x3c).
    layout_fixture = tree(
        {
            "crates/a/src/lib.rs": (
                "#[repr(u32)]\n"
                "pub enum ArmStyle { EmptyHanded = 0, OneHanded = 1 }\n"
                "#[repr(C)]\n"
                "pub struct Equip {\n"
                "    pub style: ArmStyle,\n"
                "    pub slots: [u32; 3],\n"
                "}\n"
                "bitfield! {\n"
                "    pub struct Handle(u32);\n"
                "    impl Debug;\n"
                "    pub index, _: 15, 0;\n"
                "}\n"
                "#[repr(C)]\n"
                "pub struct Asm {\n"
                "    unk0: i32,\n"
                "    pub equip: Equip,\n"
                "    pub handles: [Handle; 5],\n"
                "    pub ids: [i32; 5],\n"
                "    unkd4: u32,\n"
                "}\n"
                "pub const IDS_OFFSET: usize = core::mem::offset_of!(Asm, ids);\n"
                "pub const UNKD4_OFFSET: usize = IDS_OFFSET"
                " + 5 * core::mem::size_of::<i32>();\n"
            )
        }
    )
    check(
        "offset_of! through struct/array/bitfield/enum chain EVALUATES on a fixture",
        layout_fixture.claims(0x28).proven_unclaimed,  # 40 decimal
        False,
    )
    check(
        "...to the hand-computed offset, not a guess",
        {d.qualified for d in layout_fixture.claims(0x28).declarations},
        {"IDS_OFFSET"},
    )
    check(
        "...and the arithmetic built on it (offset + count * size_of) is ALSO evaluated",
        {d.qualified for d in layout_fixture.claims(0x3C).declarations},  # 60 decimal
        {"UNKD4_OFFSET"},
    )

    # The macro path, directly -- `solo_params!` is only read from the fromsoftware-rs sibling
    # (`_read_external`), which a `tree()` fixture's synthetic root does not stand in for. Exercise
    # `_read_solo_params` on its own fixture text instead: still evaluation of real source shape,
    # just not routed through the sibling-directory walk for this control.
    solo_probe = Index()
    solo_probe._read_solo_params(
        "<fixture>",
        "solo_params!(\n    (Alpha, ALPHA_ST, 3),\n    (Beta, BETA_ST, 7),\n);\n",
    )
    check(
        "solo_params! tuple -> Type::INDEX, read from the macro invocation's own arguments",
        solo_probe.external_consts.get("Alpha::INDEX"),
        {3},
    )
    check(
        "...every row in the same invocation, not just the first",
        solo_probe.external_consts.get("Beta::INDEX"),
        {7},
    )

    for failure in failures:
        print(f"rva_symbols selftest FAILED -- {failure}", file=sys.stderr)
    if failures:
        return 1
    print(
        f"rva_symbols selftest: OK ({live.files_read} sources, {len(live.decls)} declarations, "
        f"{live.universe_size()} address-capable, {len(live.residue())} unresolved)"
    )
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("address", nargs="*", help="an RVA or VA, e.g. 0xb0d400 or 0x140b0d400")
    parser.add_argument("--residue", action="store_true", help="list what could not be evaluated")
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    if args.selftest:
        return selftest()
    live = index()
    if args.residue:
        # With an address, the residue is the one that matters: a `u8` cannot hold 0x7ad710, so a
        # byte-string constant is not a declaration that "might be it". Without one, the whole
        # unevaluated set is printed, which is always the larger number.
        if args.address:
            for text in args.address:
                value = int(text, 16 if text.lower().startswith("0x") else 0)
                residue = live.claims(value).residue
                print(
                    f"\n0x{value:x}: {len(residue)} of {live.universe_size()} address-capable "
                    f"declaration(s) could not be evaluated AND could hold this address."
                )
                for decl in residue:
                    print(
                        f"  {decl.where()}  {decl.qualified}: {decl.type_text} = "
                        f"{' '.join(decl.expr.split())[:70]}"
                    )
            return 0
        residue = live.residue()
        print(
            f"{len(residue)} of {live.universe_size()} address-capable declaration(s) could not be "
            f"evaluated at all.\nWhile this is non-empty, no address can be PROVEN unclaimed. Pass "
            f"an address as well to see\nonly the ones wide enough to hold it, which is a much "
            f"shorter list."
        )
        for decl in residue:
            print(
                f"  {decl.where()}  {decl.qualified}: {decl.type_text} = "
                f"{' '.join(decl.expr.split())[:70]}"
            )
        return 0
    if not args.address:
        parser.error("give an address, or --residue / --selftest")
    for text in args.address:
        value = int(text, 16 if text.lower().startswith("0x") else 0)
        print(f"\n0x{value:x}:")
        describe_claims(live.claims(value))
    return 0


if __name__ == "__main__":
    sys.exit(main())
