#!/usr/bin/env python3
"""Refuse a 1.16.2 -> 1.17 ledger that declares the same source address twice.

What this catches, and why nothing else did
-------------------------------------------
`er-game-base/build.rs` reads four address ledgers, concatenates them, and finishes with

    rows.sort_unstable();
    rows.dedup_by_key(|(old, _)| *old);

`sort_unstable` orders by the whole tuple, so among rows sharing a source the surviving one is the
one with the numerically smallest destination. That is not a decision anybody made; it is what
`dedup_by_key` does with input it was never promised was unique. Two duplicated sources were sitting
in the curated ledger on 2026-08-30 -- `0x1408c47c0` (lines 156 and 267) and `0x1409b72b0` (lines 29
and 76). Both pairs happened to agree on the destination, so the emitted maps were correct; had one
pair disagreed, the map would have silently taken the lower address and no gate in the tree would
have said a word.

Nothing gated this. `check-rva-alias-drift.py` gates Rust declarations, not ledger rows.
`check-1170-translation-collisions.py` gates a destination that is also somebody else's source --
a different defect. `verify-rva-map-1170.py` verifies a pair; it does not ask whether the pair was
written down twice. A duplicate declaration had already cost an agent a full session that same day,
which is why this is a gate and not a note.

Four rules, and the one that must not be widened
------------------------------------------------
  R1  conflict              one source, two different destinations, inside one ledger.
  R2  cross-ledger conflict one source, different destinations in two different ledgers. The call
                            map and the detour map are assembled from different subsets, so a
                            disagreement here can route a call to A and a five-byte MinHook patch
                            to B for the same address.
  R3  repeat declaration    one source on more than one row of a curated ledger, even in agreement.
  R4  duplicate line        a byte-identical row twice, in any ledger. Needs no column semantics,
                            so it cannot be defeated by a column moving.

R3 is deliberately not applied to a generated ledger, and widening it would make this gate red on
arrival for a reason that is not a defect. `select-needed-1170-rows.py` emits one row per declaring
NAME: an address the workspace names under four spellings gets four rows, identical but for the
label column. Measured 2026-08-30, before any edit: 39 such sources in `needed.tsv`, 39 in
`needed-verified.tsv`, 7 in `data.tsv` -- 85 legitimate repeats. In the curated ledger the third
column is a derivation, not a name, so a second row there is redundancy, and redundancy is where
drift hides: the two `0x1408c47c0` rows disagreed about whether its `.pdata` record is a chained
continuation (it is a root; `0x8c47c6` chains to it), and a reader had no way to know which line
was current.

Which LEDGERS, and what happens to one this file has never heard of
------------------------------------------------------------------
The ledger paths are parsed out of `crates/er-game-base/build.rs` at run time rather than
transcribed, so a fifth ledger appearing there is seen immediately. Whether a ledger is curated or
generated cannot be parsed -- and must not be guessed. Header sniffing looks like it would work and
does not: `needed-verified.tsv`'s header contains the words "the curated ledger" inside a sentence
saying it is not one. So the classification is an explicit table below, and a ledger constant found
in `build.rs` that this table does not classify stops the run (exit 2) instead of being skipped. A
partial view reporting zero duplicates is this defect class wearing a green tick.

Usage
    python3 scripts/check-no-duplicate-ledger-rows.py             # the gate
    python3 scripts/check-no-duplicate-ledger-rows.py --rows      # also list every legitimate repeat
    python3 scripts/check-no-duplicate-ledger-rows.py --selftest  # positive controls, on real data
"""

from __future__ import annotations

import argparse
import collections
import os
import re
import shutil
import sys
import tempfile

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BUILD_RS = os.path.join(REPO, "crates", "er-game-base", "build.rs")
BASE = 0x140000000

# How a ledger is allowed to repeat a source address.
CURATED = "curated"  # one row per source, full stop
GENERATED = "generated"  # one row per declaring name, so repeats are expected
SINGLE_COLUMN = "single-column"  # `quarantined()` reads column 0 only; a repeat is inert

# Keyed on BASENAME, because build.rs spells the paths relative to its own crate dir.
LEDGER_KIND = {
    "rva-map-1162-to-1170.verified.tsv": CURATED,
    "rva-map-1162-to-1170.needed.tsv": GENERATED,
    "rva-map-1162-to-1170.needed-verified.tsv": GENERATED,
    "rva-map-1162-to-1170.data.tsv": GENERATED,
    "rva-1170-quarantine.tsv": SINGLE_COLUMN,
}

# `const NAME: &str = "../../docs/recon/whatever.tsv";` in build.rs.
LEDGER_CONST = re.compile(r'const\s+([A-Z0-9_]+)\s*:\s*&str\s*=\s*"([^"]*\.tsv)"')


def ledger_paths() -> tuple[list[tuple[str, str, str]], list[str]]:
    """`([(const_name, abs_path, kind)], [unclassified_const_names])`, read out of build.rs.

    Parsed, never transcribed. `check-1170-translation-collisions.py` had to learn the same lesson
    from the other side: a copied `EXHAUSTIVE_VERDICTS` made one tool report a 374-row table as 42
    rows, confidently.
    """
    with open(BUILD_RS, encoding="utf-8", errors="replace") as handle:
        text = handle.read()
    found, unknown = [], []
    for const_name, relative in LEDGER_CONST.findall(text):
        basename = os.path.basename(relative)
        kind = LEDGER_KIND.get(basename)
        if kind is None:
            unknown.append(f"{const_name} -> {relative}")
            continue
        absolute = os.path.normpath(
            os.path.join(os.path.dirname(BUILD_RS), relative)
        )
        found.append((const_name, absolute, kind))
    return found, unknown


class Row:
    """One ledger row, normalised to RVAs so a VA-spelled table compares with an RVA-spelled one."""

    __slots__ = ("line_no", "source", "destination", "text")

    def __init__(self, line_no: int, source: int, destination: int, text: str):
        self.line_no = line_no
        self.source = source
        self.destination = destination
        self.text = text


def read_rows(path: str) -> list[Row]:
    """Every pair row of one ledger. Comments, blanks and non-pair lines are skipped.

    The two spellings both occur and both are accepted: `verified.tsv` and `needed-verified.tsv`
    write full VAs (`0x1408c47c0`), `needed.tsv` and `data.tsv` write RVAs (`0x8c47c0`). Anything
    at or above the preferred image base is treated as a VA, exactly as `build.rs` and
    `select-needed-1170-rows.py` do.
    """
    rows: list[Row] = []
    if not os.path.isfile(path):
        return rows
    with open(path, encoding="utf-8", errors="replace") as handle:
        for line_no, line in enumerate(handle.read().splitlines(), 1):
            stripped = line.strip()
            if not stripped or stripped.startswith("#"):
                continue
            fields = stripped.split("\t")
            if len(fields) < 2:
                continue
            try:
                source = int(fields[0].strip(), 16)
                destination = int(fields[1].strip(), 16)
            except ValueError:
                continue
            rows.append(
                Row(
                    line_no,
                    source - BASE if source >= BASE else source,
                    destination - BASE if destination >= BASE else destination,
                    line.rstrip(),
                )
            )
    return rows


def check_ledgers(ledgers: list[tuple[str, str, str]]) -> tuple[list[str], dict]:
    """`(findings, stats)`. A finding is a line to print; an empty list is a green gate."""
    findings: list[str] = []
    stats = {"rows": 0, "sources": 0, "legitimate_repeats": []}
    by_source_globally: dict[int, dict[str, set[int]]] = collections.defaultdict(dict)

    for const_name, path, kind in ledgers:
        if kind == SINGLE_COLUMN:
            continue
        rows = read_rows(path)
        stats["rows"] += len(rows)
        shown = os.path.relpath(path, REPO)

        # R4 -- a byte-identical row twice. No column semantics, so nothing about it can rot.
        seen_text: dict[str, int] = {}
        for row in rows:
            first = seen_text.get(row.text)
            if first is None:
                seen_text[row.text] = row.line_no
                continue
            findings.append(
                f"DUPLICATE LINE  {shown}:{row.line_no} repeats line {first} verbatim\n"
                f"    {row.text}"
            )

        by_source: dict[int, list[Row]] = collections.defaultdict(list)
        for row in rows:
            by_source[row.source].append(row)
        for source, group in sorted(by_source.items()):
            destinations = {row.destination for row in group}
            by_source_globally[source][const_name] = destinations
            if len(destinations) > 1:
                # R1 -- the dangerous one. build.rs sorts and dedups by source, so the smallest
                # destination wins by accident and the other row vanishes with no diagnostic.
                lowest = min(destinations)
                findings.append(
                    f"CONFLICT        {shown} maps 0x{source:x} to "
                    + " and ".join(f"0x{d:x}" for d in sorted(destinations))
                    + f"\n    build.rs would silently keep 0x{lowest:x}: it sorts by (source,"
                    f" destination) and dedups by source, so the LOWER destination wins and the"
                    f" other row leaves no trace."
                    + "".join(
                        f"\n    line {row.line_no}: {row.text}" for row in group
                    )
                )
            elif len(group) > 1:
                if kind == CURATED:
                    # R3 -- agreement is not safety here: the two rows carry two derivations, and
                    # nothing says which is current.
                    findings.append(
                        f"REPEAT DECL     {shown} declares 0x{source:x} on "
                        f"{len(group)} rows (all -> 0x{group[0].destination:x})\n"
                        f"    This is the CURATED ledger: its third column is a DERIVATION, not a"
                        f" declaring name, so a second row is redundancy, and redundancy is where"
                        f" drift hides. Merge the derivations into one row."
                        + "".join(
                            f"\n    line {row.line_no}: {row.text}" for row in group
                        )
                    )
                else:
                    stats["legitimate_repeats"].append(
                        (shown, source, group[0].destination, len(group))
                    )

    stats["sources"] = len(by_source_globally)
    # R2 -- across ledgers. The call map and the detour map are built from different subsets, so a
    # disagreement between two files can send a call to one address and MinHook's patch to another.
    for source, per_ledger in sorted(by_source_globally.items()):
        everywhere = set().union(*per_ledger.values())
        if len(everywhere) > 1:
            findings.append(
                f"CROSS-LEDGER    0x{source:x} has different destinations in different ledgers\n"
                + "".join(
                    f"\n    {name}: "
                    + ", ".join(f"0x{d:x}" for d in sorted(dests))
                    for name, dests in sorted(per_ledger.items())
                )
            )
    return findings, stats


# --------------------------------------------------------------------------------------------
# Selftest. Every control below runs the real `check_ledgers` over a copy of the real tracked
# ledgers, so a rule that stopped matching the files as they are actually written fails here.
#
# Why a copy and not the tracked file itself. Planting into the tracked file and restoring it is
# the stronger proof and was rejected on purpose: roughly a dozen agents are editing these exact
# ledgers concurrently, and a plant/restore pair that loses the race overwrites somebody's row. The
# copy carries the real file's real bytes, so the control is over real data either way; what it
# gives up is proof that the gate reads that particular path, which `ledger_paths()` covers
# separately by parsing the paths out of build.rs.
# --------------------------------------------------------------------------------------------


def _selftest_copy(tmp: str, ledgers) -> list[tuple[str, str, str]]:
    """Copy every real ledger into `tmp`, keeping basenames so LEDGER_KIND still applies."""
    out = []
    for const_name, path, kind in ledgers:
        destination = os.path.join(tmp, os.path.basename(path))
        if os.path.isfile(path):
            shutil.copyfile(path, destination)
        else:
            open(destination, "w", encoding="utf-8").close()
        out.append((const_name, destination, kind))
    return out


def _pick(ledgers, kind: str):
    for const_name, path, ledger_kind in ledgers:
        if ledger_kind == kind and read_rows(path):
            return const_name, path, ledger_kind
    return None


def _append(path: str, line: str) -> None:
    with open(path, "a", encoding="utf-8") as handle:
        handle.write(line if line.endswith("\n") else line + "\n")


def _drop_last_line(path: str) -> None:
    with open(path, encoding="utf-8") as handle:
        text = handle.read()
    body = text.rstrip("\n").split("\n")
    with open(path, "w", encoding="utf-8") as handle:
        handle.write("\n".join(body[:-1]) + "\n")


def selftest() -> int:
    failures: list[str] = []
    ledgers, unknown = ledger_paths()
    if unknown:
        failures.append(f"build.rs names ledgers this script cannot classify: {unknown}")
    if len(ledgers) < 4:
        failures.append(
            f"only {len(ledgers)} ledger constant(s) parsed out of build.rs; the regex is not "
            "seeing them"
        )
    if not any(kind == CURATED for _n, _p, kind in ledgers):
        failures.append("no CURATED ledger found; R3 would be unexercised on real data")

    with tempfile.TemporaryDirectory() as tmp:
        copies = _selftest_copy(tmp, ledgers)

        # Negative control, and it runs first: the tree as it stands must be green. A gate that
        # cannot be green is deleted rather than obeyed.
        baseline, stats = check_ledgers(copies)
        if baseline:
            failures.append(
                "the tracked ledgers are not clean, so no positive control below can prove "
                f"anything: {baseline[0].splitlines()[0]}"
            )
        if stats["rows"] < 400:
            failures.append(
                f"only {stats['rows']} rows read from the real ledgers; the parser is not seeing "
                "them, and an empty read is green"
            )

        curated = _pick(copies, CURATED)
        generated = _pick(copies, GENERATED)
        if curated is None or generated is None:
            failures.append("no populated curated/generated ledger copy to plant into")
            for line in failures:
                print(f"SELFTEST FAIL: {line}")
            print(f"selftest: {len(failures)} failure(s)")
            return 1 if failures else 0

        curated_rows = read_rows(curated[1])
        generated_rows = read_rows(generated[1])
        victim = curated_rows[0]
        generated_victim = generated_rows[0]

        # R3 positive control -- the exact defect found on 2026-08-30: a real curated row, repeated
        # verbatim except for its derivation prose, agreeing on the destination.
        repeat = victim.text.split("\t")
        repeat[5 if len(repeat) > 5 else len(repeat) - 1] = "planted repeat declaration"
        _append(curated[1], "\t".join(repeat))
        found, _ = check_ledgers(copies)
        if not any(f.startswith("REPEAT DECL") for f in found):
            failures.append(
                f"R3: a second declaration of 0x{victim.source:x} in the curated ledger was not "
                "flagged"
            )
        _drop_last_line(curated[1])
        if check_ledgers(copies)[0]:
            failures.append("R3: removing the plant did not return the gate to green")

        # R1 positive control -- same source, different destination. This is the one build.rs
        # resolves by accident.
        conflicting = victim.text.split("\t")
        conflicting[1] = f"0x{victim.destination + 0x1000:x}"
        _append(curated[1], "\t".join(conflicting))
        found, _ = check_ledgers(copies)
        if not any(f.startswith("CONFLICT") for f in found):
            failures.append(f"R1: a conflicting destination for 0x{victim.source:x} was not flagged")
        _drop_last_line(curated[1])
        if check_ledgers(copies)[0]:
            failures.append("R1: removing the plant did not return the gate to green")

        # R1 in a generated ledger too -- R3's exemption must not exempt a conflict as well.
        conflicting = generated_victim.text.split("\t")
        conflicting[1] = f"0x{generated_victim.destination + 0x1000:x}"
        _append(generated[1], "\t".join(conflicting))
        found, _ = check_ledgers(copies)
        if not any(f.startswith("CONFLICT") for f in found):
            failures.append(
                "R1: a conflicting destination in a GENERATED ledger was not flagged, so the R3 "
                "exemption is swallowing conflicts too"
            )
        _drop_last_line(generated[1])

        # R3 false-positive control -- the reason R3 stops at curated ledgers. A second row for an
        # existing source under a different declaring name is what the generator emits by design;
        # flagging it would make the gate red on 85 rows that are not defects.
        second_name = generated_victim.text.split("\t")
        second_name[-1] = "PLANTED_SECOND_NAME_RVA"
        _append(generated[1], "\t".join(second_name))
        found, _ = check_ledgers(copies)
        if found:
            failures.append(
                "R3 false-positive control: a second DECLARING NAME for an already-mapped address "
                f"in a generated ledger was flagged: {found[0].splitlines()[0]}"
            )
        _drop_last_line(generated[1])

        # R4 positive control -- a byte-identical repeat, which no column semantics are needed to
        # see and which therefore survives any column moving.
        _append(generated[1], generated_victim.text)
        found, _ = check_ledgers(copies)
        if not any(f.startswith("DUPLICATE LINE") for f in found):
            failures.append("R4: a byte-identical repeated row was not flagged")
        _drop_last_line(generated[1])

        # R2 positive control -- the same source, mapped differently by two different ledgers.
        cross = [
            f"0x{BASE + generated_victim.source:x}",
            f"0x{BASE + generated_victim.destination + 0x2000:x}",
            "IDENTICAL-WHOLE",
            "1.000",
            "99",
            "planted cross-ledger disagreement",
            "BOTH-ENTRIES",
            "PDATA:0x10/0x10",
        ]
        _append(curated[1], "\t".join(cross))
        found, _ = check_ledgers(copies)
        if not any(f.startswith("CROSS-LEDGER") for f in found):
            failures.append(
                "R2: two ledgers disagreeing about one source's destination were not flagged"
            )
        _drop_last_line(curated[1])
        if check_ledgers(copies)[0]:
            failures.append("R2: removing the plant did not return the gate to green")

    for line in failures:
        print(f"SELFTEST FAIL: {line}")
    print(f"selftest: {len(failures)} failure(s)")
    return 1 if failures else 0


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("--rows", action="store_true", help="also list every legitimate repeat")
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    if args.selftest:
        return selftest()

    ledgers, unknown = ledger_paths()
    if unknown:
        print(
            "[dup-ledger] REFUSING to answer: er-game-base/build.rs names a ledger this gate does "
            "not classify as curated or generated:"
        )
        for line in unknown:
            print(f"    {line}")
        print(
            "  Add it to LEDGER_KIND in this script. Skipping it would let this gate report zero "
            "duplicates over a partial view, which is the failure mode it exists to prevent."
        )
        return 2

    findings, stats = check_ledgers(ledgers)
    if args.rows:
        for shown, source, destination, count in stats["legitimate_repeats"]:
            print(f"  repeat (by design) {shown}  0x{source:x} -> 0x{destination:x}  x{count}")
    for finding in findings:
        print(f"[dup-ledger] {finding}")
    if findings:
        print(
            f"[dup-ledger] {len(findings)} duplicate/conflicting ledger row(s). "
            "er-game-base/build.rs sorts by (source, destination) and dedups by source, so a "
            "conflicting pair is resolved by picking the LOWER destination -- silently, with no "
            "diagnostic anywhere. Collapse each source to one row."
        )
        return 1
    print(
        f"[dup-ledger] ok -- {stats['rows']} rows, {stats['sources']} distinct source addresses, "
        f"no source declared twice in a curated ledger and no conflicting destination "
        f"({len(stats['legitimate_repeats'])} multi-name repeats in the generated ledgers are "
        "expected; --rows lists them)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
