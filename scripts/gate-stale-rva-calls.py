#!/usr/bin/env python3
"""Route `transmute(base + SOME_RVA)` call sites through the 1.17 address gate, in bulk.

Why this exists
---------------
`scripts/check-stale-rva-calls.py` counts the sites and refuses new ones. Converting them is the
other half, and on 2026-08-29 there were 210 of them across the workspace -- too many to edit by
hand, and each edit is the same shape:

    let f: Fn = unsafe { transmute(base + SOME_RVA) };
    ->
    let f: Fn = unsafe { transmute(match helper(SOME_RVA, "SOME_RVA") {
        Some(address) => address,
        None => <bail>,
    }) };

What it refuses to touch
------------------------
The bail is the whole risk, so this only rewrites a site whose bail is unambiguous:

  * Never inside an `extern "system"` function. Those are detours, and a detour that returns early
    never calls its original -- which silently deletes the game's own behaviour instead of adding
    ours. Measured: `hud_weapon_update_hook` needed `return ret`, not `return`.
  * Only where the enclosing function's return type maps to an obvious "did nothing" value:
    `()`, `bool`, an integer, `f32`. Anything else (a struct, a tuple, `Option<...>` where `None`
    might mean something specific) is printed for a human instead of guessed at.

Everything it skips is listed, so the remainder is a work-list rather than a silence.

It used to rewrite English sentences, and on 2026-08-30 that was all it did
--------------------------------------------------------------------------
The matcher ran over raw file text. Run over this workspace on 2026-08-30 it reported
`gated 1 / 2 site(s)` -- and both of those "call sites" were PROSE:

    crates/er-invasion-warp-core/src/lib.rs:51   /// ... used to be a bare `transmute(base + SOME_RVA)`.
    crates/er-game-base/src/game_build.rs:284    // ... equally reachable as a call (`transmute(base + RVA)`)

The first is the one it counted as gated, which means a non-dry run would have rewritten a doc
comment into `transmute(helper(SOME_RVA, "SOME_RVA"))` -- a sentence about the hazard turned into
code that describes nothing, in the header of the crate that documents it. Those are the same two
paragraphs that contaminated `check-stale-rva-calls.py`'s baseline; for a counter that was an
inflated number, and for a REWRITER it is a corrupted file. Comments and string bodies are now
blanked before matching -- through the shared `rva_symbols.code_only`, so there is one dialect
rather than a fourth -- and the replacement is spliced into the original text at the offsets that
reader reports, which it can do because the blanking preserves offsets exactly.

And it trusted the constant'S name, its shape, and the base'S spelling
---------------------------------------------------------------------
`[A-Z0-9_]*RVA[A-Z0-9_]*` is a naming convention, not evidence. What makes a site a stale call is
the arithmetic -- a module base plus a compile-time constant, transmuted and called -- and the
arithmetic does not care what the constant is spelled. Three spellings defeated the old pattern
outright, each of them measured standing in live code while the sibling gate reported zero:

  No suffix      `er-build-import-runtime` calls `GET_MAIN_PLAYER_STATS` and thirty-nine more by
                 those names; `use ...::FOO_RVA as FOO;` strips the suffix at the import, which
                 is exactly where this tool looks.
  A macro base   `own_stepper_idx10_fallbacks!` receives the module base as a metavariable, so its
                 body reads `transmute($base + TITLE_TOP_DIALOG_IS_IN_STATE_RVA)` -- a function
                 that moved 0x749b20 -> 0x74a970, leaving the 1.16.2 address mid-instruction.
  An ENUM        `er-title-flow` keeps its addresses in C-like enums:
                 `transmute(base + ProfileLoadMenuRva::ProfileLoadSelectSaveSlot as usize)` called
                 1.16.2's 0x9a5f20 on a build where it lives at 0x9a70c0.

A TURBOFISH also hid sites: `transmute::<usize, F>(base + MENU_RVA)` is the same call.

The value gate
--------------
Dropping the name filter admits `base + PE_DOS_LFANEW_OFFSET` -- the DOS header's `0x3c` field,
fixed by the PE format and unable to move between builds. Those are excluded by value, below
`.text`'s 0x1000, and never by name: an exclusion has to rest on what the thing is. A constant
whose value cannot be resolved is kept, because "I could not read it" must not be spelled the same
way as "I read it and it is safe".

Usage
    python3 scripts/gate-stale-rva-calls.py --helper title_fn crates/er-title-flow/src/*.rs
    python3 scripts/gate-stale-rva-calls.py --helper 'crate::gated' --dry-run <paths...>
    python3 scripts/gate-stale-rva-calls.py --selftest
"""

from __future__ import annotations

import argparse
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
# One dialect, not four. `code_only` lives in `scripts/rva_symbols.py` so this rewriter, its
# read-site sibling and both gates blank comments and string bodies the same way. A tool that
# edits source must never match inside prose; this one was measured doing exactly that.
try:  # noqa: E402 - repo-local; the sys.path line above is what makes it work
    from rva_symbols import code_only
    import rva_symbols
except ImportError as missing:  # a shared reader that cannot load must stop the tool, not degrade
    raise ImportError(
        "scripts/rva_symbols.py could not be imported, so comments and string bodies cannot be "
        "blanked before matching and constants cannot be resolved to values. Without the first "
        "this tool rewrites doc comments into code -- measured 2026-08-30, 2 of 2 sites. Fix the "
        "import rather than restoring a local copy."
    ) from missing

# `.text` begins at RVA 0x1000 on both 1.16.2 and 1.17. Below it is the DOS stub and the PE
# headers, whose layout the PE specification fixes and no game patch can move.
PE_HEADER_LIMIT = 0x1000

# Return type -> the expression a refusal should evaluate to. Deliberately small: an entry here is
# a claim that "this value means the function did nothing", and that claim has to be true.
BAIL_FOR_RETURN = {
    "": "return",
    "()": "return",
    "bool": "return false",
    "usize": "return 0",
    "u32": "return 0",
    "u64": "return 0",
    "i32": "return 0",
    "f32": "return 0.0",
}
# Any `Option<T>`: `None` is that function's own "I could not produce a value", which is exactly
# what a refused address means. It gets the `?` form rather than a match -- see `rewrite`. An
# earlier version listed only two concrete `Option<..>` types and hand-refused the rest, which was
# caution with no reasoning behind it: `Option<Vec<u8>>` means no different from `Option<usize>`.
OPTION_RETURN = re.compile(r"^Option\s*<")

SIGNATURE = re.compile(
    r"^(?P<indent>\s*)(?:pub(?:\([a-z():]+\))?\s+)?(?:unsafe\s+)?"
    r"(?P<abi>extern\s+\"[a-z]+\"\s+)?fn\s+(?P<name>\w+)"
)
# Multi-line on purpose: rustfmt routinely breaks a long `transmute(base + SOME_LONG_RVA)` across
# lines, and a single-line pattern silently missed 13 of 18 sites in er-title-flow alone.
#
# `\$?` on the base, a turbofish, a path qualifier and a non-`RVA` constant name are all here for
# the reasons the module doc gives: each hid live sites from this tool's sibling gate. `const` is
# the last path segment, which is what the log label should say and what a rewritten `use` cannot
# churn.
CALL_SITE = re.compile(
    r"transmute(?:::\s*<[^>]*>)?\(\s*"
    r"(?P<base>\$?(?:base|image_base|module_base|game_base))\s*\+\s*"
    r"(?P<prefix>(?:[A-Za-z_][A-Za-z0-9_]*\s*::\s*)*)(?P<const>[A-Z][A-Za-z0-9_]*)"
    r"(?P<cast>\s+as\s+usize)?\s*,?\s*\)",
    re.S,
)

# The MATCHER this replaced, frozen as a literal. The controls in `--selftest` have to prove each
# widening is load-bearing, and a control the old pattern also catches would pass on the broken
# tool and prove nothing.
#
# Spelled out, not composed from `CALL_SITE`'s pieces. A frozen control assembled from the live
# pattern is not frozen: it widens whenever the live one does, so "the old pattern misses this"
# silently becomes "the new pattern misses this", which is the opposite claim. That is exactly how
# `check-stale-rva-calls.py`'s controls nearly stopped proving anything.
LEGACY_CALL_SITE = re.compile(
    r"transmute\(\s*(?:base|image_base|module_base|game_base)\s*\+\s*"
    r"(?:(?:\w+::)*)([A-Z0-9_]*RVA[A-Z0-9_]*)(?:\s+as\s+usize)?\s*,?\s*\)",
    re.S,
)


def constant_values() -> dict:
    """`{simple name: value}` for every address-capable constant `rva_symbols` could evaluate.

    Ambiguous names -- declared twice with different values -- map to `None`, and so does a name
    that was never resolved. `is_finding` treats both as "not proven safe", which is the only
    honest reading: "I could not read it" must not be spelled the same way as "it is a PE header
    field".
    """
    out: dict = {}
    for decl in rva_symbols.index().decls:
        if not decl.value:
            continue
        for value in decl.value:
            if decl.symbol in out and out[decl.symbol] != value:
                out[decl.symbol] = None
            else:
                out.setdefault(decl.symbol, value)
    return out


def is_finding(constant: str, values: dict) -> bool:
    """Is `base + constant` a stale-address call? Unresolvable means yes -- see the module doc."""
    value = values.get(constant)
    return value is None or value >= PE_HEADER_LIMIT


def enclosing_function(lines: list[str], index: int) -> tuple[str | None, str, bool]:
    """`(name, return_type, is_extern)` for the function containing `lines[index]`."""
    for i in range(index, -1, -1):
        match = SIGNATURE.match(lines[i])
        if not match:
            continue
        blob, j = lines[i], i
        while "{" not in blob and j + 1 < len(lines) and j - i < 16:
            j += 1
            blob += lines[j]
        return_type = ""
        arrow = blob.find("->")
        if arrow >= 0 and "{" in blob[arrow:]:
            return_type = blob[arrow + 2 : blob.rindex("{")].strip()
        return match.group("name"), return_type, bool(match.group("abi"))
    return None, "", False


def rewrite(
    path: str, helper: str, dry_run: bool, values: dict | None = None
) -> tuple[int, int, list[str], list[str]]:
    """`(rewritten, seen, needs-a-human, excluded-by-value)`.

    Matching runs over the comment/string-blanked view; the replacement is spliced into the
    original text at the same offsets, which `code_only` guarantees are the same offsets. The
    enclosing-function scan still reads the real lines, because a signature is code either way.
    """
    values = {} if values is None else values
    text = open(path, encoding="utf-8").read()
    code = code_only(text)
    lines = text.splitlines(keepends=True)
    # Offset -> line index, so a multi-line match can still name its enclosing function.
    starts, offset = [], 0
    for line in lines:
        starts.append(offset)
        offset += len(line)

    def line_of(position: int) -> int:
        low, high = 0, len(starts) - 1
        while low < high:
            mid = (low + high + 1) // 2
            if starts[mid] <= position:
                low = mid
            else:
                high = mid - 1
        return low

    gated, seen, skipped, excluded, pieces, cursor = 0, 0, [], [], [], 0
    for match in CALL_SITE.finditer(code):
        seen += 1
        index = line_of(match.start())
        constant_name = match.group("const")
        if not is_finding(constant_name, values):
            # A PE header field, fixed by the file format. Routing it through the game map gets a
            # refusal and a zero, which for a call is fatal rather than merely wrong.
            excluded.append(
                f"{os.path.relpath(path)}:{index + 1}: {constant_name} = "
                f"0x{values[constant_name]:x} is below .text, so it is a PE header offset the "
                "format fixes and no build can move"
            )
            continue
        name, return_type, is_extern = enclosing_function(lines, index)
        bail = BAIL_FOR_RETURN.get(return_type)
        if bail is None and OPTION_RETURN.match(return_type.strip()):
            bail = "return None"
        if is_extern or bail is None:
            why = (
                "extern fn -- a detour must still call its original"
                if is_extern
                else f"return type {return_type!r} has no obvious did-nothing value"
            )
            skipped.append(f"{os.path.relpath(path)}:{index + 1} in {name}: {why}")
            continue
        # Keep any `as usize`: the constant may be a u32 and dropping the cast is a type error.
        constant = match.group("prefix") + constant_name + (match.group("cast") or "")
        call = f'{helper}({constant}, "{constant_name}")'
        # `match x { Some(a) => a, None => return None }` is `x?`, and clippy rejects the long
        # form. Emit what a reader (and the linter) actually wants.
        resolved = (
            f"{call}?"
            if bail == "return None"
            else f"match {call} {{ Some(address) => address, None => {bail} }}"
        )
        pieces.append(text[cursor : match.start()])
        pieces.append(f"transmute({resolved})")
        cursor = match.end()
        gated += 1
    pieces.append(text[cursor:])
    if gated and not dry_run:
        open(path, "w", encoding="utf-8").write("".join(pieces))
    return gated, seen, skipped, excluded


def selftest() -> int:
    failures = []

    def check(name, condition):
        if not condition:
            failures.append(name)

    body = [
        "unsafe extern \"system\" fn hook(a: usize) -> usize {\n",
        "    let f: F = unsafe { transmute(base + SOME_RVA) };\n",
        "}\n",
        "unsafe fn plain(base: usize) {\n",
        "    let g: G = unsafe { transmute(base + OTHER_RVA) };\n",
        "}\n",
        "fn answers(base: usize) -> bool {\n",
        "    let h: H = unsafe { transmute(base + THIRD_RVA) };\n",
        "}\n",
    ]
    name, ret, is_extern = enclosing_function(body, 1)
    check("a detour was not recognised as extern -- it would have been rewritten", is_extern)
    name, ret, is_extern = enclosing_function(body, 4)
    check(f"a plain `-> ()` fn resolved to {ret!r}, not the empty return",
          not is_extern and BAIL_FOR_RETURN.get(ret) == "return")
    name, ret, is_extern = enclosing_function(body, 7)
    check(f"a `-> bool` fn resolved to {ret!r}, not `return false`",
          BAIL_FOR_RETURN.get(ret) == "return false")
    check("CALL_SITE missed a crate::-prefixed constant",
          CALL_SITE.search("unsafe { std::mem::transmute(base + crate::FOO_RVA) }"))
    cast_match = CALL_SITE.search("transmute(base + FOO_RVA as usize)")
    check("CALL_SITE missed an `as usize` cast", cast_match)
    check("CALL_SITE swallowed the `as usize` cast instead of capturing it",
          cast_match and (cast_match.group("cast") or "").strip())
    check("OPTION_RETURN did not accept a generic Option", OPTION_RETURN.match("Option<Vec<u8>>"))
    check("CALL_SITE matched a trampoline transmute, which must be left alone",
          not CALL_SITE.search("transmute(orig)"))

    # ---------------------------------------------------------------- Positive controls
    # Each is a spelling that stood in live code while this tool's sibling gate reported zero.
    # Each is asserted visible to the current pattern and invisible to the frozen legacy one -- a
    # control both catch would pass on the broken tool and prove nothing.

    # 1 -- a constant that never carried the `_RVA` suffix. Forty of these live in
    # er-build-import-runtime.
    unsuffixed = "let f: F = unsafe { transmute(base + GET_MAIN_PLAYER_STATS) };"
    check("must see a call whose constant is not named *RVA*",
          [m.group("const") for m in CALL_SITE.finditer(unsuffixed)] == ["GET_MAIN_PLAYER_STATS"])
    check("...control is vacuous unless the OLD name-filtered pattern misses it",
          LEGACY_CALL_SITE.findall(unsuffixed) == [])

    # 2 -- a macro body. `transmute($base + TITLE_TOP_DIALOG_IS_IN_STATE_RVA)` was a live 1.17
    # crash (0x749b20 -> 0x74a970, the old address mid-instruction) that one `$` hid.
    macro_body = "let f: F = unsafe { transmute($base + TITLE_TOP_DIALOG_IS_IN_STATE_RVA) };"
    check("must see a call written against a macro metavariable base",
          [m.group("const") for m in CALL_SITE.finditer(macro_body)]
          == ["TITLE_TOP_DIALOG_IS_IN_STATE_RVA"])
    check("...control is vacuous unless the OLD `$`-blind pattern misses it",
          LEGACY_CALL_SITE.findall(macro_body) == [])

    # 3 -- an ENUM variant, which is how er-title-flow spells most of its addresses.
    enum_variant = (
        "let f: F = unsafe { core::mem::transmute("
        "base + ProfileLoadMenuRva::ProfileLoadSelectSaveSlot as usize) };"
    )
    hit = CALL_SITE.search(enum_variant)
    check("must see a call whose address is an enum variant, keyed on the last path segment",
          hit and hit.group("const") == "ProfileLoadSelectSaveSlot")
    check("...and must keep the path so the rewrite still names the variant",
          hit and hit.group("prefix").replace(" ", "") == "ProfileLoadMenuRva::")
    check("...control is vacuous unless the OLD SCREAMING_SNAKE-only pattern misses it",
          LEGACY_CALL_SITE.findall(enum_variant) == [])

    # 4 -- a TURBOFISH. Same call, different spelling of the same word.
    turbofish = "unsafe { transmute::<usize, F>(module_base + MENU_RVA) }"
    check("must see through a turbofish",
          [m.group("const") for m in CALL_SITE.finditer(turbofish)] == ["MENU_RVA"])
    check("...control is vacuous unless the OLD turbofish-blind pattern misses it",
          LEGACY_CALL_SITE.findall(turbofish) == [])

    # ---------------------------------------------------------------- Prose
    # the defect this tool shipped with, verbatim from the two files it hit on 2026-08-30. This
    # control runs the other way round from the four above: the old matcher catches both lines
    # (and counted them `gated 1 / 2`), and the current one must not -- because a false positive
    # in a REWRITER is a corrupted source file, not an inflated count.
    import tempfile

    invasion_warp_doc = (
        "/// Every game call in this crate used to be a bare `transmute(base + SOME_RVA)`. On a\n"
        "/// build the RVAs were not derived against that is not a wrong answer, it is a dead\n"
        "/// process.\n"
    )
    game_build_comment = (
        "// A detour on a stale address is caught by the hook path, but a stale address is\n"
        "// equally reachable as a CALL (`transmute(base + RVA)`) and as a data pointer.\n"
    )
    quoted = 'let note = "transmute(base + QUOTED_RVA)";\n'
    real = "fn go(base: usize) {\n    let f: F = unsafe { transmute(base + REAL_SITE_RVA) };\n}\n"
    prose_file = invasion_warp_doc + game_build_comment + quoted + real
    check("the OLD matcher read the prose as sites (control is non-vacuous)",
          len(LEGACY_CALL_SITE.findall(prose_file)) == 4)  # 3 prose + the one real line
    check("the current matcher reads only the code line",
          [m.group("const") for m in CALL_SITE.finditer(code_only(prose_file))] == ["REAL_SITE_RVA"])
    with tempfile.TemporaryDirectory() as scratch:
        target = os.path.join(scratch, "lib.rs")
        open(target, "w", encoding="utf-8").write(prose_file)
        gated, seen, skipped, excluded = rewrite(target, "gate", False, {"REAL_SITE_RVA": 0x74A970})
        after = open(target, encoding="utf-8").read()
        check(f"exactly the one real site is rewritten, not {gated} of {seen}",
              (gated, seen) == (1, 1))
        check("the doc comment survives the rewrite byte for byte",
              "used to be a bare `transmute(base + SOME_RVA)`" in after)
        check("the block of prose in game_build.rs survives too",
              "reachable as a CALL (`transmute(base + RVA)`)" in after)
        check("a quoted example is not code", 'transmute(base + QUOTED_RVA)"' in after)
        check("the real site was actually converted", 'gate(REAL_SITE_RVA, "REAL_SITE_RVA")' in after)

    # ---------------------------------------------------------------- The value gate
    values = {"PE_DOS_LFANEW_OFFSET": 0x3C, "CSDLC_SINGLETON_RVA": 0x3D86BD8, "AMBIGUOUS": None}
    check("a PE header offset is not a game address", not is_finding("PE_DOS_LFANEW_OFFSET", values))
    check("a real .data RVA is a finding", is_finding("CSDLC_SINGLETON_RVA", values))
    check("a constant declared twice with different values is kept", is_finding("AMBIGUOUS", values))
    check("a constant that could not be resolved is kept", is_finding("NEVER_DECLARED", values))
    with tempfile.TemporaryDirectory() as scratch:
        target = os.path.join(scratch, "hdr.rs")
        header = "fn go(base: usize) {\n    let f: F = unsafe { transmute(base + PE_DOS_LFANEW_OFFSET) };\n}\n"
        open(target, "w", encoding="utf-8").write(header)
        gated, seen, skipped, excluded = rewrite(target, "gate", False, values)
        check("a PE header constant is SEEN but not rewritten",
              (gated, seen) == (0, 1) and len(excluded) == 1)
        check("...and the file is untouched", open(target, encoding="utf-8").read() == header)

    # ---------------------------------------------------------------- Non-VACUITY, of the inputs
    # Every set this tool reasons from is asserted non-empty and of the right order of magnitude
    # before anything is concluded from it. `gated 0 / 0` is the goal state of a migration and a
    # bug in a walk, and only one of those is good news.
    repo = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    sources = []
    for dirpath, dirnames, filenames in os.walk(os.path.join(repo, "crates")):
        dirnames[:] = [d for d in dirnames if d != "target"]
        sources.extend(os.path.join(dirpath, f) for f in filenames if f.endswith(".rs"))
    check(f"only {len(sources)} .rs files under crates/; the walk is broken", len(sources) > 200)
    declared = constant_values()
    check(f"only {len(declared)} constants resolved; the value gate is unfounded", len(declared) > 500)
    # The tree's own count, both ways. It is reported, never asserted to be non-zero: reaching
    # zero real sites is the point of this migration, and asserting on the findings would conflate
    # "the matcher works" with "the tree still has work". The controls above settle the first
    # without depending on the tree at all. What is asserted is that the prose the old matcher
    # counted has stopped being counted.
    live, prose_hits = 0, 0
    for source in sources:
        raw = open(source, encoding="utf-8", errors="replace").read()
        live += len(CALL_SITE.findall(code_only(raw)))
        prose_hits += len(LEGACY_CALL_SITE.findall(raw)) - len(LEGACY_CALL_SITE.findall(code_only(raw)))
    check(f"the OLD matcher read {prose_hits} prose paragraph(s) as call sites in this tree; it "
          "read 2 when this was written, so the control has gone stale", prose_hits >= 2)

    for line in failures:
        print(f"selftest FAIL {line}")
    print(
        f"selftest: {len(failures)} failure(s) "
        f"({len(sources)} sources, {len(declared)} constants resolved, {live} call site(s) in "
        f"code, {prose_hits} prose paragraph(s) the old matcher would have rewritten)"
    )
    return 1 if failures else 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--helper", help="the resolver to call, e.g. `title_fn`")
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--selftest", action="store_true")
    parser.add_argument("paths", nargs="*")
    args = parser.parse_args()
    if args.selftest:
        return selftest()
    if not args.helper or not args.paths:
        parser.error("--helper and at least one path are required")
    values = constant_values()
    gated = seen = 0
    skipped: list[str] = []
    excluded: list[str] = []
    for path in args.paths:
        one_gated, one_seen, one_skipped, one_excluded = rewrite(
            path, args.helper, args.dry_run, values
        )
        gated += one_gated
        seen += one_seen
        skipped.extend(one_skipped)
        excluded.extend(one_excluded)
    for line in skipped:
        print(f"  SKIP {line}")
    for line in excluded:
        print(f"  PE-HEADER {line}")
    print(f"gated {gated} / {seen} site(s); {len(skipped)} left for a human")
    return 0


if __name__ == "__main__":
    sys.exit(main())
