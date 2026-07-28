#!/usr/bin/env python3
"""Corpus save explorer: find every ER0000.sl2/.co2 under a root that contains a
character with a given name, and report that character's slot, level, runes, and
highest weapon upgrade level.

WHY THIS EXISTS (bd build-finder-tool-dont-skip-solved-problems-2026-07-20):
the save-manager corpus dirs are labeled by the manager's OWN names, NOT the
in-game character name, so a directory-name grep for "angrE" finds nothing even
though the character exists. This reads the in-game name out of each save's
plaintext BND4 body (ER PC saves are plaintext, md5-per-slot) and maps
name -> absolute file path + slot + level + top weapon upgrade.

Decode reuses the evidence-bound scripts/save-slot-oracle.py (name @ player+0x94,
level @ player+0x60), the same decoder enumerate-valid-saves.py trusts. Highest
weapon upgrade is derived from the slot's GaItem table (see max_weapon_upgrade).

CACHE (findable + clearable, self-invalidating -- see the "Decode cache" section
below): the whole-corpus scan is slow (~70 files x ~26 MB, 10 slots each). Decoded
per-slot identity is cached on disk under the repo's gitignored target/ tree at
    target/save-char-index/index.json
keyed by each save's absolute path + st_size + st_mtime_ns, so a changed save is
NEVER served stale (it re-decodes). The cache path is printed to stderr on every
run. Clear it with `--clear-cache` (or `rm -rf target/save-char-index`); bypass it
with `--no-cache`. Only the index JSON under target/ is ever written -- the save
files themselves are strictly read-only.

Usage:
    scripts/find-save-char.py <root-dir> '<name>' [--exact] [--json]
    scripts/find-save-char.py --clear-cache        # wipe the decode cache, exit 0
    scripts/find-save-char.py <root> '<name>' --no-cache   # always decode fresh
    # e.g. scripts/find-save-char.py ./ 'angrE'
"""
from __future__ import annotations

import argparse
import importlib.util
import json
import os
import sys
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent
REPO_ROOT = HERE.parent

# --- Decode cache ----------------------------------------------------------
# Findable, clearable, self-invalidating on save change. The ONLY thing written
# is this index JSON; save files stay read-only. Location is fixed and obvious
# (under the gitignored target/ tree) but env-overridable for reuse in other
# layouts. Each entry keys on abspath + st_size + st_mtime_ns, so a save that is
# rewritten/resized/touched no longer matches and is re-decoded -- a stale entry
# is never served.
CACHE_VERSION = 1


def cache_paths() -> tuple[Path, Path]:
    override = os.environ.get("ER_SAVE_CHAR_INDEX_DIR")
    cache_dir = Path(override) if override else (REPO_ROOT / "target" / "save-char-index")
    return cache_dir, cache_dir / "index.json"


def load_cache_entries(index_path: Path) -> dict[str, Any]:
    """Load the {abspath: {size, mtime_ns, slots}} map. Corrupt/missing -> empty."""
    try:
        raw = json.loads(index_path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}
    if not isinstance(raw, dict):
        return {}
    entries = raw.get("entries")
    return entries if isinstance(entries, dict) else {}


def save_cache_entries(cache_dir: Path, index_path: Path, touched: dict[str, Any]) -> None:
    """Merge freshly-decoded entries onto the on-disk index and atomically replace.

    Re-reads the index at write time so a concurrent run's additions are not
    clobbered; our re-decoded (``touched``) entries win for the files we touched.
    Best-effort: an index write failure never affects correctness (the scan
    result is already computed; the next run simply re-decodes).
    """
    if not touched:
        return
    merged = load_cache_entries(index_path)
    merged.update(touched)
    payload = {"version": CACHE_VERSION, "entries": merged}
    try:
        cache_dir.mkdir(parents=True, exist_ok=True)
        tmp = index_path.with_name(index_path.name + ".tmp")
        tmp.write_text(json.dumps(payload), encoding="utf-8")
        os.replace(tmp, index_path)
    except OSError as exc:
        print(f"# warn: could not write cache {index_path}: {exc}", file=sys.stderr, flush=True)


def clear_cache() -> int:
    index_path = cache_paths()[1]
    removed = False
    for p in (index_path, index_path.with_name(index_path.name + ".tmp")):
        try:
            p.unlink()
            removed = removed or p == index_path
        except FileNotFoundError:
            pass
        except OSError as exc:
            print(f"# error: could not remove {p}: {exc}", file=sys.stderr)
            return 1
    suffix = "" if removed else " (was already empty)"
    print(f"# cleared save-char decode cache: {index_path}{suffix}", file=sys.stderr)
    return 0


def _load_oracle():
    spec = importlib.util.spec_from_file_location("save_slot_oracle", HERE / "save-slot-oracle.py")
    assert spec and spec.loader
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


# --- highest weapon upgrade -------------------------------------------------
# ER weapon reinforcement is encoded in the weapon's param id (fullId = baseId +
# reinforceLevel, reinforceLevel 0..25). The value LIVES in the slot's GaItem
# table, but that table has NO local structural spec here (docs/bnd4-save-format.md
# stops at the container; the SL2.bt internal ChrAsm/GaItem struct is not vendored).
# A SLOT-WIDE byte scan CANNOT isolate it: empirically a fresh level-7 character
# yields 16k+ "category-0" and 2.7k "0x8000_0000" (ash-of-war, NOT weapon) pair
# matches -- pure noise -- so any %100 over them fabricates a bogus "+20". Rather
# than emit a fabricated number, this returns None until it is backed by the real
# GaItem-table offset+stride (or ChrAsm equipped-weapon param ids). See bd
# find-save-char-weapon-upgrade-needs-gaitem-table-offset-2026-07-20.
def max_weapon_upgrade(slot_data: bytes) -> int | None:
    """Highest weapon reinforcement (+N) -- UNIMPLEMENTED reliably; returns None.

    A trustworthy value requires the GaItem table structure (offset+stride) or the
    ChrAsm equipped-weapon param ids, neither of which is vendored locally. A
    slot-wide heuristic is provably noise (fresh characters read a fake +20), so we
    report None ('?') instead of a fabricated level.
    """
    _ = slot_data
    return None


def decode_file_slots(oracle, path: Path, data: bytes) -> list[dict[str, Any]]:
    """Decode every OCCUPIED slot of one save into cacheable, query-independent rows.

    Mirrors the per-slot filtering scan_file used before caching existed, so the
    cached rows are exactly what a fresh scan would have produced. Query matching
    happens later (in match_slots), so one cache entry serves every future query.
    """
    slots: list[dict[str, Any]] = []
    for slot in range(10):
        try:
            result = oracle.decode_save_slot(data, path, slot)
        except Exception:
            continue
        df = result.get("decoded_fields") or {}
        name = (df.get("name") or "").strip()
        if df.get("name_empty_like") or not name:
            continue
        try:
            slot_data, _ = oracle.extract_slot(data, slot)
        except Exception:
            slot_data = b""
        slots.append(
            {
                "slot": slot,
                "name": name,
                "level": df.get("level"),
                "runes": df.get("runes"),
                "ext": path.suffix.lower().lstrip("."),
                "max_weapon_upgrade": max_weapon_upgrade(slot_data),
            }
        )
    return slots


def get_file_slots(
    oracle,
    path: Path,
    abspath: str,
    cache_entries: dict[str, Any],
    touched: dict[str, Any],
    use_cache: bool,
    stats: dict[str, int],
) -> list[dict[str, Any]]:
    """Return the occupied-slot rows for one save, from cache when path+size+mtime
    match, else by decoding fresh and staging a cache update. Correct even if the
    cache is missing/deleted -- a miss just decodes."""
    try:
        st = path.stat()
    except OSError:
        return []
    size = st.st_size
    mtime_ns = st.st_mtime_ns
    if use_cache:
        ent = cache_entries.get(abspath)
        if ent and ent.get("size") == size and ent.get("mtime_ns") == mtime_ns:
            stats["hits"] += 1
            return ent.get("slots") or []
    try:
        data = path.read_bytes()
    except OSError:
        return []
    slots = decode_file_slots(oracle, path, data)
    stats["misses"] += 1
    if use_cache:
        touched[abspath] = {"size": size, "mtime_ns": mtime_ns, "slots": slots}
    return slots


def match_slots(slots: list[dict[str, Any]], abspath: str, query: str, exact: bool) -> list[dict[str, Any]]:
    q = query.casefold()
    matches: list[dict[str, Any]] = []
    for s in slots:
        name = s["name"]
        hit = (name.casefold() == q) if exact else (q in name.casefold())
        if not hit:
            continue
        matches.append(
            {
                "abspath": abspath,
                "slot": s["slot"],
                "name": name,
                "level": s.get("level"),
                "runes": s.get("runes"),
                "max_weapon_upgrade": s.get("max_weapon_upgrade"),
                "ext": s["ext"],
            }
        )
    return matches


def main() -> int:
    ap = argparse.ArgumentParser(description="Find ER saves containing a named character.")
    ap.add_argument("root", nargs="?", help="directory to search recursively for ER0000.sl2/.co2")
    ap.add_argument("name", nargs="?", help="in-game character name to find (substring by default)")
    ap.add_argument("--exact", action="store_true", help="require an exact (case-insensitive) name match")
    ap.add_argument("--json", action="store_true", help="emit JSON instead of human lines")
    ap.add_argument(
        "--clear-cache",
        action="store_true",
        help="wipe the on-disk decode cache (target/save-char-index/index.json) and exit 0",
    )
    ap.add_argument("--no-cache", action="store_true", help="ignore and do not update the decode cache (always fresh)")
    args = ap.parse_args()

    if args.clear_cache:
        return clear_cache()

    if not args.root or args.name is None:
        ap.error("root and name are required unless --clear-cache is given")

    root = Path(args.root)
    if not root.is_dir():
        print(f"error: not a directory: {root}", file=sys.stderr)
        return 2

    use_cache = not args.no_cache
    cache_dir, index_path = cache_paths()
    cache_entries: dict[str, Any] = load_cache_entries(index_path) if use_cache else {}
    touched: dict[str, Any] = {}
    stats: dict[str, int] = {"hits": 0, "misses": 0}
    if use_cache:
        print(
            f"# save-char decode cache: {index_path} ({len(cache_entries)} cached file(s); clear with --clear-cache)",
            file=sys.stderr,
            flush=True,
        )
    else:
        print("# save-char decode cache: DISABLED (--no-cache); decoding every save fresh", file=sys.stderr, flush=True)

    def fmt(m: dict[str, Any]) -> str:
        wl = m["max_weapon_upgrade"]
        wl_s = f"+{wl}" if wl is not None else "?"
        return (
            f"{m['abspath']}\tslot={m['slot']}\tname={m['name']!r}\t"
            f"level={m['level']}\trunes={m['runes']}\ttop_weapon={wl_s}\t({m['ext']})"
        )

    oracle = _load_oracle()
    all_matches: list[dict[str, Any]] = []
    files = [
        p
        for p in sorted(root.rglob("ER0000.*"))
        if p.suffix.lower().lstrip(".") in ("sl2", "co2")
        and "er-effects-save-redirect-stage" not in p.as_posix()
    ]
    # Stream each match the instant its file decodes (flush=True) so a background
    # run can be monitored live for the expected name instead of blocking to the end.
    for i, p in enumerate(files):
        abspath = str(p.resolve())
        slots = get_file_slots(oracle, p, abspath, cache_entries, touched, use_cache, stats)
        ms = match_slots(slots, abspath, args.name, args.exact)
        all_matches.extend(ms)
        if not args.json:
            for m in ms:
                print(fmt(m), flush=True)
        elif ms:
            print(f"# [{i + 1}/{len(files)}] {len(ms)} match(es) in {p}", file=sys.stderr, flush=True)

    if use_cache:
        save_cache_entries(cache_dir, index_path, touched)
        print(
            f"# cache: {stats['hits']} hit(s), {stats['misses']} miss(es re-decoded); index -> {index_path}",
            file=sys.stderr,
            flush=True,
        )

    if args.json:
        print(json.dumps({"query": args.name, "exact": args.exact, "matches": all_matches}, indent=2))
    elif not all_matches:
        print(f"# no save under {root} contains a character matching '{args.name}'", flush=True)
    return 0 if all_matches else 1


if __name__ == "__main__":
    raise SystemExit(main())
