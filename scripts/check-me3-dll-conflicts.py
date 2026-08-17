#!/usr/bin/env python3
"""Prove `scripts/me3-dll-conflicts.toml` still describes every ME3-loadable DLL.

The conflict table is what stands between a generated profile and a run that is corrupt
before it starts. A table is only worth that if it cannot go quietly stale, and the way it
goes stale is not by being wrong -- it is by a new cdylib crate appearing and nobody
classifying it. The closure walk in `er-dll-closure.py` would then happily include that new
DLL next to the product with no idea whether the two can share a process.

So this gate asserts four things:

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

    for name, reason in compatible.items():
        if name not in shipped:
            failures.append(
                f"[compatible]: {name!r} is not a shipped cdylib (renamed or removed?)"
            )
        if not str(reason).strip():
            failures.append(f"[compatible]: {name!r} has an empty reason")

    # A package must not be classified twice: [compatible] asserts "no conflict at all",
    # which a [[conflict]] pair directly contradicts. No package is exempt -- including the
    # product, which is classified by appearing in its own conflict pairs.
    for name in sorted(conflicted & set(compatible)):
        failures.append(
            f"{name!r} appears in a [[conflict]] pair AND in [compatible] -- "
            f"[compatible] means no conflict at all, so these cannot both be true"
        )

    unclassified = shipped - conflicted - set(compatible)
    for name in sorted(unclassified):
        failures.append(
            f"{name!r} ships as an ME3-loadable DLL but is not classified. Add a "
            f"[[conflict]] pair, or list it in [compatible] with a reason."
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

    # `prod` is classified by appearing in the conflict pair, so it must NOT also be
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
    print(
        f"me3-dll-conflicts.toml: {len(table.get('conflict', []))} conflict pairs, "
        f"{len(table.get('compatible', {}))} compatible entries, "
        f"{len(shipped_packages())} shipped shells all classified"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
