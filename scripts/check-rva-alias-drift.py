#!/usr/bin/env python3
"""Guard against re-introducing duplicate literal declarations of one game address.

An RVA *is* a game function's identity. When the same address is written out under several
names -- or the same name is written out independently in several crates -- a 1.16.x address
correction has to be found in every one of them, and CLAUDE.md records that missing one
crash-hooks (`armament-icons-cachemiss-hooks-crash-1162-address-drift`).

Worse than the maintenance cost: divergent names are divergent CLAIMS about what the address
is, and at least one of them is then a wrong reverse-engineering fact shipping in the DLL.
Three were found this way on 2026-08-01 (bd
`rva-67b750-is-save-write-not-continue-load-2026-08-01`,
`rva-4852f88-is-saveload2-slsystemimpl-not-fd4-io-worker-2026-08-01`).

The fix for a real duplicate is to declare the value ONCE -- in `er-game-base/src/rva.rs` for
cross-cutting singletons -- and derive every other name from it, so the aliases stay but the
value does not.

This follows the `check-oracle-writers.py` shape: an allowlist pins the CURRENT known
duplicates so the cleanup can proceed incrementally, and the gate fails only on NEW ones.

Two traps this encodes, both of which produced wrong answers when the audit was done by hand:

1. Scanning only `const NAME: usize = 0x...` misses `u32`-typed address constants. That
   undercounted the real duplicate set by ~35% and hid `MOUNT_GUARD_STATE_ROOT_RVA`, which
   turned out to be GameDataMan. All integer widths are scanned here.
2. Grouping by NAME finds nothing. The duplication has two shapes -- different names for one
   address, and one name re-declared independently in several crates. Grouping by VALUE is
   the only thing that catches both.

Usage:
    python3 scripts/check-rva-alias-drift.py            # gate
    python3 scripts/check-rva-alias-drift.py --selftest # prove the gate detects what it claims
    python3 scripts/check-rva-alias-drift.py --list     # show every duplicate group
"""

from __future__ import annotations

import argparse
import re
import sys
from collections import defaultdict
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
ALLOWLIST = REPO_ROOT / "scripts" / "rva-alias-allowlist.txt"

# Plausible RVA window for a ~120 MiB PE at image base 0x140000000. Values outside it are
# sizes, limits, magics and bitmasks -- 0x280000 (save slot body length), 0x3000000 /
# 0x4000000 (span fallbacks) and 0x7230203 (SPIR-V magic) all collided numerically in the
# first hand scan and were NOT addresses.
RVA_MIN = 0x100000
RVA_MAX = 0x8000000

DECL = re.compile(
    r"^\s*(?:pub(?:\([^)]*\))?\s+)?const\s+([A-Z][A-Z0-9_]*)\s*:\s*"
    r"(?:usize|u32|u64|i32|i64)\s*=\s*(0x[0-9a-fA-F_]+)\s*(?:as\s+\w+\s*)?;"
)


def scan(root: Path) -> dict[int, list[tuple[str, str, int]]]:
    """value -> [(name, repo-relative path, line)] for every LITERAL address declaration."""
    groups: dict[int, list[tuple[str, str, int]]] = defaultdict(list)
    for path in sorted((root / "crates").rglob("*.rs")):
        if "target" in path.parts:
            continue
        rel = path.relative_to(root).as_posix()
        with path.open("r", encoding="utf-8", errors="replace") as handle:
            for lineno, line in enumerate(handle, 1):
                match = DECL.match(line)
                if not match:
                    continue
                value = int(match.group(2).replace("_", ""), 16)
                if RVA_MIN <= value < RVA_MAX:
                    groups[value].append((match.group(1), rel, lineno))
    return groups


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
        # The whole point of the fix: a derived alias leaves ONE literal, so the group
        # collapses and nothing is reported.
        ("derived declaration is NOT a duplicate", 0, {
            "crates/a/src/lib.rs": "const FOO_RVA: usize = 0x3d5df38;\n",
            "crates/b/src/lib.rs": "const BAR_RVA: usize = er_game_base::rva::FOO_RVA;\n",
        }),
        # 0x280000 (the save-slot body length) sits INSIDE the RVA window, so a range filter
        # cannot tell it from a real address. Non-address collisions are expected to surface
        # and must be retired via the allowlist, not by narrowing the range -- narrowing it
        # would start hiding real addresses.
        ("in-range non-address values still surface (allowlist handles them)", 2, {
            "crates/a/src/lib.rs": "const A_LEN: usize = 0x280000;\n",
            "crates/b/src/lib.rs": "const B_LEN: usize = 0x280000;\n",
        }),
        # Genuinely out of the window (small bitmask below RVA_MIN, huge value above RVA_MAX).
        # Note how few numbers this actually excludes: 0x280000 and even the SPIR-V magic
        # 0x7230203 both land INSIDE it. The range is a coarse prefilter, not the mechanism.
        ("out-of-range values are ignored", 0, {
            "crates/a/src/lib.rs": "const A_MASK: usize = 0x7;\nconst A_BIG: usize = 0x9000000;\n",
            "crates/b/src/lib.rs": "const B_MASK: usize = 0x7;\nconst B_BIG: usize = 0x9000000;\n",
        }),
    ]
    failures = []
    for label, expected, files in cases:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for rel, body in files.items():
                target = root / rel
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_text(body, encoding="utf-8")
            found = scan(root)
            total = sum(len(v) for v in duplicates(found).values())
            if total != expected:
                failures.append(f"{label}: expected {expected} duplicate sites, got {total}")
    for failure in failures:
        print(f"selftest FAILED: {failure}", file=sys.stderr)
    if failures:
        return 1
    print(f"[check-rva-alias-drift] selftest ok ({len(cases)} cases)")
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

    dupes = duplicates(scan(REPO_ROOT))

    if args.write_allowlist:
        lines = [
            "# Addresses with more than one LITERAL declaration, pinned as the baseline so",
            "# check-rva-alias-drift.py fails only on NEW drift. Shrink this list; do not grow it.",
            "# Each entry is a value whose aliases still need adjudicating against the Ghidra",
            "# dump -- see docs/plans/experiments-module-split-audit.md section 2.",
        ]
        for value in sorted(dupes):
            names = sorted({n for n, _, _ in dupes[value]})
            lines.append(f"0x{value:x}  # {', '.join(names)}")
        ALLOWLIST.write_text("\n".join(lines) + "\n", encoding="utf-8")
        print(f"[check-rva-alias-drift] pinned {len(dupes)} groups into {ALLOWLIST.name}")
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

    if not new:
        return 0

    print("\nNEW duplicate address declarations. Declare the value ONCE (er-game-base/src/rva.rs"
          "\nfor cross-cutting singletons) and derive the other names from it:\n", file=sys.stderr)
    for value in sorted(new):
        print(f"  0x{value:x}", file=sys.stderr)
        for name, path, line in sorted(new[value]):
            print(f"      {name:46s} {path}:{line}", file=sys.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
