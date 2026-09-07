#!/usr/bin/env python3
"""Every constant whose value is an expression must evaluate, or be listed here with a reason.

Why. Two audits on 2026-08-31 hit the same wall from opposite directions, and it is the same
defect both times: a constant whose initialiser is not a bare literal is invisible to the tool that
is supposed to check it, and invisible reads exactly like checked.

  * Address side. `ADD_DEFAULT_FILE_LOAD_PROCESS_RVA: usize = 0x142658c60 - 0x140000000` was
    harvested by a regex that captures the first hex literal, so the recorded value was the
    MINUEND -- an absolute VA, not the RVA the constant holds. It matched nothing in an RVA-keyed
    map, landed in `missing`, and was neither checked nor reported as unchecked. It is a real
    `.text` function (`FD4::FD4FileCap::AddDefaultFileLoadProcess`) and it moved on 1.17.
  * Offset side. The field-offset inventory filed an initialiser it could not read as
    `kind="expr", resolved=None` and stopped there. 41 of 813 live game-struct-field offsets were
    in that state -- dropped from the census without appearing in the unattributed ratchet either.

`scripts/const_fold.py` folds the restricted grammar both tools need. This gate is what stops the
class coming back: a constant that no inventory can evaluate fails, naming the constant and its
definition site, unless it is in `UNRESOLVABLE` below.

The list and the detector check each other, in both directions, the same shape as
`rva_role.NOT_AN_ADDRESS`:

  * an unresolvable constant that is not listed fails -- that is the invisibility this closes;
  * a listed constant that now resolves fails too. A stale entry is worse than a missing one: it
    silently excuses a value the tree has since learned how to compute, so the number goes
    unchecked while the list claims that is deliberate.

To wire it into scripts/check.sh (not done here: that file's commit is held by other work):

    python3 "$repo_root/scripts/check-expression-constants.py" --selftest
    python3 "$repo_root/scripts/check-expression-constants.py"
"""

from __future__ import annotations

import argparse
import importlib.util
import os
import re
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))

import const_fold  # noqa: E402 - repo-local; the sys.path line above is what makes it work
import rva_role  # noqa: E402
import rva_usage  # noqa: E402

BOUND = re.compile(r"_(MIN|MAX|BOUND|BASE|SIZE|LEN|LENGTH|COUNT|END|START|STRIDE|ALIGN)$")

# The coverage floor. Both populations are derived -- one from a name filter plus the resolver call
# sites, the other from the field-offset inventory's own classifier -- so a constant leaves them by
# being edited somewhere else entirely, and until this file existed it left in silence. Only the 24
# names in `UNRESOLVABLE` had any departure check at all, and the population total cannot stand in
# for one: measured across the 22 minutes between d130b4ee and 4b4a9722 on 2026-08-31, 28 names left
# the address population and 30 arrived, so the total moved by +2 and hid 28 departures behind 30
# arrivals. (All 28 were the `er-build-import-runtime` rename to `*_RVA`, so nothing actually lost
# coverage -- but nothing said so either, and a rename that dropped the `_RVA` instead would have
# read exactly the same.) arrivals are free, only departures are gated: more coverage never needs
# permission, so a new constant does not touch this file and the churn is bounded to real removals.
FLOOR = Path(__file__).resolve().parent / "expression-constants.floor.txt"
FLOOR_HEADER = """\
# Constants that WERE in the gated population of scripts/check-expression-constants.py.
#
# Every name here must still be gated. A name that disappears fails the gate: coverage left, and
# a derived population loses members silently. Renames show up as one departure and one arrival --
# check the new name is gated, then DELETE THAT ONE LINE here in the same commit. Arrivals need no
# entry at all, so adding a constant never touches this file.
#
# Prefer the one-line delete. `--refresh-floor` rewrites the file wholesale from the tree it is run
# in, so in a shared checkout it bakes in every other agent's uncommitted names; the gate then
# fails for everyone who does not have that working tree. Run it only from a clean tree.
#
# Regenerate wholesale: python3 scripts/check-expression-constants.py --refresh-floor
"""

# The documented exceptions. Every entry is a constant in one of the two gated populations whose
# value genuinely cannot be established from the sources this repo has, with the reason. Nothing
# here is "we did not get to it": each is a specific missing capability, and each is re-checked
# against the tree on every run so it cannot rot into a silent excuse.
#
# The dominant class is `offset_of!` on a type declared in the sibling `fromsoftware-rs` bindings
# using enums, generics or nested game types that `detect-struct-field-drift.py`'s `repr(C)`
# modeller does not lay out. Those constants are checked, by the compiler, on every build -- the
# value is whatever `rustc` computes -- so what is missing here is only this repo's ability to
# print the number, not the number's correctness.
UNRESOLVABLE: dict[str, str] = {
    "CHR_ASM_EQUIPMENT_OFFSET": "offset_of!(ChrAsm, ...): the repr(C) modeller does not carry ChrAsm",
    "CHR_ASM_GAITEM_HANDLES_OFFSET": "offset_of!(ChrAsm, ...): ChrAsm is not modelled",
    "CHR_ASM_EQUIPMENT_PARAM_IDS_OFFSET": "offset_of!(ChrAsm, ...): ChrAsm is not modelled",
    "CHR_ASM_UNKD4_OFFSET": "chains off CHR_ASM_EQUIPMENT_PARAM_IDS_OFFSET, above",
    "CHR_ASM_UNKD8_OFFSET": "chains off CHR_ASM_UNKD4_OFFSET, above",
    "EQUIP_GAME_DATA_CHR_ASM_OFFSET": "offset_of!(EquipGameData, chr_asm): not modelled",
    "PGD_EQUIP_GAME_DATA_OFFSET": "offset_of!(PlayerGameData, equipment): not modelled",
    "PGD_FACE_DATA_OFFSET": "offset_of!(PlayerGameData, face_data): not modelled",
    "PGD_STAT_END_OFFSET": "offset_of!(PlayerGameData, base_hero_point): not modelled",
    "INVENTORY_OFFSET": "offset_of!(EquipGameData, equip_inventory_data): not modelled",
    "GAME_MAN_SAVE_SLOT_OFFSET": "offset_of!(GameMan, save_slot): GameMan is not modelled",
    "GAME_MAN_SAVE_STATE_OFFSET": "offset_of!(GameMan, save_state): not modelled",
    "GAME_MAN_REQUESTED_SAVE_SLOT_LOAD_INDEX_OFFSET": "offset_of!(GameMan, ...): not modelled",
    "GAME_MAN_REAL_LOAD_DONE_OFFSET": "offset_of!(GameMan, warp_requested): not modelled",
    "GAME_MAN_FLAG_B73_PROBE_OFFSET": "sums offset_of!(GameMan, save_requested), not modelled",
    "GAME_MAN_FLAG_B75_PROBE_OFFSET": "sums offset_of!(GameMan, save_requested), not modelled",
    "GAME_MAN_B73_FLAG_OFFSET": "chains off GAME_MAN_FLAG_B73_PROBE_OFFSET, above",
    "GAME_MAN_FLAG_BBC_OFFSET": "chains off GAME_MAN_FLAG_BC4_OFFSET -> offset_of!(GameMan, ...)",
    "SLOT_MANAGER_DATA_OFFSET": "offset_of!(GameDataMan, main_player_game_data): not modelled",
    "TITLE_OWNER_JOB_PENDING_OFFSET": "offset_of!(TitleOwnerLoadJobLayout, pending): not modelled",
    "SELECTOR_CTX_OFFSET_F8": "offset_of!(SelectorBuilderOwnerLayout, selector_ctx): not modelled",
    "EQUIP_GAME_DATA_ARM_STYLE_OFFSET": (
        "a `{ use ...; offset_of!(..) }` block expression; the grammar is deliberately"
        " statement-free, because a folder that ran blocks would be an interpreter"
    ),
    # Not actually offsets. Both are `static ... : AtomicUsize = AtomicUsize::new(0)` telemetry
    # counters that the inventory's name filter (`*OFFSET*`) sweeps up and its exclusion table does
    # not name. Listed rather than reclassified because widening `EXCLUSIONS` moves the population
    # floor that `attribute-field-offset-owners.py` ratchets on, and that is a separate change.
    "LOG_EPOCH_OFFSET_MS": "AtomicUsize::new(0) -- a telemetry counter, not a field offset",
    "LOG_EPOCH_OFFSET_LOGGED": "AtomicUsize::new(0) -- a telemetry counter, not a field offset",
}


def load_drift():
    spec = importlib.util.spec_from_file_location(
        "detect_struct_field_drift", ROOT / "scripts" / "detect-struct-field-drift.py"
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def address_population(repo: Path, constants: const_fold.Constants) -> dict[str, const_fold.Decl]:
    """The constants the 1.17 address harvest will look at: `*RVA*`-named, or handed to a resolver.

    Mirrors `select-needed-1170-rows.py::declared_rvas` deliberately -- same name filter, same
    `BOUND` and `NOT_AN_ADDRESS` choke points, same `#[cfg(test)]` blindness -- so that "this gate
    is green" means "that harvester can see a value for everything it will consider".
    """
    used = rva_usage.workspace_usage(repo)
    out: dict[str, const_fold.Decl] = {}
    for name, decls in constants.decls.items():
        if BOUND.search(name) or name in rva_role.NOT_AN_ADDRESS:
            continue
        if "RVA" not in name and name not in used:
            continue
        live = [d for d in decls if not d.cfg_test]
        if live:
            out[name] = live[0]
    return out


def unresolved(repo: Path) -> tuple[list[tuple[str, str, str]], dict[str, str], dict[str, str], int]:
    """`(failures, resolvable_names, pin_valued_names, population)`.

    `failures` is one row per constant with no value: `(name, site, why)`. `resolvable_names` is
    every gated constant that did evaluate, which is what makes the reverse direction possible --
    a name in `UNRESOLVABLE` that appears there is a stale entry.

    `pin_valued_names` is the third state, and it is returned rather than dropped because dropping
    it is what this gate went red on, 2026-08-31. A row the field-offset inventory resolves from a
    `const _: () = assert!(NAME == 0xNN)` pin, or from hex read out of the constant's own name, is
    deliberately not `resolvable` (see the comment at the branch below) -- but it is not a
    `failure` either, because a number is known. Returning only two of the three buckets meant
    every consumer had to infer the third from an absence, and `report()` inferred it as "the
    constant left the population": five live `CHR_ASM_*` offsets grew disassembly-derived pins and
    the gate demanded their (correct, still-accurate) `UNRESOLVABLE` entries be deleted. Deleting
    them would have removed documented exceptions for live constants and shrunk what the gate
    watches, which is the exact failure the module docstring warns about, pointed the other way.
    """
    constants = const_fold.Constants.scan(repo)
    failures: list[tuple[str, str, str]] = []
    resolvable: dict[str, str] = {}
    pin_valued: dict[str, str] = {}
    population = 0

    for name, decl in sorted(address_population(repo, constants).items()):
        population += 1
        folded = constants.fold(decl.init, scope=decl.file)
        if folded.value is None:
            failures.append((name, decl.where(), folded.reason))
        else:
            resolvable[name] = f"{folded.value:#x}"

    drift = load_drift()
    for row in drift.inventory():
        if not row["included"]:
            continue
        population += 1
        site = f"{row['file']}:{row['line']}"
        if row["resolved"] is None:
            why = row["unresolved"] or "offset_of! on a type whose layout is not modelled"
            failures.append((row["name"], site, why))
        elif "name-hint" not in row["kind"] and "pinned" not in row["kind"]:
            resolvable[row["name"]] = f"{row['resolved']:#x}"
        else:
            # A name-hint or a pin is not an evaluation -- one reads the constant's own name and
            # the other reads a hand-written assertion. Counting them as resolvable would let a
            # listed exception look fixed because somebody renamed it. They are named here instead
            # of vanishing, so that "in the population" stays answerable without an inference.
            pin_valued[row["name"]] = f"{row['resolved']:#x} ({row['kind']}) {site}"
    return failures, resolvable, pin_valued, population


def gated_names(
    failures: list[tuple[str, str, str]], resolvable: dict[str, str], pin_valued: dict[str, str]
) -> set[str]:
    """Every constant the gate looked at, whatever came of it -- the three buckets are the whole."""
    return {name for name, _site, _why in failures} | set(resolvable) | set(pin_valued)


def read_floor(path: Path | None = None) -> tuple[set[str], str | None]:
    """The coverage floor, or `(empty, why)` if it cannot be read.

    Missing is a problem, not a skip: a departure check that quietly does not run is the same
    invisibility the rest of this gate exists to end.
    """
    path = path or FLOOR
    shown = path.name if path.is_absolute() and ROOT not in path.parents else path
    if not path.exists():
        return set(), f"{shown} is missing, so no departure is being detected"
    names = {
        line.strip()
        for line in path.read_text(encoding="utf-8").splitlines()
        if line.strip() and not line.startswith("#")
    }
    if not names:
        return set(), f"{shown} is empty, so no departure is being detected"
    return names, None


def report(repo: Path, verbose: bool = True, floor_path: Path | None = None) -> list[str]:
    failures, resolvable, pin_valued, population = unresolved(repo)
    problems: list[str] = []
    if population < 500:
        problems.append(
            f"only {population} constants were gated; the scan is not seeing the tree "
            "(a green run over an empty population proves nothing)"
        )
    for name, site, why in failures:
        if name in UNRESOLVABLE:
            continue
        problems.append(f"{name} ({site}) has no value and is not in UNRESOLVABLE: {why}")
    for name, why in sorted(UNRESOLVABLE.items()):
        if name in resolvable:
            problems.append(
                f"UNRESOLVABLE lists {name} ({why}) but it now evaluates to {resolvable[name]} -- "
                "a stale exception excuses a number nothing is checking; delete the entry"
            )
    present = gated_names(failures, resolvable, pin_valued)
    for name in sorted(set(UNRESOLVABLE) - present):
        problems.append(
            f"UNRESOLVABLE lists {name}, which is no longer in either gated population -- "
            "the entry describes nothing; delete it"
        )
    floor, floor_problem = read_floor(floor_path)
    if floor_problem:
        problems.append(floor_problem)
    for name in sorted(floor - present):
        problems.append(
            f"{name} was in the gated population and no longer is -- coverage LEFT silently. "
            f"If it was renamed, check the NEW name is gated too, then delete this one line from "
            f"{FLOOR.name} in the same commit and say why. (--refresh-floor rewrites the whole "
            f"file and must be run from a clean tree; the one-line edit is safe in a shared one.)"
        )
    if verbose:
        print(
            f"expression-valued constants gated: {population} declarations across the address and "
            f"field-offset populations"
        )
        print(f"  evaluate to a number         : {len(resolvable)}")
        print(f"  value from a pin or name-hint: {len(pin_valued)} (a number, but not an "
              "evaluation -- see unresolved())")
        print(f"  cannot be evaluated          : {len(failures)} "
              f"({len(UNRESOLVABLE)} listed in UNRESOLVABLE)")
        print(f"  coverage floor               : {len(floor)} names must stay gated")
    return problems


def refresh_floor(repo: Path) -> int:
    """Re-record the floor from the tree as it stands, naming every name it drops."""
    failures, resolvable, pin_valued, _population = unresolved(repo)
    present = gated_names(failures, resolvable, pin_valued)
    previous, _why = read_floor()
    for name in sorted(previous - present):
        print(f"  dropping {name} -- it is no longer in either gated population")
    for name in sorted(present - previous):
        print(f"  adding   {name}")
    FLOOR.write_text(FLOOR_HEADER + "".join(f"{name}\n" for name in sorted(present)), "utf-8")
    print(f"{FLOOR.relative_to(ROOT)}: {len(present)} names recorded")
    return 0


# ------------------------------------------------------------------------------------------------
# selftest: mutants that must go red, including a blinding of the folder itself
# ------------------------------------------------------------------------------------------------
FIXTURE_UNFOLDABLE = """
pub const PLANTED_MUTANT_RVA: usize = some_extern_crate::TABLE.lookup();
"""
# Deliberately hex-literal arithmetic, i.e. the exact shape the address harvester used to half-read.
FIXTURE_FOLDABLE = """
pub const PLANTED_CONTROL_RVA: usize = 0x142658c60 - 0x140000000;
"""
# The third state, planted: an `offset_of!` on a type the layout modeller does not carry, whose only
# number comes from a `const _: () = assert!` pin. The field-offset inventory files it
# `offset_of(pinned)`, which is neither an evaluation nor a failure -- the shape that took this gate
# red on 2026-08-31.
FIXTURE_PINNED = """
pub const PLANTED_PIN_MUTANT_OFFSET: usize = core::mem::offset_of!(NotModelled, member);
const _: () = assert!(PLANTED_PIN_MUTANT_OFFSET == 0x40);
"""


def _sweep_orphaned_mutants(where: "Path") -> None:
    """Delete `_expr_mutant_<pid>.rs` files whose planting process is gone.

    The mutants below are planted into a real crate under try/finally, which covers an
    exception but not a kill: SIGTERM is not catchable by default, so `timeout 28 python3
    scripts/check-expression-constants.py --selftest` -- this gate's selftest is 9.5s and the
    vacuity auditor runs it twice, so a 30s-capped agent shell hits that -- leaves the mutant
    behind. Measured 2026-08-31: one orphan wedged both halves of this gate red for every agent
    in the shared tree, reporting `PLANTED_MUTANT_RVA ... has no value`, which reads as a real
    finding about somebody's uncommitted work rather than as this tool's own litter.

    The PID is in the filename precisely so a concurrent selftest's live mutant is not swept:
    only a file whose owner no longer exists is removed.
    """
    for stale in where.glob("_expr_mutant_*.rs"):
        try:
            pid = int(stale.stem.rsplit("_", 1)[1])
        except (IndexError, ValueError):
            continue
        if pid == os.getpid():
            continue
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            stale.unlink(missing_ok=True)
        except OSError:
            pass  # alive but not ours to signal -- leave it


def selftest(repo: Path) -> int:
    failures: list[str] = []
    constants = const_fold.Constants.scan(repo)

    # --- the evaluator's own controls, on frozen source, so a green gate means it can still read
    for source, name, expect in (
        ("const A_RVA: usize = 0x142658c60 - 0x140000000;", "A_RVA", 0x2658C60),
        ("const B_RVA: usize = 0x10 + 0x20 * 0x3;", "B_RVA", 0x70),
        ("const C_RVA: usize = (0x10 + 0x20) * 0x3;", "C_RVA", 0x90),
        ("const D_RVA: u64 = 1u64 << 32;", "D_RVA", 1 << 32),
        ("const E_RVA: usize = usize::MAX - 0xf;", "E_RVA", (1 << 64) - 16),
    ):
        init = source.split("=", 1)[1].strip(" ;")
        got = constants.fold(init)
        if got.value != expect:
            failures.append(f"{name}: folded {got.value} != {expect} ({got.reason})")
    # Precedence is the one thing a hand-rolled PARSER gets wrong silently, and getting it wrong
    # produces a plausible address rather than an error, so both associativity cases are pinned.
    if constants.fold("0x100 - 0x10 - 0x1").value != 0xEF:
        failures.append("subtraction is not left-associative")
    if constants.fold("0x2 + 0x3 * 0x4").value != 0xE:
        failures.append("`*` does not bind tighter than `+`")
    # ... and a refusal must stay a refusal: these are the shapes that must never be guessed.
    for init, expect_in in (
        ("core::mem::offset_of!(Foo, bar)", "offset_of"),
        ("{ let x = 1; x }", "block"),
        ("SOME_NAME_THAT_DOES_NOT_EXIST_RVA", "not declared"),
        ("core::mem::size_of::<NotAPrimitive>()", "not a primitive"),
    ):
        got = constants.fold(init)
        if got.value is not None or expect_in not in got.reason:
            failures.append(f"{init!r} should refuse with {expect_in!r}, got {got}")
    # `#[cfg(test)]` must stay invisible. `FREELIST_SHUTDOWN_ASSERT_RVA` is a sum whose value is
    # 0x90 bytes inside a live function; its doc comment says it is spelled that way so no scanner
    # selects it. Folding sums without honouring the attribute would turn that into a detour
    # licence -- the folder making things worse than the regex it replaced.
    guarded = constants.resolve("FREELIST_SHUTDOWN_ASSERT_RVA")
    if guarded.value is not None or "cfg(test)" not in guarded.reason:
        failures.append(
            f"a #[cfg(test)] declaration became visible: FREELIST_SHUTDOWN_ASSERT_RVA -> {guarded}"
        )
    # The address the whole exercise is about must resolve, and to the RVA, not the VA.
    real = constants.resolve("ADD_DEFAULT_FILE_LOAD_PROCESS_RVA")
    if real.value != 0x2658C60:
        failures.append(
            f"ADD_DEFAULT_FILE_LOAD_PROCESS_RVA folded to {real} -- expected 0x2658c60; "
            "0x142658c60 means the first-literal read is back"
        )

    # --- the gate must be green on the tree as it stands, or every mutant below proves nothing
    standing = report(repo, verbose=False)
    if standing:
        failures += [f"the unmutated gate is not green: {p}" for p in standing[:3]]

    # --- Mutant a: plant an unfoldable expression constant that is not listed -> red
    planted = ROOT / "crates" / "er-game-base" / "src" / f"_expr_mutant_{os.getpid()}.rs"
    try:
        planted.write_text(FIXTURE_UNFOLDABLE, encoding="utf-8")
        problems = report(repo, verbose=False)
        if not any("PLANTED_MUTANT_RVA" in p for p in problems):
            failures.append("mutant A: an unfoldable, unlisted constant did not fail the gate")
    finally:
        planted.unlink(missing_ok=True)

    # --- Mutant B: a stale exception -- list a constant that does fold -> red
    try:
        planted.write_text(FIXTURE_FOLDABLE, encoding="utf-8")
        UNRESOLVABLE["PLANTED_CONTROL_RVA"] = "planted"
        problems = report(repo, verbose=False)
        if not any("PLANTED_CONTROL_RVA" in p and "now evaluates" in p for p in problems):
            failures.append("mutant B: a stale UNRESOLVABLE entry was not reported")
    finally:
        UNRESOLVABLE.pop("PLANTED_CONTROL_RVA", None)
        planted.unlink(missing_ok=True)

    # --- Mutant C: an entry describing nothing at all -> red
    UNRESOLVABLE["NO_SUCH_CONSTANT_ANYWHERE_OFFSET"] = "planted"
    try:
        problems = report(repo, verbose=False)
        if not any("NO_SUCH_CONSTANT_ANYWHERE_OFFSET" in p for p in problems):
            failures.append("mutant C: an exception for a constant that does not exist was accepted")
    finally:
        UNRESOLVABLE.pop("NO_SUCH_CONSTANT_ANYWHERE_OFFSET", None)

    # --- Mutant E: The third state must not read as a departure. A constant the field-offset
    # inventory can only value from a pin is deliberately not `resolvable`, and it is not a
    # `failure` either; inferring "it left the population" from that double absence is what made
    # this gate demand the deletion of five accurate `CHR_ASM_*` exceptions on 2026-08-31, the day
    # somebody pinned those offsets against the ctor disassembly.
    try:
        planted.write_text(FIXTURE_PINNED, encoding="utf-8")
        UNRESOLVABLE["PLANTED_PIN_MUTANT_OFFSET"] = "planted"
        problems = report(repo, verbose=False)
        for problem in problems:
            if "PLANTED_PIN_MUTANT_OFFSET" not in problem:
                continue
            failures.append(
                f"mutant E: a listed constant whose only value is a pin was reported -- {problem}"
            )
        _f, _r, pin_valued, _n = unresolved(repo)
        if "PLANTED_PIN_MUTANT_OFFSET" not in pin_valued:
            failures.append(
                "mutant E: the planted pin-valued constant was not in the third bucket at all, so "
                "the mutant proves nothing -- the field-offset inventory did not pick the file up"
            )
    finally:
        UNRESOLVABLE.pop("PLANTED_PIN_MUTANT_OFFSET", None)
        planted.unlink(missing_ok=True)

    # --- Mutant F: A departure from the coverage floor -> red, and a name still gated -> not.
    # `tempfile` rather than a planted file: a fixture that lands in the repo becomes another
    # gate's finding when this process is killed, which is exactly what `_sweep_orphaned_mutants`
    # exists to undo.
    with tempfile.TemporaryDirectory(prefix="expr-const-floor-") as scratch:
        fabricated = Path(scratch) / "floor.txt"
        fabricated.write_text(
            "# fabricated\nA_CONSTANT_THAT_LEFT_THE_POPULATION_RVA\n"
            "ADD_DEFAULT_FILE_LOAD_PROCESS_RVA\n",
            encoding="utf-8",
        )
        problems = report(repo, verbose=False, floor_path=fabricated)
        if not any(
            "A_CONSTANT_THAT_LEFT_THE_POPULATION_RVA" in p and "coverage LEFT silently" in p
            for p in problems
        ):
            failures.append(
                "mutant F: a floor name that is no longer gated was not reported -- constants can "
                "leave the population in silence again"
            )
        if any("ADD_DEFAULT_FILE_LOAD_PROCESS_RVA" in p for p in problems):
            failures.append(
                "mutant F: a floor name that IS still gated was reported as departed; the "
                "departure check is firing on presence"
            )
        _names, why = read_floor(Path(scratch) / "there-is-no-such-floor.txt")
        if not why:
            failures.append(
                "mutant F: a MISSING floor read as clean -- a departure check that does not run "
                "must be a problem, not a skip"
            )

    # --- Mutant D: Blind the folder. This is the non-vacuity proof: with the evaluator refusing
    # everything, the gate must go red on constants it currently passes -- if it stays green, it is
    # not the folding that makes it green.
    keep = const_fold._Eval.run
    try:
        const_fold._Eval.run = lambda self, source: (_ for _ in ()).throw(
            const_fold.Unfoldable("blinded")
        )
        blinded = report(repo, verbose=False)
        if not any("ADD_DEFAULT_FILE_LOAD_PROCESS_RVA" in p for p in blinded):
            failures.append(
                "mutant D: blinding the evaluator left the gate green -- the positive control "
                "passes without any folding happening, so this gate proves nothing"
            )
    finally:
        const_fold._Eval.run = keep

    for problem in failures:
        print(f"selftest FAIL {problem}")
    print(f"[check-expression-constants] selftest: {len(failures)} failure(s)")
    return 1 if failures else 0


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("--repo", type=Path, default=ROOT)
    ap.add_argument("--selftest", action="store_true")
    ap.add_argument("--list", action="store_true", help="print every constant with no value")
    ap.add_argument(
        "--refresh-floor",
        action="store_true",
        help="re-record the coverage floor from this tree (run it on a CLEAN tree)",
    )
    args = ap.parse_args()
    # Before either half reads the tree. The live gate never enters selftest(), and an orphan
    # makes it red too -- naming a constant that is this tool's own litter as a finding about
    # somebody's crate.
    _sweep_orphaned_mutants(ROOT / "crates" / "er-game-base" / "src")
    if args.selftest:
        return selftest(args.repo)
    if args.refresh_floor:
        return refresh_floor(args.repo)
    problems = report(args.repo)
    if args.list:
        failures, _resolvable, pin_valued, _n = unresolved(args.repo)
        for name, site, why in sorted(failures):
            mark = "listed" if name in UNRESOLVABLE else "UNLISTED"
            print(f"  [{mark}] {name} ({site}): {why}")
        for name, where in sorted(pin_valued.items()):
            mark = "listed" if name in UNRESOLVABLE else "pin  "
            print(f"  [{mark}] {name} = {where}")
    for problem in problems:
        print(f"check-expression-constants: {problem}")
    if problems:
        print(f"check-expression-constants: FAIL -- {len(problems)} problem(s)")
        return 1
    print("check-expression-constants: OK -- every gated constant has a value or a listed reason.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
