#!/usr/bin/env python3
"""Pick a random valid (save file, slot) and DECODE IT BEFORE ANYONE LAUNCHES ANYTHING.

"Random" here never means "blind". The Autoload Identity Launch Gate in AGENTS.md is
explicit: a launch expected to autoload must not happen until the exact character identity
and slot are known from current save evidence. So this script's contract is decode-then-
report -- it returns the character's name, level and slot, and a caller that cannot print
those has no business launching.

VALIDITY IS THE PRODUCT'S OWN DEFINITION, NOT A GUESS
-----------------------------------------------------
  * File size must equal `EXPECTED_SAVE_FILE_BYTES`, read live from
    `crates/er-save-redirect/src/lib.rs` rather than copied here -- `validate_save_file_path`
    rejects anything else at runtime, so a picker using a different number would hand back
    saves the DLL refuses.
  * A slot counts as occupied when the decoded name is not empty-like AND level > 0, decoded
    with `save-slot-oracle.py` (the evidence-bound decoder), never inferred from filenames.

WHY IT PICKS A FILE FIRST, THEN A SLOT
--------------------------------------
Decoding all ten slots of one save costs ~0.4s, so sweeping the whole corpus up front is
~35s -- past the 30s cap every shell op here lives under. Drawing a file and decoding only
its slots costs one tenth of a second, and a file with no occupied slot simply triggers a
redraw. The distribution is therefore uniform over save FILES rather than over characters;
that is a deliberate trade for staying inside the cap, and `--all` exists when a full
inventory is actually wanted.

CORPUS ROOT
-----------
`--root`, else `$ER_SAVE_CORPUS_ROOT`, else `<repo>/save-files`. The older enumerator in this
directory still defaults to a `/mnt/a/...` WSL path that does not exist on this machine, so
every run of it silently found nothing; the default here is the corpus that is actually
present. Staged redirect subtrees (`er-effects-save-redirect-stage/`) are skipped -- they are
private copies the DLL writes, not sources.

Usage:
    python3 scripts/er-pick-save.py
    python3 scripts/er-pick-save.py --json --seed 1234
    python3 scripts/er-pick-save.py --container sl2
    python3 scripts/er-pick-save.py --all --json
    python3 scripts/er-pick-save.py --selftest
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import random
import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
ORACLE_SCRIPT = REPO_ROOT / "scripts" / "save-slot-oracle.py"
SAVE_REDIRECT_LIB = REPO_ROOT / "crates" / "er-save-redirect" / "src" / "lib.rs"
STAGE_DIR_MARKER = "er-effects-save-redirect-stage"

EXIT_OK = 0
EXIT_ERROR = 1

# A file with no occupied slot is possible (a wiped container), so a redraw needs a bound.
MAX_DRAWS = 25


def expected_save_bytes() -> int:
    """Read the product's own size invariant instead of duplicating the constant."""
    text = SAVE_REDIRECT_LIB.read_text(encoding="utf-8", errors="replace")
    match = re.search(
        r"pub const EXPECTED_SAVE_FILE_BYTES:\s*u64\s*=\s*(0x[0-9a-fA-F_]+|[0-9_]+)", text
    )
    if not match:
        raise RuntimeError(
            f"EXPECTED_SAVE_FILE_BYTES not found in {SAVE_REDIRECT_LIB} -- "
            "the constant moved; fix this reader rather than hard-coding a number"
        )
    return int(match.group(1).replace("_", ""), 0)


def oracle():
    spec = importlib.util.spec_from_file_location("save_slot_oracle", ORACLE_SCRIPT)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def resolve_root(cli_root: str | None) -> Path:
    for candidate in (cli_root, os.environ.get("ER_SAVE_CORPUS_ROOT"), REPO_ROOT / "save-files"):
        if not candidate:
            continue
        path = Path(candidate).expanduser()
        if path.is_dir():
            return path
    raise RuntimeError(
        "no save corpus found; pass --root or set ER_SAVE_CORPUS_ROOT to a directory of "
        "ER0000.sl2 / ER0000.co2 files"
    )


def eligible_saves(root: Path, container: str, expected_bytes: int) -> list[Path]:
    """Source saves of the right container and the exact product-mandated size."""
    suffixes = {"sl2": {".sl2"}, "co2": {".co2"}, "both": {".sl2", ".co2"}}[container]
    found = []
    for path in sorted(root.rglob("ER0000.*")):
        if path.suffix.lower() not in suffixes:
            continue
        if STAGE_DIR_MARKER in path.parts:
            continue
        try:
            if path.stat().st_size != expected_bytes:
                continue
        except OSError:
            continue
        found.append(path)
    return found


def occupied_slots(module, path: Path) -> list[dict]:
    """Decode every slot of one save; return the occupied ones with their identity."""
    data = path.read_bytes()
    results = []
    for slot in range(module.SLOT_COUNT):
        try:
            decoded = module.decode_save_slot(data, path, slot)
        except Exception:  # a slot that will not decode is not a launch target
            continue
        fields = decoded.get("decoded_fields") or {}
        name = fields.get("name") or ""
        level = fields.get("level")
        if fields.get("name_empty_like") or module.name_empty_like(name):
            continue
        if not isinstance(level, int) or level <= 0:
            continue
        results.append(
            {
                "slot": slot,
                "name": name,
                "level": level,
                "runes": fields.get("runes"),
                "stats": fields.get("stats_named"),
            }
        )
    return results


def describe(path: Path, entry: dict, root: Path) -> dict:
    stat = path.stat()
    return {
        "save_file": str(path),
        "save_file_relative": str(path.relative_to(root)) if path.is_relative_to(root) else None,
        "container": path.suffix.lower().lstrip("."),
        # Half this corpus is writable on disk. The DLL stages privately so the source should
        # never be written, but a run that names its exposure is easier to trust than one that
        # assumes the guarantee held.
        "source_writable": bool(stat.st_mode & 0o200),
        **entry,
    }


def pick(root: Path, container: str, seed: int) -> dict:
    module = oracle()
    expected = expected_save_bytes()
    candidates = eligible_saves(root, container, expected)
    if not candidates:
        raise RuntimeError(
            f"no eligible {container} saves under {root} "
            f"(need ER0000.* of exactly {expected} bytes, outside {STAGE_DIR_MARKER}/)"
        )

    rng = random.Random(seed)
    pool = candidates[:]
    rng.shuffle(pool)
    for attempt, path in enumerate(pool[:MAX_DRAWS], start=1):
        slots = occupied_slots(module, path)
        if not slots:
            continue
        chosen = rng.choice(slots)
        return {
            "seed": seed,
            "draws": attempt,
            "eligible_files": len(candidates),
            "occupied_slots_in_file": len(slots),
            "corpus_root": str(root),
            **describe(path, chosen, root),
        }

    raise RuntimeError(
        f"drew {min(len(pool), MAX_DRAWS)} saves under {root} and none had an occupied slot"
    )


def inventory(root: Path, container: str) -> dict:
    """Full sweep. Deliberately not the default: it is well past the 30s shell cap."""
    module = oracle()
    expected = expected_save_bytes()
    entries = []
    for path in eligible_saves(root, container, expected):
        for entry in occupied_slots(module, path):
            entries.append(describe(path, entry, root))
    return {"corpus_root": str(root), "count": len(entries), "targets": entries}


def render(result: dict) -> str:
    return "\n".join(
        [
            f"character  {result['name']}  RL{result['level']}",
            f"save       {result['save_file']}",
            f"slot       {result['slot']}   container .{result['container']}"
            + ("   SOURCE IS WRITABLE" if result["source_writable"] else "   source read-only"),
            f"draw       seed={result['seed']} draws={result['draws']} "
            f"({result['occupied_slots_in_file']} occupied slots in this file, "
            f"{result['eligible_files']} eligible files)",
        ]
    )


def selftest() -> int:
    ok = True

    def check(condition: bool, label: str) -> None:
        nonlocal ok
        if not condition:
            ok = False
        print(("  ok   " if condition else "  FAIL ") + label)

    expected = expected_save_bytes()
    check(expected == 0x1BA03D0, f"the product's size invariant is read live ({expected})")

    try:
        root = resolve_root(None)
    except RuntimeError as err:
        print(f"  skip  {err}")
        print("selftest:", "PASS" if ok else "FAIL")
        return EXIT_OK if ok else EXIT_ERROR
    check(root.is_dir(), f"corpus root resolves to a real directory ({root})")

    both = eligible_saves(root, "both", expected)
    sl2 = eligible_saves(root, "sl2", expected)
    co2 = eligible_saves(root, "co2", expected)
    check(len(both) == len(sl2) + len(co2), "container filters partition the corpus exactly")
    check(len(both) > 0, f"the corpus has eligible saves ({len(both)})")
    check(
        all(STAGE_DIR_MARKER not in path.parts for path in both),
        "staged redirect copies are excluded from the source pool",
    )
    check(
        all(path.stat().st_size == expected for path in both),
        "every eligible save matches the product's exact byte length",
    )

    first = pick(root, "both", seed=1234)
    second = pick(root, "both", seed=1234)
    check(
        (first["save_file"], first["slot"]) == (second["save_file"], second["slot"]),
        "the same seed reproduces the same (file, slot) -- a broken run can be re-run",
    )
    others = {
        (pick(root, "both", seed=s)["save_file"], pick(root, "both", seed=s)["slot"])
        for s in range(20, 26)
    }
    check(len(others) > 1, f"different seeds pick different targets ({len(others)} distinct)")

    check(bool(first["name"]) and first["level"] > 0, "the pick carries a decoded identity")
    check(
        not oracle().name_empty_like(first["name"]),
        "the decoded name is not empty-like (the occupancy rule actually applied)",
    )

    only_sl2 = pick(root, "sl2", seed=7)
    check(only_sl2["container"] == "sl2", "a container filter is honoured by the pick")

    print("selftest:", "PASS" if ok else "FAIL")
    return EXIT_OK if ok else EXIT_ERROR


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", help="save corpus root (default: $ER_SAVE_CORPUS_ROOT or ./save-files)")
    parser.add_argument(
        "--container",
        choices=("sl2", "co2", "both"),
        default="both",
        help="which save container to draw from (default: both -- ersc.dll is always loaded, "
        "and Seamless mode accepts both containers)",
    )
    parser.add_argument("--seed", type=int, help="RNG seed (default: random, always reported)")
    parser.add_argument("--all", action="store_true", help="inventory every valid target instead of picking")
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()

    if args.selftest:
        return selftest()

    try:
        root = resolve_root(args.root)
        if args.all:
            result = inventory(root, args.container)
            print(json.dumps(result, indent=2) if args.json else f"{result['count']} valid targets")
            return EXIT_OK
        seed = args.seed if args.seed is not None else random.SystemRandom().randrange(2**31)
        result = pick(root, args.container, seed)
    except RuntimeError as err:
        print(f"er-pick-save: {err}", file=sys.stderr)
        return EXIT_ERROR

    print(json.dumps(result, indent=2) if args.json else render(result))
    return EXIT_OK


if __name__ == "__main__":
    sys.exit(main())
