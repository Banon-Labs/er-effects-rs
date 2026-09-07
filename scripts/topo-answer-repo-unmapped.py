#!/usr/bin/env python3
"""Ask the call-graph topology map about the 1.16.2 addresses this workspace still cannot map.

Collects every game address declared under `crates/`, subtracts the ledgers that
already answer one (`rva-map-1162-to-1170.functions.tsv`, `.verified.tsv`, `.needed.tsv`,
`.tsv`), and reports what the topology pairing says about the remainder -- with the rule and the
tier that carried it, because a loose-tier row measured 12-18% wrong and must never be used.

What "EVERY ADDRESS" used to mean, and why it was fewer than it sounded (fixed 2026-08-30)
------------------------------------------------------------------------------------------
The collector was one regex requiring a SCREAMING_SNAKE name containing `RVA`, one of three
types, and a hex literal on the spot. The list this tool produces is the list of addresses that
get mapped, so every spelling it could not read was an address that silently never got a 1.17
row -- and then refused at runtime with nothing to say why. Four spellings it could not read:

    MenuJobWait = 0x00b0d400,                     an enum discriminant (er-title-flow keeps most
                                                  of its addresses this way)
    const FILE_OPEN_RVA: usize = other::FOO_RVA;  derived, so no literal is present here
    const GAME_HEAP_ALLOC_VA: usize = 0x141eb9ed0; a VA. The old collector read it as an RVA and
                                                  then added the image base again, asking the
                                                  topology about 0x2801eb9ed0 -- an address in
                                                  no image at all
    const LONG_RVA: usize =\n    0x9af3a0;        the literal on the next line

Values now come from `scripts/rva_symbols.py`, which resolves all of them, and a VA is folded onto
its RVA.

What counts as an address, stated because dropping the name filter needs a replacement
--------------------------------------------------------------------------------------
Not every integer constant is a game address; asking the topology about `BOOT_PUMP_MAX_MS` is
noise. A declaration is collected when its value could be an RVA at all -- at or above the end of
the PE headers, below the image size -- And one of three things is true of it:

  * it is named like an address (`*RVA*`, `*_VA`), this tree's convention;
  * it is used like an address: `base + X`, `game_rva(X)`, `game_data_addr(base, X, ..)`,
    `X.checked_add(..)`. What the code does with it is stronger evidence than what it is called;
  * its value is the 1.16.2 source of a row in one of the curated maps, which is a direct
    statement by an earlier pass that this number is a game address.

The third is required to be >= 0x100000: below that a small round number collides with a real
address by accident rather than by being one (`0x1000` is a curated row and a texture dimension
cap in `boot_progress.rs`).

  python3 scripts/topo-answer-repo-unmapped.py --pairs DIR/topo-pairs.pickle
  python3 scripts/topo-answer-repo-unmapped.py --selftest
"""
import argparse
import collections
import os
import pickle
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
# One dialect, not four. `rva_symbols` resolves every declaration spelling in this tree to a value,
# and blanks comments and string bodies before anything is read -- so a `//` paragraph quoting an
# address does not enter the list of things to go and map.
try:  # noqa: E402 - repo-local; the sys.path line above is what makes it work
    import rva_symbols
except ImportError as missing:  # a shared reader that cannot load must stop the tool, not degrade
    raise ImportError(
        "scripts/rva_symbols.py could not be imported, so addresses declared as enum discriminants "
        "or by derivation cannot be collected. Without it this tool under-reports what the repo "
        "still needs mapped, and an address it never lists is an address that never gets a 1.17 "
        "row. Fix the import rather than restoring a local regex."
    ) from missing

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BASE = 0x140000000
RECON = os.path.join(ROOT, "docs", "recon")
# The collector this file used to be, frozen as a literal so `--selftest` can prove the replacement
# is load-bearing: every control below must be invisible to this and visible to the resolver.
# Spelled out rather than composed from the live pieces, so it cannot quietly widen along with them.
LEGACY_CONST_RE = re.compile(
    r"\b([A-Z0-9_]*RVA[A-Z0-9_]*)\s*:\s*(?:usize|u64|u32)\s*=\s*(0x[0-9a-fA-F_]+)")

# `.text` starts at RVA 0x1000 in both builds; everything below is the DOS stub and the PE headers,
# whose layout the format fixes and which therefore cannot move between game versions.
PE_HEADER_LIMIT = 0x1000
# A ~120 MiB image. Above this a value is a size, a mask or a magic, not an offset into the image.
IMAGE_LIMIT = 0x8000000
# Below this, "the number appears in a map" is a coincidence rather than evidence -- see the
# docstring. It is only applied to that one disjunct; a constant named `*_RVA` is still collected.
COINCIDENCE_FLOOR = 0x100000
# This tree's naming convention for a game address, in both spellings it uses -- matched on whole
# UNDERSCORE components, not as a substring. `RVA` as a substring also occurs inside `INTERVAL`,
# which is how `PATCH_RETRY_LOG_INTERVAL: u32 = 100_000` (a log throttle) was first collected as a
# game address the topology should pair.
ADDRESS_NAME = re.compile(r"(?:^|_)(?:RVA|RVAS|VA|VAS)(?:_|$)")
# ...and what using one looks like, which is the stronger evidence of the two -- but only when the
# thing being added to is a module base.
#
# The first draft of this accepted any lowercase binding, on the reasoning that
# `crates/er-loading-portrait-core/src/lookat_stage_camera.rs` binds the module base as `b`. That
# is true and it is still the wrong rule here: `x + FOO` is how this tree walks STRUCTS as well as
# images, so the loose form promoted `dialog + PROFILE_LOAD_DIALOG_STORED_LIST_OFFSET` and two
# more struct-field offsets (0x10f0, 0x1200, 0x1260) into "addresses the repo still needs mapped",
# and the topology was asked to pair them. A one-letter base is only safe to read when the
# constant is address-shaped, which the name test already covers.
ADDRESS_USE = re.compile(
    r"(?<![.\w])\$?(?:base|module_base|image_base|game_base|game_module_base|exe_base)\s*"
    r"(?:\+|\.checked_add\(\s*)\s*((?:[A-Za-z_]\w*\s*::\s*)*[A-Za-z_]\w*)"
    r"|(?:game_rva|resolve_game_address|game_ptr)\s*\(\s*((?:[A-Za-z_]\w*\s*::\s*)*[A-Za-z_]\w*)"
    r"|game_data_addr\s*\(\s*\w+\s*,\s*((?:[A-Za-z_]\w*\s*::\s*)*[A-Za-z_]\w*)"
)


def map_source_addresses(recon=None):
    """Every 1.16.2 address a curated map already calls a game address.

    `functions.tsv` is excluded on purpose: it is a 128k-row dump of every function in the image,
    so membership in it is nearly free and would admit any round number as "a known address".
    """
    out = set()
    for name in (
        "rva-map-1162-to-1170.needed.tsv",
        "rva-map-1162-to-1170.needed-verified.tsv",
        "rva-map-1162-to-1170.verified.tsv",
        "rva-map-1162-to-1170.data.tsv",
        "rva-map-1162-to-1170.tsv",
    ):
        path = os.path.join(recon or RECON, name)
        if not os.path.exists(path):
            continue
        for line in open(path, encoding="utf-8", errors="replace"):
            if not line.startswith("0x"):
                continue
            try:
                value = int(line.split("\t")[0].strip(), 16)
            except ValueError:
                continue
            out.add(value - BASE if value >= BASE else value)
    return out


def address_use_names(index):
    """Every symbol this tree adds to a module base, or hands to an address resolver."""
    names = set()
    for text in index.text.values():
        for match in ADDRESS_USE.finditer(text):
            operand = next((group for group in match.groups() if group), None)
            if operand:
                names.add(operand.replace(" ", "").rsplit("::", 1)[-1])
    return names


def repo_consts(root=None, recon=None):
    """`{1.16.2 RVA: {(symbol, repo-relative file)}}` for every declared game address.

    Two passes, because the unit is an address and the names are only who declares it. One
    qualifying declaration makes the address a game address, and then every declaration of that
    address is listed -- otherwise `0xb0d400` would be reported as `TITLE_MENU_JOB_WAIT_RVA` alone
    and the enum discriminant that actually carries the literal, `MenuTraceRva::MenuJobWait`, would
    be missing from the one line a reader uses to go and find it.
    """
    index = rva_symbols.index(root or os.path.join(ROOT, "crates"))
    known = map_source_addresses(recon)
    used = address_use_names(index)
    declared = collections.defaultdict(set)
    qualifying = set()
    for decl in index.decls:
        if not index.in_universe(decl):
            continue
        named = bool(ADDRESS_NAME.search(decl.symbol.upper()))
        for value in decl.value or ():
            rva = value - BASE if value >= BASE else value
            if not (PE_HEADER_LIMIT <= rva < IMAGE_LIMIT):
                continue
            declared[rva].add((decl.symbol, os.path.relpath(decl.path, ROOT)))
            if named or decl.symbol in used or (rva >= COINCIDENCE_FLOOR and rva in known):
                qualifying.add(rva)
    return {rva: declared[rva] for rva in qualifying}


def ledger_rvas():
    have = {}
    for fn, is_va in (("rva-map-1162-to-1170.functions.tsv", False),
                      ("rva-map-1162-to-1170.needed.tsv", False),
                      ("rva-map-1162-to-1170.verified.tsv", True),
                      ("rva-map-1162-to-1170.tsv", True)):
        p = os.path.join(RECON, fn)
        if not os.path.exists(p):
            continue
        for line in open(p, encoding="utf-8"):
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            parts = line.split("\t")
            if len(parts) < 2 or not parts[0].startswith("0x"):
                continue
            if not parts[1].startswith("0x"):
                continue
            a = int(parts[0], 16)
            b = int(parts[1], 16)
            if is_va:
                a -= BASE
                b -= BASE
            have.setdefault(a, (b, fn))
    return have


FIXTURE = {
    # The control: 0xb0d400 is declared in the live tree only as this enum discriminant, reached
    # through TITLE_MENU_JOB_WAIT_RVA, with three live use sites on the autoload path.
    "crates/a/src/lib.rs": (
        "#[repr(u32)]\npub enum MenuTraceRva {\n    MenuJobWait = 0x00b0d400,\n}\n"
        "pub const TITLE_MENU_JOB_WAIT_RVA: usize = MenuTraceRva::MenuJobWait as usize;\n"
    ),
    "crates/b/src/lib.rs": (
        "pub const PLAIN_RVA: usize = 0x111000;\n"
        # An address whose only literal lives under a name the old filter rejected, re-exported
        # under one it would have accepted. Neither declaration is visible to the old collector:
        # the first fails the name test, the second carries no literal.
        "pub const SOME_TABLE_ENTRY: u32 = 0x444000;\n"
        "pub const DERIVED_RVA: usize = SOME_TABLE_ENTRY as usize;\n"
        "pub const ISIZE_RVA: isize = 0x555000;\n"
        "pub const GAME_HEAP_ALLOC_VA: usize = 0x141eb9ed0;\n"
        # RVA-named and VA-valued, which is not hypothetical: `gaitem_restore.rs` declares
        # ADD_DEFAULT_FILE_LOAD_PROCESS_RVA = 0x142658c60. The old collector read that as an RVA
        # and `main` then added the image base again, asking the topology about 0x282658c60.
        "pub const ADD_DEFAULT_FILE_LOAD_PROCESS_RVA: usize = 0x142658c60;\n"
        "pub const FRAME_BUDGET: usize = 0x222000;\n"
        "pub const BOOT_PUMP_MAX_MS: usize = 0x2710;\n"
        "fn probe(base: usize) { let _ = base + FRAME_BUDGET; }\n"
        "// prose: const GHOST_RVA: usize = 0x333000;\n"
    ),
}


def selftest():
    """Prove the collector sees the spellings the frozen regex could not -- and no prose.

    Every case is asserted both ways. A control the old regex also caught would pass on the broken
    collector and prove nothing, which is how a measuring instrument ends up reporting a false
    green: the assertion runs, and it was never about the thing that broke.
    """
    import tempfile

    failures = []
    with tempfile.TemporaryDirectory() as tmp:
        for rel, body in FIXTURE.items():
            path = os.path.join(tmp, rel)
            os.makedirs(os.path.dirname(path), exist_ok=True)
            with open(path, "w", encoding="utf-8") as handle:
                handle.write(body)
        found = repo_consts(root=os.path.join(tmp, "crates"), recon=tmp)
        legacy = set()
        for body in FIXTURE.values():
            for _name, literal in LEGACY_CONST_RE.findall(body):
                legacy.add(int(literal.replace("_", ""), 16))

    for address, why, hidden in (
        (0xB0D400, "an enum discriminant (MenuTraceRva::MenuJobWait, the control)", True),
        (0x111000, "a plain literal const", False),
        (0x444000, "an address reached only through a derived constant", True),
        (0x555000, "an `: isize` constant", True),
        (0x1EB9ED0, "a `*_VA` constant, folded onto its RVA", True),
        # `hidden` because the RVA is what was never asked about: the old collector held the
        # unfolded 0x142658c60 and asked about that plus the image base.
        (0x2658C60, "an *RVA*-named constant holding a VA, folded onto its RVA", True),
        (0x222000, "a constant with no *_RVA name, USED as `base + X`", True),
    ):
        if address not in found:
            failures.append(f"collector missed 0x{address:x} -- {why}")
        if hidden and address in legacy:
            failures.append(
                f"0x{address:x} ({why}) was already visible to the FROZEN regex, so this control "
                "proves nothing -- pick a spelling it genuinely could not read"
            )
        if not hidden and address not in legacy:
            failures.append(f"the fixture is wrong: 0x{address:x} should be legacy-visible")
    # The VA bug, as a fact about the old collector rather than an opinion about it. It read
    # 0x142658c60 as an RVA, and `main` adds the image base to every collected value -- so the
    # question actually put to the topology was about 0x282658c60, an address in no image.
    if 0x142658C60 not in legacy:
        failures.append("the fixture no longer reproduces the old collector's VA handling")
    for raw in (0x141EB9ED0, 0x142658C60):
        if raw in found:
            failures.append(
                f"0x{raw:x} was collected as if it were an RVA; `main` would add the image base "
                "again and ask about an address in no image"
            )
    # Negative controls. Dropping the name filter must not turn every integer into an address.
    if 0x2710 in found:
        failures.append("BOOT_PUMP_MAX_MS (a millisecond cap) was collected as a game address")
    if 0x333000 in found:
        failures.append("a `//` comment quoting a declaration was collected as a game address")

    # Non-VACUITY of the live walk. A collector that reads nothing reports that the repo needs
    # nothing mapped, which is the most comfortable wrong answer available to it.
    live = repo_consts()
    index = rva_symbols.index(os.path.join(ROOT, "crates"))
    if index.files_read < 200:
        failures.append(f"the symbol index read only {index.files_read} sources; the walk is broken")
    if len(live) < 300:
        failures.append(f"only {len(live)} addresses collected from the live tree; the walk is broken")
    live_legacy = set()
    for text in index.text.values():
        for _name, literal in LEGACY_CONST_RE.findall(text):
            live_legacy.add(int(literal.replace("_", ""), 16))
    if len(live_legacy) < 100:
        failures.append(
            f"the frozen regex found only {len(live_legacy)} addresses in the live tree, so the "
            "comparison below is against nothing"
        )
    lost = sorted(value for value in live_legacy if value < IMAGE_LIMIT and value not in live)
    if lost:
        failures.append(
            f"the widening LOST {len(lost)} address(es) the old regex found: "
            + ", ".join(f"0x{value:x}" for value in lost[:6])
        )
    if len(live) <= len(live_legacy):
        failures.append(
            f"the resolver added nothing: {len(live)} addresses against {len(live_legacy)} from "
            "the frozen regex"
        )

    for failure in failures:
        print(f"selftest FAILED: {failure}")
    if failures:
        return 1
    print(
        f"[topo-answer-repo-unmapped] selftest ok ({len(live)} live addresses collected, "
        f"{len(live_legacy)} visible to the frozen regex)"
    )
    return 0


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--pairs")
    ap.add_argument("--tsv", default=None)
    ap.add_argument("--selftest", action="store_true")
    a = ap.parse_args()
    if a.selftest:
        return selftest()
    if not a.pairs:
        ap.error("--pairs is required (or pass --selftest)")
    pairs = pickle.load(open(a.pairs, "rb"))
    pair, origin = pairs["pair"], pairs["origin"]
    consts = repo_consts()
    have = ledger_rvas()

    rows = []
    agree = disagree = 0
    for rva in sorted(consts):
        va = rva + BASE
        names = sorted(n for n, _ in consts[rva])
        files = sorted({f for _, f in consts[rva]})
        got = pair.get(va)
        o = origin.get(va, ("-", 0, "-"))
        led = have.get(rva)
        if led:
            if got is not None:
                if got - BASE == led[0]:
                    agree += 1
                else:
                    disagree += 1
                    rows.append((rva, got, o, names, files, "LEDGER-DISAGREE:0x%x(%s)" % led))
            continue
        rows.append((rva, got, o, names, files, "unmapped"))

    newly = [r for r in rows if r[5] == "unmapped" and r[1] is not None]
    still = [r for r in rows if r[5] == "unmapped" and r[1] is None]
    conflict = [r for r in rows if r[5] != "unmapped"]
    # Not "*_RVA constants" any more: the collector is keyed on the resolved address, so an enum
    # discriminant and a `*_VA` spelling count too. Saying otherwise in the header would understate
    # the set by exactly the amount that used to be invisible.
    print(f"repo game-address constants: {len(consts)} distinct addresses")
    print(f"already answered by a ledger: {len(consts) - len(newly) - len(still) - len(conflict)}"
          f"  (topology agrees {agree}, disagrees {disagree})")
    print(f"UNMAPPED and now answered by topology: {len(newly)}")
    print(f"UNMAPPED and still unanswered:         {len(still)}")
    print()
    for rva, got, o, names, files, note in newly:
        print("0x%-9x -> 0x%-11x %-6s tier=%-6s %s   [%s]"
              % (rva + BASE, got, o[0], o[2] if len(o) > 2 else "-",
                 ",".join(names)[:60], files[0]))
    if conflict:
        print("\n--- topology disagrees with an existing ledger row ---")
        for rva, got, o, names, files, note in conflict:
            print("0x%-9x topo 0x%-11x %-6s %-6s  %s   %s"
                  % (rva + BASE, got, o[0], o[2] if len(o) > 2 else "-", note, ",".join(names)[:50]))
    if still:
        print("\n--- still unanswered ---")
        for rva, got, o, names, files, note in still:
            print("0x%-9x  %s   [%s]" % (rva + BASE, ",".join(names)[:60], files[0]))
    if a.tsv:
        with open(a.tsv, "w", encoding="utf-8") as fh:
            fh.write("# 1.16.2 VA\t1.17 VA\trule\ttier\tconstant(s)\tfile\tnote\n")
            for rva, got, o, names, files, note in rows:
                fh.write("0x%x\t%s\t%s\t%s\t%s\t%s\t%s\n"
                         % (rva + BASE, ("0x%x" % got) if got else "-", o[0],
                            o[2] if len(o) > 2 else "-", ",".join(names), files[0], note))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
