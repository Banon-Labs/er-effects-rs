#!/usr/bin/env python3
"""Refuse an AOB signature that two crates hook without allowing for each other's detour.

# The failure this exists to prevent

`er-armament-icons` and `er-invasion-warp` both detour the GFx tag-parse function, and both
located it with the byte-identical 30-byte prologue, arrived at independently. me3 loads natives
in name order, so one of them installs first and its five-byte MinHook detour replaces the first
five bytes of that prologue with `E9 <rel32>`. The second one then scans a live `.text` where the
pattern it is looking for no longer exists, finds nothing, and switches itself off.

The symptom is silent and it does not look like a hook problem: on 2026-09-10 it presented as an
announcement banner that would not centre, and the refusal line said "absent or not unique", which
covers two opposite diagnoses and named neither.

So an AOB scan for a pristine prologue is not version-fragile, it is load-order-fragile, and a
second crate scanning for a function another one hooks will always lose. The fix is to match past
the bytes a detour can overwrite and subtract that width, which makes the scan indifferent to who
arrived first. This gate refuses a shared signature that has not done that.

# The one exemption, and why it is not a hole

The race needs both DLLs in one process. A pair that `scripts/me3-dll-conflicts.toml` records as
`[[conflict]]` never gets there: `scripts/er-dll-closure.py` reads that table and refuses to emit a
profile holding both, and `scripts/check-me3-dll-conflicts.py` proves the table classifies every
shell the workspace ships. So a signature shared only across declared-conflicting crates is
exempt. The exemption is mechanism-backed rather than asserted, and it is narrow: with three or
more crates on one signature, every pair has to be declared, or the group is refused as before.

It exists for `er-quickload` and `er-quit-rows`, which are the same source twice -- the product and
the copy being reduced to the System>Quit rows (bd er-effects-rs-ejfl). Their shared literals are
the anti-anti-debug patterns, identical because one file was copied from the other. Declaring
`PARSE_SIG_PATCH_BYTES` in both would be a false claim that the scan matches past a detour width;
those two DLLs simply cannot be loaded together, which is already recorded and already enforced.
"""

from __future__ import annotations

import itertools
import re
import sys
import tomllib
from collections import defaultdict
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]

# A byte-pattern literal: at least this many space-separated hex bytes or `??` wildcards. Ten is
# well past what an ordinary string happens to look like and well under any real signature.
MIN_TOKENS = 10
AOB = re.compile(r'"((?:(?:[0-9A-Fa-f]{2}|\?\?)\s+){%d,}(?:[0-9A-Fa-f]{2}|\?\?))"' % (MIN_TOKENS - 1))

# The marker a crate declares when its scan skips the bytes a detour overwrites. Named rather than
# inferred: the point is that a human wrote down how wide the patch is.
PATCH_WIDTH_MARKER = "PARSE_SIG_PATCH_BYTES"

# The table that says which shells may never share a profile. Read relative to the scanned root so
# the selftest can hand this gate its own fixture instead of the workspace's real classification.
CONFLICTS_TOML = Path("scripts/me3-dll-conflicts.toml")


def conflicting_pairs(root: Path) -> set[frozenset[str]]:
    """Package pairs recorded as unable to share one ME3 profile.

    Only `[[conflict]]` counts. A `[[shared]]` row means the opposite -- two DLLs that do detour
    one prologue and have been made co-loadable by the hook union -- so those still have to survive
    the scan, which is the whole reason that mechanism exists.
    """
    path = root / CONFLICTS_TOML
    if not path.is_file():
        return set()
    table = tomllib.loads(path.read_text(encoding="utf-8"))
    pairs = set()
    for row in table.get("conflict", []):
        a, b = row.get("a"), row.get("b")
        if a and b:
            pairs.add(frozenset({a, b}))
    return pairs


def crate_of(path: Path, root: Path) -> str:
    parts = path.relative_to(root).parts
    return parts[1] if len(parts) > 2 and parts[0] == "crates" else parts[0]


def signatures(root: Path) -> dict[str, list[tuple[Path, int]]]:
    """Every AOB literal under `root`, keyed by its normalised bytes."""
    found: dict[str, list[tuple[Path, int]]] = defaultdict(list)
    for path in sorted(root.glob("crates/**/*.rs")):
        if "/target/" in str(path):
            continue
        text = path.read_text(encoding="utf-8", errors="replace")
        for index, line in enumerate(text.splitlines(), 1):
            for match in AOB.finditer(line):
                key = " ".join(match.group(1).split()).upper()
                found[key].append((path, index))
    return found


def offenders(root: Path) -> list[str]:
    problems = []
    never_together = conflicting_pairs(root)
    for pattern, sites in signatures(root).items():
        crates = {crate_of(path, root) for path, _ in sites}
        if len(crates) < 2:
            continue
        if all(
            frozenset(pair) in never_together for pair in itertools.combinations(sorted(crates), 2)
        ):
            continue
        unguarded = []
        for path, line in sites:
            if PATCH_WIDTH_MARKER not in path.read_text(encoding="utf-8", errors="replace"):
                unguarded.append(f"{path.relative_to(root)}:{line}")
        if not unguarded:
            continue
        short = pattern if len(pattern) <= 60 else pattern[:57] + "..."
        problems.append(
            f"{sorted(crates)} share the signature `{short}` and these sites do not allow "
            f"for each other's detour: {', '.join(unguarded)}"
        )
    return problems


def selftest() -> int:
    import tempfile

    cases = [
        # One crate alone may scan for whatever it likes.
        ({"crates/a/src/lib.rs": 'const S: &str = "40 53 48 83 EC 40 48 8B 41 18 48 8B D9";'}, 0),
        # Two crates, neither allowing for a detour: refused.
        (
            {
                "crates/a/src/lib.rs": 'const S: &str = "40 53 48 83 EC 40 48 8B 41 18 48 8B D9";',
                "crates/b/src/lib.rs": 'const T: &str = "40 53 48 83 EC 40 48 8B 41 18 48 8B D9";',
            },
            1,
        ),
        # Two crates, both declaring the patch width: allowed.
        (
            {
                "crates/a/src/lib.rs": 'const PARSE_SIG_PATCH_BYTES: usize = 5;\n'
                'const S: &str = "40 53 48 83 EC 40 48 8B 41 18 48 8B D9";',
                "crates/b/src/lib.rs": 'const PARSE_SIG_PATCH_BYTES: usize = 5;\n'
                'const T: &str = "40 53 48 83 EC 40 48 8B 41 18 48 8B D9";',
            },
            0,
        ),
        # One of the two still unguarded: refused, and it is the one named.
        (
            {
                "crates/a/src/lib.rs": 'const PARSE_SIG_PATCH_BYTES: usize = 5;\n'
                'const S: &str = "40 53 48 83 EC 40 48 8B 41 18 48 8B D9";',
                "crates/b/src/lib.rs": 'const T: &str = "40 53 48 83 EC 40 48 8B 41 18 48 8B D9";',
            },
            1,
        ),
        # Differing whitespace and case is the same signature.
        (
            {
                "crates/a/src/lib.rs": 'const S: &str = "40 53 48 83 ec 40 48 8b 41 18 48 8b d9";',
                "crates/b/src/lib.rs": 'const T: &str = "40 53 48 83 EC 40  48 8B 41 18 48 8B D9";',
            },
            1,
        ),
        # Two sites in one crate are not a collision; a crate cannot race itself.
        (
            {
                "crates/a/src/lib.rs": 'const S: &str = "40 53 48 83 EC 40 48 8B 41 18 48 8B D9";',
                "crates/a/src/other.rs": 'const T: &str = "40 53 48 83 EC 40 48 8B 41 18 48 8B D9";',
            },
            0,
        ),
        # Too short to be a signature.
        (
            {
                "crates/a/src/lib.rs": 'const S: &str = "40 53 48 83 EC 40";',
                "crates/b/src/lib.rs": 'const T: &str = "40 53 48 83 EC 40";',
            },
            0,
        ),
        # A pair the conflict table says can never share a profile cannot race for the prologue.
        (
            {
                "crates/a/src/lib.rs": 'const S: &str = "40 53 48 83 EC 40 48 8B 41 18 48 8B D9";',
                "crates/b/src/lib.rs": 'const T: &str = "40 53 48 83 EC 40 48 8B 41 18 48 8B D9";',
                "scripts/me3-dll-conflicts.toml": '[[conflict]]\na = "a"\nb = "b"\n',
            },
            0,
        ),
        # ...and the exemption is that pair only, not any pair once a table exists.
        (
            {
                "crates/a/src/lib.rs": 'const S: &str = "40 53 48 83 EC 40 48 8B 41 18 48 8B D9";',
                "crates/b/src/lib.rs": 'const T: &str = "40 53 48 83 EC 40 48 8B 41 18 48 8B D9";',
                "scripts/me3-dll-conflicts.toml": '[[conflict]]\na = "a"\nb = "c"\n',
            },
            1,
        ),
        # A declared-shared pair is co-loadable, so the scan still has to survive the other detour.
        (
            {
                "crates/a/src/lib.rs": 'const S: &str = "40 53 48 83 EC 40 48 8B 41 18 48 8B D9";',
                "crates/b/src/lib.rs": 'const T: &str = "40 53 48 83 EC 40 48 8B 41 18 48 8B D9";',
                "scripts/me3-dll-conflicts.toml": '[[shared]]\na = "a"\nb = "b"\n',
            },
            1,
        ),
        # Three crates on one signature: one declared pair leaves two undeclared ones, so refused.
        (
            {
                "crates/a/src/lib.rs": 'const S: &str = "40 53 48 83 EC 40 48 8B 41 18 48 8B D9";',
                "crates/b/src/lib.rs": 'const T: &str = "40 53 48 83 EC 40 48 8B 41 18 48 8B D9";',
                "crates/c/src/lib.rs": 'const U: &str = "40 53 48 83 EC 40 48 8B 41 18 48 8B D9";',
                "scripts/me3-dll-conflicts.toml": '[[conflict]]\na = "a"\nb = "b"\n',
            },
            1,
        ),
    ]
    failures = 0
    for index, (files, expected) in enumerate(cases, 1):
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            for name, body in files.items():
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(body, encoding="utf-8")
            got = 1 if offenders(root) else 0
            mark = "ok  " if got == expected else "FAIL"
            if got != expected:
                failures += 1
            print(f"  {mark} case {index}: expected {expected}, got {got}")
    print(f"check-shared-detour-signatures selftest: {'OK' if not failures else 'FAILED'}")
    return 1 if failures else 0


def main() -> int:
    if "--selftest" in sys.argv[1:]:
        return selftest()
    problems = offenders(REPO_ROOT)
    if problems:
        print(__doc__.strip().splitlines()[0])
        for problem in problems:
            print(f"  {problem}")
        print(
            f"\nMatch past the detour and subtract its width, the way "
            f"crates/er-invasion-warp/src/map_gfx.rs does, and declare {PATCH_WIDTH_MARKER}."
        )
        return 1
    never_together = conflicting_pairs(REPO_ROOT)
    shared = exempt = 0
    for sites in signatures(REPO_ROOT).values():
        crates = {crate_of(path, REPO_ROOT) for path, _ in sites}
        if len(crates) < 2:
            continue
        shared += 1
        if all(
            frozenset(pair) in never_together for pair in itertools.combinations(sorted(crates), 2)
        ):
            exempt += 1
    print(
        f"check-shared-detour-signatures: OK -- {shared} signature(s) hooked from more than one "
        f"crate, every one of them scanning past a detour or split across crates the conflict "
        f"table keeps out of one profile ({exempt} exempt that way)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
