#!/usr/bin/env python3
"""Route every read of a game global through the build gate, mechanically.

A stale call announces itself -- 1.16.2's `0x1405eefb0` is mid-instruction on 1.17 and the
process dies on the spot. A stale read does not. Every `.data` global moved between the
builds, so `safe_read_usize(base + FOO_RVA)` succeeds on 1.17 and returns whatever now
occupies the old slot. Two of those were measured: a garbage repository pointer that made
`CreateTpfResCap` divide by zero 894ms into boot, and a stale swapchain root that left a
live process behind a black screen for twenty seconds.

There were 73 such reads, and unlike call sites they need no per-site decision: they are
already fault-tolerant and already have a "this global is not there" branch.
`game_data_addr` returns 0 on refusal, the read fails, and the existing branch runs. So the
rewrite is uniform and this script does it.

It deliberately does not touch call sites. Zero is a safe address to fail a read at and a
fatal one to jump to; a `transmute` needs its author to say what refusing means.

What it could not see until 2026-08-30, and why "0 sites" was not the same as "none left"
-----------------------------------------------------------------------------------------
`check-stale-rva-calls.py` -- the gate that counts what this tool converts -- was widened
three times that day, each time because a real ungated read had been standing in a spelling
its regex did not admit. This sweeper was never widened with it, so the two diverged: the
gate could see a site and the sweeper could not rewrite it, while the sweeper's own summary
line said `would rewrite 0 read site(s)` -- which reads as "there is nothing left to do".
Measured on this tree the same day: the widened pattern finds eight reads the narrow one
misses, in six crates. The narrow shape demanded, and the widening dropped:

  A name.        `[A-Z0-9_]*RVA[A-Z0-9_]*` required the constant to be spelled `*RVA*`. What
                 makes a read stale is the arithmetic -- a module base plus a compile-time
                 constant -- and the arithmetic does not care what the constant is called.
                 All eight sites this now sees are named `PE_*_LFANEW_OFFSET`, and see the
                 value gate below for why they are then excluded rather than rewritten.
  A literal base `own_stepper_idx10_fallbacks!` hands the module base to its body as a macro
                 metavariable, so every read inside reads `$base + FOO_RVA`. One `$` hid a
                 live 1.17 defect from the sibling gate.
  No qualifier.  `jp::GAME_MAN_GLOBAL_RVA` and `ProfileLoadMenuRva::Slot as usize` are both
                 `base + <constant>`; neither is a bare SCREAMING_SNAKE token.

And it read prose. The old matcher ran over raw file text, so a `//` paragraph or a `///`
doc comment quoting `safe_read_usize(base + FOO_RVA)` was a rewrite target -- this tool
edits, so a false positive here does not merely inflate a count, it rewrites an English
sentence into code that describes nothing. `gate-stale-rva-calls.py`, its sibling, was
measured doing exactly that on 2026-08-30 (two hits, both comments, one of them inside the
paragraph explaining the hazard). Comments and string bodies are now blanked -- through the
shared `rva_symbols.code_only`, so there is one dialect rather than a fourth -- and the
rewrite is spliced into the original text at the offsets that reader reports, which it can
do because the blanking preserves offsets exactly.

The value gate, and why widening without it would have corrupted six crates
---------------------------------------------------------------------------
Dropping the name filter admits `safe_read_u32(base + PE_DOS_LFANEW_OFFSET)`: the DOS
header's `0x3c` field, read to find the NT header. That is `base + constant` arithmetic and
it does match. It is also not an address the game map knows: routing it through
`game_data_addr` gets a refusal, `game_data_addr` returns 0, and the crash logger's PE walk
reads offset 0x3c of nothing. Eight sites across six crates, every one of them a working PE
header read, and the widened pattern hits all eight.

So the exclusion is by value -- below `.text`'s 0x1000, therefore fixed by the PE format and
unable to move between builds -- and never by name, which is the same discipline
`check-stale-rva-calls.py` applies to the same eight constants. A constant whose value cannot
be resolved is rewritten rather than skipped, because a read is fault-tolerant by
construction: the failure mode of gating one that did not need it is a refusal, and the
failure mode of skipping one that did is the silent wrong pointer this tool exists to remove.

    python3 scripts/sweep-stale-rva-reads.py            # what it would rewrite
    python3 scripts/sweep-stale-rva-reads.py --apply
    python3 scripts/sweep-stale-rva-reads.py --selftest
"""

from __future__ import annotations

import argparse
import os
import re
import sys
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
# One dialect, not four. `code_only` lives in `scripts/rva_symbols.py` so this sweeper, its
# call-site sibling and both gates blank comments and string bodies the same way. A tool that
# edits source must never match inside prose, and a local copy is how the four drift apart.
try:  # noqa: E402 - repo-local; the sys.path line above is what makes it work
    from rva_symbols import code_only
    import rva_symbols
except ImportError as missing:  # a shared reader that cannot load must stop the tool, not degrade
    raise ImportError(
        "scripts/rva_symbols.py could not be imported, so comments and string bodies cannot be "
        "blanked before matching and constants cannot be resolved to values. Without the first "
        "this tool rewrites doc comments into code; without the second it rewrites eight working "
        "PE-header reads into refusals. Fix the import rather than restoring a local copy."
    ) from missing

# `.text` begins at RVA 0x1000 on both 1.16.2 and 1.17. Below it is the DOS stub and the PE
# headers, whose layout the PE specification fixes and no game patch can move.
PE_HEADER_LIMIT = 0x1000

# The base. `\$?` because a macro body spells it `$base`; the `$` is punctuation, and one of them
# was enough to hide a live 1.17 crash from the sibling gate.
BASE_EXPR = r"\$?(?:base|module_base|game_base|image_base)"
# The constant, whole, so the rewrite can put it back verbatim. `expr` keeps any module path and
# any trailing `as usize` (the constant may be a `u32`, and dropping the cast is a type error);
# `name` is the last path segment, which is what the log label should say.
CONSTANT = (
    r"(?P<expr>(?:[A-Za-z_][A-Za-z0-9_]*\s*::\s*)*(?P<name>[A-Z][A-Za-z0-9_]*)"
    r"(?:\s+as\s+usize)?)"
)
# `safe_read_u32(base + FOO_RVA)` / `read_bytes(base + FOO_RVA, ...)`. The base expression is
# captured so the rewrite preserves whichever local name -- or metavariable -- the site uses.
READ = re.compile(
    r"\b(?P<fn>safe_read_(?:usize|u64|u32|u16|u8|i32|i64|cstr|bytes)|read_bytes)"
    r"\(\s*(?P<base>" + BASE_EXPR + r")\s*\+\s*" + CONSTANT + r"\s*(?P<tail>[,)])"
)

# The MATCHER this replaced, frozen as a literal. The controls in `--selftest` have to prove each
# widening is load-bearing, and a control the old pattern also catches would pass on the broken
# tool and prove nothing.
#
# Spelled out, not composed from `BASE_EXPR`/`CONSTANT`. A frozen control assembled from the live
# pieces is not frozen: it widens whenever they widen, so "the old pattern misses this" silently
# becomes "the new pattern misses this", which is the opposite claim. That is precisely how
# `check-stale-rva-calls.py`'s controls nearly stopped proving anything.
LEGACY_READ = re.compile(
    r"\b(safe_read_(?:usize|u64|u32|u16|u8|i32|i64|cstr|bytes)|read_bytes)"
    r"\(\s*(base|module_base|game_base|image_base)\s*\+\s*([A-Z0-9_]*RVA[A-Z0-9_]*)\s*([,)])"
)


def constant_values() -> dict:
    """`{simple name: value}` for every address-capable constant `rva_symbols` could evaluate.

    Ambiguous names -- declared twice with different values -- map to `None`, and so does a name
    that was never resolved at all. `is_finding` then treats both as "not proven safe", which is
    the only honest reading: "I could not read it" must not be spelled the same way as "I read it
    and it is a PE header field".
    """
    index = rva_symbols.index()
    out: dict = {}
    for decl in index.decls:
        if not decl.value:
            continue
        for value in decl.value:
            if decl.symbol in out and out[decl.symbol] != value:
                out[decl.symbol] = None
            else:
                out.setdefault(decl.symbol, value)
    return out


def is_finding(constant: str, values: dict) -> bool:
    """Is `base + constant` a stale-address read? Unresolvable means yes -- see the module doc."""
    value = values.get(constant)
    return value is None or value >= PE_HEADER_LIMIT


def rewrite(text: str, in_game_base: bool, values: dict | None = None) -> tuple[str, int, list]:
    """`(rewritten text, sites rewritten, sites excluded by value)`.

    Matching runs over the comment/string-blanked view and the replacement is spliced into the
    original text at the same offsets, which `code_only` guarantees are the same offsets. Doing
    it the other way round -- substituting into the blanked text -- would return a file with its
    documentation deleted, and doing it over the raw text rewrites the documentation instead.
    """
    values = {} if values is None else values
    path = "crate::mem::game_data_addr" if in_game_base else "er_game_base::mem::game_data_addr"
    code = code_only(text)
    pieces, cursor, gated, excluded = [], 0, 0, []
    for match in READ.finditer(code):
        name = match.group("name")
        if not is_finding(name, values):
            excluded.append((name, values.get(name), code[: match.start()].count("\n") + 1))
            continue
        pieces.append(text[cursor : match.start()])
        pieces.append(
            f"{match.group('fn')}({path}({match.group('base')}, {match.group('expr')}, "
            f'"{name}"){match.group("tail")}'
        )
        cursor = match.end()
        gated += 1
    pieces.append(text[cursor:])
    return "".join(pieces), gated, excluded


def selftest() -> int:
    failures = []

    def check(name, condition):
        if not condition:
            failures.append(name)

    # The baseline behaviour, unchanged.
    got, n, _ = rewrite("let x = unsafe { safe_read_usize(base + GAME_MAN_RVA) }?;", False)
    want = (
        'let x = unsafe { safe_read_usize(er_game_base::mem::game_data_addr(base, GAME_MAN_RVA, '
        '"GAME_MAN_RVA")) }?;'
    )
    check(f"read rewrite produced {got!r}", got == want and n == 1)
    call = "let f: Fn = unsafe { core::mem::transmute(base + SOME_RVA) };"
    check("a transmute call site was rewritten; only reads are safe to zero", rewrite(call, False)[1] == 0)
    check("rewriting is not idempotent", rewrite(want, False)[1] == 0)

    # ---------------------------------------------------------------- Positive controls
    # Each one is a spelling that stood in this tree, or in its sibling gate, while the narrow
    # matcher reported nothing. Each is asserted visible to the current pattern and invisible to
    # the frozen legacy one -- a control both catch would pass on the broken tool.

    # 1 -- a constant that never carried the `_RVA` suffix. All eight sites the widening newly
    # sees in this tree are of this shape.
    unsuffixed = "let n = unsafe { safe_read_u32(base + PE_DOS_LFANEW_OFFSET) };"
    check("must see a read whose constant is not named *RVA*", len(READ.findall(unsuffixed)) == 1)
    check("...control is vacuous unless the OLD name-filtered pattern misses it",
          LEGACY_READ.findall(unsuffixed) == [])

    # 2 -- a macro body. One `$` hid a live 1.17 defect from `check-stale-rva-calls.py`.
    macro_body = "let g = unsafe { safe_read_usize($base + WORLD_CHR_MAN_RVA) };"
    check("must see a read written against a macro metavariable base", len(READ.findall(macro_body)) == 1)
    check("...control is vacuous unless the OLD `$`-blind pattern misses it",
          LEGACY_READ.findall(macro_body) == [])
    rewritten, n, _ = rewrite(macro_body, False, {"WORLD_CHR_MAN_RVA": 0x3D86BD8})
    check("a macro metavariable base survives the rewrite verbatim",
          n == 1 and "game_data_addr($base, WORLD_CHR_MAN_RVA" in rewritten)

    # 3 -- a module-qualified constant. `jp::GAME_MAN_GLOBAL_RVA` was read raw two lines below a
    # sibling that resolved correctly.
    qualified = "let g = unsafe { safe_read_usize(base + jp::GAME_MAN_GLOBAL_RVA) };"
    check("must see a read through a module-qualified constant", len(READ.findall(qualified)) == 1)
    check("...control is vacuous unless the OLD unqualified pattern misses it",
          LEGACY_READ.findall(qualified) == [])
    rewritten, n, _ = rewrite(qualified, False, {"GAME_MAN_GLOBAL_RVA": 0x3D86BD8})
    check("the module path is preserved and the label is the last segment",
          n == 1
          and "game_data_addr(base, jp::GAME_MAN_GLOBAL_RVA, \"GAME_MAN_GLOBAL_RVA\")" in rewritten)

    # 4 -- an ENUM variant with an `as usize`, which is how er-title-flow spells most addresses.
    # The cast must survive: the discriminant is a `u32` and dropping it is a type error.
    enum_variant = "let g = unsafe { safe_read_usize(base + MenuTraceRva::MenuJobWait as usize) };"
    check("must see a read whose address is an enum variant", len(READ.findall(enum_variant)) == 1)
    check("...control is vacuous unless the OLD SCREAMING_SNAKE-only pattern misses it",
          LEGACY_READ.findall(enum_variant) == [])
    rewritten, n, _ = rewrite(enum_variant, False, {"MenuJobWait": 0xB0D400})
    check("the `as usize` cast survives the rewrite",
          n == 1 and "MenuTraceRva::MenuJobWait as usize," in rewritten)

    # ---------------------------------------------------------------- Prose
    # A tool that edits must never match inside a comment or a string. This control runs the other
    # way round from the four above: the old matcher catches it (and would have rewritten an
    # English sentence), the new one must not. `gate-stale-rva-calls.py` was measured doing this.
    prose = (
        "/// Every global here used to be a bare `safe_read_usize(base + SOME_RVA)`.\n"
        "// safe_read_u32(base + IN_A_COMMENT_RVA) is the shape, not a site\n"
        'let quoted = "safe_read_usize(base + QUOTED_RVA)";\n'
        "let real = unsafe { safe_read_usize(base + REAL_SITE_RVA) };"
    )
    check("the OLD matcher read the three prose lines as sites too (control is non-vacuous)",
          len(LEGACY_READ.findall(prose)) == 4)  # 3 prose + the one real line
    check("the current matcher reads only the code line",
          [m.group("name") for m in READ.finditer(code_only(prose))] == ["REAL_SITE_RVA"])
    rewritten, n, _ = rewrite(prose, False, {"REAL_SITE_RVA": 0x3D86BD8})
    check("the doc comment survives the rewrite byte for byte",
          n == 1 and "used to be a bare `safe_read_usize(base + SOME_RVA)`" in rewritten)

    # ---------------------------------------------------------------- The value gate
    # A PE header field is excluded because of what it is -- below `.text`, fixed by the file
    # format -- and never because of what it is called. Without this the widening above would
    # have rewritten eight working PE walks into refusals across six crates.
    values = {"PE_DOS_LFANEW_OFFSET": 0x3C, "CSDLC_SINGLETON_RVA": 0x3D86BD8, "AMBIGUOUS": None}
    check("a PE header offset is not a game address", not is_finding("PE_DOS_LFANEW_OFFSET", values))
    check("a real .data RVA is a finding", is_finding("CSDLC_SINGLETON_RVA", values))
    check("a constant declared twice with different values is kept", is_finding("AMBIGUOUS", values))
    check("a constant that could not be resolved is kept", is_finding("NEVER_DECLARED", values))
    header_read = "let n = unsafe { safe_read_u32(base + PE_DOS_LFANEW_OFFSET) };"
    out, n, excluded = rewrite(header_read, False, values)
    check("a PE header read is SEEN but not rewritten",
          n == 0 and out == header_read and [e[0] for e in excluded] == ["PE_DOS_LFANEW_OFFSET"])

    # ---------------------------------------------------------------- Non-VACUITY, of the inputs
    # Every set this tool reasons from is asserted non-empty and of the right order of magnitude
    # before anything is concluded from it. `would rewrite 0 site(s)` is the goal state of a
    # migration and a bug in a walk, and only one of those is good news; without these two the
    # summary line cannot tell them apart.
    repo = Path(__file__).resolve().parent.parent
    sources = sorted(repo.glob("crates/**/*.rs"))
    check(f"only {len(sources)} .rs files under crates/; the walk is broken", len(sources) > 200)
    declared = constant_values()
    check(f"only {len(declared)} constants resolved; the value gate is unfounded", len(declared) > 500)
    seen = excluded_total = 0
    for source in sources:
        text = source.read_text(encoding="utf-8", errors="replace")
        seen += len(READ.findall(code_only(text)))
        excluded_total += len(rewrite(text, "er-game-base" in source.parts, declared)[2])
    check(f"the widened pattern sees {seen} read(s) in this tree; it saw 8 when written, so the "
          "walk or the pattern has broken", seen >= 8)
    check(f"{excluded_total} of them were excluded by value; all 8 known ones are PE header "
          "reads, so a drop to zero means the value gate stopped running", excluded_total >= 8)

    for line in failures:
        print(f"SELFTEST FAIL: {line}")
    print(
        f"selftest: {len(failures)} failure(s) "
        f"({len(sources)} sources, {len(declared)} constants resolved, {seen} read site(s) seen, "
        f"{excluded_total} excluded by value)"
    )
    return 1 if failures else 0


def main() -> int:
    repo = Path(__file__).resolve().parent.parent
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--apply", action="store_true")
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()

    if args.selftest:
        return selftest()

    values = constant_values()
    total, touched, excluded = 0, [], []
    for path in sorted(repo.glob("crates/**/*.rs")):
        text = path.read_text(encoding="utf-8")
        new, n, skipped = rewrite(text, "er-game-base" in path.parts, values)
        excluded.extend((path.relative_to(repo), name, value, line) for name, value, line in skipped)
        if n:
            total += n
            touched.append((path.relative_to(repo), n))
            if args.apply:
                path.write_text(new, encoding="utf-8")
    for rel, n in touched:
        print(f"  {n:3d}  {rel}")
    verb = "rewrote" if args.apply else "would rewrite"
    print(f"{verb} {total} read site(s) across {len(touched)} file(s)")
    if excluded:
        # Not a footnote. These are `base + constant` reads the pattern sees; saying only
        # "would rewrite 0" would let a walk that found nothing and a tree with nothing left to
        # find print the same line.
        print(
            f"{len(excluded)} further read site(s) matched and were EXCLUDED BY VALUE -- the "
            "constant resolves below 0x1000, so it is a PE header field the format fixes and no "
            "game patch can move:"
        )
        for rel, name, value, line in excluded:
            print(f"  {rel}:{line}\t{name} = 0x{value:x}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
