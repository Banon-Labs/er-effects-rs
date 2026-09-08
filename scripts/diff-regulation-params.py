#!/usr/bin/env python3
"""Diff two `regulation.bin` files param-by-param, offline.

Answers the question a code-address migration structurally cannot: did a game
patch change a PARAM's *layout* (row stride, paramdef version) or its *row set*,
under the same param name? Both are silent -- a row read at the old stride yields
a plausible neighbouring value, and a retired row id yields a no-op.

Reuses the four decrypt/unpack stages of `regulation-params.py` (AES-256-CBC ->
DCX/zstd -> BND4 -> PARAM), so a format change fails loudly in one place.

    python3 scripts/diff-regulation-params.py OLD.bin NEW.bin
    python3 scripts/diff-regulation-params.py OLD.bin NEW.bin --param SpEffectParam
    python3 scripts/diff-regulation-params.py OLD.bin NEW.bin --rows SpEffectParam

Row stride is derived from the row-entry table (consecutive data offsets), not
from a paramdef -- no paramdef is needed to detect that a row got wider, which is
the layout change that matters. A stride that is unchanged does not prove the
field *meanings* are unchanged; it only rules out resizing.
"""

import argparse
import os
import struct
import sys
from collections import Counter

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

#: Repo root, so `--effects-json`'s default resolves from a gate that runs anywhere.
REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DEFAULT_EFFECTS_JSON = os.path.join(REPO, "data", "effects.json")

def _load_sibling():
    """Import `regulation-params.py` despite the hyphen in its name."""
    import importlib.util

    path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "regulation-params.py")
    spec = importlib.util.spec_from_file_location("regulation_params", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


RP = _load_sibling()


#: Explicit opt-out for an environment that genuinely cannot have the game installed (CI).
#: Set to 1 to downgrade a missing regulation from a failure to a printed skip. Absent this,
#: a missing regulation is exit 2 -- "could not look" must never read as "agreed".
ALLOW_MISSING_REGULATION_ENV = "ER_ALLOW_MISSING_REGULATION"


def missing_regulation(path, what):
    """Report an absent regulation. Returns the exit code to use."""
    if os.environ.get(ALLOW_MISSING_REGULATION_ENV) == "1":
        print(
            f"SKIPPED: no regulation.bin at {path}, and {ALLOW_MISSING_REGULATION_ENV}=1. "
            f"{what} was NOT checked.",
            file=sys.stderr,
        )
        return 0
    print(f"FAIL: no regulation.bin at {path}", file=sys.stderr)
    print(
        f"      set ER_REGULATION or pass the path, or set {ALLOW_MISSING_REGULATION_ENV}=1 on a\n"
        f"      machine that cannot have the game. This exits 2 rather than passing, because\n"
        f"      {what} drifts SILENTLY and 'could not look' is not evidence of agreement.",
        file=sys.stderr,
    )
    return 2


def installed_regulation():
    """The regulation of the game installed for the current user.

    `ER_REGULATION` wins; otherwise the native-Linux Steam library under this user's
    home. Never a hard-coded `/home/<someone>`: this has to run for whoever checks the
    repo out, and a path that silently resolves to nothing reads as "the file is
    missing" instead of "you looked in the wrong place".
    """
    return os.environ.get("ER_REGULATION") or os.path.join(
        os.path.expanduser("~"),
        ".local/share/Steam/steamapps/common/ELDEN RING/Game/regulation.bin",
    )


def short_name(bnd_path):
    """`N:\\GR\\...\\SpEffectParam.param` -> `SpEffectParam`."""
    return bnd_path.rsplit("\\", 1)[-1].removesuffix(".param")


def param_stats(blob):
    """Structural fingerprint of one PARAM file, no paramdef required.

    Returns row count, the modal row stride, the paramdef data/format versions
    carried in the header, and the row-id list.
    """
    row_count = struct.unpack_from("<H", blob, 0x0A)[0]
    # SoulsFormats names the u16 at +0x08 ParamdefDataVersion: the revision of the PARAMDEF
    # this file's rows were built against. Corroborating evidence for a layout change, not
    # proof of one -- the row stride below is the direct measurement.
    paramdef_data_version = struct.unpack_from("<H", blob, 0x08)[0]
    # +0x2C..+0x2F are PARAM's format flag bytes (SoulsFormats Format2D/2E/2F/Unk2B), read as
    # one u16 purely as an invariant: if the container format changed, this changes.
    header_format_flags = struct.unpack_from("<H", blob, 0x2C)[0] if len(blob) > 0x2E else 0

    ids, offsets = [], []
    for index in range(row_count):
        entry = 0x40 + index * 24
        ids.append(struct.unpack_from("<i", blob, entry)[0])
        offsets.append(struct.unpack_from("<Q", blob, entry + 8)[0])

    # Stride from consecutive data offsets. Rows are stored contiguously in offset
    # order, so the modal delta is the row size; a single odd delta is the tail.
    stride = None
    if len(offsets) >= 2:
        ordered = sorted(offsets)
        deltas = Counter(b - a for a, b in zip(ordered, ordered[1:]))
        stride = deltas.most_common(1)[0][0]

    return {
        "row_count": row_count,
        "stride": stride,
        "paramdef_data_version": paramdef_data_version,
        "header_format_flags": header_format_flags,
        "ids": ids,
        "offsets": offsets,
        "size": len(blob),
    }


def row_bytes(blob, stats, row_id):
    """Raw bytes of one row, or None when the id is absent."""
    try:
        index = stats["ids"].index(row_id)
    except ValueError:
        return None
    start = stats["offsets"][index]
    return blob[start : start + (stats["stride"] or 0)]


def load(path):
    files = RP.bnd4_entries(RP.dcx_unpack(RP.decrypt(path)))
    return {short_name(name): blob for name, blob in files.items()}


def validate_effects_json(path, old_files, new_files):
    """Every `sp_effect` id in the catalog must still be a `SpEffectParam` row.

    This is the pure-python equivalent of `er-param-inspect validate`, which needs a
    Smithbox checkout and a dotnet bridge to answer the same question. Row existence
    needs no paramdef, so it needs neither -- which is what lets it run in a gate.

    `old_files` is optional. With a second regulation it also reports which referenced
    rows changed bytes between the two: an id that still exists but whose row was
    rebalanced is a silent behaviour change, not a validation failure, so it is reported
    without failing. Without one -- the shape a gate uses, since only the installed
    regulation is guaranteed to be on the machine -- the existence check still runs, and
    that is the check that fails the build.
    """
    import json

    with open(path, encoding="utf-8") as handle:
        calls = json.load(handle)["calls"]
    sp = [c for c in calls if c.get("kind") == "sp_effect"]
    if not sp:
        print(f"FAIL: {path} declares no sp_effect calls, so this check would prove nothing.")
        return 1

    n = param_stats(new_files["SpEffectParam"])
    new_ids = set(n["ids"])

    missing = [c for c in sp if c["id"] not in new_ids]

    print(f"{path}: {len(sp)} sp_effect calls")
    if old_files is None:
        print(f"SpEffectParam: {n['row_count']} rows, stride {n['stride']}, "
              f"paramdefDataVersion {n['paramdef_data_version']}")
        print("  (no baseline regulation given: row-byte changes were not compared)")
    else:
        o = param_stats(old_files["SpEffectParam"])
        old_ids = set(o["ids"])
        changed = []
        for call in sp:
            if call["id"] not in old_ids or call["id"] not in new_ids:
                continue
            if row_bytes(old_files["SpEffectParam"], o, call["id"]) != \
                    row_bytes(new_files["SpEffectParam"], n, call["id"]):
                changed.append(call)
        print(f"SpEffectParam: {o['row_count']} -> {n['row_count']} rows, "
              f"stride {o['stride']} -> {n['stride']}, "
              f"paramdefDataVersion {o['paramdef_data_version']} -> {n['paramdef_data_version']}")
        print(f"  rows removed between the two regulations: {len(old_ids - new_ids)}")
        print(f"  referenced rows whose bytes changed: {len(changed)}")
        for call in changed[:40]:
            print(f"    CHANGED {call['id']}  {call.get('name', '')}")

    print(f"  referenced ids MISSING from the regulation: {len(missing)}")
    for call in missing[:40]:
        print(f"    MISSING {call['id']}  {call.get('name', '')}")

    if missing:
        print("FAIL: the catalog references SpEffect ids that are not rows.")
        return 1
    print("OK: every referenced SpEffect id is still a row.")
    return 0


def main():
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("old", nargs="?", default=None,
                        help="baseline regulation.bin; optional with --effects-json")
    parser.add_argument("new", nargs="?", default=None,
                        help="regulation.bin to check (default with --effects-json: the "
                             "installed game, or $ER_REGULATION)")
    parser.add_argument("--param", action="append", default=[],
                        help="restrict to these params (repeatable)")
    parser.add_argument("--rows", action="append", default=[],
                        help="for this param, also diff the row-id sets (repeatable)")
    parser.add_argument("--row-id", action="append", type=int, default=[],
                        help="with --rows, byte-diff exactly these row ids")
    parser.add_argument("--quiet-identical", action="store_true",
                        help="omit params whose bytes are identical")
    parser.add_argument("--effects-json", metavar="PATH", nargs="?", const=DEFAULT_EFFECTS_JSON,
                        help="check every sp_effect id in this catalog against the NEW "
                             "regulation's SpEffectParam and exit non-zero on any miss "
                             "(defaults to data/effects.json)")
    args = parser.parse_args()

    # One positional means "check this one", not "diff it against itself": with
    # --effects-json the baseline is optional, so a lone path is the regulation to check.
    old, new = args.old, args.new
    if args.effects_json and new is None:
        old, new = None, old or installed_regulation()
    if new is None or (old is None and not args.effects_json):
        parser.error("a diff needs two regulations; only --effects-json runs on one")

    for path in (p for p in (old, new) if p is not None):
        if not os.path.exists(path):
            return missing_regulation(path, "the SpEffect id list in data/effects.json")

    new_files = load(new)
    old_files = load(old) if old is not None else None

    if args.effects_json:
        return validate_effects_json(args.effects_json, old_files, new_files)

    names = sorted(set(old_files) | set(new_files))
    if args.param:
        wanted = {p.removesuffix(".param") for p in args.param}
        names = [n for n in names if n in wanted]

    changed = []
    print(f"{'param':<42} {'rows':>13} {'stride':>13} {'pdefver':>11}  status")
    print("-" * 100)
    for name in names:
        old_blob, new_blob = old_files.get(name), new_files.get(name)
        if old_blob is None:
            print(f"{name:<42} {'-':>13} {'-':>13} {'-':>11}  ADDED in new")
            changed.append(name)
            continue
        if new_blob is None:
            print(f"{name:<42} {'-':>13} {'-':>13} {'-':>11}  REMOVED in new")
            changed.append(name)
            continue

        o, n = param_stats(old_blob), param_stats(new_blob)
        identical = old_blob == new_blob
        layout_changed = (o["stride"] != n["stride"]
                          or o["paramdef_data_version"] != n["paramdef_data_version"]
                          or o["header_format_flags"] != n["header_format_flags"])
        rows_changed = o["ids"] != n["ids"]

        if identical:
            status = "identical"
        elif layout_changed:
            status = "LAYOUT CHANGED"
        elif rows_changed:
            status = "rows changed"
        else:
            status = "values changed"

        if identical and args.quiet_identical:
            continue
        if not identical:
            changed.append(name)

        rows = (f"{o['row_count']}" if o["row_count"] == n["row_count"]
                else f"{o['row_count']}->{n['row_count']}")
        stride = (f"{o['stride']}" if o["stride"] == n["stride"]
                  else f"{o['stride']}->{n['stride']}")
        pdef = (f"{o['paramdef_data_version']}/{o['header_format_flags']}"
                if (o["paramdef_data_version"], o["header_format_flags"])
                == (n["paramdef_data_version"], n["header_format_flags"])
                else f"{o['paramdef_data_version']}/{o['header_format_flags']}"
                     f"->{n['paramdef_data_version']}/{n['header_format_flags']}")
        print(f"{name:<42} {rows:>13} {stride:>13} {pdef:>11}  {status}")

    print("-" * 100)
    print(f"{len(names)} params compared, {len(changed)} differ")

    for name in args.rows:
        stem = name.removesuffix(".param")
        if stem not in old_files or stem not in new_files:
            print(f"\n{stem}: missing from one side")
            continue
        o, n = param_stats(old_files[stem]), param_stats(new_files[stem])
        old_ids, new_ids = set(o["ids"]), set(n["ids"])
        added, removed = sorted(new_ids - old_ids), sorted(old_ids - new_ids)
        print(f"\n{stem}: {len(old_ids)} -> {len(new_ids)} rows; "
              f"+{len(added)} added, -{len(removed)} removed")
        if added:
            print(f"  added:   {added[:40]}{' ...' if len(added) > 40 else ''}")
        if removed:
            print(f"  removed: {removed[:40]}{' ...' if len(removed) > 40 else ''}")

        for row_id in args.row_id:
            ob = row_bytes(old_files[stem], o, row_id)
            nb = row_bytes(new_files[stem], n, row_id)
            if ob is None or nb is None:
                print(f"  row {row_id}: old={'present' if ob else 'ABSENT'} "
                      f"new={'present' if nb else 'ABSENT'}")
                continue
            if ob == nb:
                print(f"  row {row_id}: identical ({len(ob)} bytes)")
            else:
                diffs = [i for i, (a, b) in enumerate(zip(ob, nb)) if a != b]
                print(f"  row {row_id}: {len(diffs)} byte(s) differ at offsets "
                      f"{[hex(d) for d in diffs[:24]]}{' ...' if len(diffs) > 24 else ''}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
