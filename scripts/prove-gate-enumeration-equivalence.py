#!/usr/bin/env python3
"""Did editing a gate's file enumeration change which files it reads?

Refactoring how a gate finds its input is the one change that can silently narrow a gate to
nothing while every selftest still passes. This answers the question directly: run the enumerator
from `HEAD` and the enumerator from the working tree, and compare byte-identical sorted lists
plus their sha256.

Why a/B/A, INTERLEAVED in one process
-------------------------------------
Several agents work in this tree at once. Measured 2026-08-31: a naive old-then-new comparison
came back "different" because a sibling agent added a `.rs` file between the two passes -- churn
in the input reading as a delta in the logic. Running old, new, old in one process makes that
churn visible as an unstable old (A1 != A2) instead of a false verdict. The same run also
answered a timing question honestly: a separate-process measurement of the same script had been
contaminated by this tool's own background job and read 5.15s for a 1.6s script.

Usage
    python3 scripts/prove-gate-enumeration-equivalence.py
    python3 scripts/prove-gate-enumeration-equivalence.py --gate check-no-lossy-utf8
    python3 scripts/prove-gate-enumeration-equivalence.py --base <rev>     # default head

Exit 0 = every gate's list is unchanged, or its change is exactly the one declared in
`EXPECTED_DELTAS`. Exit non-zero = a coverage change nobody declared, or an unstable baseline.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import subprocess
import sys
import time
import types
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]

# `git show` of one blob. A module constant rather than an inline literal only because that is the
# form scripts/check-no-timeouts.py can read; the bound obeys its 30s cap on every non-game op.
GIT_TIMEOUT_SECONDS = 20.0

# `(gate stem, enumerator attribute, positional args)`. A gate whose enumerator takes a root gets
# it explicitly; one that closes over `REPO_ROOT` takes none.
GATES: tuple[tuple[str, str, tuple], ...] = (
    ("check-no-lossy-utf8", "rust_source_files", ()),
    ("check-no-unguarded-cstr-from-ptr", "rust_source_files", ()),
    ("check-rust-file-sizes", "rust_files", (REPO_ROOT,)),
    ("check-fresh-run-logs", "rust_files", (REPO_ROOT,)),
    ("check-prologue-bytes", "rust_files", (REPO_ROOT,)),
    # Not a bare enumerator: this one's file selection lives inside `repo_spec_files`, which
    # returns `{path: spec_count}`. Iterating it yields the paths, which is what is compared.
    ("verify-prologue-coverage-1170", "repo_spec_files", ()),
)

# A declared coverage change: `{gate: (top-level segments the base enumerated and the working
# tree no longer does, reason)}`. Anything not listed here must come back byte-identical.
EXPECTED_DELTAS: dict[str, tuple[tuple[str, ...], str]] = {
    "check-no-lossy-utf8": (
        (".claude",),
        "2026-08-31: this gate was the only one of six whose ignore list omitted `.claude`, so it "
        "read 14,284 `.rs` copies out of other agents' worktrees (96.2% of its 14,855 files) and a "
        "stray from_utf8_lossy in any sibling sandbox failed this repo's gate.",
    ),
}


def _load_from_source(source: str, origin: str, module_name: str) -> types.ModuleType:
    module = types.ModuleType(module_name)
    # The gates derive their repo root from `__file__`; point it at the real path so a module
    # read out of git still resolves the same root the working-tree copy does.
    module.__file__ = origin
    # Registered before exec: `@dataclass` resolves `cls.__module__` through `sys.modules`, and on
    # 3.14 an unregistered module makes the decorator raise while processing the class.
    sys.modules[module_name] = module
    try:
        exec(compile(source, origin, "exec"), module.__dict__)
    finally:
        sys.modules.pop(module_name, None)
    return module


def load_base(stem: str, base: str) -> types.ModuleType:
    source = subprocess.run(
        ["git", "-C", str(REPO_ROOT), "show", f"{base}:scripts/{stem}.py"],
        capture_output=True,
        text=True,
        check=True,
        timeout=GIT_TIMEOUT_SECONDS,
    ).stdout
    return _load_from_source(
        source, str(REPO_ROOT / "scripts" / f"{stem}.py"), f"base_{stem.replace('-', '_')}"
    )


def load_worktree(stem: str) -> types.ModuleType:
    path = REPO_ROOT / "scripts" / f"{stem}.py"
    name = f"tree_{stem.replace('-', '_')}"
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    # Same `@dataclass`/`sys.modules` requirement as `_load_from_source`.
    sys.modules[name] = module
    try:
        spec.loader.exec_module(module)
    finally:
        sys.modules.pop(name, None)
    return module


def listing(module: types.ModuleType, attribute: str, args: tuple):
    started = time.perf_counter()
    paths = getattr(module, attribute)(*args)
    elapsed = time.perf_counter() - started
    relative = sorted(str(Path(p).resolve().relative_to(REPO_ROOT)) for p in paths)
    digest = hashlib.sha256("\n".join(relative).encode("utf-8")).hexdigest()[:16]
    return relative, digest, elapsed


def prove(stem: str, attribute: str, args: tuple, base: str) -> bool:
    base_module = load_base(stem, base)
    tree_module = load_worktree(stem)

    a1, sha_a1, t_a1 = listing(base_module, attribute, args)
    b, sha_b, t_b = listing(tree_module, attribute, args)
    a2, sha_a2, t_a2 = listing(base_module, attribute, args)

    print(f"\n=== {stem}")
    print(f"  {base:<10} n={len(a1):6d} sha={sha_a1}  {t_a1:7.3f}s   (A1)")
    print(f"  worktree   n={len(b):6d} sha={sha_b}  {t_b:7.3f}s   (B)")
    print(f"  {base:<10} n={len(a2):6d} sha={sha_a2}  {t_a2:7.3f}s   (A2)")

    if sha_a1 != sha_a2:
        moved = sorted(set(a1) ^ set(a2))[:5]
        print(f"  UNSTABLE BASELINE: A1 != A2, the tree moved mid-measurement ({moved}). Rerun.")
        return False

    if sha_a1 == sha_b:
        print("  BYTE-IDENTICAL sorted lists, matching sha256 -- coverage unchanged.")
        return True

    only_base = sorted(set(a1) - set(b))
    only_tree = sorted(set(b) - set(a1))
    segments = tuple(sorted({p.split("/")[0] for p in only_base}))
    print(f"  DELTA: base-only={len(only_base)}  worktree-only={len(only_tree)}")

    if only_tree:
        print(f"  UNDECLARED WIDENING: {only_tree[:5]}")
        return False

    expected = EXPECTED_DELTAS.get(stem)
    if expected is None:
        print(f"  UNDECLARED NARROWING under {segments} -- add it to EXPECTED_DELTAS or revert.")
        return False
    if segments != expected[0]:
        print(f"  NARROWING under {segments}, but only {expected[0]} was declared.")
        return False
    print(f"  DECLARED NARROWING, exactly {segments}: {expected[1]}")
    return True


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", default="HEAD", help="revision to compare against (default HEAD)")
    parser.add_argument("--gate", action="append", help="limit to these gate stems")
    arguments = parser.parse_args()

    selected = [g for g in GATES if not arguments.gate or g[0] in arguments.gate]
    if not selected:
        print(f"no gate matched {arguments.gate}; known: {[g[0] for g in GATES]}", file=sys.stderr)
        return 2

    failures = sum(0 if prove(stem, attr, args, arguments.base) else 1 for stem, attr, args in selected)
    print()
    if failures:
        print(f"{failures} gate(s) failed the enumeration-equivalence proof.", file=sys.stderr)
        return 1
    print(f"{len(selected)} gate(s): enumeration proven against {arguments.base}.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
