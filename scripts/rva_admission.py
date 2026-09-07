#!/usr/bin/env python3
"""The 1.16.2 -> 1.17 verdict admission rules, and the emptiness assertion that keeps a gate honest.

Why this file exists
--------------------
`crates/er-game-base/build.rs` decides which rows of a `verify-rva-map-1170.py` verdict table are
good enough to call and to detour. Several audits under `scripts/` reproduce that decision so they
can report on it. Every one of them that transcribed the rules instead of reading them has since
been wrong:

  * the verdict vocabulary grew from a bare `IDENTICAL` to `BYTE-IDENTICAL` / `IDENTICAL-WHOLE` /
    `IDENTICAL-LEAF` / `IDENTICAL-SHORT` / `IDENTICAL-PREFIX`. A gate still comparing against the
    literal `"IDENTICAL"` now matches nothing, and `docs/recon/rva-map-1162-to-1170.verified.tsv`
    contains that exact string zero times out of 101 rows;
  * one such transcription reported the detour table as 42 rows instead of 374;
  * `select-needed-1170-rows.py::verified_rvas` returned the empty set for the whole of 2026-08-30,
    so its documented "the verified map wins wherever both cover an address" rule was not applied
    at all.

VACUOUS QUANTIFICATION, which is the class all of those belong to
----------------------------------------------------------------
"No element of S has property P" is TRIVIALLY true when S is empty. A gate that filters rows and
then asserts something about what survived cannot, on its own, tell "checked 800 rows, all fine"
apart from "checked zero rows". Both exit 0. Both print a green tick. Only one of them looked.

So a gate here does two things it did not do before:

  1. it RE-derives the vocabulary (`rules()` below) rather than spelling it out, so a rename in
     build.rs moves the gate with it instead of silently emptying it; and
  2. it asserts the filtered set is non-empty, and non-trivially so, before asserting anything
     about its contents (`nonempty`, `admit_rows`). A gate that legitimately has nothing to check
     must say so loudly -- an empty scope is a finding about the audit, not a pass.

The one PARSER
--------------
`rules()` does not parse `build.rs` itself. It calls `check-1170-translation-collisions.py`'s
`build_rules()`, which already does it properly -- reading the ledger paths, the instruction floor,
`EXHAUSTIVE_VERDICTS`, `DETOURABLE_ENTRY_EVIDENCE`, the field indices, and (the part every stale
gate got wrong) the prefix verdict out of `detourable_pairs`'s match arm, where it is spelled as a
bare literal rather than a constant. Adding a second parser here would recreate the duplication
this module exists to delete. If that file is missing or its parse of build.rs fails, `rules()`
RAISES: four gates going red together because the rules can no longer be read is correct, and is
strictly better than four gates quietly auditing an empty set.

Usage:
    python3 scripts/rva_admission.py            # print the rules as read today
    python3 scripts/rva_admission.py --selftest # negative control: a filter that matches nothing
                                                # must go red, not green
"""

from __future__ import annotations

import argparse
import collections
import importlib.util
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
REFERENCE = os.path.join(ROOT, "scripts", "check-1170-translation-collisions.py")

# A filtered set may not fall below this fraction of the rows it was filtered from before the gate
# refuses to draw a conclusion. It is a floor on coverage, derived from the input on every run, not
# a tuned row count that has to be edited whenever a ledger grows -- and not a number any table can
# be nudged past, because the only way to satisfy it is for the filter to actually recognise the
# data. Today's real tables sit at 98% (99/101) and 99.7% (339/340); a vocabulary rename drops them
# to 0%. Anything in between is drift worth stopping on.
MIN_ADMITTED_FRACTION = 0.10


class Vacuous(Exception):
    """A gate's filtered set is empty or degenerate, so any claim about its contents is unearned."""


class Unreadable(Exception):
    """The admission rules cannot be read, so the gate must not guess at them."""


_CACHE: dict[str, object] = {}


def rules(build_rs: str | None = None) -> dict:
    """Every admission rule `emit_address_map` applies, read from the source that applies them.

    Delegates to `check-1170-translation-collisions.py::build_rules`, the one parser. Returns its
    dict unchanged: `exhaustive`, `patch_site`, `prefix_verdict`, `entry_evidence`, `refuted_verdict`,
    `min_insns`, `min_columns`, `verdict_column`, `insns_column`, `entry_column`, `paths`, ...
    """
    key = build_rs or "<default>"
    if key in _CACHE:
        return _CACHE[key]  # type: ignore[return-value]
    if not os.path.isfile(REFERENCE):
        raise Unreadable(
            f"{os.path.relpath(REFERENCE, ROOT)} is gone, and it is the only parser of\n"
            "  crates/er-game-base/build.rs's admission rules. Restore it rather than\n"
            "  re-transcribing the verdict vocabulary here: a copied vocabulary is what made four\n"
            "  gates audit the empty set."
        )
    spec = importlib.util.spec_from_file_location("_er_rva_admission_reference", REFERENCE)
    if spec is None or spec.loader is None:
        raise Unreadable(f"cannot load {os.path.relpath(REFERENCE, ROOT)} as a module")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    try:
        parsed = module.build_rules(build_rs) if build_rs else module.build_rules()
    except module.Refuse as refusal:  # noqa: F841 - re-raised with the reason attached
        raise Unreadable(
            f"the admission rules could not be read out of build.rs:\n  {refusal}"
        ) from refusal
    for required in ("exhaustive", "patch_site", "prefix_verdict", "entry_evidence", "min_insns"):
        if not parsed.get(required):
            raise Unreadable(
                f"build.rs parsed but {required!r} came back EMPTY. Every row would be refused and "
                "the gate would report a clean, meaningless zero."
            )
    _CACHE[key] = parsed
    return parsed


def nonempty(label: str, items, *, at_least: int = 1, out_of: int | None = None, why: str = ""):
    """Assert `items` is worth drawing a conclusion from, and return it. Raises `Vacuous` if not.

    This is the assertion that has to come before "and none of them is bad". Call it on the set the
    gate is about to quantify over, never on the findings the quantification produced -- an empty
    findings list is the good outcome; an empty input list is the bug.
    """
    count = len(items)
    if count >= at_least and not (out_of and count < out_of * MIN_ADMITTED_FRACTION):
        return items
    detail = f"{count} item(s)"
    if out_of is not None:
        detail += f" out of {out_of} input row(s)"
        if out_of:
            detail += f" ({count / out_of:.1%}; floor is {MIN_ADMITTED_FRACTION:.0%})"
    raise Vacuous(
        f"{label}: {detail} -- REFUSING to draw a conclusion.\n"
        "  'No element of this set is bad' is trivially true of an empty set, so a green tick here\n"
        "  would mean nothing was examined, not that nothing was wrong.\n"
        + (f"  {why}\n" if why else "")
        + "  Either the filter no longer recognises the data (check the verdict vocabulary against\n"
        "  crates/er-game-base/build.rs), or the audit's scope really is empty -- which is itself\n"
        "  a finding to report, not to smooth over."
    )


def admits(fields, rule_set) -> bool:
    """`build.rs::detourable_pairs`'s row test, with every literal re-derived rather than spelled.

    One predicate, so the four audits that reproduce this decision cannot drift apart from it or
    from build.rs one at a time -- which is exactly how three of them ended up carrying a verdict
    word the ledgers stopped writing.
    """
    if len(fields) < rule_set["min_columns"]:
        return False
    verdict = fields[rule_set["verdict_column"]]
    if verdict in rule_set["exhaustive"]:
        pass  # the whole of both bodies was compared; there is no prefix left to doubt
    elif verdict in rule_set["patch_site"]:
        # The whole of both bodies was compared and they differ -- somewhere the detour never
        # reaches. The floor is a proxy for coverage and there is nothing left for it to insure.
        pass
    elif verdict == rule_set["prefix_verdict"]:
        # A claim about a prefix of unknown remainder, so how much of it agreed is the question.
        try:
            if int(fields[rule_set["insns_column"]].strip()) < rule_set["min_insns"]:
                return False
        except ValueError:
            return False
    else:
        return False
    return fields[rule_set["entry_column"]].strip() in rule_set["entry_evidence"]


def table_rows(path: str) -> list[list[str]]:
    """Every non-comment, non-blank row of a TSV ledger, split into fields."""
    if not os.path.isfile(path):
        return []
    out = []
    with open(path, encoding="utf-8", errors="replace") as handle:
        for line in handle:
            if line.startswith("#") or not line.strip():
                continue
            out.append(line.rstrip("\n").split("\t"))
    return out


def admit_rows(path: str, rule_set, *, label: str | None = None, require: bool = True):
    """`(admitted rows, unrecognised-verdict tally)` for one ledger, refusing a vacuous result.

    The tally is the diagnosis: when a table has rows but none were admitted, the words it actually
    carries are what the reader needs, and printing them turns a mute empty set into the name of
    the thing that changed.
    """
    everything = table_rows(path)
    admitted, unknown = [], collections.Counter()
    for fields in everything:
        if admits(fields, rule_set):
            admitted.append(fields)
        elif len(fields) > rule_set["verdict_column"]:
            unknown[fields[rule_set["verdict_column"]]] += 1
    if require and everything:
        nonempty(
            label or f"admitted rows of {os.path.relpath(path, ROOT)}",
            admitted,
            out_of=len(everything),
            why=(
                "the verdict words this table actually carries are: "
                + ", ".join(f"{word}x{count}" for word, count in unknown.most_common(8))
                if unknown
                else ""
            ),
        )
    return admitted, unknown


# ------------------------------------------------------------------------------------------------
# selftest
# ------------------------------------------------------------------------------------------------


def _synthetic(directory: str, verdict: str, rows: int = 8) -> str:
    """A verdict table whose every row carries `verdict`, written where a test can point a gate."""
    os.makedirs(directory, exist_ok=True)
    path = os.path.join(directory, "table.tsv")
    with open(path, "w", encoding="utf-8") as handle:
        handle.write("# 1.16.2\t1.17\tverdict\tnote\tinsns\tstop\tentry\n")
        for index in range(rows):
            old = 0x140100000 + index * 0x100
            new = 0x140200000 + index * 0x100
            handle.write(f"0x{old:x}\t0x{new:x}\t{verdict}\t-\t99\textent\tBOTH-ENTRIES\n")
    return path


def selftest() -> int:
    import tempfile

    failures: list[str] = []

    def check(what, got, want):
        if got != want:
            failures.append(f"{what}: got {got!r}, wanted {want!r}")

    live = rules()
    check("the prefix verdict is READ, not assumed", live["prefix_verdict"], "IDENTICAL")
    check(
        "the exhaustive vocabulary is read from build.rs",
        "IDENTICAL-WHOLE" in live["exhaustive"],
        True,
    )

    with tempfile.TemporaryDirectory() as scratch:
        # POSITIVE: a table written in the vocabulary build.rs admits is admitted.
        good = _synthetic(os.path.join(scratch, "good"), live["exhaustive"][0])
        admitted, _ = admit_rows(good, live)
        check("a table in the live vocabulary is admitted whole", len(admitted), 8)

        # The negative control for this class. Not "plant a finding and see it caught" -- that
        # tests the wrong thing. Make the filter match zero rows, by feeding it a verdict word the
        # vocabulary cannot contain, and the gate must go red. Before `nonempty` existed, this
        # returned [] and every downstream "none of them is bad" passed.
        blind = _synthetic(os.path.join(scratch, "blind"), "IDENTICAL-SHORT")
        try:
            admit_rows(blind, live, label="negative control")
            failures.append(
                "A FILTER THAT MATCHED ZERO OF 8 ROWS WAS ACCEPTED. This is the vacuity bug itself: "
                "the gate would report a clean audit of nothing."
            )
        except Vacuous as refusal:
            check(
                "the refusal names the verdict word the table really carries",
                "IDENTICAL-SHORT" in str(refusal),
                True,
            )

        # ...and the same again for the exact historical spelling, so the regression that started
        # all of this cannot come back wearing its original clothes.
        stale = _synthetic(os.path.join(scratch, "stale"), "IDENTICAL-PREFIX")
        try:
            admit_rows(stale, live, label="negative control (IDENTICAL-PREFIX)")
            failures.append("an IDENTICAL-PREFIX-only table was accepted as a non-vacuous audit")
        except Vacuous:
            pass

        # A table that is empty to begin with is not the same defect and must not be forced to
        # fail: there is nothing to mis-read. `require` still refuses when rows exist.
        empty = os.path.join(scratch, "empty.tsv")
        with open(empty, "w", encoding="utf-8") as handle:
            handle.write("# nothing here\n")
        admitted, _ = admit_rows(empty, live)
        check("an genuinely empty ledger is not misreported as a filter failure", admitted, [])

    # `nonempty` itself: the floor is a fraction of the input, so it cannot be satisfied by a
    # table that merely got bigger.
    try:
        nonempty("fraction floor", list(range(5)), out_of=1000)
        failures.append("5 admitted rows out of 1000 passed the coverage floor")
    except Vacuous:
        pass
    nonempty("a healthy fraction passes", list(range(99)), out_of=101)

    for line in failures:
        print(f"SELFTEST FAIL: {line}", file=sys.stderr)
    if failures:
        return 1
    print("selftest: OK (negative control observed RED on a zero-matching filter)")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    if args.selftest:
        return selftest()
    live = rules()
    print("crates/er-game-base/build.rs, as read right now:")
    for name in (
        "exhaustive",
        "patch_site",
        "prefix_verdict",
        "entry_evidence",
        "refuted_verdict",
        "min_insns",
    ):
        print(f"  {name:16s} {live[name]!r}")
    for name in ("min_columns", "verdict_column", "insns_column", "entry_column"):
        print(f"  {name:16s} {live[name]}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
