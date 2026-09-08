#!/usr/bin/env python3
"""Pick the rows of the whole-image function map that this repo actually needs.

`docs/recon/rva-map-1162-to-1170.functions.tsv` pairs 94,111 functions. Baking
all of them into `er-game-base` would put three quarters of a megabyte of
address table into each of eighteen DLLs and make every resolve a linear scan
of 94,000 entries -- for a set of addresses this workspace names in about a
hundred constants.

So this selects the intersection: every `const *_RVA: usize = 0x...` declared
anywhere in `crates/`, looked up in the function map. The output is tracked, so
the addresses that reach the binary arrive as a reviewable diff rather than
appearing from a 94,000-row file nobody reads.

Provenance matters and is recorded per row. These pairs are established by
masked-signature identity across `.pdata`, which is a weaker claim than the
byte-for-byte instruction comparison behind
`rva-map-1162-to-1170.verified.tsv`. The verified map therefore wins wherever
both cover an address, and a row selected here is good enough to call but
should still pass `audit-1170-hook-targets.py` before anything DETOURS it --
the audit checks that the destination is a real function entry with room for
MinHook's five-byte patch, which signature identity does not.

Which ledger is WRITABLE by hand, and which is not
--------------------------------------------------
`rva-map-1162-to-1170.needed.tsv` is generated. `--refresh` rewrites it
wholesale from `functions.tsv`, so every line of it is disposable by
construction -- including a line a human typed. Its sibling
`rva-map-1162-to-1170.verified.tsv` is the hand-curated one: rows are added to
it with their own derivation recorded in the `how` column, and nothing
regenerates it from a machine source.

That asymmetry set a trap, and the trap is the reason for `unreproduced()`
below. Before 2026-08-30 a hand-added row in `needed.tsv` whose pair was absent
from `functions.tsv` was deleted by the next `--refresh`, with exit 0 and no
line of output naming it. Measured, on a scratch copy, with the two rows a
merge agent had been about to add: `0x764290 -> 0x7650e0` and
`0x8c47c0 -> 0x8c5960` both vanished and the file's sha256 returned to its
pre-edit value. The loss does not read as a loss afterwards -- the address
reads as one that was never mapped, and the feature it unblocked simply stops
working again. Short `.pdata` records and body-changed functions are exactly
the addresses that need hand derivation, and exactly the ones `functions.tsv`
cannot carry, so the rows most worth typing were the rows most certain to
evaporate.

So a row this script cannot reproduce is now preserved rather than dropped,
written back under the `HAND-CARRIED` banner at the foot of the file, listed on
stderr at every run, and never silently removed. Preserving carries its own
hazard -- a wrong row that survives forever reads as a live value, and
`er-game-base/build.rs::refuted_sources()` subtracts a `DIVERGES` row from both
maps precisely because a wrong pair is worse than a missing one -- so a
preserved row is not immortal. It leaves in one of four ways:

  * `functions.tsv` gains a pair that AGREES: the row is reproduced normally
    and the banner drops it, no hand edit;
  * `functions.tsv` gains a pair that DISAGREES: that is a contradiction, not
    weak evidence, and this script refuses to write until a human settles it;
  * the last `crates/` declaration of the address goes away: the row is still
    preserved, but it is reported as `undeclared`, which is the standing
    invitation to delete it. That word is now earned rather than assumed. It
    used to mean "my regex found no declaration", and the regex could not see a
    `_BOUND`-suffixed constant or an inline `Enum::Variant as usize`, so the
    file carried a printed instruction to delete rows that a live feature was
    still reaching. It now comes from `scripts/rva_symbols.py`, which resolves
    values rather than spellings and reports `proven_unclaimed` separately from
    "found nothing"; an address that is merely not-found is written `unproven`
    and advises nothing;
  * a human deletes the line. `--refresh` does not put it back -- preservation
    only ever carries forward what it finds.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
# Why a shared RESOLVER and not a fourth REGEX. The `undeclared` reason below is deletion advice,
# printed into a tracked ledger and read far more often than the code that produced it, and until
# 2026-08-30 it was earned by a name-and-shape scan: `declared_rvas()` requires a `const *RVA*`
# spelling, an integer type it knows, and either a hex literal or an `Enum::Variant as usize`
# alias. Two spellings this tree actually uses walk straight past it -- a `_BOUND`-suffixed name,
# which the `BOUND` filter deliberately removes from the selection and which therefore also
# vanishes from the "is it declared?" question it was never meant to answer, and an enum variant
# used inline at a call site with no aliasing constant at all. For either one the file told the
# reader "nothing consumes it; delete the line".
#
# `rva_symbols` answers by value instead, and returns `proven_unclaimed` as a field separate from
# `found_nothing`, because "I found no declaration" and "there is no declaration" are different
# facts and one of them is a deleted feature.
# Why a second shared module. The regexes below ask "is this constant SPELLED like an address?"
# That question has now been answered wrong four times, and the fourth cost the whole build
# importer: all 27 game functions `er-build-import-runtime` calls are named `GET_WEAPON_NAME`,
# `SET_REINFORCEMENT`, `EQUIP_ITEM_TO_CHR_ASM_SLOT` and so on -- no `RVA` anywhere -- so zero of
# them were ever selected, mapped or verified, and the running game refused all six item-name
# getters. Every item name failed to resolve, `read_character.rs` dropped all 18 equipped items,
# and both directions of the feature went silently inert while the telemetry reported success.
#
# `rva_usage` asks the question that does not drift: does this workspace hand the constant to the
# address resolver? A constant passed to `native::resolve` is a game address by construction. Run
# against the pre-rename tree it finds exactly those 27 and nothing else -- no value heuristic, no
# spelling.
# Why a third shared module. `rva_usage` above answers "is this constant an address?" in the
# affirmative, from the call sites. It cannot answer the NEGATIVE: 497 of the 551 named constants
# this scanner finds are never handed to a resolver in a spelling `rva_usage` recognises, and
# nearly all of them are perfectly real addresses reached through some other shape. So "not in
# `used`" is not evidence of anything, and the selection cannot be narrowed to it.
#
# `rva_role` answers the negative the only way it can be answered safely -- with a proof. A
# constant whose every use in `crates/` is a comparison operand, with no use that consumes it as
# an address and no use the reader could not classify, is a bound. That is the shape of
# `FIRST_SECTION_RVA`, which this scanner harvested into the ledger as `0x1000 -> 0x1000` because
# its name contains `RVA`, and which `er-game-base/build.rs` then admitted to
# `DETOUR_SAFE_1162_TO_1170`. It is also the shape of every `*INTERVAL*` constant in the
# workspace, all 35 of which match the name filter -- `INTERVAL` contains `RVA` -- and stay out of
# the ledgers only because they happen to be written in decimal.
try:  # noqa: E402 - repo-local; the sys.path line above is what makes it work
    import const_fold
    import rva_role
    import rva_symbols
    import rva_usage
except ImportError as missing:  # a resolver that cannot load must stop the advice, not degrade
    raise ImportError(
        "scripts/rva_symbols.py could not be imported, so this script cannot tell whether an "
        "address is still declared anywhere. Without it the `undeclared` reason -- which reads as "
        "'delete this line' -- would be back to a name-filtered regex, which is what wrote wrong "
        "deletion advice into a tracked ledger. Fix the import rather than restoring a local copy."
    ) from missing

BASE = 0x140000000
# The type is not part of what makes something a game address. Requiring `usize` here made every
# `: u32` constant invisible to this scanner -- and invisibility is total: the row is never
# selected, never mapped, never verified, and the game refuses it at runtime with no clue why.
# Measured 2026-08-30: SYSTEM_QUIT_RETURN_TITLE_ACTION_DO_CALL_RVA is declared `: u32`, so the
# whole System>Quit tab routing was refused on 1.17 -- including the guard in front of the native
# Return-to-Desktop confirmation. Accept any integer width; the name is the signal.
# A game address does not need a name to be a game address. Three crates keep theirs as bare
# `rva: 0x...` fields inside `HookSpec`/`MapSeam` table literals -- 39 in er-reload-trace, 13 in
# er-invasion-warp, 1 in er-seamless-bugfixes -- and because every tool here keyed on the constant
# name, all 53 were invisible: never selected, never mapped, never verified, and refused at
# runtime under no name anyone could search for. Underscore separators are used in those tables
# (`0x088_55b0`), so they have to be accepted and stripped.
BARE_RVA_FIELD = re.compile(r"\brva\s*:\s*(0x[0-9a-fA-F_]+)")

RVA_TYPE = r"(?:usize|u32|u64)"
# The whole INITIALISER, not its first literal. `scripts/const_fold.py` evaluates it.
#
# The form this replaced captured `(0x[0-9a-fA-F_]+)` -- the first hex literal in the initialiser --
# which is the right answer only when the initialiser is one literal. For
# `ADD_DEFAULT_FILE_LOAD_PROCESS_RVA: usize = 0x142658c60 - 0x140000000` it captured the MINUEND, so
# the harvest recorded the absolute VA 0x142658c60. Nothing in an RVA-keyed map is 1.1 GB past the
# end of the image, so the address matched nothing, landed in `missing`, and was neither checked nor
# reported as unchecked. `map-data-rvas-1162-to-1170.py` had already fixed this shape for the data
# map on 2026-08-30; the function map had not.
#
# Measured over the whole workspace, folding changes the harvest by: One value (0x142658c60 ->
# 0x2658c60, the constant above, and the only new address the fold reaches) and 62 names that are
# re-exports of addresses already carried under another name. Nothing is dropped.
CONST = re.compile(
    r"const\s+([A-Z0-9_]*RVA[A-Z0-9_]*)\s*:\s*" + RVA_TYPE + r"\s*=\s*([^;]+);"
)
# `pub const FOO_RVA: usize = SomeEnum::Variant as usize;` -- the value lives on the enum, not at
# the declaration, so a `= 0x...` scan cannot see it. 37 constants are written this way and the
# selector was blind to every one. It cost a black screen: TITLE_TOP_DIALOG_IS_IN_STATE_RVA
# (`TitleDialogRva::IsInState = 0x749b20`) never reached the map, so the running game refused it,
# `title_dialog_state` could not tell whether the title had reached Loop, and the boot cover --
# which releases on that observation -- never released. 37 `boot-view DECISION` lines, every one
# `own_menu=false render_ready=false`, in front of a title screen that was rendering fine underneath.
# Same shape as CONST but with no name requirement. Admission is decided by `rva_usage` -- the
# constant has to be handed to the address resolver somewhere in the workspace -- rather than by
# spelling or by value. A value test was measured and rejected: `>= 0x1000` admits eleven
# non-addresses, ten of them exactly `0x1000`, which is where `.text` begins and therefore pairs
# cleanly against the function map while meaning nothing (the same trap `BOUND` documents).
ANY_CONST = re.compile(
    r"const\s+([A-Z][A-Z0-9_]*)\s*:\s*" + RVA_TYPE + r"\s*=\s*([^;]+);"
)
ALIAS = re.compile(
    r"const\s+([A-Z0-9_]*RVA[A-Z0-9_]*)\s*:\s*" + RVA_TYPE + r"\s*=\s*(\w+)::(\w+)\s+as\s+" + RVA_TYPE
)
VARIANT = re.compile(r"^\s*(\w+)\s*=\s*(0x[0-9a-fA-F_]+)\s*,", re.M)
# Names that describe a range rather than an address. `AV_GAME_TEXT_RVA_MIN` is
# 0x1000, which is where .text begins and therefore also where a function
# begins -- so it pairs cleanly and means nothing. Translating a bound would be
# a category error, and it is the only kind the intersection cannot catch by
# itself, because every other non-function constant lives in .data or .rdata
# and simply is not in a table built from .pdata.
BOUND = re.compile(r"_(MIN|MAX|BOUND|BASE|SIZE|LEN|LENGTH|COUNT|END|START|STRIDE|ALIGN)$")
OUTPUT = "docs/recon/rva-map-1162-to-1170.needed.tsv"
FUNCTIONS = "docs/recon/rva-map-1162-to-1170.functions.tsv"
VERIFIED = "docs/recon/rva-map-1162-to-1170.verified.tsv"
# Every ledger that attributes a row to a named constant, and the column the name is in. All
# three, because a non-address is only inert until something resolves it, and these three are what
# `er-game-base/build.rs` reads: `needed.tsv` feeds the call map, `needed-verified.tsv` feeds
# both the call map and -- unioned with `verified.tsv` -- `DETOUR_SAFE_1162_TO_1170`, and
# `data.tsv` feeds the globals. Reading one of them and reporting a clean zero over the others is
# the defect that let `audit-1170-hook-targets.py` judge 100 of ~450 detourable addresses.
#
# `verified.tsv` is deliberately absent: its sixth column is free-text derivation prose ("unique,
# 47B signature, 26B fixed"), not a constant name, so there is nothing here to attribute. That is
# a fact about the file's schema, not a scope decision -- if it ever grows a name column, add it.
NAMED_LEDGERS = (
    ("docs/recon/rva-map-1162-to-1170.needed.tsv", 2),
    ("docs/recon/rva-map-1162-to-1170.needed-verified.tsv", 5),
    ("docs/recon/rva-map-1162-to-1170.data.tsv", 2),
)
OBSERVED = "docs/recon/rva-1170-observed-refusals.txt"
BUILD_RS = "crates/er-game-base/build.rs"
# The line the preserved rows sit under. Matched as a prefix when the file is re-read, so the rest
# of the banner can be reworded without orphaning the rows it introduces.
HAND_BANNER = "# HAND-CARRIED"


# What the fold turned away, so that nothing it declines is merely absent. Populated by every
# `declared_rvas()` call and printed by `main()`; `--selftest` asserts it is non-empty, because an
# empty exclusion report is what "the folder is not running" looks like.
excluded: list[tuple[str, str, str]] = []


def _address_value(constants, name: str, initialiser: str, rel: str) -> int | None:
    """The address a declaration holds, or None with a line appended to `excluded`.

    Admission is `hex_rooted`, and that is a spelling test. Saying so plainly is the point: the
    rule it replaces was the same test applied by accident. Selection here is by name -- `RVA` as a
    SUBSTRING -- and "INTERVAL" contains "RVA", so all 35 `*INTERVAL*` constants in this workspace
    pass the name filter and stayed out of the ledgers only because they are written in decimal and
    the old regex demanded `0x`. Fold the initialiser and that accident evaporates:
    `SAVE_SUPPRESS_WAIT_LOG_INTERVAL = 4096` becomes 0x1000, which is where `.text` begins, so it
    pairs cleanly against the function map and means nothing -- the precise shape that got
    `FIRST_SECTION_RVA` a `DETOUR_SAFE_1162_TO_1170` licence.

    `const_fold` therefore reports which literals an expression actually read, all the way down
    through named constants and enum variants, and a declaration is admitted only if every one of
    them was hexadecimal and there was at least one. `usize::MIN` reads none and is refused, which
    is right: 0 is not an address. Everything refused is appended to `excluded` and printed, so the
    35 counters are now visibly not addresses instead of invisibly absent.
    """
    folded = constants.fold(initialiser, scope=rel)
    if folded.value is None:
        excluded.append((name, rel, f"UNFOLDABLE: {folded.reason}"))
        return None
    if not folded.hex_rooted:
        excluded.append(
            (name, rel, f"not hex-rooted (= {folded.value}); {folded.other_literals} decimal literal(s)")
        )
        return None
    return folded.value


def declared_rvas(repo: Path) -> dict[str, int]:
    """Every game address declared under crates/, by name.

    Four declaration forms, and each one that was missing cost a feature:
      * a literal `const FOO_RVA: usize = 0x...`;
      * an alias onto an enum variant whose value lives in another file -- missing this one let a
        refused address black-screen the game;
      * a bare `rva: 0x...` field in a `HookSpec`/`MapSeam` table, which has no constant name at
        all. Those get a synthetic `<file>:<line>` key, because the map still needs the address
        and a refusal still needs something a human can search for;
      * a constant whose name carries no `RVA` at all, admitted because the workspace passes it to
        the address resolver. See the `rva_usage` note above the import: this is the form that
        hid all 27 of `er-build-import-runtime`'s game calls.
    """
    out: dict[str, int] = {}
    aliases: dict[str, tuple[str, str]] = {}
    variants: dict[tuple[str, str], int] = {}
    enum_of_variant: dict[str, int] = {}
    # Collected across the whole workspace first: the declaration and the call site are routinely
    # in different crates (`er-build-import-runtime` calls ten addresses `er-game-base` declares).
    used = rva_usage.workspace_usage(repo)
    constants = const_fold.Constants.scan(repo)
    excluded.clear()
    for path in sorted(repo.glob("crates/**/*.rs")):
        text = path.read_text(encoding="utf-8", errors="replace")
        # Declarations inside `#[cfg(test)]` are skipped: a test may name an address precisely to
        # assert the workspace does not use it. `er-seamless-bugfixes` names the .pdata chained-
        # continuation record 0xc57666 that way, its `_RVA` suffix pulled it into this selection,
        # and the row it produced pointed 0x86 bytes inside a live function -- the exact outcome
        # the test's own doc comment warns about. See `rva_usage.test_module_spans`.
        tests = rva_usage.test_module_spans(text)
        for match in CONST.finditer(text):
            name, initialiser = match.group(1), match.group(2)
            if BOUND.search(name) or rva_usage.in_any_span(match.start(), tests):
                continue
            admitted = _address_value(constants, name, initialiser, path.relative_to(repo).as_posix())
            if admitted is not None:
                out.setdefault(name, admitted)
        # The name says nothing, so the use has to. `BOUND` still applies: a range endpoint that
        # someone passes to a resolver is a bug in the caller, not an address to translate.
        for match in ANY_CONST.finditer(text):
            name, initialiser = match.group(1), match.group(2)
            if name not in used or BOUND.search(name):
                continue
            if rva_usage.in_any_span(match.start(), tests):
                continue
            admitted = _address_value(constants, name, initialiser, path.relative_to(repo).as_posix())
            if admitted is not None:
                out.setdefault(name, admitted)
        for name, enum_name, variant in ALIAS.findall(text):
            if not BOUND.search(name):
                aliases.setdefault(name, (enum_name, variant))
        for match in BARE_RVA_FIELD.finditer(text):
            line = text.count("\n", 0, match.start()) + 1
            rel = path.relative_to(repo).as_posix()
            out.setdefault(f"{rel}:{line}", int(match.group(1).replace("_", ""), 16))
        for variant, value in VARIANT.findall(text):
            # Variant names are matched without their enum -- the declaration and the enum body are
            # routinely in different files, and a variant name is unique enough in practice. A
            # collision would only ever re-point a constant the verifier then rejects.
            enum_of_variant.setdefault(variant, int(value.replace("_", ""), 16))
    del variants
    for name, (_enum_name, variant) in aliases.items():
        value = enum_of_variant.get(variant)
        if value is not None:
            out.setdefault(name, value)
    # The one choke point, applied after every declaration form rather than inside any of them: a
    # constant proven to be a bound is not an address whichever spelling it arrived in. The list
    # is `rva_role.NOT_AN_ADDRESS`, each entry independently re-proved from the definition site by
    # `rva_role`'s selftest, so an entry cannot rot into a silent deletion of a live address.
    for name in rva_role.NOT_AN_ADDRESS:
        out.pop(name, None)
    return out


def read_map(path: Path) -> dict[int, int]:
    out: dict[int, int] = {}
    if not path.is_file():
        return out
    for line in path.read_text(encoding="utf-8").splitlines():
        if line.startswith("#") or not line.strip():
            continue
        cols = line.split("\t")
        if len(cols) < 2:
            continue
        try:
            old, new = int(cols[0], 16), int(cols[1], 16)
        except ValueError:
            continue
        out[old - BASE if old >= BASE else old] = new - BASE if new >= BASE else new
    return out


def verified_rvas(path: Path) -> set[int]:
    """RVAs the byte-comparison verifier already covers; those win."""
    out: set[int] = set()
    if not path.is_file():
        return out
    for line in path.read_text(encoding="utf-8").splitlines():
        if line.startswith("#") or not line.strip():
            continue
        cols = line.split("\t")
        if len(cols) < 3 or cols[2] != "IDENTICAL":
            continue
        try:
            out.add(int(cols[0], 16) - BASE)
        except ValueError:
            continue
    return out


def exhaustive_verdicts(repo: Path) -> list[str]:
    """The verdict strings `er-game-base/build.rs` treats as a whole-body comparison.

    Parsed out of `build.rs`, not copied. The vocabulary has already moved once -- the plain
    `IDENTICAL` of 2026-08-28 became `BYTE-IDENTICAL` / `IDENTICAL-WHOLE` / `IDENTICAL-LEAF` -- and
    the two tools that had transcribed the old word went on reporting confident numbers about a
    string that no longer occurs in the file (one counted detour at 42 rather than 374). A
    transcription cannot notice that; a parse fails loudly and empty.
    """
    text = (repo / BUILD_RS).read_text(encoding="utf-8", errors="replace")
    # Both floor-exempt lists, because build.rs has had two since 2026-08-30 and reading only the
    # first is the same transcription error one word later: PATCH_SITE_VERDICTS carries rows whose
    # bodies differ while their patch sites do not, and they are admitted without a floor exactly
    # as the exhaustive ones are.
    out: list[str] = []
    for name in ("EXHAUSTIVE_VERDICTS", "PATCH_SITE_VERDICTS"):
        match = re.search(rf"const\s+{name}\s*:[^=]*=\s*\[([^\]]*)\]", text)
        if match:
            out += re.findall(r'"([^"]+)"', match.group(1))
    return out


def verified_covered(path: Path, verdicts: list[str]) -> dict[int, int]:
    """Every RVA `verified.tsv` covers with one of `verdicts`, mapped to its 1.17 pair.

    Reported, not subtracted. `verified_rvas` above -- which decides what this selection skips --
    still matches the literal string `IDENTICAL`, and as of 2026-08-30 that matches none of the 99
    rows in the file, so the docstring's "the verified map wins wherever both cover an address" is
    not what this script actually does. `build.rs` re-establishes that precedence itself by
    address, which is why the drift is currently duplication rather than a wrong address, and why
    narrowing the selection here is a deliberate change to the ledger's contents rather than a
    drive-by. Surfacing the count is the part that costs nothing.
    """
    out: dict[int, int] = {}
    if not path.is_file() or not verdicts:
        return out
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        if line.startswith("#") or not line.strip():
            continue
        cols = line.split("\t")
        if len(cols) < 3 or cols[2] not in verdicts:
            continue
        try:
            out[int(cols[0], 16) - BASE] = int(cols[1], 16) - BASE
        except ValueError:
            continue
    return out


def body_rows(path: Path) -> list[tuple[int, int, str]]:
    """The (old, new, label) rows the tracked file currently holds, comments dropped."""
    if not path.is_file():
        return []
    return body_rows_from_text(path.read_text(encoding="utf-8", errors="replace"))


def body_rows_from_text(text: str) -> list[tuple[int, int, str]]:
    """Same, from text. Split out so the guard's selftest never needs a file on disk."""
    out: list[tuple[int, int, str]] = []
    for line in text.splitlines():
        if line.startswith("#") or not line.strip():
            continue
        cols = line.split("\t")
        if len(cols) < 2:
            continue
        try:
            old, new = int(cols[0], 16), int(cols[1], 16)
        except ValueError:
            continue
        old = old - BASE if old >= BASE else old
        new = new - BASE if new >= BASE else new
        out.append((old, new, cols[2] if len(cols) > 2 else ""))
    return out


# The four reasons a preserved row exists, and which one licenses a deletion.
#
# Only `undeclared` is deletion advice, and it is the one that was being written on a name-filtered
# regex's silence. The split is the whole fix: three of these four now say "I could not establish
# that anything wants this", and only the fourth says "nothing wants this".
REASON_UNMAPPED = "unmapped"  # something declares it; the function map has no pair
REASON_UNDECLARED = "undeclared"  # proven unclaimed -- the only one that may advise deletion
REASON_UNPROVEN = "unproven"  # nothing found, but the resolver could not read everything
REASON_UNRESOLVED = "unresolved"  # the resolver itself could not run


# A row may be wanted for an address that is not its own. Two rows in this ledger are labelled
# `<CONSTANT> container (+0x91)` / `(+0x20)`: the pair maps the enclosing function, and the
# constant that needs it names an address inside that function, reached as container + offset. Ask
# only about the container and the answer is "nothing declares this" -- which is true, useless, and
# was printed as `undeclared`, i.e. delete the line. Both are live: 0x93bab0 carries
# SYSTEM_QUIT_COMPONENT_INDEX_PATCH_RVA at +0x91 and 0x1aeaf40 carries GX_CMD_QUEUE_WRAPPER_RVA_MAX
# at +0x20 -- the second one `_MAX`-suffixed, so `BOUND` had already removed it from the local scan
# as well, and the row was invisible from both directions at once.
CONTAINER_OFFSET = re.compile(r"container\s*\(\+\s*(0x[0-9a-fA-F_]+)\s*\)")


def reason_for(declared: set[int], index=None):
    """`(old_rva, label) -> one of the four REASON_* strings`. No deletion advice without a proof.

    `declared` is the local `declared_rvas()` scan and is consulted first only because it is free
    and already computed; a hit there is sufficient but not necessary. Everything else goes to
    `rva_symbols`, which resolves values rather than spellings, and the answer is split three ways:

      * it found a declaration, an alias or a bare literal -> the address is claimed, and the row
        is `unmapped` exactly as if the local scan had seen it. This is the case that used to read
        `undeclared`, i.e. "delete the line", for a `_BOUND`-suffixed name or an inline
        `Enum::Variant as usize`;
      * it proved nothing claims it -> `undeclared`, and the deletion advice is earned;
      * it found nothing but could not evaluate every address-capable declaration -> `unproven`.
        One of the ones it could not read may be this address. Measured on this tree 2026-08-30:
        four declarations are wide enough to hold a `.text` RVA and cannot be evaluated, so no
        address is currently provable and `undeclared` is currently unreachable here. That is the
        conservative answer, not a bug -- see `python3 scripts/rva_symbols.py --residue <addr>`.

    `label` is the row's own third column, and it is consulted because a `container (+0x91)` row is
    wanted for an address inside it, not for its own value. Every address the row could be wanted
    for must be proven unclaimed before the row may be called `undeclared`.

    `index` is injectable so the selftest can prove all four branches against a fixture tree it
    fully controls, instead of asserting that a live tree happens to be in one state.
    """
    cache: dict[tuple[int, str], str] = {}

    def reason(old: int, label: str = "") -> str:
        if old in declared:
            return REASON_UNMAPPED
        key = (old, label)
        if key in cache:
            return cache[key]
        # Every address this row could be wanted for: itself, and -- for a container row -- the
        # address inside it that the constant actually names.
        wanted = [old]
        for offset in CONTAINER_OFFSET.findall(label or ""):
            wanted.append(old + int(offset.replace("_", ""), 16))
        try:
            resolver = index if index is not None else rva_symbols.index()
            answers = [resolver.claims(address) for address in wanted]
        except (OSError, RecursionError) as failure:
            # A walk that broke must never read like a walk that found nothing.
            print(f"  (could not resolve crates/ symbols for 0x{old:x}: {failure})", file=sys.stderr)
            cache[key] = REASON_UNRESOLVED
            return cache[key]
        if any(claims.declarations or claims.literals for claims in answers):
            cache[key] = REASON_UNMAPPED
        elif all(claims.proven_unclaimed for claims in answers):
            # Every address the row could be wanted for has to be proven, not just the first.
            cache[key] = REASON_UNDECLARED
        else:
            cache[key] = REASON_UNPROVEN
        return cache[key]

    return reason


def unreproduced(
    current: list[tuple[int, int, str]],
    rows: list[tuple[str, int, int]],
    reason_of,
) -> tuple[list[tuple[int, int, str, str]], list[tuple[int, int, int, str]]]:
    """Split the rows on disk that this run would not produce into keep and conflict.

    Returns `(preserved, conflicts)`.

    `preserved` is one entry per (old, new) pair the regeneration does not contain and does not
    contradict: hand work, carried forward with the reason it is not reproducible attached, so the
    operator reads why on every run rather than discovering the row's absence months later.

    `conflicts` is the dangerous half: the file says `old -> a`, this run says `old -> b`. One of
    the two is a wrong address at a live-looking value, and this script has no standing to pick.
    The caller refuses to write.
    """
    produced: dict[int, set[int]] = {}
    for _name, old, new in rows:
        produced.setdefault(old, set()).add(new)
    preserved: list[tuple[int, int, str, str]] = []
    conflicts: list[tuple[int, int, int, str]] = []
    for old, new, label in current:
        if new in produced.get(old, ()):
            continue
        if old in produced:
            conflicts.append((old, new, sorted(produced[old])[0], label))
            continue
        if any(old == kept_old and new == kept_new for kept_old, kept_new, _l, _r in preserved):
            continue
        preserved.append((old, new, label, reason_of(old, label)))
    return preserved, conflicts


# The leading identifier of a ledger's constant column. The column is not always a bare name:
# `GX_CMD_QUEUE_WRAPPER_RVA_MAX container (+0x20)` names the enclosing function of an address, and
# `(refused at runtime 0x5eefb0)` names no constant at all. Both are answered by taking the
# leading UPPER_SNAKE run, or nothing.
LEDGER_CONSTANT = re.compile(r"^([A-Z][A-Z0-9_]*)\b")


def ledger_constants(text: str, column: int) -> list[tuple[int, str, str]]:
    """`(line number, constant name, the whole row)` for every attributable row of one ledger."""
    out: list[tuple[int, str, str]] = []
    for line_no, line in enumerate(text.splitlines(), 1):
        if line.startswith("#") or not line.strip():
            continue
        fields = line.split("\t")
        if len(fields) <= column:
            continue
        match = LEDGER_CONSTANT.match(fields[column].strip())
        if match:
            out.append((line_no, match.group(1), line))
    return out


def non_address_ledger_rows(ledgers, sources) -> list[str]:
    """Every ledger row whose constant `rva_role` proves is not a game address, as failure text.

    The gate. A non-address in an address ledger is inert exactly until something resolves or
    detours it, and the row carries no mark distinguishing it from the 411 real ones beside it --
    `FIRST_SECTION_RVA` scored `IDENTICAL-WHOLE` / `BOTH-ENTRIES` and reached
    `DETOUR_SAFE_1162_TO_1170` on evidence that was entirely genuine, because the bytes at 0x1000
    really are the same in both builds. A confident verdict over a meaningless row is what this
    stops.

    `ledgers` is `[(relative path, text, constant column)]` rather than a set of paths, so the
    blind can hand it fabricated text and get a red without writing to a tracked file.
    """
    named: dict[str, list[tuple[str, int, str]]] = {}
    for path, text, column in ledgers:
        for line_no, name, row in ledger_constants(text, column):
            named.setdefault(name, []).append((path, line_no, row))
    if not named:
        return [
            "no ledger row could be attributed to a constant at all, so 'no ledger row is a "
            "non-address' is trivially true. Check the constant columns in NAMED_LEDGERS against "
            "the files' own headers."
        ]
    failures = []
    for name, proof in sorted(rva_role.proven_non_addresses(named, sources).items()):
        where = "; ".join(f"{path}:{line}" for path, line, _row in named[name])
        failures.append(
            f"{name} occupies an address ledger row ({where}) but is PROVABLY not a game address:\n"
            + rva_role.describe(name, proof, sources)
            + "\n      Nothing detours it today, which is the only reason this is not already a "
            "corruption: er-game-base/build.rs admits such a row to DETOUR_SAFE_1162_TO_1170 like "
            "any other. Delete the row, and add the constant to rva_role.NOT_AN_ADDRESS with a "
            "reason so the next --refresh does not put it back."
        )
    return failures


# Ledgers whose first column is a 1.16.2 address, for the "does this address hold a row anywhere"
# question. Wider than `NAMED_LEDGERS`, which is about the constant column: `verified.tsv` has no
# name column to attribute a row to, but it certainly answers for the address in its first one, and
# asking about names there was a schema fact rather than a scope decision.
ADDRESS_LEDGERS = (
    "docs/recon/rva-map-1162-to-1170.needed.tsv",
    "docs/recon/rva-map-1162-to-1170.needed-verified.tsv",
    "docs/recon/rva-map-1162-to-1170.data.tsv",
    "docs/recon/rva-map-1162-to-1170.verified.tsv",
)


def ledger_addresses(repo: Path) -> set[int]:
    """Every 1.16.2 address that holds a row in any ledger, VA and RVA spellings both normalised."""
    out: set[int] = set()
    for relative in ADDRESS_LEDGERS:
        path = repo / relative
        if not path.is_file():
            continue
        for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
            if line.startswith("#") or not line.strip():
                continue
            try:
                value = int(line.split("\t")[0], 16)
            except ValueError:
                continue
            out.add(value - BASE if value >= BASE else value)
    return out


def unmapped_bare_addresses(sources: dict[str, str], mapped: set[int]) -> list[str]:
    """Bare hex literals handed to the address resolver that hold no ledger row, as failure text.

    The outcome gate for the harvest hole, and it is deliberately about the result rather than the
    mechanism. Until 2026-08-31 the data map's `--refresh` harvested `const *RVA*: usize = 0x..`
    and nothing else, so an address written as a bare literal at its use site entered neither the
    body of a ledger nor its unused list. That third state is worse than either: an absent row
    reads afterwards exactly like an address nobody has gotten to yet, so nothing ever looks at it
    again. Four addresses sat in it -- the `CSEblFileManager`/resource-repository globals the menu
    tracer reads through `game_rva` -- and on 1.17 all four were refused at runtime while the
    CAPSTATE-SUBSYS line reported -1/0 rather than saying it had gone dark.

    Admission is `rva_usage.bare_resolver_addresses`: the argument position of a resolver call,
    never the value and never the spelling. `#[cfg(test)]` scopes are skipped for the same reason
    `declared_rvas` skips them -- a test may name an address precisely to assert nothing uses it.
    """
    failures = []
    for path, text in sorted(sources.items()):
        blanked = rva_role.blank_rust(text)
        tests = rva_usage.test_module_spans(blanked)
        starts, cursor = [0], blanked.find("\n")
        while cursor >= 0:
            starts.append(cursor + 1)
            cursor = blanked.find("\n", cursor + 1)
        for line_no, value in rva_usage.bare_resolver_addresses(text):
            offset = starts[line_no - 1] if line_no - 1 < len(starts) else 0
            if rva_usage.in_any_span(offset, tests):
                continue
            rva = value - BASE if value >= BASE else value
            if rva in mapped:
                continue
            failures.append(
                f"0x{rva:x} is handed to the address resolver at {path}:{line_no} as a BARE HEX "
                "LITERAL and holds no row in any ledger, so it is neither translated for 1.17 nor "
                "listed as untranslated. Run "
                "`python3 scripts/map-data-rvas-1162-to-1170.py --refresh` (a data address) or "
                "`python3 scripts/select-needed-1170-rows.py --refresh` (a .text address); if the "
                "refresh withholds it, the address will appear in that file's UNUSED list, which "
                "is the reported state this gate asks for."
            )
    return failures


def observed_rvas(path: Path) -> list[int]:
    """Addresses the running game asked for and was refused; see record-1170-refusals.py."""
    out: list[int] = []
    if not path.is_file():
        return out
    for line in path.read_text(encoding="utf-8").splitlines():
        if line.startswith("#") or not line.strip():
            continue
        try:
            out.append(int(line, 16))
        except ValueError:
            continue
    return out


def select(repo: Path) -> tuple[list[tuple[str, int, int]], list[str]]:
    functions = read_map(repo / FUNCTIONS)
    already = verified_rvas(repo / VERIFIED)
    wanted = dict(declared_rvas(repo))
    # The declaration scan is not the whole population. An address the game reached and the
    # gate refused is wanted whether or not a constant name gave it away -- 42 of the 54
    # refused on 2026-08-29 were answerable by the function map and simply never selected.
    for rva in observed_rvas(repo / OBSERVED):
        if rva not in wanted.values():
            wanted.setdefault(f"(refused at runtime 0x{rva:x})", rva)
    rows, missing = [], []
    for name, rva in sorted(wanted.items()):
        if rva in already:
            continue
        if rva in functions:
            rows.append((name, rva, functions[rva]))
        else:
            missing.append(name)
    rows.sort(key=lambda r: r[1])
    return rows, missing


def render(rows, preserved=()) -> str:
    head = [
        "# 1.16.2 RVA\t1.17 RVA\tconstant",
        "# GENERATED -- DO NOT HAND-EDIT THE BODY OF THIS FILE.",
        "# scripts/select-needed-1170-rows.py --refresh rewrites everything below WHOLESALE",
        "# from rva-map-1162-to-1170.functions.tsv. A row typed in by hand is reproduced only",
        "# if functions.tsv already carries the same pair; anything else is moved to the",
        "# HAND-CARRIED block at the foot of the file and reported on every run. Nothing here",
        "# is deleted silently, but a hand-derived pair belongs in the curated ledger next door:",
        "# rva-map-1162-to-1170.verified.tsv, which only verify-rva-map-1170.py --tsv writes and",
        "# which carries forward every row that run did not itself produce.",
        "#",
        "# Selected by scripts/select-needed-1170-rows.py from",
        "# rva-map-1162-to-1170.functions.tsv -- the subset this workspace names in a",
        "# `const *_RVA` declaration. Pairs come from masked-signature identity across",
        "# .pdata, which is weaker evidence than the byte comparison behind",
        "# rva-map-1162-to-1170.verified.tsv; rows that map already covers are omitted",
        "# here so the stronger evidence wins. Good enough to CALL. Before DETOURING one,",
        "# run scripts/audit-1170-hook-targets.py: signature identity does not check that",
        "# the destination has room for MinHook's five-byte patch.",
    ]
    body = [f"0x{old:x}\t0x{new:x}\t{name}" for name, old, new in rows]
    tail: list[str] = []
    if preserved:
        tail = [
            "#",
            f"{HAND_BANNER} -- {len(preserved)} row(s) this selection cannot reproduce.",
            "# Kept because a wholesale regeneration that drops a hand-derived pair does it with",
            "# exit 0 and no diagnostic, and the loss reads afterwards as an address that was",
            "# never mapped. These are read by er-game-base/build.rs exactly like the rows above.",
            "# `unmapped`   -- a crates/ declaration names this address, functions.tsv has no pair",
            "#                 for it. Short .pdata records and body-changed functions live here.",
            "# `undeclared` -- PROVEN unclaimed by scripts/rva_symbols.py: every address-capable",
            "#                 declaration in crates/ was evaluated, none is this address, and no",
            "#                 bare literal of it occurs in code. This one is safe to delete.",
            "# `unproven`   -- no declaration was FOUND, but the resolver could not evaluate every",
            "#                 address-capable declaration, so one of them may be it. This is NOT",
            "#                 a licence to delete: run",
            "#                 `python3 scripts/rva_symbols.py --residue <addr>` and read them.",
            "# `unresolved` -- the resolver itself could not run. Nothing is known; fix that first.",
            "# Until 2026-08-30 the last three were all spelled `undeclared`, and `undeclared` was",
            "# earned by a name-filtered regex that cannot see a `_BOUND`-suffixed constant or an",
            "# inline `Enum::Variant as usize`. That is deletion advice written on a spelling miss.",
            "# To remove one: delete its line. --refresh will not put it back. If functions.tsv",
            "# later disagrees with a pair here, this script refuses to write until you settle it.",
        ]
        tail += [
            f"0x{old:x}\t0x{new:x}\t{label}\t{reason}"
            for old, new, label, reason in sorted(preserved)
        ]
    return "\n".join(head + body + tail) + "\n"


# --------------------------------------------------------------------------------------------
# The positive control for the deletion advice
# --------------------------------------------------------------------------------------------

# The pre-2026-08-30 scanner, frozen as LITERALS. `declared_rvas()` above decided whether an
# address was still declared, and `unreproduced()` turned its silence into the printed instruction
# "delete the line". These are that scanner's patterns, spelled out rather than composed from the
# live `CONST` / `ALIAS` / `VARIANT` / `BOUND` / `BARE_RVA_FIELD` objects.
#
# Composing them would destroy the proof. A control assembled from the live pieces widens whenever
# they widen, so "the old scanner could not see this spelling" quietly becomes "the new scanner
# cannot see it either" -- the opposite claim, asserted in the same words. That nearly happened to
# `check-stale-rva-calls.py`, whose controls were built from its live pattern.
LEGACY_CONST = re.compile(
    r"const\s+([A-Z0-9_]*RVA[A-Z0-9_]*)\s*:\s*(?:usize|u32|u64)\s*=\s*(0x[0-9a-fA-F_]+)"
)
LEGACY_ALIAS = re.compile(
    r"const\s+([A-Z0-9_]*RVA[A-Z0-9_]*)\s*:\s*(?:usize|u32|u64)\s*=\s*(\w+)::(\w+)\s+as\s+"
    r"(?:usize|u32|u64)"
)
LEGACY_VARIANT = re.compile(r"^\s*(\w+)\s*=\s*(0x[0-9a-fA-F_]+)\s*,", re.M)
LEGACY_BOUND = re.compile(r"_(MIN|MAX|BOUND|BASE|SIZE|LEN|LENGTH|COUNT|END|START|STRIDE|ALIGN)$")
LEGACY_BARE_RVA_FIELD = re.compile(r"\brva\s*:\s*(0x[0-9a-fA-F_]+)")


def legacy_declared(text: str) -> set[int]:
    """Every address the pre-fix scanner considered declared, over one blob of source."""
    out: set[int] = set()
    enum_of_variant: dict[str, int] = {}
    for name, value in LEGACY_CONST.findall(text):
        if LEGACY_BOUND.search(name):
            continue
        out.add(int(value.replace("_", ""), 16))
    for match in LEGACY_BARE_RVA_FIELD.finditer(text):
        out.add(int(match.group(1).replace("_", ""), 16))
    for variant, value in LEGACY_VARIANT.findall(text):
        enum_of_variant.setdefault(variant, int(value.replace("_", ""), 16))
    for name, _enum_name, variant in LEGACY_ALIAS.findall(text):
        if LEGACY_BOUND.search(name):
            continue
        value = enum_of_variant.get(variant)
        if value is not None:
            out.add(value)
    return out


# Three addresses and the spelling each one is written in. Frozen source, so the control keeps
# meaning what it means after the tree moves on:
#
#   0x111000  an ordinary `const *_RVA: usize = 0x..`   -- Both scanners see it. Present only to
#             prove the frozen legacy scanner still works; a control set where the old scanner
#             finds nothing at all would make every "the old one missed it" assertion vacuous.
#   0x222000  a `_END`-suffixed name. `BOUND` strips it from the selection on purpose -- it is a
#             range bound, not a function -- and the pre-fix code reused that same set to answer
#             "is anything still declaring this?", so the row read `undeclared`: delete the line.
#   0xb0d400  an enum discriminant used inline, with no aliasing constant. This is the real
#             `MenuTraceRva::MenuJobWait`; in the live tree an alias rescues it, and this fixture
#             is the same address written the way that has no alias. Three live use sites hang off
#             it on the autoload path.
CONTROL_SOURCE = """
pub const NAV_COST_TABLE_RVA: usize = 0x111000;
pub const LEGACY_SCAN_WINDOW_RVA_END: usize = 0x222000;
#[repr(u32)]
pub enum MenuTraceRva {
    TaskEnqueue = 0x007a7b60,
    MenuJobWait = 0x00b0d400,
}
pub fn drive(base: usize) -> usize {
    base + MenuTraceRva::MenuJobWait as usize
}
"""


def control_index(repo: Path | None = None):
    """`(fixture index, failures)`. Proves the widening is load-bearing before anything uses it."""
    import tempfile

    failures: list[str] = []
    scratch = Path(tempfile.mkdtemp()) / "crates" / "a" / "src"
    scratch.mkdir(parents=True, exist_ok=True)
    (scratch / "lib.rs").write_text(CONTROL_SOURCE, encoding="utf-8")
    fixture = rva_symbols.Index.build(root=str(scratch.parent.parent.parent))

    # Non-VACUITY first, before any claim about contents. An empty set makes every `not in` below
    # true and every assertion pass for the wrong reason -- the deepest of the nine false greens
    # found on 2026-08-30 was an `assert bad == 0` over a filter that matched nothing.
    old_sees = legacy_declared(CONTROL_SOURCE)
    if len(old_sees) != 1 or 0x111000 not in old_sees:
        failures.append(
            f"the frozen legacy scanner is broken, so 'the old one missed it' proves nothing: "
            f"it found {sorted(hex(v) for v in old_sees)}, expected exactly [0x111000]"
        )
    if fixture.files_read < 1 or fixture.universe_size() < 4:
        failures.append(
            f"the control fixture did not parse: {fixture.files_read} file(s), "
            f"{fixture.universe_size()} address-capable declaration(s)"
        )
    classify = reason_for(set(), fixture)

    for address, spelling in ((0x222000, "a _END-suffixed constant"), (0xB0D400, "an inline enum discriminant")):
        if address in old_sees:
            failures.append(
                f"control is worthless: the OLD scanner already sees 0x{address:x} ({spelling}), "
                "so catching it now proves nothing"
            )
        if classify(address) != REASON_UNMAPPED:
            failures.append(
                f"0x{address:x} ({spelling}) still reads {classify(address)!r}; the pre-fix code "
                "printed 'delete the line' for exactly this"
            )

    # A container row is wanted for an address that is not its own. Both of the live rows that
    # read `undeclared` before this change were this shape.
    if classify(0xB0D3C0, "TITLE_MENU_JOB_WAIT_RVA container (+0x40)") != REASON_UNMAPPED:
        failures.append("a container row is not rescued by the address it contains")
    if 0xB0D3C0 in old_sees:
        failures.append("control is worthless: the OLD scanner already sees the container address")
    # ...and it must be the label that rescued it, not the address happening to be claimed anyway.
    if classify(0xB0D3C0) != REASON_UNDECLARED:
        failures.append(
            "the container offset is not load-bearing: 0xb0d3c0 is already non-undeclared without "
            "the label, so the container control proves nothing"
        )

    # ...and the advice must still be REACHABLE, or the fix has merely disabled the feature. In a
    # tree the resolver understands completely, an address nothing declares is proven unclaimed.
    if classify(0x999000) != REASON_UNDECLARED:
        failures.append(
            f"an address nothing declares reads {classify(0x999000)!r} in a fully-resolved tree; "
            "deletion advice can no longer be earned at all, which is a different bug"
        )

    # The live path, not just the fixture. A resolver that only ever runs against its own fixture
    # is a fixture. `set()` is passed as the local-scan shortcut so the answer must come from the
    # resolver; 0xb0d400 is reachable in the real tree, through TITLE_MENU_JOB_WAIT_RVA.
    if repo is not None:
        live = rva_symbols.index(repo / "crates")
        if live.files_read < 200:
            failures.append(f"the live resolver read only {live.files_read} sources")
        if reason_for(set(), live)(0xB0D400) != REASON_UNMAPPED:
            failures.append("the live resolver does not see 0xb0d400; the real tree is not being read")
        residue = len(live.claims(0xB0D400).residue)
        print(
            f"  live resolver: {live.files_read} sources, {live.universe_size()} address-capable "
            f"declarations, {residue} unevaluated and wide enough to hold a .text RVA."
            + (
                ""
                if residue == 0
                else f" While that is non-zero NO address can be PROVEN unclaimed, so "
                f"'{REASON_UNDECLARED}' is currently unreachable here and every unfound row is "
                f"written '{REASON_UNPROVEN}'."
            )
        )
    return fixture, failures


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    repo = Path(__file__).resolve().parent.parent
    ap.add_argument("--repo", type=Path, default=repo)
    ap.add_argument("--refresh", action="store_true", help="rewrite the tracked file")
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()

    if args.selftest:
        failures = []
        names = declared_rvas(args.repo)
        if any(BOUND.search(n) for n in names):
            failures.append("a range-bound constant survived the filter")
        if len(names) < 50:
            failures.append(f"only {len(names)} *_RVA constants found; the scan is not seeing the sources")
        # The crash of 2026-08-29 is the case this must never silently drop.
        # The alias form, asserted by the constant whose absence black-screened the game on
        # 2026-08-29: `TITLE_TOP_DIALOG_IS_IN_STATE_RVA = TitleDialogRva::IsInState as usize`.
        if names.get("TITLE_TOP_DIALOG_IS_IN_STATE_RVA") != 0x749B20:
            failures.append(
                "enum-alias constants are invisible again -- "
                "TITLE_TOP_DIALOG_IS_IN_STATE_RVA did not resolve to 0x749b20"
            )
        if names.get("GET_CURRENT_MAP_ID_RVA") != 0x5EEFB0:
            failures.append("GET_CURRENT_MAP_ID_RVA did not parse to 0x5eefb0")
        rows, _missing = select(args.repo)
        observed = observed_rvas(args.repo / OBSERVED)
        if observed and not any(old in observed for _n, old, _new in rows):
            failures.append("no runtime-observed refusal made it into the selection")
        if not any(old == 0x5EEFB0 for _n, old, _new in rows):
            failures.append("0x5eefb0 is not in the selection; the live crasher would stay unmapped")

        # The wholesale-regeneration guard. Asserted on a fabricated table rather than on the
        # tracked file, so it keeps failing after the real file's hand rows are all absorbed. The
        # two `unmapped` addresses are the pair a merge agent nearly typed into needed.tsv on
        # 2026-08-30 -- both declared under crates/, neither present in functions.tsv, and both
        # measured vanishing from a scratch copy at exit 0 before this guard existed.
        fake_rows = [("REPRODUCED_RVA", 0x111111, 0x222222)]
        fake_current = [
            (0x111111, 0x222222, "REPRODUCED_RVA"),  # regeneration produces it: not preserved
            (0x764290, 0x7650E0, "MENU_CONTINUE_IDLE_INSERT_CALLER_FN_RVA"),  # declared, unmapped
            (0x8C47C0, 0x8C5960, "UPDATE_RVA"),  # declared, unmapped
            (0x333333, 0x444444, "GONE_RVA"),  # nothing declares it any more
        ]
        fixture, control_failures = control_index(args.repo)
        failures += control_failures
        classify = reason_for({0x764290, 0x8C47C0, 0x111111}, fixture)
        kept, clash = unreproduced(fake_current, fake_rows, classify)
        if {old for old, _n, _l, _r in kept} != {0x764290, 0x8C47C0, 0x333333}:
            failures.append(f"a hand-added row would be dropped silently again: kept {kept}")
        if dict((old, reason) for old, _n, _l, reason in kept).get(0x333333) != REASON_UNDECLARED:
            failures.append("an undeclared preserved row is not reported as such")
        if clash:
            failures.append(f"agreeing rows were reported as conflicts: {clash}")
        # A pair the file and the function map disagree about must stop the write, not be merged.
        _kept2, clash2 = unreproduced(
            [(0x111111, 0x999999, "REPRODUCED_RVA")], fake_rows, reason_for(set(), fixture)
        )
        if [c[:3] for c in clash2] != [(0x111111, 0x999999, 0x222222)]:
            failures.append("a contradicting pair did not raise a conflict")
        # ... and the preserved rows must survive a write/read round trip, or the guard preserves
        # them into a file the next run cannot see them in.
        round_trip = body_rows_from_text(render(fake_rows, kept))
        if not {(0x764290, 0x7650E0), (0x8C47C0, 0x8C5960)} <= {(o, n) for o, n, _l in round_trip}:
            failures.append("preserved rows do not survive render() -> body_rows()")
        verdicts = exhaustive_verdicts(args.repo)
        if "IDENTICAL-WHOLE" not in verdicts:
            failures.append(f"EXHAUSTIVE_VERDICTS no longer parses out of build.rs: {verdicts}")

        # ------------------------------------------------------------------------------------
        # The non-address gate, and its two blinds
        # ------------------------------------------------------------------------------------
        # A constant enters this ledger because its name contains `RVA`. That admitted
        # `FIRST_SECTION_RVA` -- a PE section-boundary sanity bound compared against, never
        # resolved -- as `0x1000 -> 0x1000`, whence `IDENTICAL-WHOLE` / `BOTH-ENTRIES` and
        # `DETOUR_SAFE_1162_TO_1170`. It admits every `*INTERVAL*` constant in the workspace too;
        # 35 of those match, and only decimal notation keeps them out.
        # The proof's own controls first, on frozen fixtures: a planted bound that must be caught,
        # a real address with no `RVA` in its name that must not be, a constant that is both
        # compared and resolved, a use the reader cannot classify, and a blinding of the
        # comparison detector to show it is what does the work. `check.sh` runs this selftest, so
        # controls that only fire under `rva_role.py --selftest` would never run in a gate.
        failures += rva_role.control_failures()
        sources = rva_role.index_sources()
        ledgers = []
        for relative, column in NAMED_LEDGERS:
            path = args.repo / relative
            if path.is_file():
                ledgers.append((relative, path.read_text(encoding="utf-8", errors="replace"), column))
        if len(ledgers) != len(NAMED_LEDGERS):
            failures.append(
                f"only {len(ledgers)} of {len(NAMED_LEDGERS)} named ledgers were readable. A gate "
                "over a subset of the ledgers the build reads is how audit-1170-hook-targets.py "
                "reported '0 of 100 need a look' over 350 addresses it never opened."
            )
        # Non-VACUITY before the claim. "No ledger row is a non-address" is trivially true of a
        # ledger nothing was read out of, and the two constant columns differ between the files
        # (2 and 5), so a schema change empties this silently.
        attributed = {
            name
            for _path, text, column in ledgers
            for _line, name, _row in ledger_constants(text, column)
        }
        if len(attributed) < 300:
            failures.append(
                f"only {len(attributed)} distinct constant(s) were attributed across "
                f"{len(ledgers)} ledger(s). The named columns have moved or the parse broke; a "
                "clean result over this population would mean nothing."
            )
        # One sweep, two QUESTIONS: what is in the ledgers today, and what the next `--refresh`
        # would put there. Combined because each `roles()` pass blanks and re-scans 557 sources.
        failures += non_address_ledger_rows(ledgers, sources)
        failures += rva_role.audit(declared_rvas(args.repo), sources)

        # Blind 1: plant the real row into a fabricated ledger and it must go red. Only the one
        # source that declares the constant is handed over, so the blind costs one file rather
        # than another whole-tree sweep -- and it is still the real definition site doing the
        # proving, not a fixture that could drift away from it.
        detour_site = "crates/er-hook/src/detour_site.rs"
        if detour_site not in sources:
            failures.append(f"{detour_site} was not read, so blind 1 cannot use its real declaration")
        else:
            planted = [(
                "planted.tsv",
                "# 1.16.2 RVA\t1.17 RVA\tconstant\n0x1000\t0x1000\tFIRST_SECTION_RVA\n",
                2,
            )]
            if not non_address_ledger_rows(planted, {detour_site: sources[detour_site]}):
                failures.append(
                    "a planted FIRST_SECTION_RVA row read CLEAN. The gate cannot see the exact row "
                    "it exists to stop, so its silence over the real ledgers means nothing."
                )

        # Blind 2, the frozen negative. An over-broad matcher -- one deciding from the name, or
        # from the value, or from "I found no resolver call" -- would flag a real address and
        # delete its ledger row. The hardest such address to see is one whose name carries no
        # `RVA` at all: that spelling is what hid all 27 of `er-build-import-runtime`'s game calls
        # from the old name scan and silently emptied the build importer.
        #
        # It is a frozen fixture and not a live constant on purpose, and the reason is itself a
        # finding: measured 2026-08-31, this tree no longer has a game address whose name lacks
        # `RVA` -- the build-importer 27 were all renamed to `*_RVA`, and every constant
        # `rva_usage` sees handed to the address resolver carries it. Pinning the negative to a
        # live constant would therefore pin it to nothing. `rva_role.CONTROL_ADDRESS` keeps the
        # spelling alive so the matcher is still tested against it.
        control_source = {"crates/frozen/control.rs": rva_role.CONTROL_ADDRESS}
        control = [(
            "planted.tsv",
            "# 1.16.2 RVA\t1.17 RVA\tconstant\n0x672740\t0x673590\tSET_REINFORCEMENT\n",
            2,
        )]
        if non_address_ledger_rows(control, control_source):
            failures.append(
                "SET_REINFORCEMENT -- a real game address with no `RVA` in its name -- was flagged "
                "as a non-address. This gate would delete live rows from the ledgers; the proof is "
                "over-broad."
            )
        if not rva_role.declarations("SET_REINFORCEMENT", control_source):
            failures.append(
                "the frozen negative's own source no longer declares SET_REINFORCEMENT, so blind 2 "
                "is asserting about a constant that is not there and would pass on anything."
            )

        # ------------------------------------------------------------------------------------
        # The harvest HOLE: an address with no name at all, and its three blinds
        # ------------------------------------------------------------------------------------
        # Everything above starts from an identifier. A bare `game_rva(0x485cbec)` has none, so
        # until 2026-08-31 it entered no ledger and no ledger's unused list either -- the third
        # state, which reads afterwards like an address nobody has gotten to yet. The matcher's
        # own fixture controls first (two positives including the one-hop forwarding closure, a
        # frozen negative of Win32 flags and window bounds, prose, and a blinding of the resolver
        # matcher), then the live outcome over `crates/`.
        failures += rva_usage.control_failures()
        mapped = ledger_addresses(args.repo)
        if len(mapped) < 300:
            failures.append(
                f"only {len(mapped)} address(es) were read out of {len(ADDRESS_LEDGERS)} ledgers, "
                "so 'every bare literal holds a row' is close to trivially false rather than "
                "measured. The first columns have moved or the parse broke."
            )
        failures += unmapped_bare_addresses(sources, mapped)

        # Blind 3: Plant one. A literal spelled exactly like the four this closed, in a source the
        # gate is handed, against a ledger set that does not contain it. It must go red -- if it
        # does not, the clean result over the real tree above means only that the reader found
        # nothing, which is what it did before this change.
        planted_literal = {"crates/planted/lib.rs": rva_usage.CONTROL_FORWARDED}
        if not unmapped_bare_addresses(planted_literal, set()):
            failures.append(
                "a planted `g(0x0485cbec)` read CLEAN against an empty ledger set. The gate cannot "
                "see the exact shape it exists to catch, so its silence over crates/ proves "
                "nothing. This is the harvest hole, open again."
            )
        # ...and it must be the missing row that reds it, not the literal's mere existence: the
        # same source against a ledger set that does carry both addresses must read clean.
        if unmapped_bare_addresses(planted_literal, {0x485CBEC, 0x3D5B0F8}):
            failures.append(
                "a bare literal that already holds a ledger row was still reported. The gate is "
                "not asking about the row at all, so it will fire on every address in the tree "
                "the moment one is added."
            )
        # Blind 4, the frozen negative. Hex literals that are genuinely not addresses, in a source
        # that also contains a forwarding closure. `0x1000` is the number that matters: it is
        # MEM_COMMIT, it is where `.text` begins, and it is `FIRST_SECTION_RVA`, which reached
        # DETOUR_SAFE_1162_TO_1170 on entirely genuine evidence about a meaningless row. A matcher
        # that widened until it admitted whatever it was handed would report it here.
        negative_literal = {"crates/frozen/negative.rs": rva_usage.CONTROL_NOT_AN_ADDRESS}
        if unmapped_bare_addresses(negative_literal, set()):
            failures.append(
                "a Win32 flag / RVA window bound / struct offset was reported as an unmapped game "
                "address. Admitting 0x1000 is precisely how a PE section boundary got a detour "
                "licence; this gate would now demand ledger rows for hundreds of them."
            )

        for line in failures:
            print(f"SELFTEST FAIL: {line}")
        print(f"selftest: {len(names)} constants, {len(rows)} selected, {len(failures)} failure(s)")
        return 1 if failures else 0

    rows, missing = select(args.repo)
    target = args.repo / OUTPUT
    declared = set(declared_rvas(args.repo).values())
    preserved, conflicts = unreproduced(
        body_rows(target), rows, reason_for(declared, rva_symbols.index(args.repo / "crates"))
    )

    # Printed on every run, refresh or check, before anything is written. The whole defect this
    # guards against is a deletion nobody saw, so the set is never summarised away to a count.
    for old, new, label, reason in sorted(preserved):
        print(f"  hand-carried: 0x{old:x} -> 0x{new:x}  {label}  [{reason}]")
    if preserved:
        print(
            f"{len(preserved)} row(s) this selection cannot reproduce were kept under "
            f"'{HAND_BANNER}'. Delete a line there to drop it; --refresh will not restore it."
        )
    for old, new, other, label in sorted(conflicts):
        print(
            f"CONFLICT: 0x{old:x} -> 0x{new:x} in {target.name} ({label}), "
            f"but functions.tsv now pairs it with 0x{other:x}"
        )
    if conflicts:
        print(
            f"REFUSING to write {target.name}: {len(conflicts)} row(s) disagree with the "
            "function map. One of the two addresses is wrong and reads as live either way -- "
            "er-game-base/build.rs subtracts a refuted pair from BOTH maps for the same reason. "
            "Settle it by hand: delete the row to accept the function map, or correct "
            "functions.tsv."
        )
        return 1

    overlap = verified_covered(args.repo / VERIFIED, exhaustive_verdicts(args.repo))
    both = [old for _n, old, _new in rows if old in overlap]
    if both:
        print(
            f"note: {len(both)} selected row(s) are ALSO covered by {Path(VERIFIED).name} with a "
            "whole-body verdict. verified_rvas() still filters on the literal string 'IDENTICAL', "
            "which that file no longer writes, so the 'verified map wins' rule is not being "
            "applied here; build.rs re-applies it by address. Narrowing this selection changes "
            "the ledger's contents and wants its own commit."
        )

    text = render(rows, preserved)
    if args.refresh:
        target.write_text(text, encoding="utf-8")
        print(
            f"wrote {target} ({len(rows)} rows, {len(preserved)} hand-carried); "
            f"{len(missing)} constant(s) still unmapped"
        )
        return 0
    current = target.read_text(encoding="utf-8") if target.is_file() else ""
    if current == text:
        print(f"OK: {target.name} is current ({len(rows)} rows, {len(missing)} still unmapped)")
        return 0
    print(f"FAIL: {target.name} is out of date. Re-run with --refresh.")
    return 1


if __name__ == "__main__":
    sys.exit(main())
