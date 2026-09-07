#!/usr/bin/env python3
"""Score every cdylib on whether a stale game address can still reach ELDEN RING 1.17 silently.

Why this exists
---------------
The game went 1.16.2 -> 1.17 on 2026-08-27 and every RVA in this workspace was written for 1.16.2.
`er_game_base::game_build` translates or refuses a known address, and `er-hook`'s `MhHook::new`
routes detours through it -- so a hook on a moved function fails loudly instead of corrupting the
image. That protection only covers addresses that actually go through it.

Three things a hand-built `base + SOME_RVA` can be used for, and they are not equally dangerous.
Counting them together overstates the problem by an order of magnitude, so this separates them:

  EXEC   `transmute(base + SOME_RVA)` and then call it. On 1.17 that is a call into whatever now
         occupies the address. Nothing refuses, nothing logs, and the game executes it. Worst.
         NOTE: a mapped constant used this way is just as broken as an unmapped one. The map knows
         the 1.17 destination; `base + RVA` never asks it, so the call still goes to the 1.16.2
         address. Sites are tagged `[UNMAPPED]` only to say whether the fix is one `game_rva()`
         call away or needs the address found first -- never to say the site is safe.
  Write  a raw store / `write_code_byte` at `base + SOME_RVA`. Corrupts the image for every later
         reader, not just this one caller.
  Read   `safe_read_usize(base + SOME_RVA)`, or an identity compare `vt != base + SOME_VTABLE_RVA`.
         Fault-safe by construction: a stale address yields a wrong answer or `None`, never a
         fault. This is the silent class -- the feature quietly stops working and nothing says so.
         Measured 2026-08-29: `TITLE_OWNER_VTABLE_RVA` is `CS::TitleStep` in 1.16.2 and nothing in
         1.17, and its three scans simply find no owner, forever, without a single log line.

A raw write preceded by a signature check is counted separately and is not a defect: comparing the
expected opcode before storing turns a stale address into a refusal. Measured 2026-08-29 --
`apply_splash_skip` did exactly that and logged `abort -- byte at 0x140b0c35d is 0x4a, expected
0x74` on a 1.17 image where the naive write would have smashed a `lea` displacement.

What it does not claim
----------------------
Zero ungated sites is not "this DLL works on 1.17". It means no address can reach the game without
the gate having a say. Whether the mapped destinations are the right functions is
`scripts/verify-rva-map-1170.py`'s job, and whether the DLL then behaves is a runtime question --
the `runtime` column is read from a recorded results file, never inferred.

How this divides with `check-stale-rva-calls.py`
------------------------------------------------
That script already ratchets ungated calls repo-wide, keyed on (file, RVA constant). It is the
authority on that set and on converting it; this one deliberately does not re-litigate it. What
this adds is the two things a repo-wide call count cannot say:

  * Per-CDYLIB attribution. A flat repo-wide total hides one DLL regressing while another
    improves, and "does THIS DLL have 1.17 fixes" is a per-DLL question.
  * The write and read/compare buckets, which that script does not measure at all. 0 ungated
    WRITEs across all 27 cdylibs is a safety property nothing else in the tree guards.

Refresh both baselines together when you bank an improvement.

What the `consts` and `unmapd` columns could see until 2026-08-30
-----------------------------------------------------------------
Three regexes -- `CONST`, `CONST_ALIAS`, `USE_ALIAS` -- decided which constants exist and what
address each holds. All three required the name to be spelled `*RVA*`, and the first also required
`: usize` and a hex literal on the spot. A constant they could not resolve has no address, so it is
tagged `[UNMAPPED]` at every use site: the audit says "this needs a 1.17 row" about an address that
may well already have one, and the map owner is sent to re-derive it. Five ordinary spellings were
invisible: `: u32` / `: u64` constants, enum discriminants, constants defined from a discriminant,
constants whose name never carried the suffix, and every constant declared in a crate this closure
does not include but whose value it uses.

Values now come from `scripts/rva_symbols.py`. Measured on this tree: 362 -> 640 constants resolved
across the union of all 27 cdylib closures, and `er-quickload` alone went 285 -> 534.

Two things that deliberately did not change, so the widening does not manufacture work:

  * What counts as an address. Dropping the name filter without a replacement would file every
    millisecond cap and bitmask as an unmapped address. The replacement is by what the constant is
    or does -- see `ADDRESS_NAME` / `ADDRESS_USE` / `mapped_source_addresses` below -- never by
    what it is called alone.
  * `HAND_BUILT`, the use-site matcher, keeps its `*RVA*` operand filter. Measured: dropping it
    adds 55 matches across 32 new operand names, and they are PE-header fields
    (`PE_DOS_LFANEW_OFFSET`, `DOS_PE_OFFSET_FIELD`), struct offsets (`REC_LEVEL_OFFSET`) and plain
    locals (`i`, `gap`, `size`, `offset`) -- not stale-address hazards. `check-stale-rva-calls.py`
    does drop that filter, because it has a value gate to exclude the PE fields by what they are;
    this audit has none, so here the filter earns its place. That division is the one the "how this
    divides" section above already draws.

The ratchet
-----------
`--check` compares the per-DLL counts against `docs/recon/dll-1170-ungated-ledger.tsv` and exits 1
if any went up. That is the part that makes this a proof rather than a snapshot: 0 ungated WRITEs
across all 27 cdylibs is a real safety property today, and the ledger is what stops the next commit
from quietly taking it away. `--refresh` rewrites the ledger, so a deliberate increase shows up as
a reviewable diff instead of the invisible default.

Usage
    python3 scripts/audit-1170-readiness.py
    python3 scripts/audit-1170-readiness.py --dll er-quickload
    python3 scripts/audit-1170-readiness.py --check     # the gate
    python3 scripts/audit-1170-readiness.py --refresh   # accept new counts
    python3 scripts/audit-1170-readiness.py --selftest
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
# One dialect, not four. `rva_symbols` resolves every declaration spelling in this tree to a value
# and blanks comments and string bodies before anything is matched. This file had a hand-rolled
# partial version of both: `CONST` + `CONST_ALIAS` + `USE_ALIAS` covered three of the spellings and
# missed the rest, and prose was skipped only when a `//` began the line.
try:  # noqa: E402 - repo-local; the sys.path line above is what makes it work
    import rva_symbols
    from rva_symbols import code_only
except ImportError as missing:  # a shared reader that cannot load must stop the audit, not degrade
    raise ImportError(
        "scripts/rva_symbols.py could not be imported, so a constant declared as an enum "
        "discriminant, as `: u32`, or by derivation cannot be resolved to an address. Without it "
        "this audit reports such a constant as [UNMAPPED] while its map row sits in the ledger, "
        "and omits it from the per-DLL counts entirely. Fix the import rather than restoring the "
        "local CONST/CONST_ALIAS/USE_ALIAS regexes."
    ) from missing

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CRATES = os.path.join(REPO, "crates")
NEEDED = os.path.join(REPO, "docs", "recon", "rva-map-1162-to-1170.needed.tsv")
# The GLOBALS half of the same map. Both files are `1.16.2 RVA <TAB> 1.17 RVA <TAB> label`, both
# are generated, and a constant mapped in either one is mapped. Reading only the first is why 25
# of er-invasion-warp's 44 "unmapped" constants were `*_GLOBAL_RVA` / `*_SINGLETON_RVA` /
# `*_VTABLE_RVA` rows sitting in this file with a full vote count beside them.
DATA_MAP = os.path.join(REPO, "docs", "recon", "rva-map-1162-to-1170.data.tsv")
RUNTIME_RESULTS = os.path.join(REPO, "docs", "recon", "dll-1170-runtime-results.json")
LEDGER = os.path.join(REPO, "docs", "recon", "dll-1170-ungated-ledger.tsv")

# The declaration reader this file used to be, all three regexes, frozen as LITERALS so
# `--selftest` can prove the replacement is load-bearing. Every control below must be invisible to
# these and visible to the resolver; a control both see would pass on the broken audit and prove
# nothing. Spelled out rather than composed, so they cannot quietly widen along with the live code.
#
# What they could read: `const NAME_RVA: usize = 0x...`, plus two alias shapes. What they could
# not, all of which are ordinary in this tree:
#
#     const CAP_BUILDER_RVA: u32 = 0x826510;        a `: u32` address constant
#     MenuJobWait = 0x00b0d400,                     an enum discriminant
#     const X_RVA: usize = Enum::Variant as usize;  a constant defined from one
#     const GET_MAIN_PLAYER_STATS: usize = 0x...;   an address whose name never carried the suffix
#
# (A literal wrapped onto the next line is visible to these -- `\s*` spans newlines -- which is why
# `--selftest` asserts the wrapped case as an ordinary positive and never as a hidden one. A control
# has to be checked against the frozen matcher, not assumed.)
LEGACY_CONST = re.compile(r"const\s+([A-Z0-9_]*RVA[A-Z0-9_]*)\s*:\s*usize\s*=\s*(0x[0-9a-fA-F_]+)")
LEGACY_CONST_ALIAS = re.compile(
    r"const\s+([A-Z0-9_]*RVA[A-Z0-9_]*)\s*:\s*usize\s*=\s*"
    r"((?:[A-Za-z_][A-Za-z0-9_]*\s*::\s*)*[A-Z0-9_]*RVA[A-Z0-9_]*)\s*;"
)
LEGACY_USE_ALIAS = re.compile(
    r"use\s+((?:[A-Za-z_][A-Za-z0-9_]*\s*::\s*)*[A-Z0-9_]*RVA[A-Z0-9_]*)"
    r"\s+as\s+([A-Z0-9_]*RVA[A-Z0-9_]*)\s*;"
)
# A constant-shaped identifier: SCREAMING_SNAKE, which is what a map row's label looks like when
# the label is a constant name rather than a source location.
CONST_NAME = re.compile(r"^[A-Z][A-Z0-9_]*$")

# What counts as an address constant for the per-DLL columns. Dropping the `*RVA*` name filter is
# what makes the wider set visible, so something has to replace it or every millisecond cap and
# bitmask in the tree becomes an "unmapped address" the map owner is told to go and derive.
#
# The replacement is by what the thing is or does, never by what it is called:
#   * a value that could be an RVA at all -- at or above the end of the PE headers, below the
#     image size;
#   * and one of: this tree's address naming convention; the code using it as an address
#     (`base + X`, `game_rva(X)`, `game_data_addr(base, X, ..)`); or an earlier pass having already
#     called that number a game address by giving it a row in a curated map.
# The last is required to be >= 0x100000, because a small round number lands on a real address by
# coincidence (`0x1000` is both a curated row and a texture-dimension cap in `boot_progress.rs`).
PE_HEADER_LIMIT = 0x1000
IMAGE_LIMIT = 0x8000000
COINCIDENCE_FLOOR = 0x100000
IMAGE_BASE = 0x140000000
# Matched on whole underscore COMPONENTS: `RVA` as a substring also occurs inside `INTERVAL`, and
# `PATCH_RETRY_LOG_INTERVAL: u32 = 100_000` is a log throttle, not an address.
ADDRESS_NAME = re.compile(r"(?:^|_)(?:RVA|RVAS|VA|VAS)(?:_|$)")
# Using a constant as an address is stronger evidence than naming it like one -- but only when the
# thing it is added to is a module base. `x + FOO` is how this tree walks structs as well as
# images, and the loose form promotes struct-field offsets (0x10f0, 0x1200) into "addresses".
ADDRESS_USE = re.compile(
    r"(?<![.\w])\$?(?:base|module_base|image_base|game_base|game_module_base|exe_base)\s*"
    r"(?:\+|\.checked_add\(\s*)\s*((?:[A-Za-z_]\w*\s*::\s*)*[A-Za-z_]\w*)"
    r"|(?:game_rva|resolve_game_address|game_ptr)\s*\(\s*((?:[A-Za-z_]\w*\s*::\s*)*[A-Za-z_]\w*)"
    r"|game_data_addr\s*\(\s*\w+\s*,\s*((?:[A-Za-z_]\w*\s*::\s*)*[A-Za-z_]\w*)"
)
# `base + SOMETHING_RVA` / `image_base + SOMETHING_RVA` -- an address built by hand.
#
# The OPERAND is not always a SCREAMING_SNAKE constant, and assuming it was is how this audit
# reported `er-reload-trace 0 0` on the very day that crate wrote 34 five-byte JMPs into live 1.17
# code. Its installer reads
#
#     let target = base + spec.rva;
#
# -- a lowercase field of a hook-table row, covering all 40 of its detours in one line. The old
# pattern required an identifier containing an uppercase `RVA`, and `.` was not even in its
# character class, so zero of those sites matched and the ratchet was blind at precisely the crate
# that corrupted the image. Three shapes must all match: a bare constant, a `::`-pathed constant,
# and a dotted field access whose last component is `rva` / `rva_1162` / `hook_rva`.
HAND_BUILT = re.compile(
    r"\b(?:base|image_base|module_base|game_base)\s*\+\s*"
    r"((?:[A-Za-z_][A-Za-z0-9_]*(?:::|\.))*"            # optional `mod::` / `value.` prefix
    r"(?:[A-Za-z0-9_]*RVA[A-Za-z0-9_]*"                 # SOME_RVA, RVA_TABLE, ...
    r"|(?:[a-z0-9_]*_)?rva(?:_[a-z0-9_]+)?))"           # rva, rva_1162, hook_rva
)
# The gate, in each of its spellings.
#
# `register_shared_hook` / `register_union_hook` belong here: both call `er-hook`'s `resolve_target`
# -> `er_game_base::game_build::resolve_detour_address` before MinHook sees the address, exactly as
# `MhHook::new` does. Omitting them would file a correctly-gated installer as a defect, which is the
# mirror image of the blindness above and just as useless.
GATED = re.compile(
    r"\b(?:game_rva|resolve_game_address|resolve_detour_address|resolve_target"
    r"|MhHook::new|game_ptr|register_shared_hook|register_union_hook)\s*\("
)
# A raw store through a pointer, or the byte-patch primitive.
#
# The third alternative is the one this audit was missing until 2026-08-29. Naming the pointer
# first (`let target = ...; *target = 1`) is a style, not a requirement, and the tree writes
# globals the other way round just as often:
#
#     *((base + TITLE_GLOBAL_ACCEPT_BYTE_RVA) as *mut u8) = 1;
#
# Six such stores existed while this audit reported zero ungated writes across all 27 cdylibs --
# and the ledger's whole claim is that zero. One of the six was the title's zero-input accept byte,
# writing to a stale 1.16.2 address on every 1.17 boot: the menu never opened, and nothing in the
# gate said a word, because a store to a moved global neither faults nor logs. The assignment can
# also sit on the next line, which is why this is matched against the window rather than one line.
# The fourth alternative is a MinHook install, and it is a write in the only sense that matters:
# `MH_CreateHook` copies the prologue and `MH_EnableHook` stores a five-byte `E9 rel32` over it.
# Reaching either with a hand-built address the gate never saw is how 19 live instructions got
# split on 2026-08-29 -- so it belongs in the bucket whose ledger claim is "no DLL can corrupt the
# 1.17 image with a stale address", not in the fault-safe read bucket where it used to land.
RAW_WRITE = re.compile(
    r"write_code_byte|write_code_bytes|\*\s*target\s*=|\*\s*(?:addr|address|slot)\s*="
    r"|as\s*\*mut\s+[\w:]+\s*\)\s*="
    r"|MH_CreateHook\s*\(|MH_EnableHook\s*\("
)
# The address is turned into something callable. Matched against the text immediately before the
# `base + rva`, not a window: `is_in_state(sm, base + TITLE_STATE_DESC_LOOP_RVA)` sits two lines
# from a `transmute` and is a data pointer, and a window-based match filed 13 such arguments as
# executable code. A stale data pointer is still wrong -- the comparison silently never matches --
# but it is the read/compare hazard, not the execute one, and conflating them inflates the number
# that matters most.
EXEC_USE = re.compile(r"(?:transmute|as\s+\*const\s+fn|as\s+extern)\s*[(<]?\s*$")
# The address is only read through, or compared against. Fault-safe: wrong answer, never a fault.
READ_USE = re.compile(r"safe_read_|read_usize|read_u8|read_u32|!=|==|\.contains|matches!")
# Evidence that a write validates what it is overwriting first.
SIGNATURE_CHECK = re.compile(r"EXPECTED|expected|!=\s*[A-Z0-9_]*(?:JE|OPCODE|BYTE)|prologue|signature", re.I)


def crate_closure(name: str, pkgs: dict, seen: set | None = None) -> set:
    seen = seen if seen is not None else set()
    if name in seen or name not in pkgs:
        return seen
    seen.add(name)
    for dep in pkgs[name]["dependencies"]:
        if dep["kind"] is None and dep["name"] in pkgs:
            crate_closure(dep["name"], pkgs, seen)
    return seen


def crate_sources(pkgs: dict, crate: str) -> list[str]:
    root = os.path.dirname(pkgs[crate]["manifest_path"])
    out = []
    for base, _dirs, files in os.walk(os.path.join(root, "src")):
        out.extend(os.path.join(base, f) for f in files if f.endswith(".rs"))
    return out


class MappedRows:
    """What the 1.16.2 -> 1.17 tables carry, keyed both ways.

    A map row is an address pair. Its third column is a label, and the label's spelling is decided
    by whichever generator wrote the row -- not by whether the row is mapped. `select-needed-1170-
    rows.py` writes the `const *_RVA` name when the workspace declares one and a `file.rs:line`
    source location when the address comes from somewhere with no constant name (a `MapSeam { rva:
    ... }` field, a hook-table row), and three rows carry a `(refused at runtime 0x...)` note
    instead.

    Reading the label and asking "is this a constant this crate uses" therefore answers no for
    every row of the second and third shapes, however completely mapped they are. That is not a
    near-miss. On 2026-08-30 the five `er-invasion-warp` map seams -- 0x876140, 0x885ed0, 0x888aa0,
    0x88b7b0, 0x88bac0, every one of them mapped at +0xff0, verified identical 1.000 both-entries,
    present in both generated tables -- reported `[UNMAPPED]`, and an agent spent a session
    re-deriving addresses the ledger already held.

    So `addrs` is the authority and `names` is a convenience for the sites whose operand is a
    constant the audit could not resolve to a literal. `covers()` accepts either.
    """

    def __init__(self, names=(), addrs=()):
        self.names = set(names)
        self.addrs = set(addrs)

    def __len__(self) -> int:
        return len(self.names)

    def __contains__(self, name) -> bool:
        return name in self.names

    def covers(self, name, addr) -> bool:
        return name in self.names or (addr is not None and addr in self.addrs)


def _as_mapped(mapped) -> MappedRows:
    """Accept a bare name set, so a caller that only has names still works."""
    return mapped if isinstance(mapped, MappedRows) else MappedRows(mapped)


def mapped_constants() -> MappedRows:
    """Read every map row, keeping its 1.16.2 address and, when the label is one, its name.

    The set of tables is the one `er-game-base/build.rs` generates the runtime table from, because
    "mapped" has to mean "the resolver can answer for this address" and nothing else. That file
    reads four maps -- `VERIFIED_MAP`, `FUNCTION_MAP` (needed), `DATA_MAP`, `NEEDED_VERIFIED_MAP`
    -- and drops the rows listed in `QUARANTINE`.

    This audit read two of them. That was survivable while the declaration reader could only see
    `const NAME_RVA: usize = 0x...`, and stopped being survivable the moment it could see the rest:
    of the 45 constants that became visible on 2026-08-30, roughly forty carry a `verified.tsv`
    row, so the audit would have told the map owner to go and derive addresses that were already
    derived, verified byte-for-byte, and in use. An audit that manufactures work is worse than one
    that misses some.
    """
    names, addrs = set(), set()
    tables = (
        (NEEDED, 2),
        (DATA_MAP, 2),
        # `verified.tsv` has no constant column at all -- its later fields are a verdict and a
        # signature description -- so it contributes addresses only. Reading one as a name is how
        # junk gets into a set that is then searched for constant names.
        (os.path.join(REPO, "docs", "recon", "rva-map-1162-to-1170.verified.tsv"), None),
        (os.path.join(REPO, "docs", "recon", "rva-map-1162-to-1170.needed-verified.tsv"), 5),
    )
    for table, column in tables:
        try:
            with open(table, encoding="utf-8") as handle:
                for line in handle:
                    # `#` comments out the unused rows at the foot of the data table; only a row
                    # that starts with its 1.16.2 address is a promoted mapping.
                    if not line.startswith("0x"):
                        continue
                    parts = line.rstrip("\n").split("\t")
                    if len(parts) < 2 or not parts[1].strip().startswith("0x"):
                        continue
                    try:
                        value = int(parts[0].strip(), 16)
                    except ValueError:
                        continue
                    addrs.add(value - IMAGE_BASE if value >= IMAGE_BASE else value)
                    if column is None or len(parts) <= column:
                        continue
                    label = parts[column].strip()
                    if CONST_NAME.match(label):
                        names.add(label)
        except OSError:
            pass
    # A QUARANTINED row is not a mapping. `er-hook/build.rs` drops these from the generated table
    # precisely so the address refuses, which is the same outcome as having no row -- and the whole
    # point of the column is what the resolver can answer.
    try:
        with open(
            os.path.join(REPO, "docs", "recon", "rva-1170-quarantine.tsv"), encoding="utf-8"
        ) as handle:
            for line in handle:
                if not line.startswith("0x"):
                    continue
                try:
                    value = int(line.split("\t")[0].strip(), 16)
                except ValueError:
                    continue
                addrs.discard(value - IMAGE_BASE if value >= IMAGE_BASE else value)
    except OSError:
        pass
    return MappedRows(names, addrs)


_MAP_SOURCES: set[int] | None = None


def mapped_source_addresses() -> set[int]:
    """Every 1.16.2 address a curated map already calls a game address, cached for the run.

    Used only as one of the three pieces of evidence that a constant is an address at all -- not as
    the "is it mapped" test, which is `mapped_constants()` and reads the two tables this audit
    reports against. `functions.tsv` is excluded on purpose: it is a 128k-row dump of every
    function in the image, so membership in it is nearly free and would admit any round number.
    """
    global _MAP_SOURCES
    if _MAP_SOURCES is not None:
        return _MAP_SOURCES
    out: set[int] = set()
    for name in (
        "rva-map-1162-to-1170.needed.tsv",
        "rva-map-1162-to-1170.needed-verified.tsv",
        "rva-map-1162-to-1170.verified.tsv",
        "rva-map-1162-to-1170.data.tsv",
        "rva-map-1162-to-1170.tsv",
    ):
        path = os.path.join(REPO, "docs", "recon", name)
        if not os.path.exists(path):
            continue
        for line in open(path, encoding="utf-8", errors="replace"):
            if not line.startswith("0x"):
                continue
            try:
                value = int(line.split("\t")[0].strip(), 16)
            except ValueError:
                continue
            out.add(value - IMAGE_BASE if value >= IMAGE_BASE else value)
    _MAP_SOURCES = out
    return out


def _canonical(name: str, alias: dict) -> str:
    """Follow `A -> B -> C` alias hops to the name that actually carries the literal."""
    seen = set()
    while name in alias and name not in seen:
        seen.add(name)
        name = alias[name]
    return name


def symbol_index(paths: list[str]):
    """The resolver's index for this source set, reusing the cached whole-tree one when possible.

    Every path under `crates/` is served by the one cached index, because rebuilding it per cdylib
    closure would mean 27 walks of 500 files. Fixture paths outside the tree get their own small
    index, which is what lets `--selftest` feed `scan()` a temp file.

    Resolving against the whole tree is also more correct than resolving against the closure: a
    constant written `const FILE_OPEN_RVA: usize = er_game_base::rva::TITLE_SCALEFORM_FILE_OPEN_RVA;`
    only has a value if the file that declares its target is being read too.
    """
    if all(os.path.abspath(path).startswith(CRATES) for path in paths):
        return rva_symbols.index()
    return rva_symbols.Index.build(sources=list(paths))


def _declarations(index, paths):
    """`(declared, address_constants)` for the declarations that live in `paths`.

    `declared` is every resolvable symbol -- as wide as possible, because it is what a use site's
    operand is looked up in, and a name that cannot be resolved is tagged `[UNMAPPED]` without
    anyone having checked. `address_constants` is the narrower, evidence-tested subset that the
    per-DLL `consts` and `unmapd` columns are counted over.
    """
    wanted = {os.path.abspath(path) for path in paths}
    known = mapped_source_addresses()
    used = set()
    for path, text in index.text.items():
        if os.path.abspath(path) not in wanted:
            continue
        for match in ADDRESS_USE.finditer(text):
            operand = next((group for group in match.groups() if group), None)
            if operand:
                used.add(operand.replace(" ", "").rsplit("::", 1)[-1])
    declared: dict[str, list[int]] = {}
    address_constants: dict[str, list[int]] = {}
    for decl in index.decls:
        if os.path.abspath(decl.path) not in wanted or not index.in_universe(decl):
            continue
        # Normalised to RVAs, because a map row is keyed on the 1.16.2 RVA and this tree writes
        # the same address either way -- `GAME_HEAP_ALLOC_VA = 0x141eb9ed0` is `0x1eb9ed0`.
        values = sorted(
            value - IMAGE_BASE if value >= IMAGE_BASE else value for value in (decl.value or ())
        )
        if not values:
            continue
        # A name declared twice with different values is ambiguous, and ambiguous is unresolved.
        if decl.symbol in declared and declared[decl.symbol] != values:
            declared[decl.symbol] = []
            address_constants.pop(decl.symbol, None)
            continue
        declared[decl.symbol] = values
        rvas = values
        if not all(PE_HEADER_LIMIT <= rva < IMAGE_LIMIT for rva in rvas):
            continue
        if (
            ADDRESS_NAME.search(decl.symbol.upper())
            or decl.symbol in used
            or any(rva >= COINCIDENCE_FLOOR and rva in known for rva in rvas)
        ):
            address_constants[decl.symbol] = rvas
    return declared, address_constants


def scan(paths: list[str], mapped) -> dict:
    """Count the ways a stale address could reach the game from this source set."""
    mapped = _as_mapped(mapped)
    index = symbol_index(paths)
    declared, address_constants = _declarations(index, paths)
    # An alias is another name for a declared address, and the audit reads the name at the use
    # site. `rva_symbols` collects every `use a::b::OLD as NEW;` in the tree, including the ones
    # that strip the `_RVA` suffix, which is exactly where the hand-rolled version stopped.
    alias: dict[str, str] = {
        name: target.split("::")[-1] for name, target in index.aliases.items()
    }

    def address_of(name: str):
        canon = _canonical(name, alias)
        return canon, declared.get(name) or declared.get(canon) or None

    def label(name: str) -> str:
        canon, addrs = address_of(name)
        if addrs and all(
            mapped.covers(name, addr) or mapped.covers(canon, addr) for addr in addrs
        ):
            return name
        if mapped.covers(name, None) or mapped.covers(canon, None):
            return name
        if not addrs and not CONST_NAME.match(name):
            # A local, a parameter or a struct field: there is no literal to look up, so calling
            # it unmapped would be a claim the audit never checked. `base + spec.rva` is 40 hook
            # rows, not one address.
            return name + " [NO-ADDRESS]"
        return name + " [UNMAPPED]"

    buckets = {"exec": [], "write": [], "checked_write": [], "read": []}
    for path in paths:
        text = index.text.get(path)
        if text is None:
            try:
                with open(path, encoding="utf-8", errors="replace") as handle:
                    text = code_only(handle.read())
            except OSError:
                continue
        # The matching runs on masked text. `code_only` blanks comments and string bodies to
        # spaces without moving anything, so line numbers and the +/-3 line windows still line up.
        # The old `startswith("//")` test caught a whole-line comment and nothing else: a trailing
        # `// like base + FOO_RVA`, a `/* ... */` block and a quoted example all counted as
        # hand-built addresses, and the sibling call gate had two of its three baseline rows turn
        # out to be exactly that.
        lines = text.splitlines()
        for index_of_line, line in enumerate(lines):
            hand = HAND_BUILT.search(line)
            if not hand:
                continue
            name = hand.group(1).rsplit("::", 1)[-1]
            # A window, because the gate call and the use are often a line or two apart.
            window = "\n".join(lines[max(0, index_of_line - 3) : index_of_line + 4])
            if GATED.search(window):
                continue
            # What this occurrence is used for, judged from the text right before it.
            before = line[: hand.start()].rstrip()
            where = f"{os.path.relpath(path, REPO)}:{index_of_line + 1}"
            entry = (where, label(name))
            # Order matters: a write is a write even if the same window also reads, and an exec is
            # worse than a read. Only a window with neither is the benign read/compare bucket.
            if RAW_WRITE.search(window):
                buckets["checked_write" if SIGNATURE_CHECK.search(window) else "write"].append(entry)
            elif EXEC_USE.search(before) or EXEC_USE.search(
                "\n".join(lines[max(0, index_of_line - 1) : index_of_line + 1]).rstrip()
            ):
                buckets["exec"].append(entry)
            else:
                buckets["read"].append(entry)
    # The columns are counted over the evidence-tested subset, not over every resolvable symbol:
    # `declared` deliberately holds every constant in the closure so a use site can be looked up,
    # and reporting a millisecond cap as an unmapped address would be inventing work for the map
    # owner. An address constant whose value is a table of addresses counts as mapped only when
    # every entry is.
    unmapped = sorted(
        name
        for name, addrs in address_constants.items()
        if not all(
            mapped.covers(name, addr) or mapped.covers(_canonical(name, alias), addr)
            for addr in addrs
        )
    )
    return {"declared": len(address_constants), "unmapped": unmapped, **buckets}


def runtime_results() -> dict:
    try:
        with open(RUNTIME_RESULTS, encoding="utf-8") as handle:
            return json.load(handle)
    except (OSError, ValueError):
        return {}


def _positive_control() -> list[str]:
    """Run the whole scan over the exact code that corrupted 1.17, and over its fix.

    Asserting the regexes in isolation is not enough, and this audit has now been wrong twice in
    the same way: `RAW_WRITE` missed six real cast-and-assign stores, and `HAND_BUILT` missed all
    40 of `er-reload-trace`'s detour sites -- both times the ledger reported zero and both times
    the zero was believed. A pattern test cannot catch that, because the bucket a site lands in
    also depends on the gate check and the +/-3 line window. So this feeds `scan()` the real
    before-and-after text and asserts the verdict FLIPS: the ungated install scores 1 ungated
    write, and the same installer routed through either gate spelling scores 0. Both gated forms
    are checked because the crate that caused this uses the cross-DLL registrar, not the one the
    obvious fix reaches for.
    """
    broken = """
fn install_one(base: usize, spec: &HookSpec) {
    let target = base + spec.rva;
    let mut trampoline: *mut c_void = null_mut();
    let create_status = unsafe {
        MH_CreateHook(target as *mut c_void, spec.detour as *mut c_void, &mut trampoline)
    } as i32;
    let enable_status = unsafe { MH_EnableHook(target as *mut c_void) } as i32;
}
"""
    fixed_shared = """
fn install_one(base: usize, spec: &HookSpec) -> bool {
    let target = base + spec.rva;
    match unsafe { er_hook::register_shared_hook(target, spec.detour, spec.original) } {
        Ok(route) => true,
        Err(status) => false,
    }
}
"""
    fixed_union = """
fn install_one(base: usize, spec: &HookSpec) -> bool {
    let requested = base + spec.rva;
    match unsafe { er_hook::register_union_hook(requested, spec.detour, spec.original) } {
        Ok(()) => true,
        Err(status) => false,
    }
}
"""
    out = []
    with tempfile.TemporaryDirectory() as tmp:
        for label, text, want_write, want_read in (
            ("ungated", broken, 1, 0),
            ("register_shared_hook", fixed_shared, 0, 0),
            ("register_union_hook", fixed_union, 0, 0),
        ):
            path = os.path.join(tmp, f"{label}.rs")
            with open(path, "w", encoding="utf-8") as handle:
                handle.write(text)
            got = scan([path], set())
            if len(got["write"]) != want_write:
                out.append(
                    f"positive control: the {label} installer scored "
                    f"{len(got['write'])} ungated WRITE, expected {want_write}"
                )
            if len(got["read"]) != want_read:
                out.append(
                    f"positive control: the {label} installer scored "
                    f"{len(got['read'])} read/compare, expected {want_read}"
                )
    return out


def _label_shape_control() -> list[str]:
    """Prove the reader sees a map row through both spellings of its third column.

    The 2026-08-30 failure was not a regex missing a syntax; it was the reader keying on the shape
    it expected a column to have. `mapped_constants()` returned column 3 verbatim, so a row labelled
    with a `file.rs:line` source location contributed a source location to a set that was then
    searched for constant names -- and every such row read `[UNMAPPED]` while being fully mapped.

    Asserting `mapped_constants()` in isolation would not catch a recurrence, because what broke is
    the join between the table and the use site. So this runs the whole `scan()` over source text
    that uses four real addresses under names the tables have never heard of, and asserts the tag:

      * a row whose label is a constant name                    -> recognised, by address
      * a row whose label is a `file.rs:line` source location   -> recognised, by address
      * a row that lives in the GLOBALS table, not `needed.tsv` -> recognised, by address
      * an address in neither table                             -> still says [UNMAPPED]

    The last case is the negative control, and it is the reason this cannot pass by tagging
    nothing. The three addresses are the ones the incident named, so a table that drops them fails
    here rather than in a session spent re-deriving them.
    """
    name_labelled = 0x116C70  # needed.tsv, labelled `DLSTRING_WCHAR_SUBSTR_RVA`
    srcloc_labelled = 0x876140  # needed.tsv, labelled `crates/er-invasion-warp/src/map_seams.rs:244`
    globals_table = 0x2BA4C80  # data.tsv, labelled `SCALEFORM_MEMORY_FILE_VTABLE_RVA`
    absent = 0xDEADBE0  # in neither table, and not a plausible RVA
    text = f"""
const NAME_LABELLED_RVA: usize = {name_labelled:#x};
const SRCLOC_LABELLED_RVA: usize = {srcloc_labelled:#x};
const GLOBALS_TABLE_RVA: usize = {globals_table:#x};
const ABSENT_RVA: usize = {absent:#x};
const ALIASED_RVA: usize = er_game_base::rva::SCALEFORM_MEMORY_FILE_VTABLE_RVA;
pub use er_game_base::rva::DLSTRING_WCHAR_SUBSTR_RVA as REEXPORTED_RVA;
fn probe(base: usize) {{
    let a = unsafe {{ er_game_base::mem::safe_read_usize(base + NAME_LABELLED_RVA) }};
    let b = unsafe {{ er_game_base::mem::safe_read_usize(base + SRCLOC_LABELLED_RVA) }};
    let c = unsafe {{ er_game_base::mem::safe_read_usize(base + GLOBALS_TABLE_RVA) }};
    let d = unsafe {{ er_game_base::mem::safe_read_usize(base + ABSENT_RVA) }};
    let e = unsafe {{ er_game_base::mem::safe_read_usize(base + ALIASED_RVA) }};
    let f = unsafe {{ er_game_base::mem::safe_read_usize(base + REEXPORTED_RVA) }};
    let g = unsafe {{ er_game_base::mem::safe_read_usize(base + row.rva) }};
}}
"""
    # The alias targets have to carry their literals, exactly as the real crates do.
    text += f"""
pub const SCALEFORM_MEMORY_FILE_VTABLE_RVA: usize = {globals_table:#x};
pub const DLSTRING_WCHAR_SUBSTR_RVA: usize = {name_labelled:#x};
"""
    want = {
        "NAME_LABELLED_RVA": "NAME_LABELLED_RVA",
        "SRCLOC_LABELLED_RVA": "SRCLOC_LABELLED_RVA",
        "GLOBALS_TABLE_RVA": "GLOBALS_TABLE_RVA",
        "ABSENT_RVA": "ABSENT_RVA [UNMAPPED]",
        "ALIASED_RVA": "ALIASED_RVA",
        "REEXPORTED_RVA": "REEXPORTED_RVA",
        "row.rva": "row.rva [NO-ADDRESS]",
    }
    out = []
    mapped = mapped_constants()
    for address, why in (
        (name_labelled, "a row labelled with a constant NAME"),
        (srcloc_labelled, "a row labelled with a `file.rs:line` SOURCE LOCATION"),
        (globals_table, "a row from the GLOBALS table"),
    ):
        if address not in mapped.addrs:
            out.append(f"label-shape control: {address:#x} -- {why} -- is not in mapped.addrs")
    if absent in mapped.addrs:
        out.append(f"label-shape control: negative control {absent:#x} IS in mapped.addrs")
    with tempfile.TemporaryDirectory() as tmp:
        path = os.path.join(tmp, "label_shapes.rs")
        with open(path, "w", encoding="utf-8") as handle:
            handle.write(text)
        result = scan([path], mapped)
    # Every use site above is a fault-safe read, so anything landing elsewhere is itself a defect.
    for bucket in ("exec", "write", "checked_write"):
        if result[bucket]:
            out.append(f"label-shape control: {len(result[bucket])} site(s) misfiled as {bucket}")
    got = {tag.split(" ", 1)[0]: tag for _where, tag in result["read"]}
    for operand, expected in want.items():
        if got.get(operand) != expected:
            out.append(
                f"label-shape control: `base + {operand}` tagged {got.get(operand)!r}, "
                f"expected {expected!r}"
            )
    return out


def _declaration_control() -> list[str]:
    """Prove the audit resolves constants the three frozen regexes could not -- through `scan()`.

    The tag is the DELIVERABLE, so the control asserts the tag rather than the regex. Four
    addresses that all have map rows are declared in four spellings the old reader could not
    resolve, and each is then read at a `base + X` site. A constant whose value cannot be resolved
    is tagged `[UNMAPPED]` -- which reads as "go and derive this address" for an address that is
    already derived, verified and in the ledger, and that is the failure this control exists to
    stop recurring.

    Each spelling is also asserted invisible to the frozen regexes. A control they can see would
    pass on the broken audit and prove nothing.
    """
    mapped = mapped_constants()
    enum_form = 0x876140  # needed.tsv, labelled `crates/er-invasion-warp/src/map_seams.rs:244`
    u32_form = 0x116C70  # needed.tsv, labelled `DLSTRING_WCHAR_SUBSTR_RVA`
    unsuffixed = 0x2BA4C80  # data.tsv, labelled `SCALEFORM_MEMORY_FILE_VTABLE_RVA`
    # In no table. It is inside the image window on purpose: an address outside it is discarded as
    # not-an-address before the mapped test runs, so it would pass this control without the mapped
    # test having been exercised at all.
    absent = 0x7FEEDB0
    text = f"""
#[repr(u32)]
pub enum MapSeamRva {{
    FirstSeam = {enum_form:#x},
}}
pub const ENUM_DERIVED_RVA: usize = MapSeamRva::FirstSeam as usize;
pub const U32_TYPED_RVA: u32 = {u32_form:#x};
pub const UNSUFFIXED_NAME: usize = {unsuffixed:#x};
pub const WRAPPED_RVA: usize =
    {u32_form:#x};
pub const ABSENT_RVA: usize = {absent:#x};
fn probe(base: usize) {{
    let a = unsafe {{ er_game_base::mem::safe_read_usize(base + ENUM_DERIVED_RVA) }};
    let b = unsafe {{ er_game_base::mem::safe_read_usize(base + U32_TYPED_RVA) }};
    let c = unsafe {{ er_game_base::mem::safe_read_usize(base + UNSUFFIXED_NAME) }};
    let d = unsafe {{ er_game_base::mem::safe_read_usize(base + WRAPPED_RVA) }};
    let e = unsafe {{ er_game_base::mem::safe_read_usize(base + ABSENT_RVA) }};
}}
"""
    out = []
    with tempfile.TemporaryDirectory() as tmp:
        path = os.path.join(tmp, "declaration_forms.rs")
        with open(path, "w", encoding="utf-8") as handle:
            handle.write(text)
        result = scan([path], mapped)
        _declared, address_constants = _declarations(symbol_index([path]), [path])
    got = {tag.split(" ", 1)[0]: tag for _where, tag in result["read"]}
    want = {
        "ENUM_DERIVED_RVA": "ENUM_DERIVED_RVA",
        "U32_TYPED_RVA": "U32_TYPED_RVA",
        "WRAPPED_RVA": "WRAPPED_RVA",
        # The negative control: an address in no table must still say so, or this passes by
        # tagging everything as mapped.
        "ABSENT_RVA": "ABSENT_RVA [UNMAPPED]",
    }
    for operand, expected in want.items():
        if got.get(operand) != expected:
            out.append(
                f"declaration control: `base + {operand}` tagged {got.get(operand)!r}, "
                f"expected {expected!r}"
            )
    # `UNSUFFIXED_NAME` cannot be asserted through a use site: `HAND_BUILT` still requires the
    # operand to be spelled `*RVA*` (see the docstring for why that filter stays here). It is
    # asserted through the ENUMERATOR instead, which is what this control is about -- the constant
    # is an address, is counted as one, and is recognised as mapped.
    if "UNSUFFIXED_NAME" not in address_constants:
        out.append(
            "declaration control: a constant whose name never carried the suffix was not counted "
            "as an address constant, though its value has a map row"
        )
    if "UNSUFFIXED_NAME" in result["unmapped"]:
        out.append("declaration control: a mapped constant was listed as unmapped")
    if result["declared"] != 6:
        out.append(
            f"declaration control: counted {result['declared']} address constants, expected 6 "
            "(five constants plus the enum variant the derived one is defined from)"
        )
    if result["unmapped"] != ["ABSENT_RVA"]:
        out.append(f"declaration control: unmapped list is {result['unmapped']}, expected ABSENT_RVA")
    # Non-VACUITY: the three frozen regexes must resolve none of the four spellings above. If one
    # of them can, that case proves nothing and has to be replaced.
    legacy = {name for name, _literal in LEGACY_CONST.findall(text)}
    legacy |= {name for name, _target in LEGACY_CONST_ALIAS.findall(text)}
    legacy |= {new for _old, new in LEGACY_USE_ALIAS.findall(text)}
    for operand in ("ENUM_DERIVED_RVA", "U32_TYPED_RVA", "UNSUFFIXED_NAME"):
        if operand in legacy:
            out.append(
                f"declaration control: the FROZEN regexes already resolved {operand}, so that case "
                "proves nothing -- pick a spelling they genuinely could not read"
            )
    if "ABSENT_RVA" not in legacy:
        out.append("declaration control fixture is wrong: the plain form must be legacy-visible")
    return out


def _masking_control() -> list[str]:
    """Prove a `base + FOO_RVA` inside a comment or a string is not counted as a site.

    The old skip was `line.lstrip().startswith("//")`, which sees a whole-line comment and nothing
    else. A trailing comment, a `/* */` block and a quoted example all counted -- and the sibling
    gate `check-stale-rva-calls.py` had two of its three baseline rows turn out to be exactly that,
    which is worse than a plain false positive: a baseline holding non-findings stays green while
    real sites are added beside them.
    """
    prose = (
        "fn probe(base: usize) {\n"
        "    let live = unsafe { safe_read_usize(base + REAL_SITE_RVA) };  // cf base + TRAILING_RVA\n"
        "    /* historical: base + BLOCK_RVA */\n"
        '    let note = "base + QUOTED_RVA";\n'
        "}\n"
    )
    out = []
    with tempfile.TemporaryDirectory() as tmp:
        path = os.path.join(tmp, "prose.rs")
        with open(path, "w", encoding="utf-8") as handle:
            handle.write(prose)
        result = scan([path], MappedRows())
    found = sorted(tag.split(" ", 1)[0] for _where, tag in result["read"])
    if found != ["REAL_SITE_RVA"]:
        out.append(
            f"masking control: counted {found}, expected only ['REAL_SITE_RVA'] -- the other three "
            "are a trailing comment, a block comment and a string body"
        )
    # Non-VACUITY: the pre-fix line filter really did read three of those four as sites.
    legacy_visible = [
        line
        for line in prose.splitlines()
        if not line.lstrip().startswith("//") and HAND_BUILT.search(line)
    ]
    if len(legacy_visible) < 3:
        out.append(
            f"masking control is vacuous: the old whole-line-comment filter saw only "
            f"{len(legacy_visible)} of these lines as sites, so masking changed nothing"
        )
    return out


def selftest() -> int:
    """Assert the regex set on facts established by the 2026-08-29 bisect."""
    failures = []
    # The splash-skip site is a raw write with a signature check -- it must not read as a defect.
    sample = """
    let target = (base + SPLASH_SKIP_RVA) as *mut u8;
    let existing = unsafe { *target };
    if existing != SPLASH_SKIP_EXPECTED_JE { return; }
    unsafe { *target = SPLASH_SKIP_REPLACEMENT_JG };
    """
    if not RAW_WRITE.search(sample):
        failures.append("RAW_WRITE missed a `*target = ` store")
    # The shape that went unseen for the whole 1.17 migration: cast-and-assign, no named pointer.
    if not RAW_WRITE.search("*((base + TITLE_GLOBAL_ACCEPT_BYTE_RVA) as *mut u8) = 1;"):
        failures.append("RAW_WRITE missed a cast-and-assign store")
    # Same store with the value on the following line, which is how rustfmt leaves the long ones.
    if not RAW_WRITE.search("*((module_base + SOME_RVA) as *mut u8) =\n    VALUE;"):
        failures.append("RAW_WRITE missed a cast-and-assign store split across lines")
    # A read through the same cast is not a write; misfiling it would inflate the count it guards.
    if RAW_WRITE.search("let after = unsafe { *((base + SOME_RVA) as *const u8) };"):
        failures.append("RAW_WRITE wrongly claimed a `*const` read is a store")
    if not SIGNATURE_CHECK.search(sample):
        failures.append("SIGNATURE_CHECK missed an EXPECTED-opcode compare")
    if not HAND_BUILT.search("let x = base + SOME_RVA;"):
        failures.append("HAND_BUILT missed `base + SOME_RVA`")
    # The shape the ratchet was blind to. `er-reload-trace` built all 40 of its detour targets from
    # this one line, and the ledger read `er-reload-trace 0 0` while it was writing five-byte JMPs
    # at stale 1.16.2 addresses. A lowercase, dotted field access is not an exotic spelling.
    for sample, why in (
        ("    let target = base + spec.rva;", "a lowercase dotted field `base + spec.rva`"),
        ("    let a = base + self.rva_1162;", "a suffixed lowercase field `base + self.rva_1162`"),
        ("    let b = module_base + hook.hook_rva;", "a prefixed lowercase field"),
        ("    let c = base + er_game_base::rva::FOO_RVA;", "a `::`-pathed constant"),
        ("    let d = image_base + rva;", "a bare lowercase `rva` local"),
    ):
        if not HAND_BUILT.search(sample):
            failures.append(f"HAND_BUILT missed {why}")
    if HAND_BUILT.search("let n = base + spec.offset;"):
        failures.append("HAND_BUILT wrongly claimed `base + spec.offset` is an address constant")
    if HAND_BUILT.search("let n = base + arvalue;"):
        failures.append("HAND_BUILT wrongly matched an identifier that merely contains `rva`")
    if HAND_BUILT.search("let target = base + spec.rva;").group(1) != "spec.rva":
        failures.append("HAND_BUILT captured the wrong operand for a dotted field")
    # ...and the corresponding gate spelling, or a correctly-gated installer reads as a defect.
    if not GATED.search("unsafe { er_hook::register_shared_hook(target, spec.detour, orig) }"):
        failures.append("GATED missed register_shared_hook(")
    if not GATED.search("unsafe { register_union_hook(target, handler, slot) }"):
        failures.append("GATED missed register_union_hook(")
    # A MinHook install on a hand-built address is an image write, not a fault-safe read.
    if not RAW_WRITE.search("MH_CreateHook(target as *mut c_void, detour, &mut tramp)"):
        failures.append("RAW_WRITE missed a raw MH_CreateHook install")
    if not RAW_WRITE.search("unsafe { MH_EnableHook(target as *mut c_void) }"):
        failures.append("RAW_WRITE missed a raw MH_EnableHook arm")
    failures.extend(_positive_control())
    failures.extend(_label_shape_control())
    if not EXEC_USE.search("unsafe { std::mem::transmute("):
        failures.append("EXEC_USE missed text ending in `transmute(`")
    if EXEC_USE.search("unsafe { is_in_state(sm, "):
        failures.append("EXEC_USE wrongly claimed a plain call argument is executable")
    if not READ_USE.search("if vt != base + FOO_VTABLE_RVA {"):
        failures.append("READ_USE missed an identity compare")
    if EXEC_USE.search("safe_read_usize(base + FOO_RVA)"):
        failures.append("EXEC_USE wrongly claimed a fault-safe read is executable")
    if not GATED.search("game_rva(FOO_RVA as u32)"):
        failures.append("GATED missed game_rva(")
    if not GATED.search("resolve_game_address(base + FOO_RVA, \"FOO\")"):
        failures.append("GATED missed resolve_game_address(")
    failures.extend(_declaration_control())
    failures.extend(_masking_control())
    if len(mapped_constants()) < 100:
        failures.append(f"only {len(mapped_constants())} mapped constants read from {NEEDED}")

    # Non-VACUITY of every input, before anything is concluded from it. A walk that reads nothing
    # produces `0 unmapped, 0 ungated` -- the most comfortable wrong answer this tool can give.
    index = rva_symbols.index()
    if index.files_read < 200:
        failures.append(f"the symbol index read only {index.files_read} sources; the walk is broken")
    live = mapped_constants()
    if len(live.addrs) < 300:
        failures.append(
            f"only {len(live.addrs)} mapped ADDRESSES read from the four tables "
            f"er-game-base/build.rs generates the runtime table from"
        )
    everything = rva_symbols.rust_sources()
    _, address_constants = _declarations(index, everything)
    if len(address_constants) < 300:
        failures.append(
            f"only {len(address_constants)} address constants found across the whole tree; the "
            "declaration reader is broken and every per-DLL column below it is unfounded"
        )
    # Nothing was LOST: every constant the three frozen regexes could resolve must still resolve.
    legacy_named = set()
    for text in index.text.values():
        legacy_named.update(name for name, _literal in LEGACY_CONST.findall(text))
        legacy_named.update(name for name, _target in LEGACY_CONST_ALIAS.findall(text))
        legacy_named.update(new for _old, new in LEGACY_USE_ALIAS.findall(text))
    if len(legacy_named) < 200:
        failures.append(
            f"the frozen regexes named only {len(legacy_named)} constants in the live tree, so the "
            "comparison below is against nothing"
        )
    declared_live, _ = _declarations(index, everything)
    lost = sorted(name for name in legacy_named if not declared_live.get(name))
    # An alias declares no value of its own, so it is resolved through `index.aliases`, not through
    # `declared`; only the ones that are neither are a real loss.
    lost = [name for name in lost if name not in index.aliases]
    if lost:
        failures.append(
            f"the widening LOST {len(lost)} constant(s) the frozen regexes resolved: {lost[:6]}"
        )
    if len(address_constants) <= len(legacy_named):
        failures.append(
            f"the resolver added nothing: {len(address_constants)} address constants against "
            f"{len(legacy_named)} from the three frozen regexes"
        )
    for line in failures:
        print(f"selftest FAIL {line}")
    print(f"selftest: {len(failures)} failure(s)")
    return 1 if failures else 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dll", help="report one cdylib in detail")
    parser.add_argument("--check", action="store_true", help="fail if any count rose above the ledger")
    parser.add_argument("--refresh", action="store_true", help="rewrite the ledger to current counts")
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    if args.selftest:
        return selftest()

    meta = json.loads(
        subprocess.run(
            ["cargo", "metadata", "--no-deps", "--format-version", "1"],
            capture_output=True, text=True, cwd=REPO, check=True, timeout=30,
        ).stdout
    )
    pkgs = {p["name"]: p for p in meta["packages"]}
    cdylibs = sorted(
        p["name"] for p in meta["packages"] if any("cdylib" in t["kind"] for t in p["targets"])
    )
    mapped = mapped_constants()
    runtime = runtime_results()

    if args.dll:
        if args.dll not in pkgs:
            print(f"unknown crate {args.dll}", file=sys.stderr)
            return 1
        cdylibs = [args.dll]

    quiet = args.check or args.refresh
    if not quiet:
        print(f"{'cdylib':26s} {'crates':>6} {'consts':>6} {'unmapd':>6} {'EXEC':>5} {'WRITE':>5} {'ok-wr':>5} {'read':>5}  runtime")
    totals = {"exec": 0, "write": 0, "read": 0}
    measured = {}
    for name in cdylibs:
        closure = crate_closure(name, pkgs)
        paths = [p for c in closure for p in crate_sources(pkgs, c)]
        result = scan(paths, mapped)
        for key in totals:
            totals[key] += len(result[key])
        measured[name] = (len(result["exec"]), len(result["write"]))
        if quiet:
            continue
        print(
            f"{name:26s} {len(closure):6d} {result['declared']:6d} {len(result['unmapped']):6d} "
            f"{len(result['exec']):5d} {len(result['write']):5d} "
            f"{len(result['checked_write']):5d} {len(result['read']):5d}  {runtime.get(name, '-')}"
        )
        if args.dll:
            for label in ("exec", "write", "checked_write", "read"):
                for where, const in result[label]:
                    print(f"    {label:14s} {where}  ({const})")
            if result["unmapped"]:
                print(f"    unmapped constants: {', '.join(result['unmapped'])}")
    if args.refresh:
        with open(LEDGER, "w", encoding="utf-8") as handle:
            handle.write("# cdylib\tungated_exec\tungated_write\n")
            handle.write("# Written by scripts/audit-1170-readiness.py --refresh. A RATCHET: the\n")
            handle.write("# checker fails when a count RISES. Lower is the only direction that\n")
            handle.write("# passes silently. 0 ungated writes across every cdylib is a safety\n")
            handle.write("# property of this tree -- no DLL can corrupt the 1.17 image with a\n")
            handle.write("# stale address -- and this file is what keeps it true.\n")
            for name, counts in sorted(measured.items()):
                handle.write(f"{name}\t{counts[0]}\t{counts[1]}\n")
        print(f"wrote {os.path.relpath(LEDGER, REPO)} ({len(measured)} rows)")
        return 0

    if args.check:
        ledger = {}
        try:
            with open(LEDGER, encoding="utf-8") as handle:
                for line in handle:
                    if line.startswith("#") or not line.strip():
                        continue
                    name, exec_n, write_n = line.split("\t")
                    ledger[name] = (int(exec_n), int(write_n))
        except OSError:
            print(f"[audit-1170] ERROR: no ledger at {os.path.relpath(LEDGER, REPO)}; run --refresh")
            return 1
        regressions = []
        for name, (exec_n, write_n) in sorted(measured.items()):
            was = ledger.get(name)
            if was is None:
                regressions.append(f"{name}: NEW cdylib with {exec_n} ungated exec / {write_n} ungated write")
            else:
                if exec_n > was[0]:
                    regressions.append(f"{name}: ungated EXEC {was[0]} -> {exec_n}")
                if write_n > was[1]:
                    regressions.append(f"{name}: ungated WRITE {was[1]} -> {write_n}")
        for line in regressions:
            print(f"[audit-1170] REGRESSION {line}")
        if regressions:
            print("[audit-1170] A hand-built `base + *_RVA` was added where the 1.17 gate cannot see it.")
            print("  Route it through `er_game_base::mem::game_rva` / `resolve_game_address` so a")
            print("  moved function REFUSES instead of executing whatever now sits at the address.")
            print("  If the increase is deliberate, accept it in one command and say why in the PR:")
            print("      python3 scripts/audit-1170-readiness.py --refresh")
            return 1
        improved = [n for n, c in measured.items() if n in ledger and (c[0] < ledger[n][0] or c[1] < ledger[n][1])]
        print(f"[audit-1170] ok -- no cdylib regressed" + (f"; {len(improved)} improved (run --refresh to bank it)" if improved else ""))
        return 0

    if not args.dll:
        print()
        print(f"TOTAL ungated EXEC {totals['exec']}, WRITE {totals['write']}, read/compare {totals['read']}")
        print("All three are hand-built `base + *_RVA` the gate never sees. EXEC executes a stale")
        print("address; WRITE corrupts the image; read/compare is fault-safe and merely goes")
        print("silently wrong. `ok-wr` is a write that checks the expected bytes first, which")
        print("turns a stale address into a refusal and is therefore NOT a defect.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
