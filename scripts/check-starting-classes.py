#!/usr/bin/env python3
"""Re-derive the starting-class table from the installed game and fail if the source disagrees.

`er-build-import-core`'s `STARTING_CLASSES` is a hand-written list indexed by
`PlayerGameData::archetype`. Nothing in Rust can notice when the GAME grows a class:
1.17 added `CharaInitParam` 3010/3011 ("Idus Knight", "Heavy Knight"), the list was a
`[&str; 10]`, and `class_for_archetype` answered `None` -- so build export dropped the
class and import never set one, with no panic and no log line either way.

This is the check that would have caught it, and it is deliberately dependency-free
(the same AES-256-CBC -> DCX/zstd -> BND4 -> PARAM stages as `regulation-params.py`;
no dotnet, no Smithbox, no paramdef) so it can sit in `scripts/check.sh`.

What it proves, from the game's own data:

  * `BaseChrSelectMenuParam`'s class rows (field 0 == 1) carry the `CharaInitParam` row
    id in field 2 and the `GR_MenuText` message id in field 4. Their COUNT is the number
    of starting classes -- compared against the Rust list's length.
  * Every archetype 0..N maps to `CharaInitParam` row 3000+archetype, and that row exists.
  * Row 3000+N does NOT exist -- the assertion the old doctest made as the literal
    `class_for_archetype(10) == None`, restated so a patch moves it instead of falsifying it.
  * The message id is `288100 + archetype` for every class row.
  * With an extracted `GR_MenuText.fmg.xml` reachable, the STRING at each of those ids
    equals the Rust list's name at that archetype -- the spelling, not just the count.

Usage:

    python3 scripts/check-starting-classes.py
    python3 scripts/check-starting-classes.py --regulation /path/to/regulation.bin
    python3 scripts/check-starting-classes.py --menu-fmg-xml /path/to/GR_MenuText.fmg.xml

Exit status: 0 agreement, 1 disagreement, 2 the check could not be run (missing
regulation). It never exits 0 for "I could not look" -- that failure mode is the whole
reason this file exists. The FMG name check is the one part that degrades to a warning,
because extracted assets live outside the repo and may legitimately be absent; the
structural checks above still run and still fail loudly.
"""

from __future__ import annotations

import argparse
import glob
import importlib.util
import os
import re
import struct
import sys
import xml.etree.ElementTree as ET
from collections import Counter

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(HERE)

CLASS_RS = os.path.join(REPO, "crates", "er-build-import-core", "src", "class.rs")

#: The one other place the class list is written down: a diagnostic that decodes a save slot's
#: archetype byte to a name. It was stuck at ten for the same reason `STARTING_CLASSES` was, so
#: it is checked here rather than left to drift out of sight.
DUMP_SAVE_SLOTS = os.path.join(REPO, "scripts", "dump-save-slots.py")

FIRST_CHARA_INIT_PARAM_ROW = 3000
FIRST_CLASS_NAME_MESSAGE_ID = 288100

#: `BaseChrSelectMenuParam` field 0: 1 marks a starting-class row, 0 a keepsake row.
CLASS_ROW_MARKER = 1

#: `CharaInitParam` row byte holding the class's starting level (`soulLv`).
SOUL_LEVEL_OFFSET = 192

#: First of the eight base attributes: vigour, mind, endurance, strength, dexterity,
#: intelligence, faith, arcane. Byte 193, between the two, is zero on every row.
ATTRIBUTES_OFFSET = 194

#: How many attributes a character has.
ATTRIBUTE_COUNT = 8

#: `stat_sum - level` for every starting class. Mirrors
#: `er_build_import_core::stats::CLASS_INVARIANT`; checked here against the game's own rows.
CLASS_INVARIANT = 79


def load_regulation_reader():
    """Import `regulation-params.py` despite the hyphen in its name."""
    path = os.path.join(HERE, "regulation-params.py")
    spec = importlib.util.spec_from_file_location("regulation_params", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


#: Explicit opt-out for an environment that genuinely cannot have the game installed (CI).
#: Set to 1 to downgrade a missing regulation from a failure to a PRINTED skip. Absent this,
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


def default_regulation() -> str:
    """The installed game's regulation, resolved for the current user.

    `ER_REGULATION` wins; otherwise the Steam library under this user's home. No
    hard-coded `/home/<someone>`: this script has to run for whoever checks out the repo.
    """
    env = os.environ.get("ER_REGULATION")
    if env:
        return env
    steam = os.path.join(
        os.path.expanduser("~"),
        ".local/share/Steam/steamapps/common/ELDEN RING/Game/regulation.bin",
    )
    return steam


def params(reader, path: str) -> dict[str, bytes]:
    """{param name: PARAM bytes} for one regulation."""
    blob = reader.dcx_unpack(reader.decrypt(path))
    return {
        name.rsplit("\\", 1)[-1].removesuffix(".param"): data
        for name, data in reader.bnd4_entries(blob).items()
    }


def rows(param: bytes) -> tuple[dict[int, bytes], int]:
    """{row id: row bytes} plus the modal row stride, with no paramdef involved."""
    count = struct.unpack_from("<H", param, 0x0A)[0]
    entries = []
    for index in range(count):
        base = 0x40 + index * 24
        entries.append(
            (
                struct.unpack_from("<i", param, base)[0],
                struct.unpack_from("<Q", param, base + 8)[0],
            )
        )
    offsets = sorted(offset for _, offset in entries)
    stride = Counter(b - a for a, b in zip(offsets, offsets[1:])).most_common(1)[0][0]
    return {rid: param[off : off + stride] for rid, off in entries}, stride


def rust_class_list(path: str) -> list[str]:
    """The names in `STARTING_CLASSES`, in order, straight out of the Rust source.

    Parsed rather than imported so this check needs no build, and so it reads exactly the
    text a reviewer sees.
    """
    with open(path, encoding="utf-8") as handle:
        source = handle.read()
    match = re.search(r"pub const STARTING_CLASSES\s*:[^=]*=\s*&?\[(.*?)\];", source, re.S)
    if match is None:
        raise SystemExit(f"could not find STARTING_CLASSES in {path}")
    body = re.sub(r"//[^\n]*", "", match.group(1))
    return re.findall(r'"([^"]*)"', body)


def rust_starting_stats(path: str) -> list[tuple[int, list[int]]] | None:
    """`STARTING_STATS` as [(starting level, [eight base attributes])], in archetype order.

    Parsed rather than imported, for the same reason `rust_class_list` is. Returns None
    when the const is not there, which the caller reports as a failure: this table stopped
    being test-only on 2026-09-06 and is now the per-attribute FLOOR the build importer
    holds a character to, so "could not look" must not read as "agreed".
    """
    with open(path, encoding="utf-8") as handle:
        source = handle.read()
    match = re.search(r"pub const STARTING_STATS\s*:[^=]*=\s*&?\[(.*?)\n\];", source, re.S)
    if match is None:
        return None
    body = re.sub(r"//[^\n]*", "", match.group(1))
    rows = []
    for level, attributes in re.findall(r"\(\s*(\d+)\s*,\s*\[([^\]]*)\]\s*\)", body):
        rows.append((int(level), [int(value) for value in re.findall(r"\d+", attributes)]))
    return rows


def rust_attribute_keys(path: str) -> list[str] | None:
    """The planner keys in `ATTRIBUTE_KEYS`, in the order they index `STARTING_STATS`."""
    with open(path, encoding="utf-8") as handle:
        source = handle.read()
    match = re.search(r"pub const ATTRIBUTE_KEYS\s*:[^=]*=\s*\[(.*?)\];", source, re.S)
    if match is None:
        return None
    return re.findall(r'"([^"]*)"', re.sub(r"//[^\n]*", "", match.group(1)))


def check_starting_stats(chara_rows, listed, class_rs, failures, quiet):
    """Re-read every base attribute out of `CharaInitParam` and compare against the Rust table.

    The row layout, byte for byte: 192 is the starting level (`soulLv`), 193 is zero on
    every row, and 194..=201 are the eight attributes in the game's order -- vigour, mind,
    endurance, strength, dexterity, intelligence, faith, arcane.

    Why this is a gate and not just a unit test: `STARTING_STATS` is what the build importer
    raises a sub-base attribute to (`er_build_import_core::stats::normalise`), and the
    native it then calls, `ApplyMainPlayerStats` @ 0x140788cf0, does no clamping of its own.
    A number wrong here is a character minted with an illegal stat and no complaint from the
    engine, so the numbers are re-derived from the installed game rather than trusted.
    """
    stats = rust_starting_stats(class_rs)
    if stats is None:
        failures.append(
            f"could not find STARTING_STATS in {os.path.relpath(class_rs, REPO)}; it is the "
            f"build importer's per-attribute floor and cannot go unchecked"
        )
        return

    keys = rust_attribute_keys(class_rs)
    if keys is None:
        failures.append(f"could not find ATTRIBUTE_KEYS in {os.path.relpath(class_rs, REPO)}")
    elif len(keys) != ATTRIBUTE_COUNT:
        failures.append(
            f"ATTRIBUTE_KEYS has {len(keys)} entries, not {ATTRIBUTE_COUNT}: it indexes "
            f"STARTING_STATS, so the two must be the same width"
        )
    elif len(set(keys)) != len(keys):
        failures.append(f"ATTRIBUTE_KEYS repeats a key: {keys}")

    if len(stats) != len(listed):
        failures.append(
            f"STARTING_STATS has {len(stats)} rows, STARTING_CLASSES has {len(listed)}. "
            f"One grew without the other."
        )

    for archetype, name in enumerate(listed):
        if archetype >= len(stats):
            break
        row_id = FIRST_CHARA_INIT_PARAM_ROW + archetype
        row = chara_rows.get(row_id)
        if row is None:
            continue  # already reported by the caller's row-existence check
        want_level, want_attributes = stats[archetype]
        got_level = row[SOUL_LEVEL_OFFSET]
        got_attributes = list(row[ATTRIBUTES_OFFSET : ATTRIBUTES_OFFSET + ATTRIBUTE_COUNT])
        if got_level != want_level:
            failures.append(
                f"archetype {archetype} ({name}): STARTING_STATS says starting level "
                f"{want_level}, CharaInitParam {row_id} byte {SOUL_LEVEL_OFFSET} says {got_level}"
            )
        if got_attributes != want_attributes:
            failures.append(
                f"archetype {archetype} ({name}): STARTING_STATS says base attributes "
                f"{want_attributes}, CharaInitParam {row_id} bytes "
                f"{ATTRIBUTES_OFFSET}..={ATTRIBUTES_OFFSET + ATTRIBUTE_COUNT - 1} say "
                f"{got_attributes}"
            )
        # The arithmetic invariant the importer derives every level from. Checked against the
        # GAME's numbers rather than the table's, so it is evidence and not a tautology.
        total = sum(got_attributes)
        if total - CLASS_INVARIANT != got_level:
            failures.append(
                f"archetype {archetype} ({name}): CharaInitParam {row_id} has attributes "
                f"totalling {total} and level {got_level}, but {total} - {CLASS_INVARIANT} = "
                f"{total - CLASS_INVARIANT}. The identity the build importer derives every "
                f"character's level from no longer holds."
            )

    if not quiet and not failures:
        print(f"STARTING_STATS:                    {len(stats)} rows, every base attribute agrees")


def python_archetype_map(path: str) -> list[str] | None:
    """The `ARCHETYPES` dict in `dump-save-slots.py`, as a list indexed by archetype.

    Returns None when the file or the dict is not there, which is reported as a warning
    rather than a failure: the gate's job is the product table, and this one rides along.
    """
    if not os.path.exists(path):
        return None
    with open(path, encoding="utf-8") as handle:
        source = handle.read()
    match = re.search(r"^ARCHETYPES\s*=\s*\{(.*?)^\}", source, re.S | re.M)
    if match is None:
        return None
    body = re.sub(r"#[^\n]*", "", match.group(1))
    pairs = {int(index): name for index, name in re.findall(r'(\d+)\s*:\s*"([^"]*)"', body)}
    if not pairs or sorted(pairs) != list(range(len(pairs))):
        return None
    return [pairs[index] for index in range(len(pairs))]


def menu_fmg_candidates(explicit: str | None) -> list[str]:
    """Extracted `GR_MenuText.fmg.xml` paths, most recently written first.

    Extracted game assets never live in this repo (they are game-derived binaries), so
    this walks an env-overridable root instead of hard-coding one person's extraction.
    A root routinely holds SEVERAL versions side by side -- the drift audit's own corpus
    has a `v1162` and a `v1170` tree with identical mtimes -- so this returns every
    candidate and the caller picks the one that can actually answer, rather than betting
    on a timestamp.
    """
    if explicit:
        return [explicit] if os.path.exists(explicit) else []
    direct = os.environ.get("ER_MENU_FMG_XML")
    if direct:
        return [direct] if os.path.exists(direct) else []
    root = os.environ.get("ER_MSG_EXTRACT_ROOT") or os.path.join(os.path.expanduser("~"), "er-extract")
    if not os.path.isdir(root):
        return []
    # Bounded, non-recursive globs rather than `**`: an extraction root holds tens of
    # thousands of unpacked assets and a recursive walk of it takes longer than this whole
    # check. Four levels reaches `<root>/<extraction>/witchy/<version>/menu-msgbnd-dcx/`.
    found: list[str] = []
    prefix = ""
    for _ in range(4):
        found += glob.glob(os.path.join(root, prefix, "menu-msgbnd-dcx", "GR_MenuText.fmg.xml"))
        prefix = os.path.join(prefix, "*") if prefix else "*"
    return sorted(set(found), key=os.path.getmtime, reverse=True)


def fmg_entries(path: str) -> dict[int, str]:
    out: dict[int, str] = {}
    for element in ET.parse(path).iter("text"):
        raw = element.get("id")
        if raw is None:
            continue
        out[int(raw)] = element.text or ""
    return out


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("--regulation", default=None, help="regulation.bin (default: the installed game)")
    parser.add_argument("--class-rs", default=CLASS_RS, help="the Rust source holding STARTING_CLASSES")
    parser.add_argument("--menu-fmg-xml", default=None, help="extracted GR_MenuText.fmg.xml for the name check")
    parser.add_argument("--quiet", action="store_true", help="print only failures")
    args = parser.parse_args()

    regulation = args.regulation or default_regulation()
    if not os.path.exists(regulation):
        return missing_regulation(regulation, "the starting-class table")

    listed = rust_class_list(args.class_rs)
    reader = load_regulation_reader()
    table = params(reader, regulation)

    for name in ("BaseChrSelectMenuParam", "CharaInitParam"):
        if name not in table:
            print(f"FAIL: {regulation} has no {name}", file=sys.stderr)
            return 2

    select_rows, select_stride = rows(table["BaseChrSelectMenuParam"])
    chara_rows, _ = rows(table["CharaInitParam"])

    failures: list[str] = []

    # field 0 marks the row kind; fields 2 and 4 are the CharaInitParam and GR_MenuText ids.
    classes: dict[int, int] = {}  # chara_init row -> message id
    for rid in sorted(select_rows):
        fields = struct.unpack_from("<8i", select_rows[rid], 0)
        if fields[0] != CLASS_ROW_MARKER:
            continue
        classes[fields[2]] = fields[4]

    if not args.quiet:
        print(f"regulation:            {regulation}")
        print(f"class table source:    {args.class_rs}")
        print(f"BaseChrSelectMenuParam class rows: {len(classes)} (stride {select_stride})")
        print(f"STARTING_CLASSES:                  {len(listed)}")

    if len(classes) != len(listed):
        failures.append(
            f"the game has {len(classes)} starting classes, STARTING_CLASSES lists {len(listed)}. "
            f"CharaInitParam rows in the game: {sorted(classes)}"
        )

    expected_rows = [FIRST_CHARA_INIT_PARAM_ROW + index for index in range(len(listed))]
    for index, row_id in enumerate(expected_rows):
        if row_id not in chara_rows:
            failures.append(f"archetype {index} ({listed[index]!r}) wants CharaInitParam {row_id}, which does not exist")
        if row_id not in classes:
            failures.append(f"archetype {index} ({listed[index]!r}) wants BaseChrSelectMenuParam to reference CharaInitParam {row_id}, which it does not")

    # The old doctest's `class_for_archetype(10) == None`, restated against live data so a
    # patch that adds a class breaks THIS instead of quietly making the literal wrong.
    past_the_end = FIRST_CHARA_INIT_PARAM_ROW + len(listed)
    if past_the_end in chara_rows:
        failures.append(
            f"CharaInitParam {past_the_end} exists, so the game has at least "
            f"{len(listed) + 1} starting classes and STARTING_CLASSES is short. "
            f"Its GR_MenuText id is {classes.get(past_the_end, FIRST_CLASS_NAME_MESSAGE_ID + len(listed))}"
        )

    # The base attributes themselves, not just which row belongs to which name. They became
    # product data on 2026-09-06 (the build importer's per-attribute floor), so they are gated.
    check_starting_stats(chara_rows, listed, args.class_rs, failures, args.quiet)

    for row_id, message_id in sorted(classes.items()):
        archetype = row_id - FIRST_CHARA_INIT_PARAM_ROW
        wanted = FIRST_CLASS_NAME_MESSAGE_ID + archetype
        if message_id != wanted:
            failures.append(
                f"CharaInitParam {row_id} (archetype {archetype}) names message {message_id}, "
                f"not {wanted}: the `288100 + archetype` rule the Rust side encodes no longer holds"
            )

    wanted_ids = [FIRST_CLASS_NAME_MESSAGE_ID + index for index in range(len(listed))]
    candidates = menu_fmg_candidates(args.menu_fmg_xml)
    entries: dict[int, str] = {}
    fmg_path: str | None = None
    for candidate in candidates:
        loaded = fmg_entries(candidate)
        if fmg_path is None:
            fmg_path, entries = candidate, loaded
        if all(message_id in loaded for message_id in wanted_ids):
            fmg_path, entries = candidate, loaded
            break

    if fmg_path is None:
        print(
            "WARNING: no extracted GR_MenuText.fmg.xml found, so class NAMES were not checked "
            "(only the count and the row mapping, which came from the installed regulation and "
            "did run). Set ER_MENU_FMG_XML or ER_MSG_EXTRACT_ROOT, or pass --menu-fmg-xml.",
            file=sys.stderr,
        )
    else:
        if not args.quiet:
            print(f"GR_MenuText.fmg.xml:   {fmg_path} ({len(entries)} entries)")
        for index, name in enumerate(listed):
            message_id = FIRST_CLASS_NAME_MESSAGE_ID + index
            actual = entries.get(message_id)
            if actual is None:
                # The regulation already proved this class exists, so a missing message id
                # says the extracted corpus predates the installed game -- not that the name
                # is wrong. Loud, but not a failure: the count check above is the one that
                # catches the drift, and it reads the installed game directly.
                print(
                    f"WARNING: message {message_id} (archetype {index}, {name!r}) is absent from "
                    f"{fmg_path}; that corpus is older than the installed game, so this name "
                    f"was not checked. Re-extract msg/engus to check spellings.",
                    file=sys.stderr,
                )
            elif actual != name:
                failures.append(
                    f"archetype {index}: STARTING_CLASSES says {name!r}, message {message_id} says {actual!r}"
                )

    mirror = python_archetype_map(DUMP_SAVE_SLOTS)
    if mirror is None:
        print(
            f"WARNING: could not read the ARCHETYPES map in {DUMP_SAVE_SLOTS}; the second copy "
            f"of the class list was not checked.",
            file=sys.stderr,
        )
    elif mirror != listed:
        failures.append(
            f"{os.path.basename(DUMP_SAVE_SLOTS)}'s ARCHETYPES map is {mirror}, which is not "
            f"STARTING_CLASSES ({listed}). The two copies have drifted apart."
        )
    elif not args.quiet:
        print(f"second copy in {os.path.relpath(DUMP_SAVE_SLOTS, REPO)}: agrees")

    if failures:
        print("", file=sys.stderr)
        for failure in failures:
            print(f"FAIL: {failure}", file=sys.stderr)
        print(
            f"\n{len(failures)} disagreement(s) between the installed game and "
            f"{os.path.relpath(args.class_rs, REPO)}.",
            file=sys.stderr,
        )
        return 1

    if not args.quiet:
        for index, name in enumerate(listed):
            message_id = FIRST_CLASS_NAME_MESSAGE_ID + index
            seen = entries.get(message_id)
            mark = "=" if seen == name else "?"
            print(
                f"  archetype {index:2d} -> CharaInitParam {FIRST_CHARA_INIT_PARAM_ROW + index}"
                f" -> GR_MenuText {message_id} {mark} {name!r}"
            )
        print(f"OK: {len(listed)} starting classes, and the game agrees on every one.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
