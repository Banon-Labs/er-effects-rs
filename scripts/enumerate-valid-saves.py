#!/usr/bin/env python3
"""Enumerate valid (file, slot) character targets across an Elden Ring save corpus.

For the repeatable-multi-save-load proof (docs/goals/repeatable-multi-save-load-acceptance.md):
walks a corpus of ER0000.sl2/.co2 files, decodes every slot with the evidence-bound
save-slot-oracle.py decoder, and writes a JSON inventory of the VALID (occupied, real)
(file, slot) pairs -- the concrete test targets a proof harness loops over.

"Valid save" per the acceptance doc = the slot is occupied (a real character), which we take
as: decoded name is non-empty (not name_empty_like) AND decoded level > 0. Files/slots that
fail to decode are cataloged as skipped-with-reason, never silently dropped.

Corpus root precedence: --root -> $ER_SAVE_CORPUS_ROOT ->
  /mnt/a/Code Projects/Elden Ring Save Manager/data/save-files  (WSL default).
Staged redirect subtrees (er-effects-save-redirect-stage/) are skipped -- they are private
copies produced by the save-redirect probe path, not source saves.

Usage:
  enumerate-valid-saves.py [--root DIR] [--ext sl2|co2|both] [--json OUT] [--quiet]
Exit 0 always (enumeration is diagnostic); a nonzero exit only on a hard root-missing error.
"""
from __future__ import annotations

import argparse
import importlib.util
import json
import os
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
DEFAULT_ROOT = "/mnt/a/Code Projects/Elden Ring Save Manager/data/save-files"


def _load_oracle():
    spec = importlib.util.spec_from_file_location("save_slot_oracle", HERE / "save-slot-oracle.py")
    assert spec and spec.loader
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def resolve_root(cli_root: str | None) -> Path:
    for cand in (cli_root, os.environ.get("ER_SAVE_CORPUS_ROOT"), DEFAULT_ROOT):
        if cand:
            p = Path(cand)
            if p.is_dir():
                return p
    raise SystemExit(f"error: no readable corpus root (tried --root, $ER_SAVE_CORPUS_ROOT, {DEFAULT_ROOT})")


def enumerate_corpus(root: Path, exts: tuple[str, ...]) -> dict:
    oracle = _load_oracle()
    files: list[Path] = []
    for p in sorted(root.rglob("ER0000.*")):
        if p.suffix.lower().lstrip(".") not in exts:
            continue
        if "er-effects-save-redirect-stage" in p.as_posix():
            continue
        files.append(p)

    inventory: list[dict] = []
    skipped: list[dict] = []
    for p in files:
        rel = os.path.relpath(p, root)
        ext = p.suffix.lower().lstrip(".")
        try:
            data = p.read_bytes()
        except OSError as exc:
            skipped.append({"file": rel, "reason": f"read-error: {exc}"})
            continue
        valid_slots = []
        for slot in range(10):
            try:
                df = oracle.decode_save_slot(data, p, slot).get("decoded_fields", {})
            except Exception as exc:  # decoder is best-effort per slot
                continue
            name = (df.get("name") or "").strip()
            level = df.get("level")
            if df.get("name_empty_like") or not name or not isinstance(level, int) or level <= 0:
                continue
            valid_slots.append(
                {
                    "slot": slot,
                    "name": name,
                    "level": level,
                    "runes": df.get("runes"),
                    "saved_map_c30": df.get("saved_map_c30"),
                    "archetype": df.get("archetype"),
                    "stats": df.get("stats"),
                }
            )
        entry = {"file": rel, "abspath": str(p), "ext": ext, "seamless": ext == "co2", "valid_slots": valid_slots}
        inventory.append(entry)
        if not valid_slots:
            skipped.append({"file": rel, "reason": "no occupied slots decoded (all empty/invalid)"})

    vanilla = [e for e in inventory if not e["seamless"] and e["valid_slots"]]
    seamless = [e for e in inventory if e["seamless"] and e["valid_slots"]]
    return {
        "root": str(root),
        "counts": {
            "files_total": len(files),
            "vanilla_files_with_valid": len(vanilla),
            "seamless_files_with_valid": len(seamless),
            "total_valid_pairs": sum(len(e["valid_slots"]) for e in inventory),
        },
        "inventory": inventory,
        "skipped": skipped,
    }


def print_human(result: dict) -> None:
    c = result["counts"]
    print(f"# corpus root: {result['root']}")
    print(
        f"# {c['files_total']} save files scanned; "
        f"{c['vanilla_files_with_valid']} vanilla + {c['seamless_files_with_valid']} seamless have >=1 valid slot; "
        f"{c['total_valid_pairs']} valid (file,slot) pairs total"
    )
    print("# --- VANILLA (.sl2) valid targets ---")
    for e in result["inventory"]:
        if e["seamless"] or not e["valid_slots"]:
            continue
        slots = " ".join(f"s{v['slot']}:{v['name']}/L{v['level']}" for v in e["valid_slots"])
        print(f"  {e['file']}  ->  {slots}")
    seam = [e for e in result["inventory"] if e["seamless"] and e["valid_slots"]]
    if seam:
        print("# --- SEAMLESS (.co2) valid targets ---")
        for e in seam:
            slots = " ".join(f"s{v['slot']}:{v['name']}/L{v['level']}" for v in e["valid_slots"])
            print(f"  {e['file']}  ->  {slots}")
    if result["skipped"]:
        print(f"# --- skipped/cataloged ({len(result['skipped'])}) ---")
        for s in result["skipped"]:
            print(f"  {s['file']}: {s['reason']}")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--root", default=None, help="corpus root (default: $ER_SAVE_CORPUS_ROOT or WSL default)")
    ap.add_argument("--ext", choices=["sl2", "co2", "both"], default="both")
    ap.add_argument("--json", type=Path, default=None, help="write the full inventory JSON here")
    ap.add_argument("--quiet", action="store_true", help="suppress the human-readable summary")
    args = ap.parse_args()

    root = resolve_root(args.root)
    exts = ("sl2", "co2") if args.ext == "both" else (args.ext,)
    t0 = time.time()
    result = enumerate_corpus(root, exts)
    result["elapsed_s"] = round(time.time() - t0, 1)
    if args.json:
        args.json.write_text(json.dumps(result, indent=1))
        if not args.quiet:
            print(f"# wrote {args.json} ({result['counts']['total_valid_pairs']} valid pairs, {result['elapsed_s']}s)")
    if not args.quiet:
        print_human(result)
    return 0


if __name__ == "__main__":
    sys.exit(main())
