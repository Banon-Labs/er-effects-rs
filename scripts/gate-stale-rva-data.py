#!/usr/bin/env python3
"""Route hand-built `base + SOME_RVA` data addresses through the 1.17 data map.

Why this is a separate tool from the call one
---------------------------------------------
`gate-stale-rva-calls.py` handles addresses that get executed, where a refusal has to become a
control-flow decision (return what, exactly?) and the risk is deleting behaviour. Data addresses
have no such problem: `er_game_base::mem::game_data_addr` returns `0` when the running build has
no verified mapping, and every one of these sites is already a read or an identity compare with an
existing "not the object I wanted" branch. Handing it `0` puts a refusal down the path the caller
already had.

The hazard being fixed is the quiet one. A stale data address does not crash -- the reads are
fault-safe -- so the comparison simply never matches and the feature behind it stops working with
nothing said. Measured 2026-08-29: `TITLE_OWNER_VTABLE_RVA` is `CS::TitleStep` in 1.16.2 and not a
vtable at all in 1.17, and its three scans had been finding no title owner, forever, silently.

Only constants one of the four LEDGERS in `MAPS` already carries are rewritten. A constant with no row would gain
nothing but noise: `game_data_addr` would return 0 where the raw value at least had a chance of
being right on some build. Getting the row is `map-data-rvas-1162-to-1170.py`'s job first.

Which constants those are was decided by a REGEX that could only read one spelling (fixed 2026-08-30)
-----------------------------------------------------------------------------------------------------
`mapped_constants()` used to learn a constant's address from `const FOO_RVA: usize = 0x1234;` and
nothing else, and the comment beside that regex said so out loud: "a constant defined from an enum
discriminant has no value here and falls back to matching by name." Falling back to the name means
the constant is only ever recognised when a generator happened to write its name into a map's label
column -- and the labels are written from a different spelling of the same set, so most do not.

The code now knows what the comment knew. Values come from `scripts/rva_symbols.py`, which resolves
every declaration form this tree uses -- `: u32` / `: u64` as well as `: usize`, enum discriminants,
`const A: usize = B;` re-exports across crates, `use X as Y` aliases -- so "mapped" is decided by
the address, which is what a map row is keyed on, instead of by the spelling.

Measured, on the tree as of 2026-08-30: 530 -> 728 constants recognised as mapped, and the sweep
went from 7 ungated data sites to 16. (728 -> 749 on 2026-08-31, when `needed.tsv` was added to
`MAPS` -- see the comment there. The site count did not move: all 21 of the newly-named constants
already resolved to addresses another ledger carried.) All nine of the newly-visible sites are real
raw
`base + FOO_RVA` reads whose constant is fully mapped and was invisible because it is declared by
derivation or as an enum discriminant:

    FILE_OPEN_RVA                    = er_game_base::rva::TITLE_SCALEFORM_FILE_OPEN_RVA
    PROFILE_SLOT_ACTIVATE_RVA        = ProfileLoadMenuRva::ProfileSlotActivate as usize
    NODE_FINALIZER_RVA               = er_game_base::rva::SL_RELEASE_REQUEST_RVA
    TITLE_CUSTOM_COVER_PROFILE_RENDER_INIT_RVA
                                     = er_loading_portrait_core::PROFILE_TABLE_BUILDER_RVA

Nothing was lost: the old set is a subset of the new one.

Reporting is the default; Rewriting takes `--write`
---------------------------------------------------
Inverted 2026-08-30. The old default was to rewrite, and `--dry-run` was the flag you had to
remember -- which put the destructive action one forgotten word away, on a tool whose whole job is
editing source in bulk across the workspace. It cost exactly what it was always going to cost: an
agent ran the bare command while investigating, silently rewrote
`crates/er-quickload/src/experiments/trace/native_result_map_hooks.rs`, and had to `git checkout`
it back out. Nobody asks a scanner for a report and gets edits; they do the reverse by accident.
`--dry-run` is still accepted and still means "do not write", so old invocations keep working.

Usage
    python3 scripts/gate-stale-rva-data.py                # report only
    python3 scripts/gate-stale-rva-data.py --write        # actually rewrite
    python3 scripts/gate-stale-rva-data.py --selftest
"""

from __future__ import annotations

import argparse
import glob
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
# One dialect, not four. `rva_symbols` resolves every declaration spelling in this tree to a value,
# and blanks comments and string bodies before anything is matched. Both matter here: the first
# decides which constants the resolver can answer for, and the second decides whether a `//`
# paragraph describing `base + FOO_RVA` is treated as a site to rewrite.
try:  # noqa: E402 - repo-local; the sys.path line above is what makes it work
    import rva_symbols
    from rva_symbols import code_only
except ImportError as missing:  # a shared reader that cannot load must stop the tool, not degrade
    raise ImportError(
        "scripts/rva_symbols.py could not be imported, so constants declared as enum discriminants "
        "or by derivation cannot be resolved to addresses and prose cannot be blanked. Without it "
        "this tool silently under-reports (7 sites instead of 16, measured 2026-08-30) and can "
        "rewrite text inside a comment. Fix the import rather than restoring a local regex."
    ) from missing

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RECON = os.path.join(REPO, "docs", "recon")
# `game_data_addr` -> `resolve_game_address` answers out of one table, `VERIFIED_1162_TO_1170`
# (crates/er-game-base/game_build.rs:764), and `crates/er-game-base/build.rs::emit_address_map`
# seeds that table from three ledgers: `verified.tsv`, `needed.tsv` and `data.tsv`. So "already
# mapped" has to mean their union. Scoring against the data map alone said 167 sites needed a new
# row; against the union it is 110, and 57 of the difference were free wins sitting in plain sight.
#
# `needed.tsv` was missing from this TUPLE while the comment above it claimed all three (fixed
# 2026-08-31). It cost nothing measurable only by coincidence: `needed-verified.tsv` is the same
# 357 source addresses put through the byte comparison, so the RVA half of the union was already
# complete -- but the two files' name columns are not the same set (414 labels against 394, 21 of
# them reachable only through `needed.tsv`), and nothing keeps them identical. The next `--refresh`
# that adds a row to one and not the other silently narrows the "already mapped" set, and a
# narrower set means a real ungated `base + FOO_RVA` is skipped with no output at all. A comment
# that names three files and code that opens two is not a documentation slip; it is the reviewer
# being told the union is complete when it is not.
#
# `needed-verified.tsv` stays even though it seeds the detour table rather than this one: its rows
# are a subset of `needed.tsv`'s addresses, and its column 5 carries labels the others do not.
# The two ledgers `build.rs` reads and this tool must NOT: `rva-1170-quarantine.tsv` and the
# `DIVERGES` rows, which build.rs SUBTRACTS. A subtracted address is one `game_data_addr` refuses
# and answers `0` for -- which is precisely the refusal this rewrite exists to deliver -- so those
# constants still want routing through the resolver, not excluding from it.
#
# Name column per map, or None where the map has no constant column at all. `verified.tsv` has
# none -- its column 5 is a signature description -- and reading it as a name pulled junk into the
# "already mapped" set. Match by RVA there, which is the only key every map actually shares.
MAPS = (
    (os.path.join(RECON, "rva-map-1162-to-1170.data.tsv"), 2),
    (os.path.join(RECON, "rva-map-1162-to-1170.needed.tsv"), 2),
    (os.path.join(RECON, "rva-map-1162-to-1170.needed-verified.tsv"), 5),
    (os.path.join(RECON, "rva-map-1162-to-1170.verified.tsv"), None),
)
# Which ledgers `build.rs` actually seeds `VERIFIED_1162_TO_1170` from, re-derived from that file
# rather than trusted from the comment above. The omission this replaces was invisible precisely
# because the only statement of the intended set was prose; `_map_coverage_control` turns the
# claim into an assertion, so dropping one from maps -- or build.rs gaining a fourth source --
# fails the selftest instead of quietly shrinking what this tool considers mapped.
BUILD_RS = os.path.join(REPO, "crates", "er-game-base", "build.rs")
# `emit_address_map` up to the point where it starts SUBTRACTING. Every ledger constant named in
# there is a source of the table; the ones named after it (`QUARANTINE`, and `AUDITED_DETOURS`
# under its `let _ =`) are the subtraction and the deliberately-unwired file, which this tool must
# not treat as coverage.
BUILD_TABLE_REGION = re.compile(r"fn emit_address_map(.*?)let mut held_back", re.S)
BUILD_LEDGER_DECL = re.compile(r'const\s+(\w+)\s*:\s*&str\s*=\s*"([^"]*docs/recon/[^"]+\.tsv)"')
IMAGE_BASE = 0x140000000
# The MATCHER this file used to decide "which constant is which address", frozen as a literal so
# `--selftest` can prove the replacement is load-bearing. Its own comment admitted the hole:
#
#     # `const FOO_RVA: usize = 0x1234;` -- only the literal form; a constant defined from an enum
#     # discriminant has no value here and falls back to matching by name.
#
# Spelled out, not composed. A control assembled from live pattern pieces widens when they widen,
# and "the old matcher misses this" quietly becomes "the new matcher misses this" -- the opposite
# claim. `check-stale-rva-calls.py` nearly shipped exactly that.
LEGACY_DECLARATION = re.compile(
    r"const\s+([A-Z0-9_]*RVA[A-Z0-9_]*)\s*:\s*usize\s*=\s*(0x[0-9a-fA-F_]+)"
)
RESOLVER = "er_game_base::mem::game_data_addr"

# `$base` included on purpose: several sites live inside macro bodies, and the dollar has to stay
# attached to the identifier rather than being swallowed into the replacement.
#
# The base is any LOWERCASE binding, not a fixed list of four spellings. The list used to be
# `(base|module_base|image_base|game_base)`, and `crates/er-loading-portrait-core/src/lookat_stage_camera.rs`
# writes `if let Ok(b) = game_module_base()` -- so `b + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA`
# was invisible to this scanner and stayed on a 1.16.2 address through the whole 1.17 migration, with
# no refusal logged because a raw add never reaches the resolver. A scanner keyed on what the author
# happened to call the variable finds only the sites written by authors who agreed with it.
#
# What keys the match instead is the CONSTANT: `*_RVA*`, and only when `mapped_constants()` says the
# resolver can answer for it. Two deliberate restrictions remain on the base itself:
#   * it must start lowercase, so a SCREAMING_CASE constant added to an RVA (`SOME_OFFSET + FOO_RVA`)
#     is not mistaken for a module base;
#   * it must not be preceded by `.`, because rewriting `self.base + FOO_RVA` would drop the
#     receiver and produce `game_data_addr(base, ...)` -- which can still compile where a local
#     `base` exists, and that is a silent wrong answer rather than a loud one.
SITE = re.compile(
    r"(?<![.\w])(?P<base>\$?[a-z_][A-Za-z0-9_]*)\s*\+\s*"
    r"(?P<prefix>(?:\w+::)*)(?P<const>[A-Z0-9_]*RVA[A-Z0-9_]*)\b"
)
# `base.checked_add(FOO_RVA)?` is the same hand-built address wearing a different costume, and the
# `base + FOO_RVA` pattern walked straight past six of them. That gap black-screened the game on
# 2026-08-29: `find_title_owner_by_vtable` ended up comparing a resolved state table against a raw
# 1.16.2 vtable, so the scan could never match, the autoload waited forever for a title owner that
# would never be found, and the boot cover -- which holds until the game's own loading screen
# lights -- never released. A converter that covers one spelling of an idiom covers none of it.
CHECKED_ADD_SITE = re.compile(
    r"(?<![.\w])(?P<base>\w+)\.checked_add\(\s*(?P<prefix>(?:\w+::)*)(?P<const>[A-Z0-9_]*RVA[A-Z0-9_]*)\s*\)\?"
)
# Tested against the ENCLOSING call, never a window of surrounding lines.
#
# It used to be tested against a +/-2-line window, and that is precisely backwards: the shape this
# whole tool exists to catch is a half-converted site, one gated address sitting beside a raw one,
# and a window makes the gated neighbour hide the raw one. Both of the addresses found broken on
# 2026-08-30 were hidden this way -- `dialog_active.rs` computes `want_a` raw and `want_b` through
# the resolver four lines apart, and `profile_select_flow.rs` had both halves of a single `||`
# written the two different ways, on one line.
#
# What "already gated" actually means is structural, not positional: the raw add is an argument to a
# resolver, as in `resolve_game_address(base + SPLASH_SKIP_FN_RVA, "SPLASH_SKIP_FN_RVA")`, which
# takes an absolute address and is correct as written. A resolver call merely near the site says
# nothing. `enclosing_calls_gated` answers the structural question by walking outward through the
# unclosed parentheses that contain the match, so a neighbour cannot hide anything and an argument
# is still recognised across the line breaks of a multi-line call.
ALREADY_GATED = re.compile(r"game_data_addr|game_rva|resolve_game_address|game_ptr")
# How far back `enclosing_calls_gated` looks for the parentheses that contain a match. Wide enough
# for a multi-line call's argument list, deliberately too narrow to reach the enclosing `fn` name of
# a typical body -- a function that merely has a resolver-ish name should not silence its contents.
ENCLOSING_SCAN_CHARS = 400
# A hook target must stay a raw `base + rva`: `MhHook::new` resolves it itself, through the detour
# resolver, which is a stricter test than the call one. Pre-resolving it does one of two bad
# things -- translates the address twice (the bug in bd resolve-twice-refuses-double-translation),
# or hands MinHook the `0` that `game_data_addr` returns on a refusal, which is an install at
# address zero. Measured 2026-08-29: an earlier version of this tool rewrote 18 such sites,
# including a whole `let targets = [...]` list whose `MhHook::new` sat fourteen lines below the
# addresses it collected -- hence the deliberately wide window.
#
# `register_shared_hook` was missing, and it is the same footgun with a different name (2026-08-31).
# Its target arrives unresolved by contract -- `er_hook::register_shared_hook_with_budget` resolves
# once, after the branch, in whichever image ends up owning the detour, and its own doc says so:
# "`target` arrives UNRESOLVED and each branch resolves it exactly once". Two live sites pass
# `base + FOO_RVA` straight into it and say the same thing in their own comments:
#
#     crates/er-armament-icons/src/gfx_equip_hook.rs:714
#     crates/er-diag-harness/src/dlc_roots_trace.rs:139   ("handed over UNTRANSLATED on purpose")
#
# `--write` would have routed both through `game_data_addr` and produced exactly the double-resolve
# that `scripts/check-double-resolved-hook-targets.py` gates against -- or, on a refusal, an install
# at address 0. `register_union_hook` was already here and covers its `_runtime_derived` /
# `_resolved` spellings by substring; `register_shared_hook` covers `_with_budget` the same way.
# `selftest`'s registrar control re-derives the list from er-hook rather than trusting this comment,
# so a new registrar added there turns this file red instead of silently becoming a rewrite target.
HOOK_TARGET = re.compile(
    r"MhHook::new|MH_CreateHook|register_union_hook|register_shared_hook"
    r"|detour|trampoline|hook as \*mut",
    re.I,
)
HOOK_WINDOW_LINES = 14
# Where the registrar names are re-derived from, for the selftest's drift control.
ER_HOOK_SOURCE = os.path.join(REPO, "crates", "er-hook", "src", "lib.rs")
HOOK_REGISTRAR_DECL = re.compile(r"\bfn\s+(register_\w*hook\w*)\s*\(")


def map_rvas() -> tuple[set[int], set[str]]:
    """`(1.16.2 addresses, label-column names)` carried by the ledgers in [`MAPS`]."""
    names: set[str] = set()
    rvas: set[int] = set()
    for path, column in MAPS:
        try:
            with open(path, encoding="utf-8") as handle:
                for line in handle:
                    if not line.startswith("0x"):
                        continue
                    parts = line.rstrip("\n").split("\t")
                    try:
                        value = int(parts[0], 16)
                    except ValueError:
                        continue
                    rvas.add(value - IMAGE_BASE if value >= IMAGE_BASE else value)
                    if column is not None and len(parts) > column:
                        names.add(parts[column].strip())
        except OSError:
            continue
    return rvas, names


def constants_at(rvas: set[int], root=None) -> set[str]:
    """Every symbol in `crates/` whose resolved value is one of `rvas`.

    The address is the key, not the spelling. A map row is an address pair; whether this tool can
    use one depends on whether it can tell that `FOO_RVA` is that address, and this tree writes
    that fact five different ways. `rva_symbols` evaluates all of them:

        const FILE_OPEN_RVA: usize = er_game_base::rva::TITLE_SCALEFORM_FILE_OPEN_RVA;
        const PROFILE_SLOT_ACTIVATE_RVA: usize = ProfileLoadMenuRva::ProfileSlotActivate as usize;
        const CAP_BUILDER_RVA: u32 = 0x826510;
        MenuJobWait = 0x00b0d400,            // inside #[repr(u32)] enum MenuTraceRva
        use er_game_base::rva::GAME_MAN_SINGLETON_RVA as GAME_MAN_GLOBAL_RVA;

    The old literal-only regex saw only the third of those with a `usize` type, and every other
    constant fell back to matching by name against a label column that mostly does not carry it.
    """
    index = rva_symbols.index(root)
    found: set[str] = set()
    for decl in index.decls:
        if not index.in_universe(decl):
            continue
        for value in decl.value or ():
            rva = value - IMAGE_BASE if value >= IMAGE_BASE else value
            if rva in rvas:
                found.add(decl.symbol)
                break
    # An alias is another name for a mapped value, and the rewrite reads the name at the use site.
    for alias, target in index.aliases.items():
        if target.split("::")[-1] in found:
            found.add(alias)
    return found


def mapped_constants(root=None) -> set[str]:
    """Every constant the resolver can answer for, by name -- resolved through both keys.

    A constant counts as mapped when its name appears in a map that has a name column, or when its
    resolved value appears in any map. The second half is what carries the load: `verified.tsv`
    carries no names at all, and the label columns of the other two are written from one spelling
    of a set that is declared in five.
    """
    rvas, names = map_rvas()
    return names | constants_at(rvas, root)


def enclosing_calls_gated(text: str, pos: int, limit: int = ENCLOSING_SCAN_CHARS) -> bool:
    """Is the expression at `pos` an argument to a call whose name is already a resolver?

    Walks left from `pos` matching parentheses. A `)` deepens; a `(` at depth zero is a call that
    still contains `pos`, so the identifier immediately before it is read and tested, then the walk
    continues outward through the next enclosing call. Nothing else is examined -- a resolver call
    that is a sibling, a neighbour or a previous statement never reaches depth zero from here, which
    is exactly the difference between "this address is already resolved" and "an address near it
    is".
    """
    depth = 0
    index = pos - 1
    stop = max(0, pos - limit)
    while index >= stop:
        char = text[index]
        if char == ")":
            depth += 1
        elif char == "(":
            if depth == 0:
                start = index
                while start > 0 and (text[start - 1].isalnum() or text[start - 1] in "_:!."):
                    start -= 1
                if ALREADY_GATED.search(text[start:index]):
                    return True
                index = start - 1
                continue
            depth -= 1
        index -= 1
    return False


def rewrite(path: str, mapped: set[str], dry_run: bool, found: list | None = None) -> int:
    """Route every ungated `base + FOO_RVA` in `path` through the resolver. Returns the count.

    Both idioms are collected against the original line and the replacements are spliced in by
    offset, rather than run as two chained `re.sub` passes. The chained form was fine while the
    decisions were purely local, but `enclosing_calls_gated` needs each match's offset in the
    UNMODIFIED file, and the first substitution moves every offset after it.
    """
    text = open(path, encoding="utf-8").read()
    lines = text.splitlines(keepends=True)
    # Match the code, SPLICE the source. `code_only` blanks comments and string bodies to spaces
    # without moving anything, so `masked` has byte-for-byte the same offsets and line breaks as
    # `text` -- the matches are found in code and the replacements are still built from the real
    # bytes. The old `startswith("//")` test caught a whole-line comment and nothing else: a
    # trailing `// like base + BAR_RVA`, a `/* ... */` block and a quoted example all read as
    # sites, which on a tool that REWRITES files means editing a sentence.
    masked = code_only(text)
    masked_lines = masked.splitlines(keepends=True)
    out, changed = [], 0
    offset = 0
    for index, line in enumerate(lines):
        line_start = offset
        offset += len(line)
        code_line = masked_lines[index] if index < len(masked_lines) else line
        # The hook window stays wide (and stays a window): `MhHook::new` can sit fourteen lines
        # below the `let targets = [...]` list whose addresses it installs, and pre-resolving one of
        # those is a real bug. Being gated is the opposite case and is decided per match, below.
        #
        # And it deliberately reads the raw lines, not the masked ones. Masking exists so prose is
        # not counted as a finding; this test is not looking for a finding, it is looking for a
        # reason to keep its hands off the file. A comment that says "the MhHook::new below
        # installs these" is exactly such a reason, and reading it costs a rewrite that could have
        # been made -- while ignoring it costs a hook target pre-resolved to `0`.
        hook_window = "".join(
            lines[max(0, index - HOOK_WINDOW_LINES) : index + HOOK_WINDOW_LINES + 1]
        )
        if HOOK_TARGET.search(hook_window):
            out.append(line)
            continue

        matches = sorted(
            list(SITE.finditer(code_line)) + list(CHECKED_ADD_SITE.finditer(code_line)),
            key=lambda found: found.start(),
        )
        rebuilt, cursor = [], 0
        for match in matches:
            if match.start() < cursor:
                # The two idioms overlapped; the earlier match already consumed these bytes.
                continue
            if match.group("const") not in mapped:
                continue
            if ALREADY_GATED.search(match.group(0)):
                continue
            # ...on the masked text: a resolver name inside a comment gates nothing.
            if enclosing_calls_gated(masked, line_start + match.start()):
                continue
            rebuilt.append(line[cursor : match.start()])
            constant = match.group("prefix") + match.group("const")
            rebuilt.append(
                f'{RESOLVER}({match.group("base")}, {constant}, "{match.group("const")}")'
            )
            cursor = match.end()
            changed += 1
            if found is not None:
                found.append((path, index + 1, match.group("const"), line.strip()))
        rebuilt.append(line[cursor:])
        out.append("".join(rebuilt))
    if changed and not dry_run:
        open(path, "w", encoding="utf-8").write("".join(out))
    return changed


ENUM_AND_DERIVED_SOURCE = """
#[repr(u32)]
pub enum MenuTraceRva {
    TaskEnqueue = 0x007a7b60,
    MenuJobWait = 0x00b0d400,
}
pub const TITLE_MENU_JOB_WAIT_RVA: usize = MenuTraceRva::MenuJobWait as usize;
pub const FILE_OPEN_RVA: usize = other_crate::TITLE_SCALEFORM_FILE_OPEN_RVA;
pub const TITLE_SCALEFORM_FILE_OPEN_RVA: usize = 0x11ced80;
pub const CAP_BUILDER_RVA: u32 = 0x826510;
"""


def _declaration_control() -> list[str]:
    """Prove the value resolver sees constants the frozen literal-only regex could not.

    The control is the address the old MATCHER called UNDECLARED. 0xb0d400 is declared in this
    tree only as an enum discriminant -- `MenuJobWait` inside `#[repr(u32)] enum MenuTraceRva`,
    reached through `TITLE_MENU_JOB_WAIT_RVA` -- and a sibling gate recommended deleting its map
    row on the strength of a `const NAME: usize = 0x..;` search coming back empty.

    Each case asserts both halves: the frozen pre-fix regex must miss it, and the resolver must
    catch it. A case both see would pass on the broken tool and prove nothing.
    """
    import tempfile

    out = []
    with tempfile.TemporaryDirectory() as tmp:
        path = os.path.join(tmp, "crates", "a", "src", "lib.rs")
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w", encoding="utf-8") as handle:
            handle.write(ENUM_AND_DERIVED_SOURCE)
        # Pretend all four addresses have map rows, which is the state the sweep cares about.
        found = constants_at({0xB0D400, 0x11CED80, 0x826510, 0x7A7B60}, root=tmp)
    legacy = {name for name, _ in LEGACY_DECLARATION.findall(ENUM_AND_DERIVED_SOURCE)}
    for symbol, why in (
        ("MenuJobWait", "an enum discriminant"),
        ("TITLE_MENU_JOB_WAIT_RVA", "a constant defined FROM an enum discriminant"),
        ("FILE_OPEN_RVA", "a constant re-exported from another crate"),
        ("CAP_BUILDER_RVA", "a `: u32` constant, not `: usize`"),
    ):
        if symbol in legacy:
            out.append(
                f"declaration control: the FROZEN pre-fix regex already saw {symbol} ({why}), so "
                "this control proves nothing -- pick a spelling it genuinely could not read"
            )
        if symbol not in found:
            out.append(f"declaration control: the resolver did not see {symbol} ({why})")
    # The negative half: only the literal `usize` form was ever visible to the old regex, and the
    # resolver must still see that one -- a widening that lost the old set would be a swap, not a
    # widening.
    if "TITLE_SCALEFORM_FILE_OPEN_RVA" not in legacy:
        out.append("declaration control fixture is wrong: the plain literal form must be legacy-visible")
    return out


def _masking_control() -> list[str]:
    """Prove a `base + FOO_RVA` inside a comment is no longer rewritten -- and a real one still is.

    The old skip was `line.lstrip().startswith("//")`, which sees a whole-line comment and nothing
    else. A trailing comment, a `/* */` block and a quoted example all read as sites, and this is a
    tool that edits FILES: the finding it invents is a sentence it rewrites.
    """
    import tempfile

    prose = (
        "fn probe(base: usize) {\n"
        "    let live = base + FOO_VTABLE_RVA;         // and once more: base + BAR_VTABLE_RVA\n"
        "    /* historical: base + BLOCK_VTABLE_RVA */\n"
        '    let note = "base + QUOTED_VTABLE_RVA";\n'
        "}\n"
    )
    mapped = {
        "FOO_VTABLE_RVA",
        "BAR_VTABLE_RVA",
        "BLOCK_VTABLE_RVA",
        "QUOTED_VTABLE_RVA",
    }
    out = []
    with tempfile.TemporaryDirectory() as tmp:
        path = os.path.join(tmp, "prose.rs")
        with open(path, "w", encoding="utf-8") as handle:
            handle.write(prose)
        sites: list = []
        count = rewrite(path, mapped, dry_run=True, found=sites)
    names = sorted(constant for _p, _l, constant, _t in sites)
    if names != ["FOO_VTABLE_RVA"]:
        out.append(
            f"masking control: rewrote {names}, expected only ['FOO_VTABLE_RVA'] -- the other "
            "three are a trailing comment, a block comment and a string body"
        )
    if count != 1:
        out.append(f"masking control: counted {count} site(s), expected 1")
    # Non-VACUITY: the pre-fix line filter really did read three of those four as sites, so the
    # control is not asserting something that was already true.
    legacy_visible = [
        line
        for line in prose.splitlines()
        if not line.lstrip().startswith("//") and SITE.search(line)
    ]
    if len(legacy_visible) < 3:
        out.append(
            f"masking control is vacuous: the old whole-line-comment filter saw only "
            f"{len(legacy_visible)} of these lines as sites, so masking changed nothing"
        )
    return out


SHARED_HOOK_SOURCE = """fn install() {
    let base = game_module_base().unwrap();
    match unsafe {
        er_hook::register_shared_hook(base + FILE_OPEN_RVA, file_open_hook, &FILE_OPEN_ORIG)
    } {
        Ok(route) => log(route),
        Err(status) => log(status),
    }
}
"""


def _shared_hook_control() -> list[str]:
    """A `register_shared_hook(base + FOO_RVA, ...)` target must be left raw.

    The exclusion is the assertion, and it is made end to end through `rewrite` rather than against
    the regex alone -- what matters is that no edit is produced, not that a pattern matches. The
    second half is the non-vacuity control: the same fixture with the registrar renamed to a
    function this tool has never heard of must produce one rewrite, or "0 sites" would be true
    because the fixture matches nothing and the exclusion would be proving itself.

    Why the site must stay raw: `er_hook::register_shared_hook_with_budget` resolves `target` once,
    after it has picked the image that will own the detour. Handing it a `game_data_addr`-resolved
    address translates twice -- and a 1.17 destination can itself be another row's 1.16.2 source, so
    the second lookup does not merely miss, it lands on a third unrelated function
    (`scripts/check-double-resolved-hook-targets.py` is the gate that forbids the shape). On a
    refusal instead, `game_data_addr` returns 0 and MinHook is asked to install at address zero.
    """
    import tempfile

    out = []
    with tempfile.TemporaryDirectory() as tmp:
        excluded = os.path.join(tmp, "shared.rs")
        with open(excluded, "w", encoding="utf-8") as handle:
            handle.write(SHARED_HOOK_SOURCE)
        sites: list = []
        count = rewrite(excluded, {"FILE_OPEN_RVA"}, dry_run=True, found=sites)
        if count:
            out.append(
                f"shared-hook control: rewrote {count} site(s) inside a register_shared_hook call "
                f"({[c for _p, _l, c, _t in sites]}) -- that address must reach the registrar "
                "UNRESOLVED or it is translated twice"
            )
        # Non-VACUITY: the same fixture, same constant, a registrar nobody excludes.
        loud = os.path.join(tmp, "loud.rs")
        with open(loud, "w", encoding="utf-8") as handle:
            handle.write(SHARED_HOOK_SOURCE.replace("register_shared_hook", "note_the_address"))
        if rewrite(loud, {"FILE_OPEN_RVA"}, dry_run=True) != 1:
            out.append(
                "shared-hook control is vacuous: the fixture produces no rewrite even with the "
                "registrar renamed, so the exclusion above proved nothing"
            )

    # Drift control. Re-derive the registrar names from er-hook instead of trusting the hand-list in
    # HOOK_TARGET's comment. A new `register_*_hook` added there is a new way to hand this tool a
    # deliberately-unresolved address, and it should turn this file red rather than quietly become a
    # rewrite target -- which is exactly how `register_shared_hook` went unnoticed.
    try:
        source = open(ER_HOOK_SOURCE, encoding="utf-8", errors="replace").read()
    except OSError:
        out.append(f"registrar control: could not read {ER_HOOK_SOURCE} to re-derive the names")
        return out
    registrars = sorted(set(HOOK_REGISTRAR_DECL.findall(source)))
    if len(registrars) < 2:
        out.append(
            f"registrar control is vacuous: found {len(registrars)} registrar(s) in er-hook, so "
            "the check below compares against nothing"
        )
    for name in registrars:
        if not HOOK_TARGET.search(f"    er_hook::{name}(base + FOO_RVA, handler, &ORIG)"):
            out.append(
                f"HOOK_TARGET does not know er_hook::{name}; a `base + FOO_RVA` handed to it would "
                "be pre-resolved and then resolved again by the registrar"
            )
    return out


def _map_coverage_control() -> list[str]:
    """Every ledger `build.rs` seeds the resolver's table from must be in [`MAPS`].

    The claim was prose and the prose was wrong. `MAPS` carried three files while the comment above
    it said "ALL THREE maps" -- and one of the three it meant, `needed.tsv`, was simply absent, with
    nothing anywhere to say so. It cost nothing measurable only because `needed.tsv` and
    `needed-verified.tsv` happen to hold the same 357 source addresses right now; they are written
    by different scripts and nothing keeps them equal.

    So the set is re-derived from `build.rs` here instead of being described. A ledger dropped from
    `MAPS`, or a fourth source added to `build.rs`, fails this rather than silently narrowing what
    counts as mapped -- and a narrower "already mapped" set means a real ungated `base + FOO_RVA`
    is skipped with no output at all, which is the one failure this tool cannot report on itself.
    """
    out = []
    try:
        source = open(BUILD_RS, encoding="utf-8", errors="replace").read()
    except OSError:
        return [f"map-coverage control: could not read {BUILD_RS} to re-derive the ledger set"]
    ledgers = dict(BUILD_LEDGER_DECL.findall(source))
    if len(ledgers) < 3:
        return [
            f"map-coverage control is vacuous: found {len(ledgers)} ledger declaration(s) in "
            "build.rs, so the comparison below is against nothing"
        ]
    region = BUILD_TABLE_REGION.search(source)
    if not region:
        return [
            "map-coverage control: could not find where build.rs assembles the call/read table, so "
            "which ledgers seed it cannot be re-derived"
        ]
    seeds = {
        name: rel
        for name, rel in ledgers.items()
        if re.search(rf"\b{re.escape(name)}\b", region.group(1))
    }
    if len(seeds) < 3:
        return [
            f"map-coverage control is vacuous: only {len(seeds)} ledger(s) are named in the "
            "table-construction region, which is fewer than build.rs demonstrably reads"
        ]
    listed = {os.path.realpath(path) for path, _column in MAPS}
    for name, rel in sorted(seeds.items()):
        path = os.path.realpath(os.path.join(os.path.dirname(BUILD_RS), rel))
        if path not in listed:
            out.append(
                f"map-coverage control: build.rs seeds the resolver's table from {name} "
                f"({os.path.relpath(path, REPO)}) and MAPS does not list it. Its rows are invisible "
                "to `mapped_constants`, so every constant reachable only through it is scored "
                "UNMAPPED and its ungated sites are skipped in silence"
            )
    # Non-VACUITY: with one ledger removed, the same check must object. If it does not, the loop
    # above is comparing a set against itself.
    survivor = sorted(seeds)[0]
    trimmed = listed - {
        os.path.realpath(os.path.join(os.path.dirname(BUILD_RS), seeds[survivor]))
    }
    if len(trimmed) == len(listed):
        out.append(
            f"map-coverage control is vacuous: dropping {survivor} changed nothing, so MAPS does "
            "not actually contain the ledgers this check compares against"
        )
    return out


def selftest() -> int:
    failures = []
    mapped = mapped_constants()
    failures.extend(_map_coverage_control())
    if len(mapped) < 200:
        failures.append(f"only {len(mapped)} constants read from the data map")
    failures.extend(_declaration_control())
    failures.extend(_masking_control())
    failures.extend(_shared_hook_control())

    # Non-VACUITY of the inputs, before anything is concluded from them. A walk that reads nothing
    # makes "no sites" and "I did not look" the same sentence, and only one is good news.
    index = rva_symbols.index()
    if index.files_read < 200:
        failures.append(f"the symbol index read only {index.files_read} sources; the walk is broken")
    if index.universe_size() < 500:
        failures.append(
            f"only {index.universe_size()} address-capable declarations; the resolver is not reading"
        )
    map_addresses, map_names = map_rvas()
    if len(map_addresses) < 200:
        failures.append(f"only {len(map_addresses)} addresses read from the ledgers in MAPS")
    # Nothing was lost. The frozen regex's answers must all survive into the new set, or this is a
    # swap rather than a widening.
    legacy_names = set()
    for path in glob.glob(os.path.join(REPO, "crates", "**", "*.rs"), recursive=True):
        try:
            source = open(path, encoding="utf-8", errors="replace").read()
        except OSError:
            continue
        for name, literal in LEGACY_DECLARATION.findall(source):
            if int(literal.replace("_", ""), 16) in map_addresses:
                legacy_names.add(name)
    if len(legacy_names) < 50:
        failures.append(
            f"the frozen regex resolved only {len(legacy_names)} mapped constants in the live "
            "tree, so the 'nothing was lost' check below compares against nothing"
        )
    lost = sorted(legacy_names - mapped)
    if lost:
        failures.append(f"the widening LOST {len(lost)} constant(s) the old regex found: {lost[:6]}")
    if len(mapped) <= len(legacy_names | map_names):
        failures.append(
            f"the resolver added nothing: {len(mapped)} mapped constants against "
            f"{len(legacy_names | map_names)} from the frozen regex plus the label columns"
        )
    match = SITE.search("if vt != base + FOO_VTABLE_RVA {")
    if not match or match.group("base") != "base":
        failures.append("SITE did not capture a plain `base +`")
    match = SITE.search("if vt == $base + FOO_VTABLE_RVA {")
    if not match or match.group("base") != "$base":
        failures.append("SITE lost the `$` on a macro base -- the rewrite would not compile")
    checked = CHECKED_ADD_SITE.search("let want = base.checked_add(FOO_VTABLE_RVA)?;")
    if not checked or checked.group("const") != "FOO_VTABLE_RVA":
        failures.append("CHECKED_ADD_SITE missed `base.checked_add(FOO_RVA)?` -- six such sites once black-screened the game")
    if not ALREADY_GATED.search("game_data_addr(base, FOO_RVA, \"FOO_RVA\")"):
        failures.append("ALREADY_GATED would rewrite an already-gated site twice")
    if not HOOK_TARGET.search("let hook = unsafe { MhHook::new(target as *mut c_void, detour) };"):
        failures.append("HOOK_TARGET missed an MhHook::new install -- it would be pre-resolved")
    if HOOK_TARGET.search("if vt != base + FOO_VTABLE_RVA {"):
        failures.append("HOOK_TARGET wrongly claimed a plain vtable compare is a hook install")

    # The half-converted line. One gated address and one raw one in a single `||`, which is
    # verbatim the shape of `crates/er-title-flow/src/profile_select_flow.rs`. Under the old
    # window-based ALREADY_GATED test the gated half hid the raw half and this line was skipped.
    half = (
        'vt == base + MSGBOX_DIALOG_VTABLE_RVA || vt == '
        'er_game_base::mem::game_data_addr(base, SAVE_RETRY_DIALOG_VTABLE_RVA, '
        '"SAVE_RETRY_DIALOG_VTABLE_RVA")'
    )
    gated = SITE.sub(
        lambda m: f'{RESOLVER}({m.group("base")}, {m.group("const")}, "{m.group("const")}")',
        half,
    )
    if "base + MSGBOX_DIALOG_VTABLE_RVA" in gated:
        failures.append(
            "a raw `base + RVA` beside an already-gated one was skipped -- the half-converted "
            "shape is the whole point of this tool"
        )
    if gated.count(RESOLVER) != 2:
        failures.append(f"half-converted line produced {gated.count(RESOLVER)} resolver calls, want 2")

    # The base named `b`. `lookat_stage_camera.rs` binds the module base as `b`, which the old
    # hard-coded `(base|module_base|image_base|game_base)` alternation could not see.
    match = SITE.search("== b + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA")
    if not match or match.group("base") != "b" or not match.group("const").endswith("_RVA"):
        failures.append("SITE missed a one-letter module base -- a real site hid behind that for the whole 1.17 migration")

    # ...but the base still has to look like a binding. These two must not match, because rewriting
    # them produces something wrong rather than something gated.
    if SITE.search("let addr = TITLE_OWNER_SCAN_START_ADDRESS + FOO_VTABLE_RVA;"):
        failures.append("SITE treated a SCREAMING_CASE constant as a module base")
    if SITE.search("if vt == self.base + FOO_VTABLE_RVA {"):
        failures.append("SITE would drop the `self.` receiver, which can still compile and be silently wrong")

    # ALREADY_GATED must remain a real guard on the expression it is handed.
    if ALREADY_GATED.search("base + FOO_VTABLE_RVA"):
        failures.append("ALREADY_GATED matched a raw site -- it would skip every rewrite")

    # The ENCLOSING-call test, both directions. `resolve_game_address` takes an absolute address, so
    # a raw add inside its argument list is correct as written (`constants_autoload_state.rs:224`)
    # and must be left alone -- while the same raw add merely next to a resolver call must not be.
    argument = 'er_game_base::game_build::resolve_game_address(base + SPLASH_SKIP_FN_RVA, "SPLASH_SKIP_FN_RVA")'
    found = SITE.search(argument)
    if not found:
        failures.append("SITE stopped matching the resolve_game_address argument fixture")
    elif not enclosing_calls_gated(argument, found.start()):
        failures.append("a raw add INSIDE resolve_game_address(...) was not recognised as gated")
    sibling = 'vt == base + MSGBOX_DIALOG_VTABLE_RVA || game_data_addr(base, BAR_RVA, "BAR_RVA") != 0'
    found = SITE.search(sibling)
    if not found or enclosing_calls_gated(sibling, found.start()):
        failures.append("a resolver call SIBLING to a raw add was treated as if it gated it")
    # Across line breaks, which is what makes this different from testing the line alone.
    multiline = 'resolve_game_address(\n    base + SPLASH_SKIP_FN_RVA,\n    "SPLASH_SKIP_FN_RVA",\n)'
    found = SITE.search(multiline)
    if not found or not enclosing_calls_gated(multiline, found.start()):
        failures.append("a multi-line resolver call did not gate the argument on its own line")

    # Reporting must not write. A real `.rs` fixture with real sites, because this file can no
    # longer be its own: every `base + FOO_RVA` in it lives inside a comment or a string, and those
    # are now blanked before matching -- which is the point. A fixture that matches nothing would
    # make "the dry run wrote nothing" true for the wrong reason, so the count is asserted first.
    import tempfile

    with tempfile.TemporaryDirectory() as tmp:
        fixture = os.path.join(tmp, "fixture.rs")
        with open(fixture, "w", encoding="utf-8") as handle:
            handle.write(
                "fn probe(base: usize) -> bool {\n"
                "    let a = base + DRY_RUN_FIXTURE_VTABLE_RVA;\n"
                "    let b = base.checked_add(DRY_RUN_FIXTURE_VTABLE_RVA)?;\n"
                "    a == b\n"
                "}\n"
            )
        before = open(fixture, "rb").read()
        counted = rewrite(fixture, {"DRY_RUN_FIXTURE_VTABLE_RVA"}, dry_run=True)
        if counted != 2:
            failures.append(
                f"dry-run fixture found {counted} site(s), expected 2 -- the write check below "
                "proves nothing over a fixture that matches nothing"
            )
        if open(fixture, "rb").read() != before:
            failures.append("a dry run WROTE to a file; the safe default is not safe")
        # ...and the same fixture with --write on must actually change, or "did not write" is
        # indistinguishable from "cannot write".
        rewrite(fixture, {"DRY_RUN_FIXTURE_VTABLE_RVA"}, dry_run=False)
        if open(fixture, "rb").read() == before:
            failures.append("a --write run did NOT rewrite the fixture; the tool cannot convert")

    for line in failures:
        print(f"selftest FAIL {line}")
    print(f"selftest: {len(failures)} failure(s); {len(mapped)} mapped constants")
    return 1 if failures else 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--write",
        action="store_true",
        help="actually rewrite the files. Without it this only reports, which is the default "
        "because the bare command used to edit source across the whole workspace",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="accepted for compatibility; reporting is already the default and this cannot "
        "re-enable writing",
    )
    parser.add_argument(
        "--show",
        action="store_true",
        help="name every site: file, line, constant. A count alone says work exists without "
        "saying where, which is the one thing a report has to answer",
    )
    parser.add_argument("--selftest", action="store_true")
    parser.add_argument("paths", nargs="*")
    args = parser.parse_args()
    if args.selftest:
        return selftest()
    # `--dry-run` can only ever turn writing off, never on, so the two flags cannot contradict.
    write = args.write and not args.dry_run
    mapped = mapped_constants()
    paths = args.paths or glob.glob(os.path.join(REPO, "crates", "**", "*.rs"), recursive=True)
    total = 0
    sites: list = []
    for path in paths:
        count = rewrite(path, mapped, dry_run=not write, found=sites if args.show else None)
        if count:
            print(f"  {os.path.relpath(path, REPO)}: {count}")
            total += count
    for path, line_no, constant, text in sites:
        print(f"    {os.path.relpath(path, REPO)}:{line_no}  {constant}\n      {text}")
    print(f"{'routed' if write else 'would route'} {total} data site(s) through {RESOLVER}")
    if total and not write:
        print("nothing was written; re-run with --write to apply")
    return 0


if __name__ == "__main__":
    sys.exit(main())
