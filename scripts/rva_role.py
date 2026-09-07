#!/usr/bin/env python3
"""Which `*RVA*`-named constants are not game addresses, proven from how the workspace uses them.

The door this closes
--------------------
`select-needed-1170-rows.py` and `map-data-rvas-1162-to-1170.py` both decide what to translate
with the same regex:

    const\\s+([A-Z0-9_]*RVA[A-Z0-9_]*)\\s*:\\s*(usize|u32|u64)\\s*=\\s*0x...

That is a name test standing in for a semantic one, and the name it tests is a SUBSTRING. Two
things follow, both measured on this tree on 2026-08-31:

  * `INTERVAL` contains `RVA`. Every `*INTERVAL*` constant in the workspace -- 35 of them, from
    `OWN_STEPPER_LOG_INTERVAL` to `SNAPSHOT_INTERVAL_MS` to `PATCH_RETRY_LOG_INTERVAL` -- matches
    the name filter. Not one of them is an address; all are tick counts and millisecond periods.
    They stay out of the ledgers for one reason only: they are written in decimal, and the regex
    demands `0x`. Rewrite `const LOG_INTERVAL: usize = 30;` as `0x1e` and it is harvested.
  * Three constants that are written in hex are harvested today: `FIRST_SECTION_RVA` (0x1000),
    `GAME_TEXT_RVA_LIMIT` (0x4000000) and `SW_BP_RVA_LIMIT` (0x5000000). Only the first reached a
    ledger, and only because 0x1000 happens to be a `.pdata`-declared function start in both
    builds; the other two sit past `.text`, so `functions.tsv` had no pair to give them. Luck,
    not a rule.

`FIRST_SECTION_RVA` is what a harvested non-address looks like once it lands. It is the PE
section-boundary sanity bound in `er-hook/src/detour_site.rs` (`if table_rva <
FIRST_SECTION_RVA`), it entered `rva-map-1162-to-1170.needed.tsv` as `0x1000 -> 0x1000`, the byte
comparison then scored it `IDENTICAL-WHOLE` / `BOTH-ENTRIES` -- because the bytes at 0x1000 really
are identical across the two builds -- and `er-game-base/build.rs` admitted it to
`DETOUR_SAFE_1162_TO_1170`, the table detour targets are drawn from. Nothing detours it, so it is
inert. It is still a non-address in the address table, and a confident verdict is exactly what a
meaningless row earns.

Which direction this is allowed to be wrong in
----------------------------------------------
Refusing a real address is the expensive mistake and this repo has made it four times: a
`_BOUND`-suffixed name, an `Enum::Variant as usize` alias, a bare `rva: 0x..` table field, and all
27 of `er-build-import-runtime`'s game calls whose names carry no `RVA` at all. Each miss made the
address invisible end to end -- never selected, never mapped, never verified -- and the running
game refused it while the telemetry reported success. Admitting a non-address, by contrast, adds
an inert row.

So this module never classifies by name and never guesses. It answers one question, with a proof
or not at all:

    Is every use of this constant in `crates/` a comparison, with no use that consumes it as an
    address?

A constant used only as the right-hand side of `<`, `<=`, `>`, `>=`, `==` or `!=` is a bound, a
threshold or a sentinel. It is not something the workspace resolves or offsets from a module base
-- and resolving or offsetting is the only thing one does with an address. One use this module
cannot classify is enough to withhold the verdict: the answer becomes `None`, the constant stays
in the ledger exactly as before, and nothing breaks. There is no heuristic fallback and no
value threshold; a value test was already measured and rejected upstream, because `>= 0x1000`
admits eleven non-addresses, ten of them exactly `0x1000`.

Run against this tree the proof fires on three constants and no others: out of 694 constants whose
name carries `RVA`, 515 that the harvesters can admit, and 477 that already occupy a row in
`rva-map-1162-to-1170.{needed,needed-verified,data}.tsv`.

Usage:
    python3 scripts/rva_role.py             # report every proven non-address in crates/
    python3 scripts/rva_role.py --selftest  # controls: a planted bound, and a real address that
                                            # must not be proven
"""

from __future__ import annotations

import argparse
import bisect
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


# The documented exclusion list. Every entry here is a constant the harvesters' name filter admits
# and that `prove_not_an_address` independently proves is not an address. The list is what makes
# the absence deliberate and visible: an address missing from a ledger with no record of why is
# indistinguishable from one nobody ever mapped, which is the failure mode
# `wholesale-refresh-deletes-hand-rows-silently-2026-08-30` is about.
#
# The list and the proof check each other, and the selftest asserts both directions:
#   * every name here must still be provable -- otherwise the entry is stale, the constant may have
#     become an address, and silently keeping it out of the ledgers would break the feature using
#     it;
#   * every provable name must be here -- otherwise a non-address is being dropped with no record,
#     which is the same invisibility one door along.
NOT_AN_ADDRESS: dict[str, str] = {
    "FIRST_SECTION_RVA": (
        "crates/er-hook/src/detour_site.rs -- PE section-boundary sanity bound. Its own doc "
        "comment says 'Lower bound of the .text sanity window, and the smallest sensible RVA', "
        "and its single use is `if table_rva < FIRST_SECTION_RVA`. It was harvested into "
        "rva-map-1162-to-1170.needed.tsv as `0x1000 -> 0x1000`, scored IDENTICAL-WHOLE / "
        "BOTH-ENTRIES, and reached DETOUR_SAFE_1162_TO_1170."
    ),
    "GAME_TEXT_RVA_LIMIT": (
        "crates/er-quickload/src/crashlog/module_resolution.rs -- upper bound of the window a "
        "crash-log RVA is accepted in (`if rva < GAME_TEXT_RVA_LIMIT`). 0x4000000 is past the "
        "game's first .text section entirely; it stayed out of the ledgers only because "
        "functions.tsv has no pair that far out, not because anything refused it."
    ),
    "SW_BP_RVA_LIMIT": (
        "crates/er-quickload/src/constants/software_breakpoints.rs -- the same shape one crate "
        "over: the software-breakpoint RVA window's upper bound, used three times and every time "
        "as `rva < SW_BP_RVA_LIMIT` / `rva >= SW_BP_RVA_LIMIT`."
    ),
}


# ---------------------------------------------------------------------------------------------
# Reading Rust without a Rust parser
# ---------------------------------------------------------------------------------------------

def blank_rust(text: str) -> str:
    """`text` with comments and string/char literals replaced by spaces, offsets preserved.

    An identifier inside a doc comment or a log message is not a use of it, and this repo is
    unusually full of both -- the constant names appear in `//!` module prose, in `///` RE notes
    and in the log strings that name the address being resolved. Counting those as uses would make
    almost every constant unclassifiable, which is the safe direction but also the useless one.

    Newlines are preserved through the blanking so line numbers still line up.
    """
    out = list(text)
    index, length = 0, len(text)
    while index < length:
        char = text[index]
        if char == "/" and index + 1 < length and text[index + 1] == "/":
            end = text.find("\n", index)
            end = length if end < 0 else end
            for position in range(index, end):
                out[position] = " "
            index = end
        elif char == "/" and index + 1 < length and text[index + 1] == "*":
            end = text.find("*/", index + 2)
            end = length if end < 0 else end + 2
            for position in range(index, end):
                if out[position] != "\n":
                    out[position] = " "
            index = end
        elif char == '"':
            end = index + 1
            while end < length:
                if text[end] == "\\":
                    end += 2
                    continue
                if text[end] == '"':
                    end += 1
                    break
                end += 1
            for position in range(index, min(end, length)):
                if out[position] != "\n":
                    out[position] = " "
            index = end
        else:
            index += 1
    return "".join(out)


USE_ITEM = re.compile(r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?use\s")
IDENT = re.compile(r"\b([A-Z][A-Z0-9_]{2,})\b")
COMPARISONS = ("<=", ">=", "==", "!=", "<", ">")
DECLARATION = re.compile(r"\b(?:const|static)\s+(?:mut\s+)?$")
# `getter_rva: rva::GET_WEAPON_NAME,` -- a table field that holds an address. The named twin of
# the bare `rva: 0x..` literal, and how four of the six MsgRepository name getters reach the
# resolver without ever appearing inside a resolver call. See `rva_usage.FIELD_CONST`.
ADDRESS_FIELD = re.compile(r"\brva\w*\s*:\s*(?:\w+\s*::\s*)*$", re.IGNORECASE)

# The roles one occurrence can have. Only compare is evidence against an address; only address is
# evidence for one. DECL, import and unknown are silence, and silence withholds the verdict.
COMPARE, ADDRESS, DECL, IMPORT, UNKNOWN = "compare", "address", "decl", "import", "unknown"


def _line_starts(text: str) -> list[int]:
    starts, position = [0], text.find("\n")
    while position >= 0:
        starts.append(position + 1)
        position = text.find("\n", position + 1)
    return starts


def roles_in_source(names: set[str], text: str, path: str) -> dict[str, list[tuple[str, str, int, str]]]:
    """`{name: [(role, path, line, source line), ...]}` for every occurrence of `names` in `text`.

    One pass over the file for all names at once. The per-name form is what made the first cut of
    this take 45 seconds over 556 files: it re-blanked and re-scanned the whole tree once per
    constant, 477 times.
    """
    blanked = blank_rust(text)
    lines = text.splitlines()
    starts = _line_starts(blanked)
    imports = []
    for match in USE_ITEM.finditer(blanked):
        end = blanked.find(";", match.end())
        imports.append((match.start(), end + 1 if end > 0 else len(blanked)))
    found: dict[str, list[tuple[str, str, int, str]]] = {}
    for match in IDENT.finditer(blanked):
        name = match.group(1)
        if name not in names:
            continue
        start, end = match.start(), match.end()
        line_no = bisect.bisect_right(starts, start)
        source = lines[line_no - 1].strip() if line_no - 1 < len(lines) else ""
        role = _role_at(blanked, start, end, imports)
        found.setdefault(name, []).append((role, path, line_no, source))
    return found


def _role_at(blanked: str, start: int, end: int, imports: list[tuple[int, int]]) -> str:
    if any(begin <= start < finish for begin, finish in imports):
        return IMPORT
    before = blanked[max(0, start - 200) : start]
    if DECLARATION.search(before):
        return DECL
    left = before.rstrip()
    right = blanked[end : end + 200].lstrip()
    if left.endswith(COMPARISONS) or any(right.startswith(op) for op in COMPARISONS):
        return COMPARE
    # `module_base + FOO_RVA` in either order. This is address arithmetic and nothing else is
    # written this way; a tick interval is never added to a base.
    if left.endswith("+") or right.startswith("+"):
        return ADDRESS
    if ADDRESS_FIELD.search(left):
        return ADDRESS
    return UNKNOWN


def index_sources(root: str | None = None) -> dict[str, str]:
    """Every `crates/**/*.rs` file, read once."""
    base = root or os.path.join(ROOT, "crates")
    sources: dict[str, str] = {}
    for directory, _dirs, files in os.walk(base):
        for name in sorted(files):
            if not name.endswith(".rs"):
                continue
            path = os.path.join(directory, name)
            with open(path, encoding="utf-8", errors="replace") as handle:
                sources[os.path.relpath(path, ROOT)] = handle.read()
    return sources


def roles(names, sources: dict[str, str]) -> dict[str, list[tuple[str, str, int, str]]]:
    """`{name: occurrences}` across every source, for the whole `names` set in one sweep."""
    wanted = set(names)
    out: dict[str, list[tuple[str, str, int, str]]] = {name: [] for name in wanted}
    for path, text in sources.items():
        for name, hits in roles_in_source(wanted, text, path).items():
            out[name].extend(hits)
    return out


def prove_not_an_address(occurrences) -> list[tuple[str, int, str]] | None:
    """The comparison sites that prove this constant is a bound, or `None` if unproven.

    Proven requires all three, and the third is the one that keeps this honest:
      * at least one compare -- positive evidence of a predicate role, not merely an absence;
      * zero address -- nothing resolves it or offsets a module base by it;
      * zero unknown -- every remaining use was understood. A use this module cannot read might be
        the one that hands the value to a resolver, so it withholds the verdict rather than
        guessing. `FREELIST_SHUTDOWN_ASSERT_FN_RVA` is the live example: it is compared once and
        also address-consumed twice, and it is a real function address.

    Declarations and `use` items are ignored either way: neither says what the value is for.
    """
    compares = [(path, line, text) for role, path, line, text in occurrences if role == COMPARE]
    if not compares:
        return None
    if any(role in (ADDRESS, UNKNOWN) for role, _p, _l, _t in occurrences):
        return None
    return compares


def declarations(name: str, sources: dict[str, str]) -> list[tuple[str, int, str]]:
    """Where `name` is declared: `(path, line, the declaration line)`."""
    pattern = re.compile(r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?(?:const|static)\s+" + re.escape(name) + r"\s*:")
    out = []
    for path, text in sorted(sources.items()):
        for match in pattern.finditer(text):
            line_no = text.count("\n", 0, match.start()) + 1
            out.append((path, line_no, text.splitlines()[line_no - 1].strip()))
    return out


def proven_non_addresses(names, sources: dict[str, str]) -> dict[str, list[tuple[str, int, str]]]:
    """`{name: proof}` for every name in `names` this module can prove is not an address."""
    table = roles(names, sources)
    out = {}
    for name, occurrences in table.items():
        proof = prove_not_an_address(occurrences)
        if proof is not None:
            out[name] = proof
    return out


def describe(name: str, proof, sources: dict[str, str]) -> str:
    """One reader-facing block: what the constant is, where it is declared, and the proof."""
    lines = [f"  {name}"]
    for path, line_no, text in declarations(name, sources):
        lines.append(f"      declared  {path}:{line_no}  {text[:120]}")
    for path, line_no, text in proof:
        lines.append(f"      compared  {path}:{line_no}  {text[:120]}")
    reason = NOT_AN_ADDRESS.get(name)
    if reason:
        lines.append(f"      listed as not-an-address: {reason}")
    return "\n".join(lines)


# ---------------------------------------------------------------------------------------------
# Controls
# ---------------------------------------------------------------------------------------------

# A planted bound. Frozen source, so the positive control keeps meaning what it means after the
# tree moves. `PLANTED_WINDOW_RVA` is spelled exactly the way the harvesters' name filter wants
# and is used exactly the way a bound is used.
CONTROL_BOUND = """
pub const PLANTED_WINDOW_RVA: usize = 0x1000;
pub fn accept(rva: usize) -> bool {
    rva >= PLANTED_WINDOW_RVA
}
"""

# The frozen negative, and why it is spelled with no `RVA` in the name. An over-broad matcher --
# one that decided from the name, or from the value being small, or from "I found no resolver
# call" -- would classify a real address as a bound and delete it from the ledger. The address
# below is the shape that is hardest for a name-based tool to see and easiest for it to lose:
# `SET_REINFORCEMENT` is one of the 27 game functions `er-build-import-runtime` calls, none of
# whose names carry `RVA`, and whose invisibility to the old name scan cost the whole build
# importer. It must come back UNPROVEN.
CONTROL_ADDRESS = """
pub const SET_REINFORCEMENT: usize = 0x672740;
pub fn install(module_base: usize) -> usize {
    let target = module_base + SET_REINFORCEMENT;
    if target > module_base { target } else { 0 }
}
"""

# A constant that is both compared and resolved. The live counter-example, frozen: proof must be
# withheld the moment one address-consuming use exists, however many comparisons sit beside it.
CONTROL_MIXED = """
pub const MIXED_SITE_RVA: usize = 0xc575e0;
pub fn pick(base: usize, candidate: usize) -> usize {
    if candidate == MIXED_SITE_RVA { base + MIXED_SITE_RVA } else { 0 }
}
"""

# A use the reader cannot classify. Silence must withhold the verdict, not grant it: this one is
# compared once and then passed to something opaque, and a tool that answered "no address use
# found, therefore a bound" would delete it.
CONTROL_OPAQUE = """
pub const OPAQUE_USE_RVA: usize = 0x2000;
pub fn drive(sink: &mut Vec<usize>, probe: usize) {
    if probe < OPAQUE_USE_RVA { sink.push(OPAQUE_USE_RVA); }
}
"""


def control_failures() -> list[str]:
    """The fixture-only half of the selftest, so another gate can run it without a tree sweep.

    `select-needed-1170-rows.py --selftest` is what `check.sh` actually runs, and it calls this;
    a control set that only ever executes when somebody types `rva_role.py --selftest` by hand is
    a control set nobody runs.
    """
    failures: list[str] = []

    def check(source: str, name: str, expect_proven: bool, why: str):
        found = roles({name}, {"control.rs": source})
        proof = prove_not_an_address(found[name])
        if bool(proof) != expect_proven:
            failures.append(
                f"{name}: expected {'PROVEN not-an-address' if expect_proven else 'UNPROVEN'}, "
                f"got {'PROVEN' if proof else 'UNPROVEN'} -- {why} (roles: "
                f"{sorted(role for role, _p, _l, _t in found[name])})"
            )

    check(CONTROL_BOUND, "PLANTED_WINDOW_RVA", True,
          "a constant used only as a comparison operand is a bound and must be refused")
    check(CONTROL_ADDRESS, "SET_REINFORCEMENT", False,
          "a real game address whose name carries no RVA must never be proven a bound; deleting "
          "one is how all 27 build-importer calls went silently inert")
    check(CONTROL_MIXED, "MIXED_SITE_RVA", False,
          "one address-consuming use must withhold the proof no matter how many comparisons "
          "accompany it")
    check(CONTROL_OPAQUE, "OPAQUE_USE_RVA", False,
          "an unreadable use must withhold the proof; 'I found no address use' is not 'there is "
          "no address use'")

    # Non-VACUITY of the reader itself. Blind the comparison detector and the positive control must
    # stop passing -- otherwise the three negatives above are all satisfied by a reader that sees
    # nothing at all, and the whole selftest is green for the wrong reason.
    global COMPARISONS
    keep = COMPARISONS
    try:
        COMPARISONS = ("\x00",)
        blinded = prove_not_an_address(roles({"PLANTED_WINDOW_RVA"}, {"c.rs": CONTROL_BOUND})["PLANTED_WINDOW_RVA"])
        if blinded:
            failures.append(
                "the reader still proved the planted bound after the comparison detector was "
                "blinded, so it is not the comparison that is doing the work"
            )
    finally:
        COMPARISONS = keep

    # Comments and strings must not count as uses, or every constant becomes unclassifiable and the
    # module quietly stops refusing anything.
    prose = (
        "/// FIRST_SECTION_RVA is described here and `base + FIRST_SECTION_RVA` appears in prose.\n"
        'pub const FIRST_SECTION_RVA: usize = 0x1000;\n'
        'pub fn f(x: usize) -> bool { log("base + FIRST_SECTION_RVA"); x < FIRST_SECTION_RVA }\n'
    )
    if not prove_not_an_address(roles({"FIRST_SECTION_RVA"}, {"p.rs": prose})["FIRST_SECTION_RVA"]):
        failures.append(
            "a mention inside a doc comment or a log string was counted as an address use; every "
            "constant in this tree is named in prose and none would ever be classifiable"
        )

    return failures


def selftest(verbose: bool = True) -> int:
    failures = control_failures()

    # The live tree, both directions. The list and the proof must agree, or one of them is stale.
    sources = index_sources()
    if len(sources) < 200:
        failures.append(f"only {len(sources)} sources read from crates/; the live scan is not running")
    population = harvestable_names(sources)
    if len(population) < 300:
        failures.append(
            f"the harvestable population came back at {len(population)} constant(s). The live half "
            "of this selftest quantifies over it, and 'none of them is a non-address' is trivially "
            "true of a set that stopped matching."
        )
    failures += audit(population, sources)

    reachable = named_but_not_harvestable(sources)
    if verbose:
        print(
            f"  {len(reachable)} constant(s) carry `RVA` in the name yet are NOT harvestable by "
            f"the literal-hex shape, {len([n for n in reachable if 'INTERVAL' in n])} of them "
            "`*INTERVAL*` -- tick counts that match the name filter because the substring is in "
            "the word, and that stay out only because they are written in decimal. Rewrite one in "
            "hex and this gate asks for a NOT_AN_ADDRESS entry instead of letting it into a ledger."
        )

    for line in failures:
        print(f"SELFTEST FAIL: {line}")
    if verbose:
        proven = proven_non_addresses(population | set(NOT_AN_ADDRESS), sources)
        print(
            f"scripts/rva_role.py: selftest {'FAILED' if failures else 'OK'} "
            f"({len(sources)} sources, {len(population)} harvestable constants, "
            f"{len(NOT_AN_ADDRESS)} listed non-addresses, {len(proven)} proven, "
            "4 controls + 2 non-vacuity blinds)"
        )
    return 1 if failures else 0


def audit(population, sources: dict[str, str]) -> list[str]:
    """Both consistency directions between [`NOT_AN_ADDRESS`] and the proof, as failure strings.

    Callable from another gate with its own population -- `select-needed-1170-rows.py` passes the
    exact constant set it is about to write into a ledger, which is the population that actually
    matters. The standalone selftest passes [`harvestable_names`].
    """
    failures: list[str] = []
    listed = set(NOT_AN_ADDRESS)
    proven = proven_non_addresses(set(population) | listed, sources)
    for name in sorted(listed - set(proven)):
        failures.append(
            f"{name} is on the NOT_AN_ADDRESS list but can no longer be proven a bound. Either it "
            "became an address -- in which case keeping it out of the ledgers is breaking whatever "
            "resolves it -- or its uses changed shape. Re-read the definition site before editing "
            "either side."
        )
    for name in sorted(set(proven) - listed):
        failures.append(
            f"{name} is provably not a game address but is not on the NOT_AN_ADDRESS list, so it "
            "would enter an address ledger -- or be dropped from one with no record of why. Add it "
            "with a reason, or explain why the proof is wrong:\n" + describe(name, proven[name], sources)
        )
    return failures


# The population selector, and why it is not a decision. To ask "is any constant the harvesters
# can admit a non-address?" this has to know which constants they can admit, so it reproduces
# their SHAPE: an `RVA`-substring name, an integer type they recognise, and a hex initialiser
# (`map-data-rvas-1162-to-1170.py` also accepts hex arithmetic, so `0x142658c60 - 0x140000000`
# counts). Nothing downstream is decided by it -- every verdict comes from
# `prove_not_an_address`. It is deliberately a touch wider than either harvester: `BOUND`-suffixed
# names are kept in, because a range endpoint is precisely the thing worth proving is a range
# endpoint, and the selftest is happier asking about too many constants than too few.
HARVEST_SHAPE = re.compile(
    r"(?m)\b(?:const|static)\s+([A-Z0-9_]*RVA[A-Z0-9_]*)\s*:\s*(?:usize|u32|u64)\s*=\s*"
    r"(0x[0-9a-fA-F_]+(?:\s*[-+]\s*0x[0-9a-fA-F_]+)*)\s*;"
)
NAMED_RVA = re.compile(r"(?m)\b(?:const|static)\s+([A-Z0-9_]*RVA[A-Z0-9_]*)\s*:")


def harvestable_names(sources: dict[str, str]) -> set[str]:
    """Constants the `*RVA*` harvesters can admit today: the name, the type and the hex literal."""
    names: set[str] = set()
    for text in sources.values():
        names |= {match.group(1) for match in HARVEST_SHAPE.finditer(text)}
    return names


def named_but_not_harvestable(sources: dict[str, str]) -> set[str]:
    """`RVA`-named constants the harvesters cannot admit today -- the decimal near-misses.

    179 of these on 2026-08-31, out of 694 constants whose name carries `RVA`. Most are real
    addresses written in a form the literal-hex shape does not cover -- an `Enum::Variant as
    usize` alias, a `const A = path::B` indirection -- and they are harvested by the other
    declaration forms instead. 35 are not addresses at all: they are `*INTERVAL*` tick counts and
    millisecond periods, matching the name filter because `INTERVAL` contains the letters `R`,
    `V`, `A` in a row, and the only thing keeping them out of an address ledger is that nobody has
    written one in hex.
    """
    named: set[str] = set()
    for text in sources.values():
        named |= set(NAMED_RVA.findall(text))
    return named - harvestable_names(sources)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    if args.selftest:
        return selftest()
    sources = index_sources()
    candidates = harvestable_names(sources) | set(NOT_AN_ADDRESS)
    proven = proven_non_addresses(candidates, sources)
    print(
        f"{len(candidates)} constant(s) are harvestable by the `*RVA*` name filter; "
        f"{len(proven)} are PROVABLY not game addresses:"
    )
    for name in sorted(proven):
        print(describe(name, proven[name], sources))
    unlisted = sorted(set(proven) - set(NOT_AN_ADDRESS))
    if unlisted:
        print(f"\nNOT on the NOT_AN_ADDRESS list: {', '.join(unlisted)}")
    reachable = named_but_not_harvestable(sources)
    reachable_proven = proven_non_addresses(reachable, sources)
    intervals = sorted(name for name in reachable if "INTERVAL" in name)
    print(
        f"\n{len(reachable)} further constant(s) carry `RVA` in the name but do not match the "
        f"literal-hex shape ({len(reachable_proven)} of them provably bounds). {len(intervals)} "
        "are `*INTERVAL*` -- the substring is in the word -- and the only thing keeping those out "
        "of a ledger is that they are written in decimal:"
    )
    print("  " + ", ".join(intervals))
    return 0


if __name__ == "__main__":
    sys.exit(main())
