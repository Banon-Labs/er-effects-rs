#!/usr/bin/env python3
r"""Fail when a crate calls game functions that the 1.17 address map cannot answer.

Why this exists
===============
On 2026-08-30 a user pressed "Load Build from URL" and the importer did nothing. It also
pressed "Generate Build Link" and got a build with no items in it. Neither surface reported a
problem: the telemetry said `imported_count=2, accepted_count=2, failed_count=0`.

The cause was one fact nobody could see at build time. `er-build-import-runtime` calls 27 game
functions, and every one of its constants is named without `RVA` in it --
`GET_WEAPON_NAME`, `SET_REINFORCEMENT`, `EQUIP_ITEM_TO_CHR_ASM_SLOT`. Every tool in this repo
that decides which addresses to translate keyed on the constant name, so all 27 were invisible:
never selected, never mapped, never verified. On 1.17 the running game refused all of them.

Refusing was correct -- calling a stale address transfers control into whatever moved there --
and the refusal was even logged. It was logged into a 2.3-million-line runtime log that somebody
has to launch the game to produce. Sixteen `ADDRESS REFUSED` lines, six of them the item-name
getters, and the consequence of those six was total: every item name failed to resolve, so
`read_character.rs:812` dropped all 18 equipped items, so the export was empty and the import had
nothing to apply.

That is the gap this closes. Two facts were in the tree the whole time -- "this crate calls game
address X" and "X is not in the map" -- and no gate put them next to each other. The failure
class is not new either: `select-needed-1170-rows.py` documents three earlier instances of it in
its own comments (`_BOUND` suffixes, enum aliases, bare `rva:` fields). This was the fourth.

What it is not
==============
* Not `check-detour-rva-coverage.py`. That gates DETOURS against the stricter detour map. Every
  address here is a direct call and never goes near MinHook; the two maps are different sets and
  a row can be callable without being detourable.
* Not `verify-rva-map-1170.py`. That asks whether a mapped destination is the right function.
  This asks the prior question -- whether there is a destination at all.

The vocabulary is derived, never transcribed
============================================
The map file paths and the refuted-verdict word are PARSED out of `crates/er-game-base/build.rs`,
because that file is what actually assembles the table the DLL consults. The resolver entry
points come from `scripts/rva_usage.py`, which reads them off the call sites. Addresses are
resolved from names by `scripts/rva_symbols.py`, which evaluates declarations to numbers and so
sees enum aliases and derived constants no `_RVA` regex would.

And the ledger set is discovered by path, not by the spelling of a Rust constant (fixed 2026-08-31)
Discovery used to be `const (\w*MAP\w*): &str = "..."`. `QUARANTINE` -- the ledger whose rows
`build.rs` removes from the translation table -- has no `MAP` in its name, so this gate never
opened it, and an address deliberately withdrawn from the table was reported covered. Nothing
looked wrong only because that ledger has no data rows yet: the first row anybody writes there is
a call this gate licenses while the running game refuses it, which is the 2026-08-30 defect above
with the evidence already written down and still unread. A name filter standing in for a semantic
test is the same substitution all four historical misses were made of.

What replaced it: every `const NAME: &str` whose value points into `docs/recon/*.tsv` is a ledger,
whatever it is called; its role (seeded, subtracted, or deliberately unwired) is read off what
`build.rs` does with it -- `let _ = AUDITED_DETOURS;` is unwired, and a ledger reached from the
`held_back` construction without a verdict test is a quarantine. And because a widened regex can
still miss a file, the parse is checked against the DISK: every
`docs/recon/rva-map-1162-to-1170*.tsv` and `docs/recon/rva-1170-*.tsv` must be accounted for by
name or pinned in `LEDGERS_NOT_BUILD_INPUTS` with a reason, and an unaccounted one is a
`VocabularyError` rather than a silent omission.

Nine audits in this repo have printed a confident zero because they transcribed a literal that
later drifted -- `verified_rvas()` filtered on `"IDENTICAL"` and matched 0 of 99 rows, and
`check-rva-alias-drift.py` then ran `assert bad == 0` over an empty set and passed. So this
script prints coverage rather than a bare verdict, and holds a frozen floor under the control
crate: if the matcher is ever blinded, the count collapses and the floor fails, instead of an
empty set reporting a clean tree.
"""

from __future__ import annotations

import argparse
import os
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))

import rva_symbols  # noqa: E402
import rva_usage  # noqa: E402

BUILD_RS = ROOT / "crates/er-game-base/build.rs"
RECON = ROOT / "docs/recon"


def _pinned_ledger_in_main_worktree(base):
    """Where a gitignored pinned ledger lives when this checkout is a linked worktree.

    One of the two `LEDGERS_NOT_BUILD_INPUTS` entries, `rva-map-1162-to-1170.functions.tsv`, is a
    2.4 MB generated table that `.gitignore` covers, and a gitignored file is never copied into a
    `git worktree`. This gate then raised `VocabularyError` -- exit 2, the loudest verdict it has
    -- at an agent who had done nothing but branch, and told them to find out where the ledger
    went. It is one directory away. Only the pinned names take this path, so a tracked ledger
    still has to be present in the checkout being gated.
    """
    try:
        import subprocess

        common = subprocess.run(
            ["git", "-C", str(ROOT), "rev-parse", "--git-common-dir"],
            capture_output=True,
            text=True,
            timeout=10,
            check=False,
        )
        if common.returncode != 0:
            return None
        main_root = (ROOT / common.stdout.strip()).resolve().parent
        candidate = (main_root / "docs/recon" / base).resolve()
        return candidate if candidate.is_file() else None
    except Exception:
        return None

# A ledger is something that points at `docs/recon/*.tsv`, not something with `MAP` in its name.
# See the module docstring: the previous `const (\w*MAP\w*)` filter could not see `QUARANTINE`,
# which is the one ledger whose rows are SUBTRACTED, so a withdrawn address read as covered.
LEDGER_DECLARATION = re.compile(
    r'const\s+(\w+)\s*:\s*&str\s*=\s*"([^"]*docs/recon/[^"]+\.tsv)"'
)
# `let _ = AUDITED_DETOURS;` -- declared, and deliberately wired into neither table (build.rs gives
# the reason at length: feeding those rows to detours put the 2026-08-29 crash straight back).
# Reading it here would hand a detour-audit row a call licence build.rs never granted.
UNWIRED_LEDGER = re.compile(r"let\s+_\s*=\s*(\w+)\s*;")
# The statement that assembles the held-back set, from its first line to the retain that applies
# it. Everything subtracted from the table is named in here, directly or through the function that
# reads it.
HELD_BACK_REGION = re.compile(r"let mut held_back\s*=(.*?)rows\.retain", re.S)
# Which files on disk the parse above is answerable for. Both prefixes, because a ledger that this
# gate cannot see is exactly the defect being fixed and the glob is what makes the miss loud.
LEDGER_GLOBS = ("rva-map-1162-to-1170*.tsv", "rva-1170-*.tsv")
# Files those globs catch that `build.rs` genuinely does not read. Each needs a reason, and each is
# asserted to still exist -- a pin describing a file that is gone reads as current while covering
# nothing.
LEDGERS_NOT_BUILD_INPUTS = {
    "rva-map-1162-to-1170.functions.tsv": (
        "the whole-image alignment of both .pdata function tables (128,602 pairs). It is the INPUT "
        "`scripts/select-needed-1170-rows.py` selects `needed.tsv` out of, not a build input; "
        "build.rs never opens it"
    ),
    "rva-map-1162-to-1170.tsv": (
        "candidate pairs from `scripts/map-rvas-1162-to-1170.py`, and its own header says so: "
        "'Candidates, not verified hook sites'. Most rows are UNRESOLVED with no 1.17 address at "
        "all; build.rs never opens it"
    ),
}

# The frozen control. `er-build-import-runtime` is the crate the whole defect was found in, and
# it declares its 27 game addresses in its own source. The floor is what makes a blinded matcher
# fail rather than report a clean tree: if `rva_usage` stops seeing resolver call sites, or
# `rva_symbols` stops resolving these names to numbers, this count drops and the run fails loudly.
# It is a floor, not an equality, so adding a native call is not a gate failure.
CONTROL_CRATE = "er-build-import-runtime"
# 37 = the 27 this crate declares itself, plus the 10 it imports from `er-game-base::rva` under a
# `use ... as` alias. The floor is deliberately set at the aliased total rather than the 27,
# because a regression that stopped following aliases would still clear a floor of 27 and report
# full coverage over three quarters of the set -- which is the exact shape of the defect this
# gate was written for.
CONTROL_MIN_ADDRESSES = 37
# One address from the control crate, spelled out, so a matcher that finds 27 of the wrong things
# still fails. `MsgRepositoryImp::GetWeaponName` -- the getter whose refusal emptied the export.
CONTROL_ADDRESS = 0xD11370


class VocabularyError(RuntimeError):
    """`build.rs` could not be parsed. Failing loudly beats licensing every call in the tree."""


def _balanced(text: str, opener: int, pair: str = "()") -> str:
    """The substring between the bracket at `opener` and the one that closes it."""
    depth = 0
    for index in range(opener, len(text)):
        if text[index] == pair[0]:
            depth += 1
        elif text[index] == pair[1]:
            depth -= 1
            if depth == 0:
                return text[opener + 1 : index]
    return ""


def _function_body(text: str, name: str) -> str:
    """`fn name(..) { BODY }` from `build.rs`, or `""`."""
    match = re.search(rf"\bfn\s+{re.escape(name)}\s*\(", text)
    if not match:
        return ""
    brace = text.find("{", match.end())
    return _balanced(text, brace, "{}") if brace >= 0 else ""


def _held_back_ledgers(text: str, ledgers: dict) -> tuple[set[str], set[str]]:
    """Split the ledgers `build.rs` SUBTRACTS into unconditional ones and verdict-filtered ones.

    The split is the whole point, and it is read off what build.rs does rather than off a name.
    `held_back` is assembled from two shapes:

        let mut held_back = quarantined(root_dir);
        held_back.extend(refuted_sources(&Path::new(root_dir).join(VERIFIED_MAP)));

    The second names its ledger in the argument and drops only the rows carrying one verdict, so
    that ledger is still a source of coverage -- it is `refuted` that decides row by row. The first
    names no ledger at all: the reader holds its own path, and every row it carries is withdrawn.
    That is a quarantine, and it is the one this gate used to miss entirely because the constant is
    spelled `QUARANTINE` rather than `*_MAP*`.
    """
    region = HELD_BACK_REGION.search(text)
    if not region:
        raise VocabularyError(
            "could not find where `build.rs::emit_address_map` assembles `held_back`; that block is "
            "what removes addresses from the table, and a gate that cannot read it reports a "
            "WITHDRAWN address as callable"
        )
    body = region.group(1)
    unconditional: set[str] = set()
    filtered: set[str] = set()
    for call in re.finditer(r"\b(\w+)\s*\(", body):
        arguments = _balanced(body, call.end() - 1)
        named = {n for n in ledgers if re.search(rf"\b{re.escape(n)}\b", arguments)}
        if named:
            filtered |= named
            continue
        reader = _function_body(text, call.group(1))
        unconditional |= {n for n in ledgers if re.search(rf"\b{re.escape(n)}\b", reader)}
    return unconditional, filtered - unconditional


def assert_ledgers_accounted(ledgers: dict) -> None:
    """Every ledger on disk is read, subtracted, or pinned as not-a-build-input. No fourth case.

    A widened regex is still a regex. This is the assertion that a ledger cannot go missing by
    spelling again: the check is against the files that exist, so hiding one from the parse -- by a
    narrower pattern, a renamed constant, a moved path -- fails here instead of quietly shrinking
    the set of addresses this gate believes are unusable.
    """
    known = {(BUILD_RS.parent / rel).resolve() for rel in ledgers.values()}
    exempt = {}
    for base, why in LEDGERS_NOT_BUILD_INPUTS.items():
        path = (RECON / base).resolve()
        if not path.is_file():
            path = _pinned_ledger_in_main_worktree(base) or path
        if not path.is_file():
            raise VocabularyError(
                f"{base} is pinned in LEDGERS_NOT_BUILD_INPUTS as deliberately not a build input "
                f"({why}) and is no longer on disk. The pin now describes a file that is gone while "
                "still reading as current; delete it, or find out where the ledger went"
            )
        exempt[path] = why
    unaccounted = []
    for pattern in LEDGER_GLOBS:
        for path in sorted(RECON.glob(pattern)):
            resolved = path.resolve()
            if resolved not in known and resolved not in exempt:
                unaccounted.append(path.name)
    if unaccounted:
        raise VocabularyError(
            f"{len(unaccounted)} ledger(s) on disk are not named by any `const ...: &str` in "
            f"{BUILD_RS} and are not pinned in LEDGERS_NOT_BUILD_INPUTS: "
            + ", ".join(sorted(set(unaccounted)))
            + ". Either build.rs reads it -- in which case this gate must too, or its rows are "
            "invisible here -- or it does not, in which case pin it with the reason. An unread "
            "ledger is how a QUARANTINED address came to be reported as covered"
        )


def read_build_vocabulary() -> dict:
    """Ledger paths with their roles, and the refuted-verdict word, parsed out of `build.rs`."""
    text = BUILD_RS.read_text(encoding="utf-8", errors="replace")
    ledgers = dict(LEDGER_DECLARATION.findall(text))
    if not ledgers:
        raise VocabularyError(
            f'no `const NAME: &str = "..../docs/recon/*.tsv"` declarations found in {BUILD_RS}; the '
            "ledger paths are the whole input to this gate and guessing them would license every call"
        )
    assert_ledgers_accounted(ledgers)
    unwired = {name for name in UNWIRED_LEDGER.findall(text) if name in ledgers}
    quarantine, filtered = _held_back_ledgers(text, ledgers)
    if not quarantine:
        raise VocabularyError(
            "build.rs subtracts a quarantine ledger from the translation table and this gate could "
            "not work out which constant holds it. Scoring the maps without it reports an address "
            "that was deliberately WITHDRAWN as covered -- the exact defect this parse replaced"
        )
    for name in set(ledgers) - unwired:
        path = (BUILD_RS.parent / ledgers[name]).resolve()
        if not path.is_file():
            raise VocabularyError(
                f"{name} points at {path}, which does not exist. A ledger that cannot be opened "
                "reads as zero rows, and zero rows in a SOURCE map under-reports coverage while "
                "zero rows in the QUARANTINE over-reports it"
            )
    refuted = re.search(r'fields\[2\]\s*!=\s*"([A-Z-]+)"', text)
    if not refuted:
        raise VocabularyError(
            "could not find the verdict literal `build.rs::refuted_sources` subtracts on; without "
            "it this gate would count a KNOWN-WRONG pair as coverage"
        )
    return {
        "ledgers": ledgers,
        # The maps rows are counted from. Named `paths` because that is what the rest of this file
        # has always called them; what changed is that the set is now a subtraction of roles rather
        # than a match on the letters `MAP`.
        "paths": {name: ledgers[name] for name in sorted(set(ledgers) - unwired - quarantine)},
        # Read row-for-row and removed. Every address in here is unusable on 1.17 by a human
        # decision, which is a stronger statement than "no row exists yet".
        "quarantine": {name: ledgers[name] for name in sorted(quarantine)},
        "unwired": sorted(unwired),
        "verdict_filtered": sorted(filtered),
        "refuted": refuted.group(1),
    }


IMAGE_BASE = 0x140000000


def _rows(path: Path, min_columns: int = 2):
    """`(source RVA, columns)` per data row. `min_columns` because the ledgers differ in width.

    A quarantine row is `RVA <TAB> reason` and `build.rs::quarantined` reads column 0 alone -- it
    imposes no width at all. Demanding two columns of it, as the map reader does, would silently
    skip a row somebody wrote without a reason and hand that address back its coverage.
    """
    if not path.is_file():
        return
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        if line.startswith("#") or not line.strip():
            continue
        cols = line.split("\t")
        if len(cols) < min_columns:
            continue
        try:
            yield int(cols[0], 16), cols
        except ValueError:
            continue


def _normalise(source: int) -> int:
    """A ledger writes either a VA or an RVA; `build.rs` reads both. So does this."""
    return source - IMAGE_BASE if source >= IMAGE_BASE else source


def callable_sources(vocab: dict) -> tuple[set[int], set[int], set[int]]:
    """1.16.2 RVAs the generated call table can answer, and the two ways it refuses them.

    `build.rs::emit_address_map` seeds the call map from the curated ledger, then adds every row
    of the function and data maps it does not already hold, then SUBTRACTS two things:

    * sources any verdict table marks with the refuted verdict -- a comparison that ran and
      disagreed, so the row is positive evidence of a wrong address rather than a missing one;
    * every source in the quarantine ledger, whose rows verify and are withheld anyway because the
      handler at that address turned out to be stale on 1.17.

    Both leave the address unanswerable at runtime, which is the only question this gate asks. The
    second was not read here at all until 2026-08-31, so a quarantined address -- one a human had
    deliberately withdrawn, with a reason written beside it -- was reported covered.
    """
    have: set[int] = set()
    refused: set[int] = set()
    quarantined: set[int] = set()
    for rel in vocab["paths"].values():
        path = (BUILD_RS.parent / rel).resolve()
        for source, cols in _rows(path):
            rva = _normalise(source)
            if len(cols) > 2 and cols[2] == vocab["refuted"]:
                refused.add(rva)
            else:
                have.add(rva)
    for rel in vocab["quarantine"].values():
        path = (BUILD_RS.parent / rel).resolve()
        for source, _cols in _rows(path, min_columns=1):
            quarantined.add(_normalise(source))
    return have - refused - quarantined, refused, quarantined


def crate_of(path: Path) -> str:
    parts = path.relative_to(ROOT).parts
    return parts[1] if len(parts) > 2 and parts[0] == "crates" else "<workspace>"


def audit(vocab: dict | None = None) -> dict:
    """`vocab` is injectable so `--selftest` can plant a quarantine row without editing a ledger."""
    vocab = vocab if vocab is not None else read_build_vocabulary()
    have, refused, quarantined = callable_sources(vocab)
    index = rva_symbols.index()

    # Which names each crate hands to an address resolver. Test modules are skipped: a test may
    # name an address precisely to assert the workspace does not use it.
    wanted: dict[str, dict[str, set[int]]] = {}
    unresolved: dict[str, set[str]] = {}
    all_paths = sorted(ROOT.glob("crates/**/*.rs"))
    # Files that are a foreign module are not priced against the game map.
    #
    # `er-invasion-warp` resolves four RVAs against Seamless Co-op's base, not the game's:
    # `GetModuleHandleA("ersc.dll")` + `0x241a0` / `0x25850` / `0x258d0` / `0xad6e0`. Priced against
    # the 1.17 eldenring.exe map they are of course absent, and this gate reported
    # `er-invasion-warp 0/5 zero coverage -- every call it makes is refused at runtime and the
    # feature is silently inert`. That verdict was a category error in the GATE: the addresses are
    # correct, they simply describe a different module.
    #
    # The rule is not invented here. `audit-1170-coverage-inventory.py` already owns it, and its
    # own selftest asserts it (`a constant declared inside mod ersc is not attributed to ersc.dll`),
    # including the `mod ersc;`-beside-`ersc.rs` file form that a 2026-09-02 refactor introduced.
    # Importing it rather than re-deriving it means one definition of "foreign", so the two gates
    # cannot drift into disagreeing about which module an address belongs to.
    #
    # Failing open is deliberate: if the sibling cannot be imported, every file stays game-priced,
    # which is the stricter reading. A gate that silently stopped checking would be worse than one
    # that occasionally over-reports.
    foreign: dict[str, str] = {}
    try:
        import importlib.util as _ilu

        _spec = _ilu.spec_from_file_location(
            "audit_1170_coverage_inventory", ROOT / "scripts" / "audit-1170-coverage-inventory.py"
        )
        if _spec and _spec.loader:
            _mod = _ilu.module_from_spec(_spec)
            _spec.loader.exec_module(_mod)
            foreign = _mod.foreign_module_files([str(p) for p in all_paths])
    except Exception:
        foreign = {}
    for path in all_paths:
        if str(path) in foreign:
            continue
        text = path.read_text(encoding="utf-8", errors="replace")
        spans = rva_usage.test_module_spans(text)
        if spans:
            keep, last = [], 0
            for start, end in spans:
                keep.append(text[last:start])
                last = end
            keep.append(text[last:])
            text = "".join(keep)
        names = rva_usage.used_as_game_address(text)
        if not names:
            continue
        crate = crate_of(path)
        for name in names:
            values = _values_of(index, name)
            if values:
                wanted.setdefault(crate, {}).setdefault(name, set()).update(values)
            else:
                # Not dropped. A name this gate cannot price is a name it cannot check, and
                # silently skipping it is the same invisibility the gate exists to end -- it is
                # how 27 addresses went unnoticed in the first place. Measured: dropping these
                # quietly hid ten real addresses that `er-build-import-runtime` imports under an
                # alias from `er-game-base`.
                unresolved.setdefault(crate, set()).add(name)
    return {
        "vocab": vocab,
        "have": have,
        "refused": refused,
        "quarantined": quarantined,
        "wanted": wanted,
        "unresolved": unresolved,
    }


def _values_of(index, name: str) -> set[int]:
    """Every numeric value `name` can denote, following `use ... as ALIAS` one hop.

    The alias hop is not a nicety. `er-build-import-runtime` reaches ten of its game functions as
    `use er_game_base::rva::GET_EQUIP_INVENTORY_DATA_RVA as GET_EQUIP_INVENTORY_DATA;` and then
    calls the short name, which is declared nowhere. Without this the gate prices 27 of that
    crate's 37 addresses and calls it full coverage.
    """
    values: set[int] = set()
    seen: set[str] = set()
    queue = [name]
    while queue:
        current = queue.pop()
        if current in seen:
            continue
        seen.add(current)
        for decl in index.by_simple.get(current) or []:
            values |= {v for v in (decl.value or set()) if isinstance(v, int)}
        target = index.aliases.get(current)
        if target:
            queue.append(target.rsplit("::", 1)[-1])
    return values


def coverage(result: dict) -> list[tuple]:
    """Per crate: (crate, total, mapped, [(name, rva) unmapped])."""
    out = []
    for crate, names in sorted(result["wanted"].items()):
        total = mapped = 0
        gaps = []
        for name, values in sorted(names.items()):
            for rva in sorted(values):
                total += 1
                if rva in result["have"]:
                    mapped += 1
                else:
                    gaps.append((name, rva))
        out.append((crate, total, mapped, gaps))
    return out


def report(result: dict, show_gaps: bool) -> None:
    rows = coverage(result)
    width = max((len(r[0]) for r in rows), default= 10)
    quarantined = result.get("quarantined") or set()
    print("native CALL address coverage against the 1.17 map assembled by er-game-base/build.rs")
    print(
        f"  map rows callable: {len(result['have'])}   refuted and subtracted: "
        f"{len(result['refused'])}   quarantined and subtracted: {len(quarantined)}"
    )
    print()
    for crate, total, mapped, gaps in rows:
        pct = (100.0 * mapped / total) if total else 0.0
        flag = "  <-- ZERO COVERAGE" if total and not mapped else ""
        print(f"  {crate:<{width}}  {mapped:3d}/{total:3d}  {pct:5.1f}%{flag}")
        if show_gaps:
            for name, rva in gaps:
                # Why it is unmapped is the difference between "nobody has mapped this yet" and
                # "somebody read the handler and withdrew it"; only the second names a decision.
                why = "  QUARANTINED" if rva in quarantined else ""
                print(f"      unmapped 0x{rva:x}  {name}{why}")
    unresolved = result.get("unresolved") or {}
    if unresolved:
        print("  names handed to a resolver that this gate could not price (NOT checked above):")
        for crate, names in sorted(unresolved.items()):
            print(f"    {crate}: {', '.join(sorted(names))}")
    print()


def verdict(result: dict) -> int:
    rows = coverage(result)
    failures: list[str] = []

    for crate, total, mapped, gaps in rows:
        if total and not mapped:
            failures.append(
                f"{crate} resolves {total} game address(es) and NOT ONE is in the map. Every call "
                f"it makes is refused at runtime and the feature is silently inert. "
                f"First few: " + ", ".join(f"{n} 0x{r:x}" for n, r in gaps[:4])
            )

    # A QUARANTINED address a crate still calls fails on its own, at any coverage level.
    #
    # It is not the ordinary gap. An address with no row yet is a thing nobody has got to; a row in
    # the quarantine ledger is a human who read the handler, found it stale on 1.17, and withdrew
    # the mapping on purpose -- `build.rs` drops it from the table and the call then refuses. Left
    # to the zero-coverage rule alone, withdrawing one of `er-build-import-runtime`'s 45 addresses
    # would print 44/45, pass, and leave exactly the silent-inert call this whole gate was written
    # after. Quarantining an address something still calls is a decision about that FEATURE: either
    # the call goes, or the quarantine row does.
    quarantined = result.get("quarantined") or set()
    if quarantined:
        ledgers = ", ".join(sorted(result["vocab"].get("quarantine", {}).values())) or "the quarantine ledger"
        for crate, names in sorted(result["wanted"].items()):
            hits = sorted(
                {(name, rva) for name, values in names.items() for rva in values if rva in quarantined}
            )
            if hits:
                failures.append(
                    f"{crate} calls {len(hits)} address(es) held back by {ledgers}. Those rows are "
                    "withdrawn from the translation table on purpose, so every one of these calls "
                    "is REFUSED at runtime and whatever it drives is inert with nothing said. "
                    "First few: " + ", ".join(f"{n} 0x{r:x}" for n, r in hits[:4])
                )

    control = dict((c, (t, m)) for c, t, m, _ in rows).get(CONTROL_CRATE)
    if control is None:
        failures.append(
            f"the frozen control crate {CONTROL_CRATE} produced NO resolver sites at all. That is "
            "a blinded matcher, not a clean tree -- this gate cannot see anything and must not "
            "report success."
        )
    else:
        total, mapped = control
        if total < CONTROL_MIN_ADDRESSES:
            failures.append(
                f"the frozen control {CONTROL_CRATE} resolved {total} addresses, below the floor "
                f"of {CONTROL_MIN_ADDRESSES}. Either the matcher regressed or those calls were "
                "removed; both need a human, because an under-counting matcher reports a clean "
                "tree over a broken one."
            )
        control_rvas = set()
        for values in result["wanted"].get(CONTROL_CRATE, {}).values():
            control_rvas |= values
        if CONTROL_ADDRESS not in control_rvas:
            failures.append(
                f"the frozen control ADDRESS 0x{CONTROL_ADDRESS:x} "
                "(MsgRepositoryImp::GetWeaponName) was not among the control crate's resolved "
                "addresses. A matcher can find the right NUMBER of wrong things; this is the "
                "check that it found the right thing."
            )

    for failure in failures:
        print(f"check-native-call-rva-coverage: {failure}", file=sys.stderr)
    if failures:
        return 1
    total = sum(r[1] for r in rows)
    mapped = sum(r[2] for r in rows)
    print(
        f"check-native-call-rva-coverage: OK -- {mapped}/{total} resolved call addresses across "
        f"{len(rows)} crate(s) are answerable on 1.17; no crate is wholly unmapped; control "
        f"{CONTROL_CRATE} at {control[1]}/{control[0]} over a floor of {CONTROL_MIN_ADDRESSES}."
    )
    return 0


def selftest() -> int:
    """Prove the gate is non-vacuous: blind the matcher and watch the frozen control fail."""
    failures: list[str] = []

    def check(label, got, want):
        if got != want:
            failures.append(f"{label}: got {got!r}, want {want!r}")

    live = audit()
    rows = dict((c, (t, m)) for c, t, m, _ in coverage(live))
    check("the control crate is found", CONTROL_CRATE in rows, True)
    total, mapped = rows.get(CONTROL_CRATE, (0, 0))
    check("...and clears its frozen floor", total >= CONTROL_MIN_ADDRESSES, True)
    check("...and is fully mapped today", (total, mapped) == (total, total), True)
    check("...and the live tree passes", verdict(live), 0)

    # Non-VACUITY. Blind `rva_usage` exactly the way the four historical misses blinded the older
    # tools -- make the resolver call sites unmatchable -- and the run must fail. If it still
    # passes, every assertion above is decoration.
    keep = rva_usage.RESOLVERS
    try:
        rva_usage.RESOLVERS = re.compile(r"\bnothing_matches_this_resolver\s*\(")
        blinded = audit()
        blind_rows = dict((c, (t, m)) for c, t, m, _ in coverage(blinded))
        check(
            "BLINDED: the control crate collapses",
            blind_rows.get(CONTROL_CRATE, (0, 0))[0] < CONTROL_MIN_ADDRESSES,
            True,
        )
        check("BLINDED: and the gate FAILS rather than reporting a clean tree", verdict(blinded), 1)
    finally:
        rva_usage.RESOLVERS = keep

    # The quarantine control. Until 2026-08-31 this gate never opened the quarantine ledger, so an
    # address a human had deliberately withdrawn -- reason written beside it -- was reported
    # covered. It was invisible because that ledger has no data rows: with none, reading it and not
    # reading it produce the same answer. Plant one and the two answers must diverge.
    #
    # The row names the frozen control address, which the live tree covers today (asserted below,
    # first, so the mutant is not proving something that was already false).
    import tempfile

    check(
        "the control address is callable in the LIVE tree",
        CONTROL_ADDRESS in live["have"],
        True,
    )
    for name, rel in live["vocab"]["quarantine"].items():
        check(
            f"the quarantine ledger {name} is on disk and was actually read",
            (BUILD_RS.parent / rel).resolve().is_file(),
            True,
        )
    with tempfile.TemporaryDirectory() as tmp:
        planted = Path(tmp) / "rva-1170-quarantine.tsv"
        planted.write_text(
            "# selftest fixture\n"
            f"0x{CONTROL_ADDRESS:x}\tselftest: a handler proved stale, mapping withheld\n",
            encoding="utf-8",
        )
        mutant = dict(live["vocab"])
        mutant["quarantine"] = {"QUARANTINE": str(planted)}
        held = audit(vocab=mutant)
        check(
            "QUARANTINED: the address leaves the callable set",
            CONTROL_ADDRESS in held["have"],
            False,
        )
        check(
            "QUARANTINED: ...and it is reported as withheld, not merely absent",
            CONTROL_ADDRESS in held["quarantined"],
            True,
        )
        check(
            "QUARANTINED: ...and the gate FAILS even though the crate keeps 44 of 45 addresses",
            verdict(held),
            1,
        )

    # The ledger-discovery control, in the shape of the defect itself. This is the exact matcher
    # this file used to discover ledgers with, spelled out as a literal rather than composed from
    # the live one -- a control assembled from the live pattern widens when it widens, and then
    # "the old matcher misses this" quietly becomes "the new matcher misses this".
    legacy = re.compile(r'const\s+(\w*MAP\w*)\s*:\s*&str\s*=\s*"([^"]+)"')
    # ...and a second mutant that hides one ordinary map, to prove the assertion is about files on
    # disk rather than about the word quarantine.
    hides_the_data_map = re.compile(
        r'const\s+(\w+)\s*:\s*&str\s*=\s*"([^"]*docs/recon/(?!rva-map-1162-to-1170\.data)[^"]+\.tsv)"'
    )
    saved_declaration = globals()["LEDGER_DECLARATION"]
    for label, pattern, expected in (
        ("the frozen pre-fix `*MAP*` name filter", legacy, "rva-1170-quarantine.tsv"),
        ("a parse that hides one ordinary map", hides_the_data_map, "rva-map-1162-to-1170.data.tsv"),
    ):
        try:
            globals()["LEDGER_DECLARATION"] = pattern
            try:
                read_build_vocabulary()
                failures.append(
                    f"HIDDEN LEDGER: {label} still produced a vocabulary. A ledger this gate cannot "
                    "see contributes no rows, and for the quarantine that means a withdrawn address "
                    "reads as covered -- which is the defect, restored"
                )
            except VocabularyError as raised:
                if expected not in str(raised):
                    failures.append(
                        f"HIDDEN LEDGER: {label} raised, but did not name {expected}: {raised}"
                    )
        finally:
            globals()["LEDGER_DECLARATION"] = saved_declaration
    # Non-VACUITY of the two above: unpatched, the same call must succeed.
    try:
        read_build_vocabulary()
    except VocabularyError as raised:
        failures.append(f"the LIVE build.rs no longer parses, so the mutants prove nothing: {raised}")

    # Every pinned exemption must still describe a file that exists; a pin over a deleted ledger
    # reads as current while covering nothing.
    for base in LEDGERS_NOT_BUILD_INPUTS:
        # Resolved the way the gate resolves it, not as `RECON / base`. One pinned ledger is
        # gitignored, so in a linked worktree the second spelling is False for a file that is
        # present and readable one directory away -- and this control would then be red about a
        # pin that is perfectly current.
        found = (RECON / base).is_file() or bool(_pinned_ledger_in_main_worktree(base))
        check(f"the pinned non-input {base} still exists", found, True)

    # And the vocabulary must fail loudly rather than default.
    saved = globals()["BUILD_RS"]
    try:
        globals()["BUILD_RS"] = ROOT / "docs/recon/rva-map-1162-to-1170.needed.tsv"
        try:
            read_build_vocabulary()
            failures.append("an unparsable build.rs did not raise VocabularyError")
        except VocabularyError:
            pass
    finally:
        globals()["BUILD_RS"] = saved

    for failure in failures:
        print(f"check-native-call-rva-coverage selftest FAILED -- {failure}", file=sys.stderr)
    if failures:
        return 1
    print(
        f"check-native-call-rva-coverage selftest: OK (control {CONTROL_CRATE} {mapped}/{total}, "
        f"floor {CONTROL_MIN_ADDRESSES}, blinding observed to fail, a planted quarantine row "
        f"observed to withdraw 0x{CONTROL_ADDRESS:x} and fail the gate, every ledger on disk "
        "accounted for, and hiding one from the parse fails loudly)"
    )
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--gaps", action="store_true", help="list every unmapped address")
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    if args.selftest:
        return selftest()
    try:
        result = audit()
    except VocabularyError as error:
        print(f"check-native-call-rva-coverage: {error}", file=sys.stderr)
        return 2
    report(result, show_gaps=args.gaps)
    return verdict(result)


if __name__ == "__main__":
    raise SystemExit(main())
