#!/usr/bin/env python3
"""Guard against re-introducing duplicate literal declarations of one game address.

An RVA *is* a game function's identity. When the same address is written out under several
names -- or the same name is written out independently in several crates -- a 1.16.x address
correction has to be found in every one of them, and CLAUDE.md records that missing one
crash-hooks (`armament-icons-cachemiss-hooks-crash-1162-address-drift`).

Worse than the maintenance cost: divergent names are divergent claims about what the address
is, and at least one of them is then a wrong reverse-engineering fact shipping in the DLL.
Three were found this way on 2026-08-01 (bd
`rva-67b750-is-save-write-not-continue-load-2026-08-01`,
`rva-4852f88-is-saveload2-slsystemimpl-not-fd4-io-worker-2026-08-01`).

The fix for a real duplicate is to declare the value once -- in `er-game-base/src/rva.rs` for
cross-cutting singletons -- and derive every other name from it, so the aliases stay but the
value does not.

This follows the `check-oracle-writers.py` shape: an allowlist pins the current known
duplicates so the cleanup can proceed incrementally, and the gate fails only on new ones.

Two traps this encodes, both of which produced wrong answers when the audit was done by hand:

1. Scanning only `const NAME: usize = 0x...` misses `u32`-typed address constants. That
   undercounted the real duplicate set by ~35% and hid `MOUNT_GUARD_STATE_ROOT_RVA`, which
   turned out to be GameDataMan. All integer widths are scanned here.
2. Grouping by name finds nothing. The duplication has two shapes -- different names for one
   address, and one name re-declared independently in several crates. Grouping by value is
   the only thing that catches both.

...AND A THIRD, WHICH IS TRAP 1 AGAIN ONE LEVEL UP (fixed 2026-08-30)
--------------------------------------------------------------------
Widening the TYPE was not enough, because the type was never the only thing the single `DECL`
regex demanded. It also demanded a SCREAMING_SNAKE name, a hex literal on the same LINE as the
`const`, and that the declaration be a `const` at all. This tree writes the same fact four other
ways, and every one of them was invisible:

    MenuJobWait = 0x00b0d400,          inside `#[repr(u32)] pub enum MenuTraceRva`
    const FOO_VA: usize = 0x140b0d400; the VA spelling of an address written elsewhere as an RVA
    const LONG_RVA: usize =            the literal on the next line, which rustfmt does routinely
        0x9af3a0;
    const TABLE: [usize; 2] = [..];    a table of addresses with no name per element

Values now come from `scripts/rva_symbols.py`, which resolves all of those, so the grouping key is
the address however it is spelled -- and a VA is folded onto its RVA, because 0x141eb9ed0 and
0x1eb9ed0 are one function and a 1.17 correction has to reach both.

Measured on this tree: 33 -> 81 duplicate address groups, none lost. 30 of the 48 new ones are a
`*_VA` in a crate's `build.rs` beside the `*_RVA` in its `src/` (`GAME_HEAP_ALLOC_VA` /
`GAME_HEAP_ALLOC_RVA`), and 15 are an enum discriminant beside a const -- including 0x262250,
declared as both `PROFILE_MARK_SLOT_USED_RVA` and `ProfileLoadMenuRva::ProfileSlotActivate`, which
is this tool's real subject: two names are two claims about what the address is, and at least one
of them is a wrong reverse-engineering fact shipping in the DLL.

What is still not a duplicate, and must not become one: a derived declaration. `const BAR_RVA:
usize = er_game_base::rva::FOO_RVA;` is the fix for a duplicate -- it leaves exactly one literal --
so a declaration counts here only when the address is written out in its own initialiser. Counting
resolved values alone would report every correctly-centralised alias as drift and invert the tool.

An integer that merely looks like one (fixed 2026-08-30)
--------------------------------------------------------
Grouping by value is the only thing that catches both duplication shapes -- and it is also why a
number that is not an address at all can collide with one. `0x989680` blocked the whole of
`scripts/check.sh` for hours as a "duplicate": one site is an element of a powers-of-ten log ladder
in `er-game-base/src/game_build.rs`, the other is the FILETIME tick rate (10_000_000 100ns ticks per
second) in the save picker. Neither is an address. They collide because 10,000,000 happens to land
inside the RVA window -- and because this gate runs before `cargo fmt` under `set -euo pipefail`,
that one false positive hid every check after it from every agent and from the user.

Pinning that one value in the allowlist would have been a per-incident patch. 1e7 is not exotic:
timer scales, byte budgets and log ladders all produce round decimal numbers in this range, so the
next collision is a matter of time, and it would block the same 265 lines of `check.sh` again.

So a declaration now claims an address only when the address is written in HEXADECIMAL. That is not
a style preference dressed up as a rule; it was measured on this tree before it was adopted:

  * of the 647 in-window literal declaration sites, 9 write the value in decimal, and not one of
    them is a game address -- `HASH_MULTIPLIERS`, `TICKS_PER_SECOND`, `MAX_PLAUSIBLE_FUNCTIONS`,
    `PORTRAIT_IDLE_ANIM_IDS`, `DEFAULT_EFFECT_ID`, `EXPECTED_CANDIDATE_ID`, a test `BASE`, and the
    two halves of the 0x989680 collision;
  * dropping them removes exactly one duplicate group -- 0x989680 -- and leaves the other 81
    unchanged, with not one group even losing a site.

The three alternatives were rejected on evidence rather than taste:

  * Type does not separate this case at all. `u64` is an address type here (44 hex sites: every
    `*_VA` in every `build.rs`), and the log ladder is a `[u64; 8]`. `[usize; 2]`, `&[usize]` and
    even an `i32` carry real addresses in this tree too.
  * Name is the discriminator this repo has already been burned by: 27 addresses were invisible to
    `select-needed-1170-rows.py` earlier the same day precisely because they are not spelled
    `*RVA*`. An inclusion filter keyed on the name would rebuild that blindness inside this gate.
  * Container ("an element of an ascending powers-of-ten array is not an address") is true of one
    of the two colliding sites and useless for the other.

Name is still used -- As the alarm on the blind spot this creates, never as the filter. A hex-only
rule goes silently blind the day somebody writes a real address in decimal, and a gate that passes
because it stopped looking is the failure mode this migration has hit repeatedly. So every decimal
in-window literal whose symbol carries the repo's address convention (`*_RVA`, `*_VA`,
`SomethingRva::Variant`) is a hard failure with its own message. Name is unreliable for deciding
what is an address; it is entirely sound for catching a constant that claims to be one. Zero
declarations trip it today, so it needs no baseline.

Usage:
    python3 scripts/check-rva-alias-drift.py            # gate
    python3 scripts/check-rva-alias-drift.py --selftest # prove the gate detects what it claims
    python3 scripts/check-rva-alias-drift.py --list     # show every duplicate group
"""

from __future__ import annotations

import argparse
import os
import re
import sys
from collections import defaultdict
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
# One dialect, not four. `rva_symbols` resolves every declaration spelling in this tree to a value
# and blanks comments and string bodies first. Both halves matter here: the first is what makes an
# enum discriminant or a wrapped literal visible, and the second is what stops the widened matcher
# from reading `// const GHOST_RVA: usize = 0x3d5df38;` as a declaration -- the old regex was
# immune to prose only because it was anchored to the start of a line.
try:  # noqa: E402 - repo-local; the sys.path line above is what makes it work
    import rva_symbols
except ImportError as missing:  # a shared reader that cannot load must stop the gate, not degrade
    raise ImportError(
        "scripts/rva_symbols.py could not be imported, so addresses declared as enum discriminants, "
        "as a VA, or with the literal on the next line cannot be seen. Without it this gate reports "
        "33 duplicate groups where there are 81 (measured 2026-08-30) and calls that green. Fix the "
        "import rather than restoring a local regex."
    ) from missing

REPO_ROOT = Path(__file__).resolve().parents[1]
ALLOWLIST = REPO_ROOT / "scripts" / "rva-alias-allowlist.txt"
IMAGE_BASE = 0x140000000

# Plausible RVA window for a ~120 MiB PE at image base 0x140000000. Values outside it are
# sizes, limits, magics and bitmasks -- 0x280000 (save slot body length), 0x3000000 /
# 0x4000000 (span fallbacks) and 0x7230203 (SPIR-V magic) all collided numerically in the
# first hand scan and were not addresses.
RVA_MIN = 0x100000
RVA_MAX = 0x8000000

# The whole MATCHER this file used to be, frozen as a literal so `--selftest` can prove the
# replacement is load-bearing. Every case below is asserted to be invisible to this and visible to
# the resolver; a case both see would pass on the broken gate and prove nothing.
#
# Spelled out, not composed. A control assembled from live pattern pieces widens when they widen,
# and "the old matcher misses this" quietly becomes "the new matcher misses this" -- the opposite
# claim. `check-stale-rva-calls.py` came within one edit of shipping exactly that.
LEGACY_DECL = re.compile(
    r"^\s*(?:pub(?:\([^)]*\))?\s+)?const\s+([A-Z][A-Z0-9_]*)\s*:\s*"
    r"(?:usize|u32|u64|i32|i64)\s*=\s*(0x[0-9a-fA-F_]+)\s*(?:as\s+\w+\s*)?;"
)

def legacy_scan(root: Path) -> dict[int, list[tuple[str, str, int]]]:
    """The whole pre-2026-08-30 scan, verbatim, kept only as `--selftest`'s negative half.

    Freezing the regex alone would not have been enough: two of the spellings this gate now sees
    were dropped by the value filter rather than by the pattern -- a VA is a perfectly ordinary
    `const NAME: usize = 0x...;` line that the old window then threw away for being above
    `RVA_MAX`. A control that only froze the regex would have called that case vacuous and been
    wrong. So the old scan is frozen end to END: line-anchored match, hex literal, no VA folding,
    same window.
    """
    groups: dict[int, list[tuple[str, str, int]]] = defaultdict(list)
    for path in sorted((Path(root) / "crates").rglob("*.rs")):
        if "target" in path.parts:
            continue
        rel = path.relative_to(root).as_posix()
        with path.open("r", encoding="utf-8", errors="replace") as handle:
            for lineno, line in enumerate(handle, 1):
                match = LEGACY_DECL.match(line)
                if not match:
                    continue
                value = int(match.group(2).replace("_", ""), 16)
                if RVA_MIN <= value < RVA_MAX:
                    groups[value].append((match.group(1), rel, lineno))
    return groups


HEX_LITERAL = re.compile(r"\b0[xX][0-9a-fA-F_]+\b")
DEC_LITERAL = re.compile(r"(?<![\w.])[0-9][0-9_]*(?![\w.])")


# `FOO_RVA`, `GAME_HEAP_ALLOC_VA`, `CS_INGAME_PAD_TYPEID_RVAS`, `MenuTraceRva::MenuJobWait`.
#
# Tokens, not a SUBSTRING. The repo's own selector (`scripts/select-needed-1170-rows.py`) matches
# `[A-Z0-9_]*RVA[A-Z0-9_]*`, which also matches OBSE-RVA-TION and RESE-RVA-TION; manufacturing a
# fresh false positive while removing one would be a poor trade. CamelCase is split as well,
# because the enum spelling puts the marker in the owner (`MenuTraceRva`), not in the variant.
NAME_TOKEN = re.compile(r"[A-Z]+(?![a-z])|[A-Z][a-z0-9]*|[a-z0-9]+")
ADDRESS_NAME_TOKENS = {"RVA", "RVAS", "VA", "VAS"}


def _address_named(name: str) -> bool:
    """Does this symbol claim to be a game address by the repo's naming convention?

    Used only as the alarm on the hex-only rule's blind spot (see the module docstring). It must
    never become an inclusion filter: 27 real addresses in this tree are not spelled `*RVA*`.
    """
    return any(token.upper() in ADDRESS_NAME_TOKENS for token in NAME_TOKEN.findall(name))


def _literals(expr: str, pattern: re.Pattern[str], base: int) -> set[int]:
    out: set[int] = set()
    for match in pattern.finditer(expr):
        try:
            out.add(int(match.group(0).replace("_", ""), base))
        except ValueError:
            continue
    return out


def _own_hex_literals(expr: str) -> set[int]:
    """Every address written out, in HEX, in this INITIALISER.

    Two separate restrictions, and both are load-bearing.

    In this INITIALISER is what separates a declaration that writes an address from one that
    derives it, and it is the property the whole gate turns on. `const BAR_RVA: usize = FOO_RVA;`
    resolves to the same number as `FOO_RVA` and contributes no literal, so the centralised form
    -- the fix -- collapses the group instead of doubling it. `const NEXT: usize = FOO_RVA + 0x10;`
    writes `0x10`, which is not the value, so it does not claim the address either.

    In HEX is what separates an address from an integer that merely lands in the same numeric
    window. `const TICKS_PER_SECOND: i64 = 10_000_000;` is the FILETIME tick rate, not 0x989680.
    Measured: every one of the 9 decimal in-window sites in this tree is a non-address, and
    excluding them costs exactly one duplicate group -- the false one.
    """
    return _literals(expr, HEX_LITERAL, 16)


def _own_decimal_literals(expr: str) -> set[int]:
    """The same, in decimal -- read only to feed the address-named alarm in `_walk`."""
    return _literals(expr, DEC_LITERAL, 10)


def _walk(
    root: Path,
) -> tuple[dict[int, list[tuple[str, str, int]]], list[tuple[int, str, str, int]]]:
    """One index build, two answers: the duplicate groups, and the address-named decimal alarm.

    Kept as a single walk on purpose -- `rva_symbols.index()` takes ~4s over this tree, and a gate
    that pays it twice invites the next agent to drop the second half to save the time.

    The value comes from `rva_symbols`, so `const`, `static`, enum discriminants, `: u32` / `: u64`
    and a literal wrapped onto the next line are all read; the HEX-literal test above then keeps
    the unit of the gate what it has always been -- a place where the address itself is written
    out. A VA is folded onto its RVA: `0x1401eb9ed0` and `0x1eb9ed0` are one game function, and a
    gate that filed them as two addresses would let the pair through as no duplicate at all.
    """
    index = rva_symbols.index(str(Path(root) / "crates"))
    groups: dict[int, set[tuple[str, str, int]]] = defaultdict(set)
    suspects: set[tuple[int, str, str, int]] = set()
    for decl in index.decls:
        if not index.in_universe(decl):
            continue
        hex_literals = _own_hex_literals(decl.expr)
        decimal_literals = _own_decimal_literals(decl.expr)
        if not (hex_literals or decimal_literals):
            continue
        rel = os.path.relpath(decl.path, str(root)).replace(os.sep, "/")
        for value in decl.value or ():
            rva = value - IMAGE_BASE if value >= IMAGE_BASE else value
            if not (RVA_MIN <= rva < RVA_MAX):
                continue
            if value in hex_literals or rva in hex_literals:
                groups[rva].add((decl.symbol, rel, decl.line))
            elif value in decimal_literals or rva in decimal_literals:
                # Not counted as a claim -- but if the symbol is named like an address, the
                # hex-only rule has just gone blind to a real one, and that must be loud.
                if _address_named(decl.qualified):
                    suspects.add((rva, decl.qualified, rel, decl.line))
    return {value: sorted(sites) for value, sites in groups.items()}, sorted(suspects)


def scan(root: Path) -> dict[int, list[tuple[str, str, int]]]:
    """value -> [(name, repo-relative path, line)] for every literal address declaration."""
    return _walk(root)[0]


def duplicates(groups: dict[int, list[tuple[str, str, int]]]) -> dict[int, list]:
    return {v: s for v, s in groups.items() if len(s) > 1}


def load_allowlist() -> dict[int, str]:
    """value -> classification. `exempt:*` is permanent and legitimate; `todo:*` is debt."""
    if not ALLOWLIST.exists():
        return {}
    allowed: dict[int, str] = {}
    for line in ALLOWLIST.read_text(encoding="utf-8").splitlines():
        line = line.split("#", 1)[0].strip()
        if not line:
            continue
        parts = line.split()
        value = int(parts[0], 16)
        cls = parts[1] if len(parts) > 1 else "todo:unadjudicated"
        if not (cls.startswith("exempt:") or cls.startswith("todo:")):
            raise SystemExit(
                f"{ALLOWLIST.name}: 0x{value:x} has unknown class {cls!r}; "
                "expected exempt:* (permanent) or todo:* (debt)"
            )
        allowed[value] = cls
    return allowed


def append_allowlist(dupes: dict[int, list]) -> list[int] | None:
    """Add the unlisted duplicate groups to the allowlist, keeping every existing line.

    APPEND-only, and every existing line is kept byte for byte. This used to rewrite the file from
    scratch as `0x<value>  # <names>`, which silently destroyed the two things the allowlist is
    actually FOR: the `exempt:` / `todo:` classification on all 33 pinned values, and the Ghidra
    adjudication note beside each one (`|| FUN_14067b750(uint slot) -- request an asynchronous
    read ...`). The documented "accept the current duplicates" command therefore cost more than it
    recorded -- and it became far likelier to be reached for on 2026-08-30, when widening the
    scanner took the set from 33 groups to 81. A baseline writer that deletes adjudications is a
    worse defect than the count it was updating.

    Returns the values appended, or None when the file already covers every group.
    """
    existing = load_allowlist()
    fresh = [value for value in sorted(dupes) if value not in existing]
    if not fresh:
        return None
    body = (
        ALLOWLIST.read_text(encoding="utf-8")
        if ALLOWLIST.exists()
        else "# Duplicate literal declarations of one value, CLASSIFIED.\n"
    )
    if not body.endswith("\n"):
        body += "\n"
    added = [
        "",
        "# Appended by --write-allowlist. Each is UNADJUDICATED: read the two declarations against",
        "# the 1.16.2 Ghidra dump and decide whether they are one function under two role names",
        "# (exempt:role-alias) or one address written out twice (todo:centralize-*).",
    ]
    for value in fresh:
        names = ", ".join(sorted({name for name, _, _ in dupes[value]}))
        added.append(f"0x{value:x}    todo:unadjudicated        # {names}")
    ALLOWLIST.write_text(body + "\n".join(added) + "\n", encoding="utf-8")
    return fresh


def selftest() -> int:
    """Prove the scanner sees what the docstring claims, so the gate is never trusted on its
    own say-so. Mirrors check-oracle-writers.py --selftest."""
    import tempfile

    cases = [
        ("usize duplicate across files", 2, {
            "crates/a/src/lib.rs": "const FOO_RVA: usize = 0x3d5df38;\n",
            "crates/b/src/lib.rs": "const BAR_RVA: usize = 0x3d5df38;\n",
        }),
        ("u32 duplicate (the width the hand scan missed)", 2, {
            "crates/a/src/lib.rs": "const FOO_RVA: usize = 0x3d5df38;\n",
            "crates/b/src/lib.rs": "    const BAR_RVA: u32 = 0x03d5df38;\n",
        }),
        ("same NAME re-declared in two crates", 2, {
            "crates/a/src/lib.rs": "const SAME_RVA: usize = 0x3d6b7b0;\n",
            "crates/b/src/lib.rs": "const SAME_RVA: usize = 0x3d6b7b0;\n",
        }),
        # The whole point of the fix: a derived alias leaves one literal, so the group
        # collapses and nothing is reported.
        ("derived declaration is NOT a duplicate", 0, {
            "crates/a/src/lib.rs": "const FOO_RVA: usize = 0x3d5df38;\n",
            "crates/b/src/lib.rs": "const BAR_RVA: usize = er_game_base::rva::FOO_RVA;\n",
        }),
        # 0x280000 (the save-slot body length) sits inside the RVA window, so a range filter
        # cannot tell it from a real address. Non-address collisions are expected to surface
        # and must be retired via the allowlist, not by narrowing the range -- narrowing it
        # would start hiding real addresses.
        ("in-range non-address values still surface (allowlist handles them)", 2, {
            "crates/a/src/lib.rs": "const A_LEN: usize = 0x280000;\n",
            "crates/b/src/lib.rs": "const B_LEN: usize = 0x280000;\n",
        }),
        # Genuinely out of the window (small bitmask below RVA_MIN, huge value above RVA_MAX).
        # Note how few numbers this actually excludes: 0x280000 and even the SPIR-V magic
        # 0x7230203 both land inside it. The range is a coarse prefilter, not the mechanism.
        ("out-of-range values are ignored", 0, {
            "crates/a/src/lib.rs": "const A_MASK: usize = 0x7;\nconst A_BIG: usize = 0x9000000;\n",
            "crates/b/src/lib.rs": "const B_MASK: usize = 0x7;\nconst B_BIG: usize = 0x9000000;\n",
        }),
        # The collision that blocked check.sh, and its control, in one case. 10_000_000 lands
        # inside the RVA window, so a value-keyed gate calls it a duplicate; neither site is an
        # address. Both halves are asserted together on purpose: the genuine hex duplicate beside
        # it must still be reported. A gate that went green by no longer looking would pass the
        # decimal half and fail this one.
        ("a genuine hex duplicate stays red WHILE a decimal non-address collision goes green", 2, {
            "crates/a/src/lib.rs": (
                "const FOO_RVA: usize = 0x3d5df38;\n"
                "const TICKS_PER_SECOND: i64 = 10_000_000;\n"
            ),
            "crates/b/src/lib.rs": (
                "const BAR_RVA: usize = 0x3d5df38;\n"
                "const REFUSAL_MILESTONES: [u64; 8] = [100, 1_000, 10_000_000];\n"
            ),
        }),
        # ...and the other direction, so "decimal is not an address" can never decay into "this
        # value is not an address": the same number, spelled in hex, is still a duplicate.
        ("0x989680 spelled in HEX is still a duplicate", 2, {
            "crates/a/src/lib.rs": "const A_RVA: i64 = 0x989680;\n",
            "crates/b/src/lib.rs": "const B_RVA: u64 = 0x989680;\n",
        }),
    ]
    # The SPELLINGS the single `DECL` REGEX could not read. Each of these is asserted twice: the
    # duplicate must be found now, and the frozen pre-fix regex must miss the half that hid. A case
    # the old matcher also caught would pass on the broken gate and is worthless as proof.
    #
    # 0xb0d400 is the known-good control: in the live tree it is declared only as the enum
    # discriminant `MenuTraceRva::MenuJobWait`, and a sibling gate once recommended deleting its
    # map row because a `const NAME: usize = 0x..;` search came back empty.
    widened = [
        ("enum discriminant beside a const (0xb0d400, the control)", 2, {
            "crates/a/src/lib.rs":
                "#[repr(u32)]\npub enum MenuTraceRva {\n    MenuJobWait = 0x00b0d400,\n}\n",
            "crates/b/src/lib.rs": "const TITLE_MENU_JOB_WAIT_RVA: usize = 0xb0d400;\n",
        }),
        ("a VA beside its own RVA", 2, {
            "crates/a/src/lib.rs": "const GAME_HEAP_ALLOC_VA: usize = 0x141eb9ed0;\n",
            "crates/b/src/lib.rs": "const GAME_HEAP_ALLOC_RVA: usize = 0x1eb9ed0;\n",
        }),
        ("the literal wrapped onto the next line", 2, {
            "crates/a/src/lib.rs": "const LONG_RVA: usize =\n    0x3d5df38;\n",
            "crates/b/src/lib.rs": "const SHORT_RVA: usize = 0x3d5df38;\n",
        }),
        ("an element of a const table", 2, {
            "crates/a/src/lib.rs": "const TABLE: [usize; 2] = [0x3d5df38, 0x3d6b7b0];\n",
            "crates/b/src/lib.rs": "const FOO_RVA: usize = 0x3d5df38;\n",
        }),
        ("a static, not a const", 2, {
            "crates/a/src/lib.rs": "static STATIC_RVA: u32 = 0x3d5df38;\n",
            "crates/b/src/lib.rs": "const FOO_RVA: usize = 0x3d5df38;\n",
        }),
    ]
    failures = []

    def sites_of(files, scanner=scan):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for rel, body in files.items():
                target = root / rel
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_text(body, encoding="utf-8")
            return scanner(root)

    for label, expected, files in cases:
        total = sum(len(v) for v in duplicates(sites_of(files)).values())
        if total != expected:
            failures.append(f"{label}: expected {expected} duplicate sites, got {total}")
        # The cases above are the pre-existing contract, so the frozen scan must agree with them.
        # If it does not, the widening changed an answer it was supposed to leave alone.
        was = sum(len(v) for v in duplicates(sites_of(files, legacy_scan)).values())
        if was != expected:
            failures.append(
                f"{label}: the frozen pre-fix scan answered {was}, not {expected} -- the widening "
                "moved an answer that was already correct"
            )
    for label, expected, files in widened:
        total = sum(len(v) for v in duplicates(sites_of(files)).values())
        if total != expected:
            failures.append(f"{label}: expected {expected} duplicate sites, got {total}")
        # Non-VACUITY, against the whole frozen scan rather than the frozen regex alone. Two of
        # these cases are lines the old regex matched happily and the old value filter then threw
        # away, so a regex-only control would have called them vacuous and been wrong.
        was = sum(len(v) for v in duplicates(sites_of(files, legacy_scan)).values())
        if was >= expected:
            failures.append(
                f"{label}: the FROZEN pre-fix scan already reported {was} of {expected} duplicate "
                "sites, so this case proves nothing -- pick a spelling it genuinely could not read"
            )

    # The alarm on the blind spot the HEX rule creates. Reading hex only is right for every
    # declaration in this tree today -- measured, not assumed -- but it is silent by construction
    # the day somebody writes a real address in decimal, and silence is how instrument after
    # instrument in this migration went false-green. So a decimal in-window literal in an
    # address-named constant is a hard failure, and the discrimination is proven from inside the
    # tool rather than argued in a docstring.
    def suspects_of(files):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for rel, body in files.items():
                target = root / rel
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_text(body, encoding="utf-8")
            return _walk(root)[1]

    tripwire = [
        ("a real address written in DECIMAL, named like an address", 1, {
            "crates/a/src/lib.rs": "const FOO_RVA: usize = 64347960;\n",  # 0x3d5df38
        }),
        ("...the identical address in hex does NOT trip it", 0, {
            "crates/a/src/lib.rs": "const FOO_RVA: usize = 0x3d5df38;\n",
        }),
        ("the enum spelling carries the marker in the OWNER, not in the variant", 1, {
            "crates/a/src/lib.rs":
                "#[repr(u32)]\npub enum MenuTraceRva {\n"
                "    MenuJobWait = 11588608,\n}\n",  # 0xb0d400
        }),
        ("the two halves of the 0x989680 collision are not named like addresses", 0, {
            "crates/a/src/lib.rs":
                "const TICKS_PER_SECOND: i64 = 10_000_000;\n"
                "const REFUSAL_MILESTONES: [u64; 8] = [100, 1_000, 10_000_000];\n",
        }),
        # Why the alarm splits name tokens instead of searching for the substring `RVA` the way
        # scripts/select-needed-1170-rows.py does: OBSE-RVA-TION contains it, and a fresh false
        # positive would be a poor trade for the one being removed.
        ("a name that merely CONTAINS the letters is not address-named", 0, {
            "crates/a/src/lib.rs": "const OBSERVATION_WINDOW_NS: u64 = 10_000_000;\n",
        }),
    ]
    for label, expected, files in tripwire:
        found = suspects_of(files)
        if len(found) != expected:
            failures.append(
                f"{label}: expected {expected} address-named decimal suspect(s), got "
                f"{[name for _, name, _, _ in found]}"
            )

    # Prose is not a declaration. This one is not a hidden finding; it is the hazard the widening
    # creates. The old matcher was anchored to the start of a line, which made it accidentally
    # immune to a commented-out declaration; reading declarations anywhere in the file is not, so
    # `rva_symbols` blanks comments and string bodies before anything is matched.
    prose = sites_of({
        "crates/a/src/lib.rs": "const REAL_RVA: usize = 0x3d5df38;\n",
        "crates/b/src/lib.rs": (
            "// const GHOST_RVA: usize = 0x3d5df38;\n"
            "/// or in a doc comment: const DOC_GHOST_RVA: usize = 0x3d5df38;\n"
            'const NOTE: &str = "const STR_GHOST_RVA: usize = 0x3d5df38;";\n'
        ),
    })
    if duplicates(prose):
        failures.append(
            "a commented-out or quoted declaration was counted as a second literal: "
            f"{sorted(n for sites in duplicates(prose).values() for n, _, _ in sites)}"
        )
    if 0x3D5DF38 not in prose:
        failures.append("...and the real declaration beside the prose was lost as well")

    # Non-VACUITY of the live walk, asserted on the input rather than on the findings: a scan that
    # reads nothing reports zero duplicates and looks exactly like a clean tree. That is not a
    # hypothetical here -- running this file from outside the repo makes `REPO_ROOT` point at the
    # copy, `root/crates` does not exist, and the gate prints `0 addresses with >1 literal
    # declaration` and exits 0.
    live, live_suspects = _walk(REPO_ROOT)
    # The live tree must be clean of address-named decimals: this half of the gate ships with no
    # baseline, so a single one would make check.sh red the moment it lands.
    if live_suspects:
        failures.append(
            "the live tree already holds an address-named decimal literal, so the gate is red: "
            f"{[name for _, name, _, _ in live_suspects]}"
        )
    if len(live) < 200:
        failures.append(
            f"only {len(live)} distinct literal addresses found under {REPO_ROOT}/crates; this "
            "tree normally holds several hundred, so the walk is broken and a green gate is empty"
        )
    if sum(len(sites) for sites in live.values()) < 300:
        failures.append("fewer than 300 literal declaration sites in the live tree; the walk is broken")

    # The baseline writer must not eat the ADJUDICATIONS. `--write-allowlist` is the documented way
    # to accept a new set, and widening the scanner from 33 groups to 81 makes it far likelier to be
    # run -- so what it does to the 33 existing classifications and their Ghidra notes is now
    # load-bearing. It used to rewrite the file from scratch and lose every one of them.
    global ALLOWLIST
    kept = ALLOWLIST
    try:
        with tempfile.TemporaryDirectory() as tmp:
            ALLOWLIST = Path(tmp) / "allowlist.txt"
            original = (
                "# header comment\n"
                "0x7ad1c0    exempt:role-alias    # LEAF_UPDATE_RVA || CS::MenuWindowJob::Run\n"
            )
            ALLOWLIST.write_text(original, encoding="utf-8")
            appended = append_allowlist({
                0x7AD1C0: [("LEAF_UPDATE_RVA", "a.rs", 1), ("MENU_WINDOW_JOB_RUN_RVA", "b.rs", 2)],
                0x262250: [("PROFILE_MARK_SLOT_USED_RVA", "a.rs", 3), ("ProfileSlotActivate", "b.rs", 4)],
            })
            after = ALLOWLIST.read_text(encoding="utf-8")
            if appended != [0x262250]:
                failures.append(f"--write-allowlist appended {appended}, expected only [0x262250]")
            if not after.startswith(original):
                failures.append("--write-allowlist did not keep the existing lines byte for byte")
            if "exempt:role-alias" not in after or "CS::MenuWindowJob::Run" not in after:
                failures.append("--write-allowlist destroyed a classification or its Ghidra note")
            if "0x262250" not in after or "todo:unadjudicated" not in after:
                failures.append("--write-allowlist did not record the new group as unadjudicated")
            if append_allowlist({0x7AD1C0: [], 0x262250: []}) is not None:
                failures.append("--write-allowlist appended a group the file already covers")
    finally:
        ALLOWLIST = kept

    for failure in failures:
        print(f"selftest FAILED: {failure}", file=sys.stderr)
    if failures:
        return 1
    print(
        f"[check-rva-alias-drift] selftest ok ({len(cases) + len(widened) + len(tripwire)} cases; "
        f"{len(live)} live addresses, {len(duplicates(live))} of them duplicated)"
    )
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--selftest", action="store_true")
    parser.add_argument("--list", action="store_true", help="print every duplicate group")
    parser.add_argument("--write-allowlist", action="store_true",
                        help="pin the current duplicates as the baseline")
    args = parser.parse_args()

    if args.selftest:
        return selftest()

    groups, suspects = _walk(REPO_ROOT)
    dupes = duplicates(groups)

    if args.write_allowlist:
        fresh = append_allowlist(dupes)
        if fresh is None:
            print(f"[check-rva-alias-drift] {ALLOWLIST.name} already covers all {len(dupes)} groups")
        else:
            print(
                f"[check-rva-alias-drift] appended {len(fresh)} new group(s) to {ALLOWLIST.name} "
                f"as todo:unadjudicated; existing classifications left untouched"
            )
        return 0

    if args.list:
        for value in sorted(dupes):
            print(f"0x{value:x}")
            for name, path, line in sorted(dupes[value]):
                print(f"    {name:46s} {path}:{line}")
        return 0

    allowed = load_allowlist()
    new = {v: s for v, s in dupes.items() if v not in allowed}
    stale = sorted(set(allowed) - set(dupes))

    todo = sorted(v for v in dupes if allowed.get(v, "").startswith("todo:"))
    exempt = sorted(v for v in dupes if allowed.get(v, "").startswith("exempt:"))

    print(f"[check-rva-alias-drift] {len(dupes)} addresses with >1 literal declaration: "
          f"{len(exempt)} exempt (permanent), {len(todo)} todo (debt), {len(new)} new")
    if todo:
        by_kind: dict[str, int] = {}
        for value in todo:
            by_kind[allowed[value]] = by_kind.get(allowed[value], 0) + 1
        print("  burn-down remaining: "
              + ", ".join(f"{k.split(':', 1)[1]}={n}" for k, n in sorted(by_kind.items())))

    if stale:
        print("  cleaned up since the baseline (remove these lines from the allowlist): "
              + ", ".join(f"0x{v:x}" for v in stale))

    failed = False

    if suspects:
        failed = True
        print(
            "\nA DECIMAL literal in the RVA window, in a constant NAMED like a game address."
            "\nAddresses are written in hex in this repo, and the duplicate scan above reads hex"
            "\nonly -- so this declaration is INVISIBLE to it and its aliases would never be found."
            "\nRewrite the literal as hex, or rename the constant if it is not an address:\n",
            file=sys.stderr,
        )
        for rva, name, path, line in suspects:
            print(f"  {rva} == 0x{rva:x}", file=sys.stderr)
            print(f"      {name:46s} {path}:{line}", file=sys.stderr)

    if new:
        failed = True
        print("\nNEW duplicate address declarations. Declare the value ONCE (er-game-base/src/rva.rs"
              "\nfor cross-cutting singletons) and derive the other names from it:\n", file=sys.stderr)
        for value in sorted(new):
            print(f"  0x{value:x}", file=sys.stderr)
            for name, path, line in sorted(new[value]):
                print(f"      {name:46s} {path}:{line}", file=sys.stderr)

    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
