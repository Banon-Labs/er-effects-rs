#!/usr/bin/env python3
"""Prove `scripts/me3-dll-conflicts.toml` still describes every ME3-loadable DLL.

The conflict table is what stands between a generated profile and a run that is corrupt
before it starts. A table is only worth that if it cannot go quietly stale, and the way it
goes stale is not by being wrong -- it is by a new cdylib crate appearing and nobody
classifying it. The closure walk in `er-dll-closure.py` would then happily include that new
DLL next to the product with no idea whether the two can share a process.

So this gate asserts five things:

1. **Coverage.** Every package in the `me3_shells` array (the single source of truth for
   "which cdylibs does this workspace ship", parsed via `me3-dll-list.py`) is classified --
   either it appears in at least one `[[conflict]]` pair, or it is listed in `[compatible]`.
2. **Reasons exist.** Both a `reason` and an `evidence` field on every conflict, and a
   non-empty reason for every compatible entry. "No conflict" must be a finding someone
   made, not a default a forgotten crate inherits.
3. **No dead entries.** Every package named anywhere in the table is still a shipped shell,
   so a rename or a crate-type change cannot leave the table pointing at nothing.
4. **No double classification.** A package in a `[[conflict]]` pair must not also claim to
   be `[compatible]`, which would let the closure walk read whichever it liked.
5. **`[[shared]]` rows are checkable.** A `[[shared]]` pair is the one thing that licenses two
   DLLs to detour one prologue and still share a profile, so it must carry the `target`, the
   `mechanism`, and both handler symbols -- those are what `check-shared-hook-rvas.py` uses to
   prove each detour reaches a union registrar and never an `MhHook::new`. A pair may not be
   declared shared and conflicting at once: the closure walk reads `[[conflict]]` only, and would
   co-load a pair it had been told to keep apart.

Usage:
    python3 scripts/check-me3-dll-conflicts.py
    python3 scripts/check-me3-dll-conflicts.py --selftest

Exit status is 1 on any failure, so this can gate.
"""

from __future__ import annotations

import argparse
import importlib.util
import sys
import tomllib
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
CONFLICTS_TOML = REPO_ROOT / "scripts" / "me3-dll-conflicts.toml"
DLL_LIST = REPO_ROOT / "scripts" / "me3-dll-list.py"

VALID_KINDS = {
    "hook-collision",
    "present-compositor",
    "drives-input",
    "diagnostic-drive",
    # Two DLLs statically linking the same feature crate. A linked crate's statics are per-DLL, so
    # each gets its own copy of that feature's state machine, its own worker threads and its own
    # game tasks -- all driving one piece of game state with no shared lock. Nothing is detoured,
    # so it looks harmless to every other check here; the damage is two owners of one mutation.
    "duplicate-owner",
    # A function-pointer slot the game later calls holds a value that is not a function entry.
    # Distinct from `hook-collision`, which corrupts code at a hooked prologue and presents as
    # silent inertness: this corrupts data and presents as a hard fault at a fixed address, with
    # `rcx == rip` at the fault because the call went through the pointer. Added 2026-09-02 for
    # er-quickload X er-invasion-warp rather than mislabel it `hook-collision`, which is what it
    # was first recorded as and what the register capture then falsified. Use this kind when the
    # pair is REPRODUCIBLE but the writer of the bad pointer is not yet identified -- the honest
    # label for "we know it dies and we know how, not why".
    "corrupt-callback-pointer",
}

# How a [[shared]] pair was made safe on the address they both detour. One value today, and it is
# spelled out rather than left free-text so a future "we looked at it and it seemed fine" cannot be
# written into the field that licenses two DLLs to hook one prologue.
VALID_MECHANISMS = {
    # Both handlers register through one MinHook instance -- the product's union, reached from a
    # companion image through the `er_effects_union_register` export -- and chain.
    "hook-union",
}


def shipped_packages() -> list[str]:
    """Every package in the `me3_shells` array, plus the product, via me3-dll-list.py."""
    spec = importlib.util.spec_from_file_location("me3_dll_list", DLL_LIST)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return [package for package, _artifact in module.dll_pairs()]


def load_table(path: Path = CONFLICTS_TOML) -> dict:
    with path.open("rb") as handle:
        return tomllib.load(handle)


def audit(table: dict, packages: list[str]) -> list[str]:
    """Return a list of human-readable failures; empty means the table is sound."""
    failures: list[str] = []
    shipped = set(packages)

    conflicts = table.get("conflict", [])
    compatible = table.get("compatible", {})
    opt_in_only = table.get("opt_in_only", {})

    conflicted: set[str] = set()
    for index, entry in enumerate(conflicts):
        where = f"[[conflict]] #{index + 1}"
        a, b = entry.get("a"), entry.get("b")
        for side, name in (("a", a), ("b", b)):
            if not name:
                failures.append(f"{where}: missing `{side}`")
            elif name not in shipped:
                failures.append(
                    f"{where}: `{side} = {name!r}` is not a shipped cdylib "
                    f"(renamed or removed? update the table)"
                )
        if a and b and a == b:
            failures.append(f"{where}: a package cannot conflict with itself ({a!r})")
        kind = entry.get("kind")
        if kind not in VALID_KINDS:
            failures.append(
                f"{where}: kind={kind!r} is not one of {sorted(VALID_KINDS)}"
            )
        if not (entry.get("reason") or "").strip():
            failures.append(f"{where}: empty `reason` -- say what actually breaks")
        if not (entry.get("evidence") or "").strip():
            failures.append(
                f"{where}: empty `evidence` -- cite the file:line or profile that proves it"
            )
        conflicted.update(name for name in (a, b) if name)

    # [[shared]]: two DLLs that do detour one prologue but were made co-loadable by routing both
    # handlers through a single MinHook instance (the hook union). It is a third answer alongside
    # conflict/compatible, and the loosest one, so its fields are mandatory: without `target` and
    # the two handler symbols, `check-shared-hook-rvas.py` cannot prove the mechanism and the row
    # decays into an unverifiable promise that two DLLs are safe together.
    for index, entry in enumerate(table.get("shared", [])):
        where = f"[[shared]] #{index + 1}"
        a, b = entry.get("a"), entry.get("b")
        for side, name in (("a", a), ("b", b)):
            if not name:
                failures.append(f"{where}: missing `{side}`")
            elif name not in shipped:
                failures.append(
                    f"{where}: `{side} = {name!r}` is not a shipped cdylib "
                    f"(renamed or removed? update the table)"
                )
            if not str(entry.get(f"handler_{side}") or "").strip():
                failures.append(
                    f"{where}: empty `handler_{side}` -- name the detour symbol, or nothing can "
                    f"prove it goes through the union rather than a private MinHook"
                )
        if a and b and a == b:
            failures.append(f"{where}: a package cannot share an address with itself ({a!r})")
        mechanism = entry.get("mechanism")
        if mechanism not in VALID_MECHANISMS:
            failures.append(
                f"{where}: mechanism={mechanism!r} is not one of {sorted(VALID_MECHANISMS)}"
            )
        for field in ("target", "reason", "evidence"):
            if not str(entry.get(field) or "").strip():
                failures.append(f"{where}: empty `{field}`")
        # Sharing an address safely is the opposite of conflicting on it, so a pair cannot claim
        # both -- the closure walk reads [[conflict]] and would co-load a pair it was told not to.
        if a and b and frozenset({a, b}) in {
            frozenset({c.get("a"), c.get("b")}) for c in conflicts
        }:
            failures.append(
                f"{where}: {a!r} and {b!r} are ALSO a [[conflict]] pair -- shared means co-loadable "
                f"and conflict means never co-loaded; pick one"
            )

    for name, reason in compatible.items():
        if name not in shipped:
            failures.append(
                f"[compatible]: {name!r} is not a shipped cdylib (renamed or removed?)"
            )
        if not str(reason).strip():
            failures.append(f"[compatible]: {name!r} has an empty reason")

    # [opt_in_only] is the third classification: co-loadable, but a dependency-closure walk must
    # never select it, because it changes what the player sees and being reachable is not consent.
    for name, reason in opt_in_only.items():
        if name not in shipped:
            failures.append(
                f"[opt_in_only]: {name!r} is not a shipped cdylib (renamed or removed?)"
            )
        if not str(reason).strip():
            failures.append(f"[opt_in_only]: {name!r} has an empty reason")

    # A package must not be classified twice: [compatible] asserts "no conflict at all",
    # which a [[conflict]] pair directly contradicts. No package is exempt -- including the
    # product, which is classified by appearing in its own conflict pairs.
    for name in sorted(conflicted & set(compatible)):
        failures.append(
            f"{name!r} appears in a [[conflict]] pair AND in [compatible] -- "
            f"[compatible] means no conflict at all, so these cannot both be true"
        )

    # ...and the same for the third bucket. [compatible] means "load it freely"; [opt_in_only]
    # means "never load it unless asked". A package in both leaves the closure free to pick the
    # reading it likes, which is precisely the ambiguity that let a mushroom mod into a user's run.
    for name in sorted(set(opt_in_only) & set(compatible)):
        failures.append(
            f"{name!r} is in [compatible] AND [opt_in_only] -- [compatible] lets the closure "
            f"load it freely, [opt_in_only] forbids that without --with; pick one"
        )

    unclassified = shipped - conflicted - set(compatible) - set(opt_in_only)
    for name in sorted(unclassified):
        failures.append(
            f"{name!r} ships as an ME3-loadable DLL but is not classified. Add a "
            f"[[conflict]] pair, or list it in [compatible] or [opt_in_only] with a reason."
        )

    return failures


def selftest() -> int:
    """Exercise the audit against synthetic tables, including the failure modes it exists to catch."""
    ok = True

    def check(condition: bool, label: str) -> None:
        nonlocal ok
        if not condition:
            ok = False
            print(f"  FAIL {label}")
        else:
            print(f"  ok   {label}")

    packages = ["prod", "safe", "bad"]

    def pair(a: str, b: str, **overrides) -> dict:
        entry = {
            "a": a,
            "b": b,
            "kind": "hook-collision",
            "reason": "same prologue",
            "evidence": "src/lib.rs:1",
        }
        entry.update(overrides)
        return entry

    # `prod` is classified by appearing in the conflict pair, so it must not also be
    # listed compatible -- exactly the shape the real table uses.
    sound = {"conflict": [pair("prod", "bad")], "compatible": {"safe": "installs no detour"}}
    check(audit(sound, packages) == [], "a sound table produces no failures")

    missing = {"conflict": [], "compatible": {"prod": "x", "safe": "y"}}
    check(
        any("'bad'" in f and "not classified" in f for f in audit(missing, packages)),
        "an unclassified shipped DLL is caught",
    )

    no_reason = {"conflict": [pair("prod", "bad", reason="  ")], "compatible": {"safe": "y"}}
    check(
        any("empty `reason`" in f for f in audit(no_reason, packages)),
        "a conflict with no reason is caught",
    )

    no_evidence = {"conflict": [pair("prod", "bad", evidence="")], "compatible": {"safe": "y"}}
    check(
        any("empty `evidence`" in f for f in audit(no_evidence, packages)),
        "a conflict with no evidence is caught",
    )

    dead = {
        "conflict": [pair("prod", "ghost")],
        "compatible": {"safe": "y", "bad": "z"},
    }
    check(
        any("'ghost'" in f and "not a shipped cdylib" in f for f in audit(dead, packages)),
        "a dead entry naming a removed crate is caught",
    )

    doubled = {
        "conflict": [pair("prod", "bad")],
        "compatible": {"safe": "y", "bad": "also fine?"},
    }
    check(
        any("appears in a [[conflict]] pair AND in [compatible]" in f for f in audit(doubled, packages)),
        "double classification is caught",
    )

    bad_kind = {"conflict": [pair("prod", "bad", kind="vibes")], "compatible": {"safe": "y"}}
    check(
        any("vibes" in f and "not one of" in f for f in audit(bad_kind, packages)),
        "an unknown failure-mode kind is caught",
    )

    self_pair = {"conflict": [pair("prod", "prod")], "compatible": {"safe": "y", "bad": "z"}}
    check(
        any("cannot conflict with itself" in f for f in audit(self_pair, packages)),
        "a package conflicting with itself is caught",
    )

    def shared(a: str, b: str, **overrides) -> dict:
        entry = {
            "a": a,
            "b": b,
            "mechanism": "hook-union",
            "target": "rva::SOME_RVA",
            "handler_a": "a_detour",
            "handler_b": "b_detour",
            "reason": "both chain through one MinHook instance",
            "evidence": "src/lib.rs:1",
        }
        entry.update(overrides)
        return entry

    # A [[shared]] pair is co-loadable, so both members still need their own classification --
    # here via [compatible], which is not contradicted by sharing an address safely.
    shared_sound = {
        "shared": [shared("prod", "bad")],
        "compatible": {"prod": "axis", "safe": "y", "bad": "unioned"},
    }
    check(audit(shared_sound, packages) == [], "a sound [[shared]] row produces no failures")

    for field in ("target", "reason", "evidence", "handler_a", "handler_b"):
        table = {
            "shared": [shared("prod", "bad", **{field: "  "})],
            "compatible": {"prod": "axis", "safe": "y", "bad": "unioned"},
        }
        check(
            any(f"`{field}`" in f for f in audit(table, packages)),
            f"a [[shared]] row missing `{field}` is caught",
        )

    bad_mechanism = {
        "shared": [shared("prod", "bad", mechanism="we checked")],
        "compatible": {"prod": "axis", "safe": "y", "bad": "unioned"},
    }
    check(
        any("we checked" in f and "not one of" in f for f in audit(bad_mechanism, packages)),
        "an unknown [[shared]] mechanism is caught",
    )

    both_ways = {
        "conflict": [pair("prod", "bad")],
        "shared": [shared("prod", "bad")],
        "compatible": {"safe": "y"},
    }
    check(
        any("pick one" in f for f in audit(both_ways, packages)),
        "a pair declared BOTH shared and conflicting is caught",
    )

    # --- [opt_in_only], the third classification -----------------------------------------
    opt_sound = {
        "conflict": [pair("prod", "bad")],
        "opt_in_only": {"safe": "wears a costume the player did not ask for"},
    }
    check(audit(opt_sound, packages) == [], "[opt_in_only] alone classifies a shipped DLL")

    opt_empty = {"conflict": [pair("prod", "bad")], "opt_in_only": {"safe": "   "}}
    check(
        any("[opt_in_only]" in f and "empty reason" in f for f in audit(opt_empty, packages)),
        "an [opt_in_only] entry with no reason is caught",
    )

    opt_unknown = {
        "conflict": [pair("prod", "bad")],
        "compatible": {"safe": "y"},
        "opt_in_only": {"ghost": "renamed away"},
    }
    check(
        any("[opt_in_only]" in f and "'ghost'" in f for f in audit(opt_unknown, packages)),
        "an [opt_in_only] entry naming a package that no longer ships is caught",
    )

    opt_doubled = {
        "conflict": [pair("prod", "bad")],
        "compatible": {"safe": "load me freely"},
        "opt_in_only": {"safe": "never load me unasked"},
    }
    check(
        any("[opt_in_only]" in f and "pick one" in f for f in audit(opt_doubled, packages)),
        "a package in BOTH [compatible] and [opt_in_only] is caught",
    )

    print("selftest:", "PASS" if ok else "FAIL")
    return 0 if ok else 1


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--selftest", action="store_true", help="run the audit against synthetic tables and exit"
    )
    args = parser.parse_args()

    if args.selftest:
        return selftest()

    if not CONFLICTS_TOML.is_file():
        print(f"missing conflict table: {CONFLICTS_TOML}", file=sys.stderr)
        return 1

    failures = audit(load_table(), shipped_packages())
    if failures:
        print(f"{CONFLICTS_TOML.relative_to(REPO_ROOT)} is not sound:", file=sys.stderr)
        for failure in failures:
            print(f"  - {failure}", file=sys.stderr)
        return 1

    table = load_table()
    shared_count = len(table.get("shared", []))
    print(
        f"me3-dll-conflicts.toml: {len(table.get('conflict', []))} conflict pairs, "
        f"{shared_count} shared-address pair{'' if shared_count == 1 else 's'}, "
        f"{len(table.get('compatible', {}))} compatible entries, "
        f"{len(table.get('opt_in_only', {}))} opt-in-only entries, "
        f"{len(shipped_packages())} shipped shells all classified"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
