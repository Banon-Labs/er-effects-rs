#!/usr/bin/env python3
"""Guard the game singletons the telemetry oracles read: resolved in source, corroborated in the image.

What went wrong, and why nothing caught it
==========================================
`er-telemetry-core`'s `standalone_tick` read four game singletons through a closure that added the
1.16.2 RVA straight to the module base:

    let read_singleton = |rva: usize| -> usize {
        unsafe { er_game_base::mem::safe_read_usize(base + rva) }.unwrap_or(0)
    };

Every `.data` global moved on 1.17. The reads did not fail -- they succeeded against whatever now
occupies the old address. Across all 4,350 records of run `br-20260831-160354-2513`:

    oracle_game_data_man_ptr  = "0x6e614d6e6f697463"   little-endian ASCII "ctionMan"
    oracle_game_man_ptr       = "0x0"
    oracle_cs_menu_man_ptr    = "0x0"
    oracle_play_time_ms       = -1                     derived from GameDataMan
    oracle_flip_task_delta    = -1.0                   derived from CSFlipperImp
    oracle_flip_fixed_spf     = -1.0

`"ctionMan"` is eight bytes out of the middle of the RTTI type name
`.?AVNWSteamConnectionManager@DLNW3@@`, which 1.17 parks at the old GameDataMan address. The other
three stale slots landed in still-blank `.data`, and a zero pointer is indistinguishable from a
global the game has not created yet. That is the failure mode this gate exists for: a wrong
pointer oracle does not go quiet, it goes constant, and a constant is invisible.

Three existing gates all reported clean the whole time, correctly, because none of them is looking
at this:

  * `check-stale-rva-calls.py` keys on a named constant next to the module base. Here the constant
    is a closure parameter, so there is nothing at the read site to name.
  * `check-no-stale-callsite-rva.py` is about comparing live stack addresses to raw RVAs.
  * `check-oracle-writers.py` asks whether a counter has a writer, not where a read points.

What this gate checks
=====================
source half (always runs, ratcheted by `docs/recon/ungated-module-base-arithmetic.txt`). Any
arithmetic on the game module base whose right-hand side is a compiled-in 1.16.2 claim, when the
result is not handed to something that resolves it. The address of a game global has to come from
`er_game_base::mem::game_data_addr` (or `game_rva` / `read_global_ptr` / `resolve_game_address`),
which translates through the verified 1.16.2 -> 1.17 map and answers `0` for an address with no
mapping.

Widened 2026-08-31, because the fixed site's spelling is not the class. A sweep of the whole
workspace for the shape found three more live 1.17 defects, and not one was a `safe_read_*` with a
lowercase RVA:

  * `safe_read_usize(module_base + 0x3d6b7b0)` -- a HEX literal on a base that arrives as a
    function parameter (`er-title-flow/src/title_tick_cover.rs`, three sites; both RVAs are in the
    shipped ledger, and the same file resolves the same two addresses correctly elsewhere);
  * `CS_INGAME_PAD_TYPEID_RVAS.map(|rva| base + rva)` -- the RVA is an element of a const array and
    the result is compared, not read (`er-input-harness/src/pad_inject.rs`). That is the silent
    end of the family: the std::map walk simply never matches, so the in-world input drive is inert
    with no fault, no refusal and no counter moving;
  * `base.wrapping_add(MSGBOX_DIALOG_UPDATE_RVA)` -- a spelling no `+`-shaped pattern can see,
    including `check-stale-rva-calls.py`, whose entire job is `base + CONSTANT`.

So the two halves of the rule are decided from evidence, never from either identifier's name:

  Is it the module base?  From the binding. A `let` from `game_module_base`/`GetModuleHandle`, or
                          -- new -- a parameter that the same file passes as the first argument to
                          `game_data_addr` and friends, whose first parameter is the module base.
                          A name shadowed by a `for` pattern or a closure is not the module base,
                          which is what keeps `for &(base, cnt) in GROUPS` (a table of struct
                          offsets) and `let base = ersc_module_base()` (a different DLL) out.
  Is it a 1.16.2 claim?   From the RVA's provenance. A literal in [0x1000, 0x0800_0000), or a
                          binding that was not read out of the running image. An RVA the code read
                          from the live PE (`base + e_lfanew`, `base + vaddr`) is already correct
                          for the running build and translating it would be the bug -- the same
                          distinction `MhHook::new_runtime_derived` draws.

And a sum handed to `resolve_game_address` / `resolve_detour_address` / `MhHook::new` /
`register_union_hook` is correct and excluded: those perform the single resolve themselves, and
resolving first would translate twice, which lands on a third unrelated function whenever an
address is both one row's destination and another row's source.

Image half (skips when the two de-Arxan'd images are absent, e.g. in CI). For each singleton the
oracles read, the 1.17 address is RE-derived from the two images -- not read out of the ledger and
believed -- and compared against the shipped `rva-map-1162-to-1170.data.tsv` row:

  1. find every `mov <r64>, [rip+disp]` in the 1.16.2 image whose target is the 1.16.2 global;
  2. take a short window at each such site, blank the four displacement bytes, and keep only the
     windows that are unique in 1.16.2 -- a shape that already occurs twice cannot identify
     anything;
  3. find that masked window in the 1.17 image; keep only the ones that occur exactly once there;
  4. read the 1.17 displacement and let each site vote for a target.

A site whose surrounding code was edited between the builds simply fails to match and casts no
vote, so an edit costs evidence rather than producing a wrong answer. `MIN_VOTES` independent
agreeing sites and unanimity among the sites that did match are required.

Then the shape of the destination is checked, because the ledger being self-consistent is not the
same as the address being a manager pointer:

  * the derived address is 8-byte aligned and its qword is zero at rest in the static image, which
    is what an uninitialised pointer global looks like;
  * it does not sit inside a printable-ASCII run -- the RTTI/string-literal test that the stale
    GameDataMan address fails on 1.17. That failure is asserted as a positive control, so a
    detector that has quietly stopped detecting cannot pass this gate.

    python3 scripts/check-oracle-singleton-globals.py             # enforce
    python3 scripts/check-oracle-singleton-globals.py --selftest  # prove the assertions can fail

Wiring (for whoever lands the current `scripts/check.sh` -- this file must not touch it). The two
lines already exist there, commented out at 253-254 with a note saying they were disarmed on
2026-08-31 only because `crates/er-input-harness/src/pad_inject.rs` and `crates/er-title-flow/*`
were mid-edit in someone's working tree. Those files have landed (f49f5e9b), the blinds are
re-derived against their current text, and both commands below exit 0 on this tree. Un-comment:

    python3 "$repo_root/scripts/check-oracle-singleton-globals.py" --selftest
    python3 "$repo_root/scripts/check-oracle-singleton-globals.py"

Leaving them commented is not neutral. This gate and `check-stale-rva-calls.py` are the two halves
of one defect class, and the sibling is wired; running only the half that keys on a named constant
is the configuration that let four telemetry pointer oracles report garbage for two whole runs
while three gates said clean.
"""

from __future__ import annotations

import argparse
import os
import re
import struct
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "scripts"))
try:  # a shared reader that cannot load must stop the gate, not degrade it into reading prose
    from rva_symbols import code_only
except ImportError as missing:
    raise ImportError(
        "scripts/rva_symbols.py could not be imported, so comments and string bodies cannot be "
        "blanked before matching. Without it the SOURCE half reads documentation as findings -- "
        "which is how another gate in this repo ended up with two of its three baseline rows "
        "being paragraphs about the hazard rather than instances of it."
    ) from missing
try:  # the shared vocabulary this gate and check-stale-rva-calls.py must not answer differently
    from module_base_arith import (
        ADD_METHODS,
        ADD_SITE_RE,
        BASE_NAMES,
        CORROBORATORS,
        LET_CONSUMER_WINDOW,
        MODULE_BASE_SOURCE_RE,
        MODULE_BASE_SOURCES,
        MODULE_SPAN_LIMIT,
        PE_HEADER_LIMIT,
        RESOLVING_CONSUMERS,
        RUNTIME_DERIVED_MARKS,
        SOURCE_EXEMPT,
        _identifiers,
        balanced_rhs as _balanced_rhs,
        binders,
        enclosing_call,
        is_module_base,
        is_resolver_fed,
        names_module_base,
        nearest_binder,
    )
except ImportError as missing:
    raise ImportError(
        "scripts/module_base_arith.py could not be imported. It holds the decisions this gate and "
        "scripts/check-stale-rva-calls.py must agree on -- what counts as the module base, what "
        "counts as reaching a resolver, and which VALUES are PE-header fields or image extents. "
        "Two copies of those answers is exactly how 29 sites ended up owned by neither gate on "
        "2026-08-31. Fix the import rather than restoring a local copy."
    ) from missing

IMAGE_BASE = 0x140000000
IMAGE_1162 = Path(os.environ.get("ER_DEOBF_1162", REPO / "eldenring-deobf.bin"))
IMAGE_1170 = Path(os.environ.get("ER_DEOBF_1170", REPO / "eldenring-deobf-1.17.bin"))
DATA_MAP = REPO / "docs" / "recon" / "rva-map-1162-to-1170.data.tsv"

# The singletons `er-telemetry-core::standalone_tick` reads to emit `oracle_game_data_man_ptr`,
# `oracle_game_man_ptr`, `oracle_cs_menu_man_ptr`, `oracle_play_time_ms` and the `oracle_flip_*`
# pair. Each was a constant for the entire 1.17 run cited above.
ORACLE_SINGLETONS = {
    "GAME_DATA_MAN_GLOBAL_RVA": 0x3D5DF38,
    "GAME_MAN_SINGLETON_RVA": 0x3D69918,
    "CS_MENU_MAN_GLOBAL_RVA": 0x3D6B7B0,
    "CS_FLIPPER_SINGLETON_RVA": 0x4589AD8,
}

# The one stale address whose 1.17 occupant is provably a string. It is the positive control for
# the literal detector: if this stops looking like a literal, the detector is broken, not the game.
LITERAL_CONTROL_RVA = 0x3D5DF38
LITERAL_CONTROL_TEXT = "ctionManager@DLNW3"

# `mov <r64>, [rip+disp32]` -- REX.W (or REX.WR) + 8B + a modrm whose mod=00 rm=101. That is how
# every one of these globals is loaded, and restricting to it keeps the scan to one instruction
# length so no decoder is needed.
MOV_RIP_SCAN = re.compile(rb"[\x48\x4c]\x8b[\x05\x0d\x15\x1d\x25\x2d\x35\x3d]", re.S)
MOV_RIP_LENGTH = 7
DISPLACEMENT_AT = 3
# Bytes of context kept with each reference site. Long enough that the shape is rarely ambiguous,
# short enough that it usually clears an unrelated edit elsewhere in the same function.
WINDOW = 24
# Independent agreeing sites required. One reference inside a function that happens to have been
# edited is exactly how a confident wrong address gets produced; three is past the point where a
# coincidence explains it.
MIN_VOTES = 3
# Reference sites examined per global before stopping. GameDataMan has 734 of them and each one
# costs two whole-image searches, so this is what keeps the gate at seconds rather than minutes.
# Unanimity is asserted over the sites actually examined, and the cap is comfortably above
# `MIN_VOTES` so a handful of edited neighbourhoods cannot starve the vote.
MAX_SITES = 24
# A printable-ASCII run this long, straddling the address, means it is inside a string literal.
LITERAL_RUN = 8


# --------------------------------------------------------------------------------------------
# Source half
# --------------------------------------------------------------------------------------------

# `safe_read_usize(base + rva)` -- a raw read at a module base plus a local binding. A constant
# would be SCREAMING_SNAKE or path-qualified; a lowercase identifier here is a value that came
# from somewhere else, which is precisely the indirection that hid the original defect from every
# name-keyed tool in the repo.
RAW_READ_RE = re.compile(
    r"\b(safe_read_\w+|read_usize|read_u8|read_u16|read_u32|read_bytes)\s*\(\s*"
    r"\$?(base|module_base|game_base|image_base)\s*\+\s*"
    r"([a-z_][a-z0-9_]*)\s*[,)]",
    re.S,
)
# What makes an identifier the module base rather than any other pointer called `base` now lives in
# `scripts/module_base_arith.py` (`MODULE_BASE_SOURCE_RE`, `names_module_base`, `is_module_base`),
# because `check-stale-rva-calls.py` has to answer the identical question and answering it twice is
# what let 29 sites fall between the two gates. The reasoning is preserved there in full.
#
# `binds_module_base` stays here: it is not the shared decision, it is the narrow pre-2026-08-31
# matcher, kept only so `--selftest` can prove each widening is load-bearing by showing the old
# shape misses the control.
def binds_module_base(text: str, name: str, before: int) -> bool:
    """Was `name` most recently bound from something that yields the game module base?"""
    binding = None
    for match in re.finditer(rf"\blet\s+(?:mut\s+)?{re.escape(name)}\s*(?::[^=;]+)?=", text):
        if match.start() < before:
            binding = match
        else:
            break
    if binding is None:
        # No binding in this file: a parameter or a field. Nothing says it is the module base, and
        # guessing that it is would re-create the false positive this function exists to remove.
        return False
    end = text.find(";", binding.end())
    initialiser = text[binding.end() : end if end != -1 else len(text)]
    return names_module_base(initialiser)


# ============================================================================================
# source half, widened 2026-08-31: the arithmetic, not the read
# ============================================================================================
#
# `RAW_READ_RE` above is the shape of the one site that was found. It is not the shape of the
# class, and the sweep that followed proved it: every remaining live instance in the tree was
# spelled some other way.
#
#   crates/er-title-flow/src/title_tick_cover.rs:860,2179,2182
#       `safe_read_usize(module_base + 0x3d6b7b0)` -- a HEX literal, not an identifier, so
#       `RAW_READ_RE`'s `[a-z_][a-z0-9_]*` could not match it. Both RVAs are in the shipped
#       ledger (0x3d6b7b0 -> 0x3d6f820, 0x3d856a0 -> 0x3d89720), and the same file resolves the
#       same two addresses correctly through `game_data_addr` a few hundred lines away.
#   crates/er-input-harness/src/pad_inject.rs:154
#       `CS_INGAME_PAD_TYPEID_RVAS.map(|rva| base + rva)` -- the RVA is an element of a const
#       array and the base is a function parameter. It was not read at all; it was compared
#       (`key == target`) while walking a std::map, which is the silent end of the family --
#       no fault, no refusal, the tree walk simply never matches and the in-world drive is inert.
#   crates/er-title-flow/src/product_autoload_gates.rs:942
#       `base + 0x9b3070usize` inside a `format_args!`, five lines below a sibling line that
#       resolves its address properly. A log that names the wrong address during a migration is
#       how the next reader is sent to the wrong function.
#
# So this half now keys on the addition and on what the two sides are, and it deliberately does
# not duplicate `check-stale-rva-calls.py`. That gate owns `base + NAMED_CONSTANT`; this one owns
# every other right-hand side -- literal, local, parameter, field, index, closure binding -- plus
# the one spelling a `+`-shaped pattern structurally cannot see, `base.wrapping_add(CONST)`.

# The value bounds, the addition pattern, the CORROBORATORS, the resolving consumers and the
# runtime-derived marks all moved to `scripts/module_base_arith.py` on 2026-08-31 and are imported
# at the top of this file. They are the vocabulary this gate and `check-stale-rva-calls.py` have to
# share; keeping a second copy here is precisely how the two ended up disagreeing about which sites
# each owned.


DECLARATION_RE = re.compile(
    r"const\s+([A-Z][A-Z0-9_]*)\s*:\s*[\w:]+\s*=\s*(0x[0-9a-fA-F_]+|\d+)\s*;"
)
_CONSTANT_VALUES: dict[str, int | None] | None = None


def constant_values(root: Path) -> dict[str, int | None]:
    """`{name: value}` for every unambiguous `const NAME: T = <literal>;` under `crates/`.

    Only the method add forms need this -- `base.saturating_add(MAX_IMAGE_SPAN)`, which is
    `er-invasion-warp-core`'s image-extent bound, not an address in the image. A name declared
    twice with different values records `None`, and an unresolved constant is kept as a finding:
    "I could not read it" must never be spelled the same way as "I read it and it is safe".
    """
    global _CONSTANT_VALUES  # noqa: PLW0603 - one scan of the tree, reused across files
    if _CONSTANT_VALUES is None:
        seen: dict[str, int | None] = {}
        for path in (root / "crates").rglob("*.rs"):
            text = code_only(path.read_text(encoding="utf-8", errors="replace"))
            for name, literal in DECLARATION_RE.findall(text):
                value = int(literal.replace("_", ""), 0)
                if name in seen and seen[name] != value:
                    seen[name] = None
                else:
                    seen.setdefault(name, value)
        _CONSTANT_VALUES = seen
    return _CONSTANT_VALUES


# `_identifiers`, `binders`, `nearest_binder`, `is_module_base` and `_balanced_rhs` moved to
# `scripts/module_base_arith.py` on 2026-08-31 and are imported at the top of this file. Only
# `rhs_is_compiled_in` stays: it is the question this gate asks and the sibling does not --
# whether a right-hand side that is not a named constant is a compiled-in 1.16.2 claim.


def _without_outer_parens(expression: str) -> str:
    """`(lfanew & 0xffff_ffff)` -> `lfanew & 0xffff_ffff`, and `(a) + (b)` unchanged.

    Every test below reads the first token of the expression -- is it a literal, does it name the
    module base, which local binds it. A wrapping paren hid all three: `base + (lfanew & mask)` in
    `local_invasion_filter.rs`'s PE walk fell through to "no leading name" and was reported as a
    compiled-in address, against a site the frozen-negative list names. Only a paren that spans
    the whole expression is removed, so an expression that merely starts and ends with one is
    left alone.
    """
    while expression.startswith("(") and expression.endswith(")"):
        depth = 0
        for index, character in enumerate(expression):
            if character == "(":
                depth += 1
            elif character == ")":
                depth -= 1
                if depth == 0 and index != len(expression) - 1:
                    return expression
        expression = expression[1:-1].strip()
    return expression


def rhs_is_compiled_in(text: str, bound, rhs: str, base_name: str, before: int) -> bool:
    """Is this right-hand side a compiled-in 1.16.2 claim rather than a runtime-derived value?"""
    stripped = _without_outer_parens(rhs.strip())
    if not stripped:
        return False
    literal = re.match(r"^(0x[0-9a-fA-F_]+|\d+)(?:_?u?size|u32|u64)?\s*$", stripped)
    if literal:
        value = int(literal.group(1).replace("_", ""), 0)
        return PE_HEADER_LIMIT <= value < MODULE_SPAN_LIMIT
    # An expression that reads the running image, or that folds the base back in, is derived from
    # the build in front of it. `base + read_u32(base + DOS_PE_OFFSET_FIELD)?` is the PE walk in
    # `er-hook/src/detour_site.rs`, not a stale address.
    if any(mark in stripped for mark in RUNTIME_DERIVED_MARKS):
        return False
    if re.search(rf"\b{re.escape(base_name)}\b", stripped):
        return False
    leading = re.match(r"^([a-z_][a-z0-9_]*)", stripped)
    if leading is None:
        # SCREAMING_SNAKE, a path, or an enum variant: `check-stale-rva-calls.py`'s territory for
        # the `+` form. The caller decides whether to keep it (it does, for `.wrapping_add`).
        return True
    hit = nearest_binder(bound, leading.group(1), before)
    if hit is None:
        # A parameter or a table element with no local derivation. `pad_inject.rs`'s `|rva|` over
        # `CS_INGAME_PAD_TYPEID_RVAS` lands here, which is the point.
        return True
    kind, initialiser = hit
    if kind in ("let", "closure"):
        if any(mark in initialiser for mark in RUNTIME_DERIVED_MARKS):
            return False
        # Only for `let`: `let e_lfanew = ... base ...` derived the offset from the running image.
        # A closure's "initialiser" is the chain it is mapped over, where a mention of the base is
        # ordinary neighbouring code and says nothing about where the value came from.
        if kind == "let" and re.search(rf"\b{re.escape(base_name)}\b", initialiser):
            return False
    return True


# `enclosing_call` and `is_resolver_fed` moved to `scripts/module_base_arith.py` on 2026-08-31.
# `check-stale-rva-calls.py` needs the identical answer once it stops looking at named constants
# in only three syntactic contexts, and two copies of "does this reach a resolver" is the same
# drift that opened the seam in the first place.


def source_offenders(root: Path) -> list[str]:
    found = []
    for path in sorted((root / "crates").rglob("*.rs")):
        relative = path.relative_to(root).as_posix()
        if relative in SOURCE_EXEMPT:
            continue
        text = code_only(path.read_text(encoding="utf-8", errors="replace"))
        found.extend(offenders_in(text, relative))
    return sorted(found)


# A ratchet, not a freeze -- the same shape and the same reasoning as
# `docs/recon/stale-rva-call-sites.txt`. The set may shrink freely; growth is refused.
#
# It exists because the sweep that widened this gate found three live defects in a file another
# agent was editing at the time, and silently exempting them would rebuild the contaminated
# baseline this repo has now been bitten by twice. Every row below is a real finding with a known
# fix written next to it, not a shape someone decided was acceptable.
SOURCE_BASELINE = REPO / "docs" / "recon" / "ungated-module-base-arithmetic.txt"
SOURCE_BASELINE_HEADER = """\
# Module-base arithmetic that never asks where the address lives on this build.
#
# Generated by scripts/check-oracle-singleton-globals.py --refresh. One line per
# (file, addition), so ordinary edits do not churn it and two identical sites in one file
# collapse to one row. This set may SHRINK; growth is refused.
#
# The sibling ratchet docs/recon/stale-rva-call-sites.txt owns `base + NAMED_CONSTANT` IN ANY
# CONTEXT -- it was three syntactic contexts until 2026-08-31, and the 37 sites that fell through
# that gap were reported by neither gate. This one owns every OTHER right-hand side -- a literal, a
# local, a parameter, a struct field, an array element, a closure binding -- plus
# `base.wrapping_add(CONST)`, which no `+`-shaped pattern can see. Those are the spellings a
# name-keyed gate is structurally blind to, which is exactly why the four telemetry pointer oracles
# read garbage for two entire runs while three gates reported clean. Both gates decide "is this the
# module base" and "does this reach a resolver" in scripts/module_base_arith.py, once.
#
# Fix a row by resolving through er_game_base::mem::game_data_addr (reads and comparisons) or
# game_rva / game_rva_for_hook (calls and detours -- note they consult DIFFERENT tables), screen
# the 0 that a refusal returns before comparing it to anything, then re-run --refresh.
"""


def source_baseline() -> set[tuple[str, str]]:
    if not SOURCE_BASELINE.exists():
        return set()
    out = set()
    for line in SOURCE_BASELINE.read_text(encoding="utf-8").splitlines():
        if line.startswith("#") or not line.strip():
            continue
        path, _, expression = line.rstrip().partition("\t")
        out.add((path, expression))
    return out


def keyed(findings: list[str]) -> set[tuple[str, str]]:
    """`(file, addition)` for each finding, dropping the line number so edits do not churn."""
    out = set()
    for line in findings:
        path, _, rest = line.partition(":")
        _, _, expression = rest.partition(": ")
        out.add((path, expression.strip()))
    return out


def write_source_baseline(current: set[tuple[str, str]]) -> None:
    SOURCE_BASELINE.parent.mkdir(parents=True, exist_ok=True)
    with SOURCE_BASELINE.open("w", encoding="utf-8") as handle:
        handle.write(SOURCE_BASELINE_HEADER)
        handle.write(
            f"# Currently {len(current)} site(s) in "
            f"{len({path for path, _ in current})} file(s).\n"
        )
        for path, expression in sorted(current):
            handle.write(f"{path}\t{expression}\n")


def offenders_in(text: str, relative: str) -> list[str]:
    """Every ungated module-base addition in one already-comment-stripped file."""
    found = []
    bound = binders(text)
    for match in ADD_SITE_RE.finditer(text):
        base_name = match.group(2)
        plus, method = match.group(3), match.group(4)
        rhs = _balanced_rhs(text, match.end())
        if not is_module_base(text, bound, base_name, match.start()):
            continue
        if not rhs_is_compiled_in(text, bound, rhs, base_name, match.start()):
            continue
        if plus and re.match(r"^\s*(?:[A-Za-z_][A-Za-z0-9_]*::)*[A-Z]", rhs):
            # A named constant reached with `+`. That is `check-stale-rva-calls.py`'s ratchet, and
            # duplicating it here would put the same site in two baselines that drift apart. The
            # method forms below are not duplicated: no `+`-shaped pattern can see them.
            #
            # The delegation is now AIRTIGHT, and it was not until 2026-08-31. This `continue`
            # hands the site to a sibling that, until then, recognised a named constant in only
            # three SYNTACTIC contexts -- `transmute(base + C)`, `safe_read_*(base + C)` and
            # `== base + C`. Anywhere else it matched nothing, so the pair described as a partition
            # of the class had a hole in the middle of it.
            #
            # Measured, not asserted: 37 sites reached this `continue` while
            # `check-stale-rva-calls.py` reported 0 findings tree-wide, so every one of them was
            # reported by neither gate. Twenty-two were `format_args!` log lines naming a 1.16.2
            # address -- `title_scaleform_msgbox.rs:361` printed one directly above a correctly
            # resolved `game_data_addr` call in the same `format_args!` -- and the other fifteen
            # were correct code (resolver arguments, install-table rows, image extents). All
            # twenty-two are converted and the sibling now matches the addition in any context,
            # with the module-base and resolver-fed questions it needed the moment it stopped
            # relying on a syntactic frame. Those questions, and this gate's, are the same code:
            # `scripts/module_base_arith.py`. A fourth-context site planted in a real file is
            # caught by the sibling and still delegated away here, which is the shape a partition
            # is supposed to have.
            #
            # bd `two-gate-partition-seam-named-const-outside-three-contexts-2026-08-31`.
            continue
        if not plus:
            # A named constant reached by `.wrapping_add`. It is a real address unless its value
            # says otherwise -- below `.text` it is a PE-header field, at or above the image span
            # it is an extent bound.
            named = re.match(r"^\s*(?:[A-Za-z_][A-Za-z0-9_]*::)*([A-Z][A-Z0-9_]*)\s*$", rhs)
            if named:
                value = constant_values(REPO).get(named.group(1))
                if value is not None and not (PE_HEADER_LIMIT <= value < MODULE_SPAN_LIMIT):
                    continue
        expression = f"{base_name} + {rhs.strip()}" if plus else None
        if is_resolver_fed(text, match.start(), expression):
            continue
        line = text[: match.start()].count("\n") + 1
        shown = rhs.strip()[:60]
        spelled = f"{base_name} + {shown}" if plus else f"{base_name}.{method}({shown})"
        found.append(f"{relative}:{line}: {spelled}")
    return found


# --------------------------------------------------------------------------------------------
# Image half
# --------------------------------------------------------------------------------------------


def reference_sites(image: bytes, target_rva: int) -> list[int]:
    """Every `mov r64,[rip+disp]` in `image` whose displacement resolves to `target_rva`.

    Scanned with one C-speed regex rather than sixteen `bytes.find` loops: the three-byte opcode
    prefixes occur hundreds of thousands of times in a 98 MB image, and stepping over those in
    Python is the difference between a gate that runs in a second and one that does not finish.
    """
    sites = []
    for match in MOV_RIP_SCAN.finditer(image):
        at = match.start()
        (displacement,) = struct.unpack_from("<i", image, at + DISPLACEMENT_AT)
        if at + MOV_RIP_LENGTH + displacement == target_rva:
            sites.append(at)
    return sites


def masked(window: bytes) -> bytes:
    return window[:DISPLACEMENT_AT] + b"\0" * 4 + window[DISPLACEMENT_AT + 4 :]


def occurrences(image: bytes, pattern: bytes, hole: tuple[int, int]) -> list[int]:
    """Offsets where `pattern` matches `image` with the bytes in `hole` ignored."""
    start, length = hole
    anchor = pattern[start + length :]
    if not anchor:
        return []
    hits = []
    at = image.find(anchor)
    while at != -1:
        head = at - (start + length)
        if head >= 0 and image[head : head + start] == pattern[:start]:
            hits.append(head)
        at = image.find(anchor, at + 1)
    return hits


# The derivation depends only on the two images and the 1.16.2 address -- never on the ledger --
# so the answer is the same every time it is asked. `--selftest` asks four times over (once green,
# three times with a deliberately broken ledger), and without this it pays for all four.
_DERIVED: dict[int, tuple[int | None, dict]] = {}


def derive_1170(old: bytes, new: bytes, rva_1162: int) -> tuple[int | None, dict]:
    """Re-derive a data global's 1.17 address by carrying its reference sites across."""
    if rva_1162 in _DERIVED:
        return _DERIVED[rva_1162]
    votes: dict[int, list[int]] = {}
    considered = ambiguous = unmatched = 0
    for site in reference_sites(old, rva_1162):
        if considered >= MAX_SITES:
            break
        window = old[site : site + WINDOW]
        if len(window) < WINDOW:
            continue
        shape = masked(window)
        considered += 1
        if len(occurrences(old, shape, (DISPLACEMENT_AT, 4))) != 1:
            ambiguous += 1  # the shape does not identify this site even in its own image
            continue
        landings = occurrences(new, shape, (DISPLACEMENT_AT, 4))
        if len(landings) != 1:
            unmatched += 1  # edited, or the shape is not unique on the far side
            continue
        at = landings[0]
        (displacement,) = struct.unpack_from("<i", new, at + DISPLACEMENT_AT)
        votes.setdefault(at + MOV_RIP_LENGTH + displacement, []).append(site)
    detail = {
        "sites": considered,
        "ambiguous": ambiguous,
        "unmatched": unmatched,
        "votes": {target: len(v) for target, v in votes.items()},
    }
    if len(votes) != 1:
        return _remember(rva_1162, None, detail)
    (target, voters), = votes.items()
    if len(voters) < MIN_VOTES:
        return _remember(rva_1162, None, detail)
    return _remember(rva_1162, target, detail)


def _remember(rva: int, target: int | None, detail: dict) -> tuple[int | None, dict]:
    _DERIVED[rva] = (target, detail)
    return _DERIVED[rva]


def literal_around(image: bytes, rva: int) -> str | None:
    """The printable-ASCII run straddling `rva`, when it is long enough to be a string literal."""
    lo = rva
    while lo > 0 and rva - lo < 64 and 0x20 <= image[lo - 1] < 0x7F:
        lo -= 1
    hi = rva
    while hi < len(image) and hi - rva < 64 and 0x20 <= image[hi] < 0x7F:
        hi += 1
    run = image[lo:hi]
    return run.decode("ascii") if len(run) >= LITERAL_RUN else None


def load_data_map() -> dict[str, tuple[int, int]]:
    rows = {}
    for line in DATA_MAP.read_text(encoding="utf-8").splitlines():
        if line.startswith("#") or not line.strip():
            continue
        fields = line.split("\t")
        if len(fields) < 3:
            continue
        rows[fields[2].strip()] = (int(fields[0], 16), int(fields[1], 16))
    return rows


def image_findings(old: bytes, new: bytes, mapped: dict[str, tuple[int, int]]) -> list[str]:
    problems = []

    control = literal_around(new, LITERAL_CONTROL_RVA)
    if control is None or LITERAL_CONTROL_TEXT not in control:
        problems.append(
            f"POSITIVE CONTROL FAILED: 1.17 0x{IMAGE_BASE + LITERAL_CONTROL_RVA:x} (the stale "
            f"GameDataMan address) should sit inside the RTTI name containing "
            f"{LITERAL_CONTROL_TEXT!r}; the literal detector found {control!r}. Until this reads "
            "as a literal, a clean run of the checks below proves nothing."
        )

    for name, rva_1162 in ORACLE_SINGLETONS.items():
        row = mapped.get(name)
        if row is None:
            problems.append(f"{name}: no row in {DATA_MAP.relative_to(REPO)}")
            continue
        ledger_old, ledger_new = row
        if ledger_old != rva_1162:
            problems.append(
                f"{name}: this gate holds 1.16.2 0x{rva_1162:x}, the ledger row says "
                f"0x{ledger_old:x}. One of the two is describing a different global."
            )
            continue
        derived, detail = derive_1170(old, new, rva_1162)
        if derived is None:
            problems.append(
                f"{name}: could not re-derive a 1.17 address from the images "
                f"({detail['sites']} reference site(s), {detail['ambiguous']} ambiguous, "
                f"{detail['unmatched']} unmatched, votes {detail['votes']})"
            )
            continue
        if derived != ledger_new:
            problems.append(
                f"{name}: the images say 1.16.2 0x{rva_1162:x} -> 1.17 0x{derived:x} "
                f"({detail['votes'][derived]} agreeing sites), the ledger row says "
                f"0x{ledger_new:x}"
            )
            continue
        if derived % 8:
            problems.append(f"{name}: 1.17 0x{derived:x} is not 8-byte aligned")
        if struct.unpack_from("<Q", new, derived)[0] != 0:
            problems.append(
                f"{name}: 1.17 0x{derived:x} is not zero at rest "
                f"(0x{struct.unpack_from('<Q', new, derived)[0]:016x}) -- a pointer global is "
                "uninitialised in the static image, so this is initialised data, not a slot"
            )
        run = literal_around(new, derived)
        if run is not None:
            problems.append(
                f"{name}: 1.17 0x{derived:x} sits inside the string literal {run!r}"
            )
    return problems


# --------------------------------------------------------------------------------------------
# selftest
# --------------------------------------------------------------------------------------------

# The exact closure that was live until 2026-08-31, kept verbatim so the source half is proved
# against the thing it was written for rather than against a paraphrase of it.
BEFORE_FIX = """
    let base = er_game_base::mem::game_module_base().unwrap_or(0);
    let read_singleton = |rva: usize| -> usize {
        if base == 0 {
            return 0;
        }
        unsafe { er_game_base::mem::safe_read_usize(base + rva) }.unwrap_or(0)
    };
"""
AFTER_FIX = """
    let base = er_game_base::mem::game_module_base().unwrap_or(0);
    let read_singleton = |rva: usize, what: &'static str| -> Option<usize> {
        let address = er_game_base::mem::game_data_addr(base, rva, what);
        unsafe { er_game_base::mem::safe_read_usize(address) }
    };
"""
# The hang watchdog's `base` is a heap `CS::LoadingScreenData` pointer, not the module base. It
# matched the first cut of this matcher, and it is kept as a frozen negative so a future widening
# of the pattern cannot quietly start reporting struct field reads as stale-address defects.
NOT_A_MODULE_BASE = """
    let base = current_loading_screen_data();
    let word = |offset: usize| unsafe { safe_read_u32(base + offset) };
"""

# ---------------------------------------------------------------------------------------------
# Controls for the widened matcher, all frozen from real sites in this tree (2026-08-31 sweep).
#
# Each positive is a spelling that the pre-widening source half could not see, so a control that
# both versions catch would pass on the broken gate and prove nothing. Each negative is a real
# site the widening had to be taught to leave alone -- they are the reason the matcher decides
# what a module base is from the binding and what an RVA is from its provenance, rather than from
# either identifier's name.
# ---------------------------------------------------------------------------------------------

# POSITIVE: a HEX literal RVA on a module base that arrives as a function parameter, corroborated
# by the file's own `game_data_addr` call. `er-title-flow/src/title_tick_cover.rs`, three sites.
# Both RVAs are in the shipped ledger, so these read the wrong `.data` slot on 1.17.
HEX_LITERAL_ON_PARAM = """
pub unsafe fn product_core_autoload_tick(module_base: usize, slot: i32) -> bool {
    let owner = er_game_base::mem::game_data_addr(module_base, TITLE_OWNER_VTABLE_RVA, "vt");
    let menu_man = unsafe { safe_read_usize(module_base + 0x3d6b7b0) }.filter(|&m| m > 0x10000);
}
"""
# POSITIVE: the RVA is an element of a const array, reaching the addition as a closure parameter,
# and the result is compared rather than read. `er-input-harness/src/pad_inject.rs`.
TABLE_ELEMENT_RVA = """
const CS_INGAME_PAD_TYPEID_RVAS: [usize; 2] = [0x3d5df27, 0x3d5df28];
pub unsafe fn stamp_vk_direct(base: usize, id: u32, val: u8) {
    let manager = rd(er_game_base::mem::game_data_addr(base, FD4_PAD_MANAGER_RVA, "mgr"));
    let targets = CS_INGAME_PAD_TYPEID_RVAS.map(|rva| base + rva);
}
"""
# POSITIVE: `.wrapping_add`, which no `+`-shaped pattern in this repo can see -- including
# `check-stale-rva-calls.py`, whose whole job is `base + CONSTANT`.
WRAPPING_ADD_CONSTANT = """
pub unsafe fn note(base: usize) {
    let manager = er_game_base::mem::game_data_addr(base, SOME_OTHER_RVA, "other");
    log(format_args!("want 0x{:x}", base.wrapping_add(MSGBOX_DIALOG_UPDATE_RVA)));
}
"""

# NEGATIVE: `base` shadowed by a `for` pattern over a table of STRUCT OFFSETS, in a file that does
# use the name for the module base elsewhere. `er-title-flow/src/title_tick_cover.rs:1270`.
# Corroboration alone would report this; the shadowing binder is what stops it.
SHADOWED_BY_LOOP = """
pub unsafe fn tick(module_base: usize) {
    let vt = er_game_base::mem::game_data_addr(module_base, SOME_RVA, "x");
    for &(base, cnt) in GROUPS {
        for i in 0..cnt {
            let slot = base + i * 8;
        }
    }
}
"""
# NEGATIVE: a different module. `ersc_module_base()` contains the substring `module_base(`, and
# reading it as one reported `er-invasion-warp/src/local_invasion_filter.rs` -- whose addresses
# are Seamless Co-op's, guarded by a prologue byte-check against a shipped ersc build.
ANOTHER_MODULES_BASE = """
fn ersc_action(rva: usize, prologue: &[u8]) -> Option<ErscActionFn> {
    let base = ersc_module_base()?;
    let address = base + rva;
}
"""
# NEGATIVE: a PE-header walk. `e_lfanew` was read out of the running image, so it is already
# correct for the running build and translating it would be the bug -- the same distinction
# `MhHook::new_runtime_derived` draws. Six crates write this shape.
RUNTIME_DERIVED_PE_WALK = """
fn image_span(base: usize) -> Option<usize> {
    let _ = er_game_base::mem::game_data_addr(base, SOME_RVA, "x");
    let e_lfanew = unsafe { safe_read_usize(base + PE_E_LFANEW_OFFSET) }? & 0xffff_ffff;
    let nt = base + e_lfanew;
    Some(nt)
}
"""
# NEGATIVE: handed to an API that resolves it itself. Resolving here too would translate twice,
# which lands on a third unrelated function whenever an address is both one row's destination and
# another row's source. `er-armament-icons/src/hud_badge.rs`, and the same shape in five crates.
RESOLVED_BY_THE_HOOK_API = """
fn install(base: usize) {
    let _ = er_game_base::mem::game_data_addr(base, SOME_RVA, "x");
    for (rva, detour, slot, label) in plan {
        let target = base + rva;
        let hook = match unsafe { MhHook::new(target as *mut c_void, detour) } { _ => return };
    }
}
"""
# NEGATIVE: an image extent, not an address in the image. Excluded by the constant's value, never
# by its name. `er-invasion-warp-core/src/msb_invasion_points.rs:532`.
IMAGE_EXTENT_BOUND = """
fn upper(base: usize) -> usize {
    let _ = er_game_base::mem::game_data_addr(base, SOME_RVA, "x");
    base.saturating_add(MAX_IMAGE_SPAN)
}
"""

# The real files the mutation blinds run against, and the site each one must produce when the
# conversion is undone. Mutating the real text (in memory, never on disk) is the point: a control
# written by hand proves the regex matches the control.
MUTATION_BLINDS = [
    # RE-derived 2026-08-31 (the second time in one day), because the site this blind used to
    # revert no longer exists -- and the reason it stopped existing is the whole point of keeping a
    # blind here rather than dropping the file.
    #
    # The morning's sweep converted `CS_INGAME_PAD_TYPEID_RVAS.map(|rva| base + rva)` to a
    # `game_data_addr` call. That afternoon the deeper defect was found and the entire padMaps
    # tree-walk was DELETED: every call site of the game's own per-key writer loads `rcx` from
    # `manager + 0x18 + dev*8`, so the array is a field of `FD4::FD4PadDevice` and the walk was
    # aimed at the wrong object -- one only 152 bytes long, which ids from 1008 up wrote past the
    # end of. `stamp_vk_direct` now resolves `padDevices[dev]` instead, and the const array, the
    # closure and the comparison all went with it. A blind pinned to deleted text does not fail
    # loudly as a missed defect; it fails as "the converted form is gone", which reads like the
    # gate's problem rather than the file's, and is what left this gate red and unwired.
    #
    # The mutation must not destroy the file'S only CORROBORATOR. `base` reaches every function in
    # this file as a parameter, so `is_module_base` cannot decide it from a binding; the sole
    # evidence is the one `game_data_addr(base, ...)` call at `stamp_vk_direct`. Reverting that
    # call to `base + FD4_PAD_MANAGER_RVA` -- the obvious choice -- removes the corroboration in
    # the same edit, `base` stops being recognised as the module base anywhere in the file, and the
    # gate reports nothing. Measured: that mutant yields `[]`, and reading it as "the gate is
    # blind" would be exactly the wrong-reason pass this file's other comments warn about.
    #
    # So the blind adds a raw read in `install_one`, where `base` and `rva` are both parameters and
    # the corroborator upstream stays intact. It exercises the three decisions unique to this file:
    # a parameter base recognised only by file-level corroboration, a parameter RVA with no local
    # derivation (so its provenance is compiled-in), and the `is_resolver_fed` exclusion, which must
    # keep clearing the `let addr = (base + rva)` on the line above while still reporting this one.
    # `crate::win32::read_usize` is the file's own fault-safe read primitive, used verbatim this way
    # at `stamp_vk_direct`, and `usize + usize -> usize` is the type it already takes, so the mutant
    # compiles.
    (
        "crates/er-input-harness/src/pad_inject.rs",
        "    let addr = (base + rva) as *mut c_void;\n"
        "    match unsafe { MhHook::new(addr, detour) } {",
        "    let addr = (base + rva) as *mut c_void;\n"
        "    let _stale = unsafe { crate::win32::read_usize(base + rva) };\n"
        "    match unsafe { MhHook::new(addr, detour) } {",
        "base + rva",
    ),
    (
        "crates/er-title-flow/src/product_autoload_gates.rs",
        'er_game_base::mem::game_data_addr(\n            base,\n            '
        'TITLE_CONTINUE_LOAD_DISPATCHER_RVA,\n            '
        '"TITLE_CONTINUE_LOAD_DISPATCHER_RVA"\n        )',
        "base + 0x9b3070usize",
        "base + 0x9b3070usize",
    ),
    # The two sites this gate'S own baseline header used to name as live defects (2026-08-31).
    #
    # `title_tick_cover.rs` read `CS::MenuMan` and the ending-request force flag as raw
    # `module_base + 0x3d6b7b0` / `+ 0x3d856a0` -- 1.16.2 offsets on a 1.17 image, feeding four
    # telemetry pointer oracles. Both are now resolved, the ratchet is at zero rows, and that
    # header has been deleted because `--refresh` re-emitted it verbatim above an empty file,
    # announcing three defects that no longer existed.
    #
    # A fix with no blind is a fix that regresses quietly, and this one is more exposed than most:
    # the raw form is shorter and reads more naturally, so it is what a reverting hand writes. The
    # replacement text below is the verbatim pre-fix source from head, so both mutants compile --
    # a mutant that fails to build never exercises the matcher and produces a false "the gate is
    # blind" verdict.
    (
        "crates/er-title-flow/src/title_tick_cover.rs",
        'safe_read_usize(er_game_base::mem::game_data_addr(\n                module_base,\n'
        '                CS_MENU_MAN_GLOBAL_RVA,\n                "CS_MENU_MAN_GLOBAL_RVA",\n'
        '            ))\n        }\n        .filter(|&m| m > 0x10000);',
        "safe_read_usize(module_base + 0x3d6b7b0) }.filter(|&m| m > 0x10000);",
        "module_base + 0x3d6b7b0",
    ),
    (
        "crates/er-title-flow/src/title_tick_cover.rs",
        'safe_read_u8(er_game_base::mem::game_data_addr(\n                    module_base,\n'
        '                    ENDING_REQUEST_FORCE_FLAG_3D856A0_RVA,\n'
        '                    "ENDING_REQUEST_FORCE_FLAG_3D856A0_RVA",\n                ))\n            };',
        "safe_read_u8(module_base + 0x3d856a0) };",
        "module_base + 0x3d856a0",
    ),
]
# Files whose current text must stay clean. Frozen negatives taken from the sweep: a name shadowed
# by a table of struct offsets, and a base that belongs to another DLL entirely.
FROZEN_NEGATIVE_FILES = [
    ("crates/er-invasion-warp/src/local_invasion_filter.rs", "ersc.dll module base"),
    ("crates/er-crash-logging-core/src/hang.rs", "heap CS::LoadingScreenData pointer"),
    ("crates/er-armament-icons/src/hud_badge.rs", "address handed to MhHook::new, which resolves"),
    ("crates/er-save-loader/src/profile_summary.rs", "offset into a save record, not an image"),
]


def selftest() -> int:
    failures = []

    def flags(snippet: str) -> bool:
        return bool(offenders_in(code_only(snippet), "snippet.rs"))

    if not flags(BEFORE_FIX):
        failures.append("SOURCE matcher does not flag the closure it was written for")
    if flags(AFTER_FIX):
        failures.append("SOURCE matcher flags the resolved form")
    if flags(NOT_A_MODULE_BASE):
        failures.append("SOURCE matcher flags a heap pointer named `base`")
    # A comment describing the hazard is not an instance of it.
    if flags("let base = game_module_base();\n// was safe_read_usize(base + rva) until today\n"):
        failures.append("SOURCE matcher reads prose as a finding")

    for name, snippet in (
        ("hex literal on a parameter base", HEX_LITERAL_ON_PARAM),
        ("an RVA taken from a const array", TABLE_ELEMENT_RVA),
        ("a `.wrapping_add` constant", WRAPPING_ADD_CONSTANT),
    ):
        if not flags(snippet):
            failures.append(f"SOURCE matcher misses {name}")
        # ...and the pre-widening pattern must miss it, or the control proves nothing.
        legacy = code_only(snippet)
        if any(
            binds_module_base(legacy, m.group(2), m.start()) for m in RAW_READ_RE.finditer(legacy)
        ):
            failures.append(f"control for {name} was already visible to the narrow matcher")

    for name, snippet in (
        ("a `base` shadowed by a loop over struct offsets", SHADOWED_BY_LOOP),
        ("another module's base (ersc.dll)", ANOTHER_MODULES_BASE),
        ("a PE-header walk on a runtime-read offset", RUNTIME_DERIVED_PE_WALK),
        ("an address handed to MhHook::new, which resolves it", RESOLVED_BY_THE_HOOK_API),
        ("an image-extent bound", IMAGE_EXTENT_BOUND),
    ):
        if flags(snippet):
            failures.append(f"SOURCE matcher flags {name}")

    # Mutation blinds, run against the real files. Undoing a conversion must produce exactly that
    # finding; the shipped text must produce none. Both directions, because a matcher that fires
    # on everything passes the first half.
    for relative, fixed, reverted, expected in MUTATION_BLINDS:
        path = REPO / relative
        if not path.exists():
            failures.append(f"BLIND: {relative} is gone; the mutation cannot be performed")
            continue
        # Mutate the raw text and strip comments afterwards. Stripping first would blank the
        # string literals the converted form contains (`"CS_INGAME_PAD_TYPEID_RVAS"`), so the
        # needle could never be found and the blind would silently degrade into a skip.
        raw = path.read_text(encoding="utf-8", errors="replace")
        if fixed not in raw:
            failures.append(
                f"BLIND: {relative} no longer contains the converted form, so reverting it "
                "proves nothing. Re-derive the blind against the current text."
            )
            continue
        if offenders_in(code_only(raw), relative):
            failures.append(f"BLIND: {relative} is not clean as shipped")
        mutant = offenders_in(code_only(raw.replace(fixed, reverted)), relative)
        if not any(line.endswith(f": {expected}") for line in mutant):
            failures.append(
                f"BLIND: reverting {relative} to `{expected}` produced {mutant} -- the gate "
                "cannot see the defect it exists for"
            )

    for relative, why in FROZEN_NEGATIVE_FILES:
        path = REPO / relative
        if not path.exists():
            failures.append(f"FROZEN NEGATIVE: {relative} is gone")
            continue
        text = code_only(path.read_text(encoding="utf-8", errors="replace"))
        found = offenders_in(text, relative)
        if found:
            failures.append(
                f"FROZEN NEGATIVE: {relative} ({why}) is now reported: {found}. The matcher has "
                "become over-broad; that is a false positive, not a newly discovered defect."
            )

    if IMAGE_1162.exists() and IMAGE_1170.exists():
        old = IMAGE_1162.read_bytes()
        new = IMAGE_1170.read_bytes()
        mapped = load_data_map()

        if image_findings(old, new, mapped):
            failures.append("IMAGE half is not green on the shipped ledger")

        # Blind 1: revert every row to its 1.16.2 value, which is what a stale constant looks
        # like. Every singleton must go red, or the gate cannot see the defect it exists for.
        reverted = {name: (old_rva, old_rva) for name, (old_rva, _) in mapped.items()}
        red = image_findings(old, new, reverted)
        for name in ORACLE_SINGLETONS:
            if not any(line.startswith(f"{name}:") for line in red):
                failures.append(f"BLIND(revert to 1.16.2) did not turn {name} red")

        # Blind 2: move one row by a plausible-looking eight bytes. A gate that only rejects the
        # 1.16.2 value would pass this, and an off-by-one-slot ledger row is a real way to be
        # wrong.
        nudged = dict(mapped)
        nudged["GAME_DATA_MAN_GLOBAL_RVA"] = (0x3D5DF38, 0x3D61F98 + 8)
        if not any(
            line.startswith("GAME_DATA_MAN_GLOBAL_RVA:")
            for line in image_findings(old, new, nudged)
        ):
            failures.append("BLIND(+8 bytes) did not turn GAME_DATA_MAN_GLOBAL_RVA red")

        # Blind 3: blind the literal detector. The positive control must then fail, so a
        # detector that has stopped detecting cannot let the run look clean.
        global LITERAL_RUN  # noqa: PLW0603 - deliberately blinding the detector for one call
        keep, LITERAL_RUN = LITERAL_RUN, 4096
        blinded = image_findings(old, new, mapped)
        LITERAL_RUN = keep
        if not any(line.startswith("POSITIVE CONTROL FAILED") for line in blinded):
            failures.append("BLIND(literal detector) did not fail the positive control")
    else:
        print(
            f"selftest: IMAGE half skipped -- {IMAGE_1162.name} / {IMAGE_1170.name} not present "
            "(set ER_DEOBF_1162 / ER_DEOBF_1170)"
        )

    for line in failures:
        print(f"selftest FAILED: {line}")
    if failures:
        return 1
    print("selftest: ok")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--selftest", action="store_true", help="prove the assertions can fail")
    parser.add_argument(
        "--refresh", action="store_true", help="accept the current set of ungated additions"
    )
    args = parser.parse_args()
    if args.selftest:
        return selftest()

    found = source_offenders(REPO)
    current, known = keyed(found), source_baseline()
    if args.refresh:
        write_source_baseline(current)
        print(f"wrote {SOURCE_BASELINE.relative_to(REPO)}: {len(current)} site(s)")
        return 0

    problems: list[str] = []
    added, removed = current - known, known - current
    if added:
        problems.append(f"{len(added)} ungated module-base addition(s) not in the baseline")
        for line in found:
            if keyed([line]) & added:
                print(line)
        print(
            "    A game address must come from er_game_base::mem::game_data_addr (reads and\n"
            "    comparisons) or game_rva / game_rva_for_hook (calls and detours -- they consult\n"
            "    DIFFERENT tables, so swapping them double-translates). A raw `base + rva` uses\n"
            "    the 1.16.2 slot: the read SUCCEEDS against whatever now lives there and reports a\n"
            "    constant, and the comparison simply never matches, in silence. Screen the 0 a\n"
            "    refusal returns before comparing it to anything. If the value is genuinely\n"
            "    version-independent, say so at the site and re-run --refresh."
        )
    if removed:
        problems.append(f"{len(removed)} site(s) resolved since the baseline -- run --refresh")
        for path, expression in sorted(removed):
            print(f"  RESOLVED {path}\t{expression}")
    if not added and not removed:
        print(f"ungated module-base arithmetic: {len(current)} known site(s), none new")

    if IMAGE_1162.exists() and IMAGE_1170.exists():
        findings = image_findings(
            IMAGE_1162.read_bytes(), IMAGE_1170.read_bytes(), load_data_map()
        )
        for line in findings:
            print(line)
        problems += findings
    else:
        print(
            f"IMAGE half skipped: {IMAGE_1162} / {IMAGE_1170} not present "
            "(set ER_DEOBF_1162 / ER_DEOBF_1170 to run it)"
        )

    if problems:
        print(f"\n{len(problems)} problem(s)")
        return 1
    print("oracle singleton globals: source resolved, 1.17 addresses corroborated from the images")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
