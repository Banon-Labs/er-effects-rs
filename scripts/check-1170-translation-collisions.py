#!/usr/bin/env python3
"""Refuse a 1.16.2 -> 1.17 row whose destination is also some other row's source.

What goes wrong, in the one sentence that matters
-------------------------------------------------
Translating an address twice does not fail. It succeeds, and returns a third, unrelated
function.

`er-game-base/build.rs` emits a table keyed by 1.16.2 RVA whose values are 1.17 RVAs. Given a
bare address the table cannot tell which side of the arrow it came from -- an address is just an
address. So if `A -> B` is a row and `B -> C` is also a row, then resolving A gives B (correct),
and resolving B gives C (wrong, and silent). No error, no log line, no refusal: the second lookup
hits a real entry and hands back a confident answer for a function nobody asked about. A detour
installed on C writes five bytes into an unrelated live body; a call through C enters it.

Measured, 2026-08-30. `er-reload-trace`'s `native_submit_7ac890` resolved `0x7ac890 -> 0x7ad710`
through `register_shared_hook`, which handed the resolved address to the product's union register,
which resolved again -- and `0x7ad710` is itself a tracked source, `-> 0x7ae590`. Both rows are
verdict-clean (`BYTE-IDENTICAL`, `BOTH-ENTRIES`), so nothing anywhere had a reason to complain.
That call path has since been restructured so the address travels unresolved and each branch
resolves exactly once, and `register_shared_hook_resolved` was deleted. Nothing prevents the next
row from recreating the shape, which is what this is for.

How a collision is born, and it is not by accident
--------------------------------------------------
All three collisions in the tree share one provenance. Their second row's `constant` column reads
`(refused at runtime 0x<its own source>)` -- somebody read `ADDRESS REFUSED ... 0x7ad710` out of a
game log and added a mapping for it. But that refusal was the symptom of a double resolve: the
address was refused because it is a 1.17 destination, and destinations are not keys. Adding the
row did not fix the refusal, it converted a loud refusal into a silent misroute -- exactly the
trade this repo refuses everywhere else, since a missing address costs a feature and a confident
wrong one corrupts.

So the gate calls that provenance out by name, and reports whether anything in `crates/` actually
declares the address -- through `scripts/rva_symbols.py`, which resolves values rather than
searching for a spelling.

It used to search for a spelling, and that advice was destructive (fixed 2026-08-30)
------------------------------------------------------------------------------------
The test was `grep -E "const [A-Z0-9_]+: *usize *= *0x<addr>;"` over `crates/`, and a miss printed
"row B is claimed by no feature: deleting it removes this collision at zero cost." For 0xb0d400
that sentence was wrong: the address is declared `MenuJobWait = 0x00b0d400` inside
`#[repr(u32)] pub enum MenuTraceRva` and reached through
`pub const TITLE_MENU_JOB_WAIT_RVA: usize = MenuTraceRva::MenuJobWait as usize;`. It is an enum
DISCRIMINANT, so the demanded shape never occurs, and it has live uses on the autoload path.
Following this gate's own remediation line would have deleted a working feature's address.
`check-stale-rva-calls.py` was caught the same morning, on the same spelling, for the same reason.

Two things changed. The question is now asked of a resolver that evaluates every declaration form
this tree uses -- literal consts and statics, enum discriminants, `use X as Y` aliases,
`const A = path::B` indirection, module-qualified names, arrays, `Range` bands, and bare hex
literals in table fields. And the answer now has three values, not two: Claimed, proven UNCLAIMED,
and not proven. Only the second may license a deletion. "I found no reference" and "there is no
reference" are different facts, and printing them as the same sentence is what made a silent gate
into a destructive one.

What is and is not a collision
------------------------------
Fatal shape -- `A -> B` with `A != B`, and `B -> C` with `B != C`. Double resolution yields C.

Not a collision -- a row that did not move (`X -> X`). It is a destination and a source at once by
construction, and both answers are the same address, so a second resolve is idempotent. There are
22 such rows and flagging them would bury the three that matter. Nor is `A -> B` where the only
row keyed by B is `B -> B`: the second resolve returns B, which is the right answer.

Is a collision always fatal?
----------------------------
No -- and the gate fails anyway, on purpose. A collision is inert while every address is resolved
exactly once, because a single resolve of A is simply correct. It becomes wrong the moment any
path resolves twice, and "resolve exactly once" is a convention spread across call graphs in six
crates and two DLLs. It has already been violated once, by a helper that looked entirely
reasonable at its call site. A convention that has been broken once, cannot be seen when it is
broken, and is one refactor away from breaking again is not a safeguard; it is a hope. The data
condition is checkable, so it is checked.

The runtime's own mitigation does not close this. `already_translated_in` hands an address back
untranslated when it is a destination -- but only when it is not also a source of a move, because
translation has to win for real sources. A collision address is both, so the shortcut declines and
the table answers. And `game_build.rs`'s doc for `resolve_on_running_build` states the opposite
("the only addresses that are both a 1.17 destination and a 1.16.2 source are the ones that did
not move"), citing `verified_map_is_idempotent` as its enforcement -- a test that filters to rows
where `from != moved` and then asks a predicate that requires `from == moved`, so it is a tautology
that cannot fail. There is no existing machine check for this. That is why this file exists.

Why the rules are PARSED out of `build.rs` and not copied
---------------------------------------------------------
Because copying them is a defect this repo has committed twice in one day. A sibling simulator
mirrored a stale `EXHAUSTIVE_VERDICTS` and reported the detour table as 42 rows instead of 374, a
confident number wrong by nine-fold; another gate still string-compares `"IDENTICAL"` while
`build.rs` carries three exhaustive verdicts. So every admission rule, every field index, every
ledger path and the `DIVERGES` literal are read from `build.rs` at run time. If its shape changes
so the parse fails -- or if it grows a table constant this file does not model -- the gate refuses
rather than reporting a collision count it cannot stand behind.

Baseline
--------
`scripts/1170-translation-collisions.baseline.tsv` records the collisions that have been looked at.
A collision that is not in it, or a baselined one that reaches a table it did not reach before,
fails the run. Baselined ones are printed in full on every run -- an allowlist that hides its
contents is how this class stays invisible. `--strict` ignores the baseline entirely and is the
mode to run once a collision has actually been cleared from the ledgers.

Proven UNCLAIMED is not a deletion licence (2026-08-30, the second correction)
------------------------------------------------------------------------------
`rva_symbols` can now evaluate every address-capable declaration in `crates/`, so all three
baselined addresses come back proven unclaimed. That is a fact about this repo's source, and this
gate used to turn it straight into "deleting it removes this collision at zero cost." It must not,
for two reasons the resolver cannot see:

  * an address can be asked for by a value the source never spells -- computed from another
    constant, read out of a live vtable, recovered from a call site. Not hypothetical:
    `docs/recon/rva-1170-observed-refusals.txt` exists because 42 of the 54 addresses the running
    game asked for on 2026-08-29 were invisible to a declaration scan;
  * Deleting the row does not make a later request fail loudly. With row B gone its source is a
    destination and no longer a source, so `already_translated_in` claims it and
    `resolve_on_running_build` hands the address back UNTRANSLATED -- no refusal, no log line --
    and on 1.17 that address is a different function. `deletion_failure_mode` computes this per row
    from the tables and prints it, because "if I am wrong the failure is loud" is the assumption
    the delete-it advice rested on, and for this shape it is false.

So the proven branch reports what is proven and what is still owed, and no branch may print a
sentence this file's own `COSTLESS_CLAIM` / `DELETION_ADVICE` detectors would refuse in a baseline
note -- the gate held notes to a standard its own prose did not meet. What actually settles a row
is a RUN: the log must show the address arriving only as some other row's translated output, never
as a first request. For all three baselined rows it does; the evidence is in the baseline notes.

Usage
    python3 scripts/check-1170-translation-collisions.py            # gate mode
    python3 scripts/check-1170-translation-collisions.py --strict   # no baseline
    python3 scripts/check-1170-translation-collisions.py --list     # every collision, verbose
    python3 scripts/check-1170-translation-collisions.py --against target/.../address_map_1170.rs
    python3 scripts/check-1170-translation-collisions.py --selftest
"""

import argparse
import glob
import os
import re
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
try:  # noqa: E402 - repo-local; the sys.path line above is what makes it work
    import rva_symbols
except ImportError as missing:
    raise ImportError(
        "scripts/rva_symbols.py could not be imported, so this gate cannot tell whether an address "
        "is claimed by a feature. It must NOT fall back to a hex-literal search: that is the exact "
        "test that told a reader to delete a live autoload address on 2026-08-30."
    ) from missing

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BUILD_RS = os.path.join(ROOT, "crates", "er-game-base", "build.rs")
BASELINE = os.path.join(ROOT, "scripts", "1170-translation-collisions.baseline.tsv")
BASE = 0x140000000

# The table constants in `build.rs` and the job each one does in `emit_address_map`. Parsing tells
# us where the files are; this says what they mean, which is structure and cannot be read off a
# string literal. A constant that appears in build.rs and is not here (and is not inert) stops the
# run: a table this file does not model is a table whose collisions it cannot see, and reporting
# "0 collisions" over a partial view is the failure mode the whole file is about.
ROLES = {
    "VERIFIED_MAP": "verdict table; seeds the CALL map and the DETOUR map",
    "NEEDED_VERIFIED_MAP": "verdict table; DETOUR map only",
    "FUNCTION_MAP": "signature pairs; CALL map, unless VERIFIED_MAP already keys the source",
    "DATA_MAP": "globals carried by reference; CALL map, unless the source is already known",
    "QUARANTINE": "sources held back from both maps",
}

CALL = "CALL"
DETOUR = "DETOUR"


def shown(path):
    """A path relative to the repo when it is inside it, absolute when it is not."""
    absolute = os.path.abspath(path)
    return os.path.relpath(absolute, ROOT) if absolute.startswith(ROOT + os.sep) else absolute


class Refuse(Exception):
    """The gate cannot answer the question it was asked. Never downgraded to a warning."""


# --------------------------------------------------------------------------------------------
# Reading the rules out of build.rs
# --------------------------------------------------------------------------------------------


def _one(pattern, text, what, flags=0):
    found = re.search(pattern, text, flags)
    if not found:
        raise Refuse(
            f"cannot read {what} out of {BUILD_RS}.\n"
            f"  Its shape changed and this gate's model of it is now guesswork. Fix the pattern\n"
            f"  ({pattern!r}) rather than letting the gate report a collision count derived from\n"
            f"  rules that are no longer the build's."
        )
    return found


def build_rules(path=BUILD_RS):
    """Every admission rule `emit_address_map` applies, read from the source that applies them."""
    with open(path, encoding="utf-8") as handle:
        text = handle.read()

    # Only the table constants: build.rs also declares plain string constants (`SHA_LENGTH`,
    # `UNKNOWN`) that are not ledgers. A path ending in `.tsv` is what a ledger constant looks
    # like, and a new one appearing is exactly the drift this must not sleep through.
    declared = dict(re.findall(r'const (\w+): &str = "([^"]*\.tsv)";', text))
    # `let _ = NAME;` is build.rs's own way of saying a constant is declared but not read
    # (AUDITED_DETOURS, kept as a reading aid after it was unwired). Not modelling an unread
    # table is correct; not modelling a read one is not.
    inert = set(re.findall(r"let _ = (\w+);", text))
    unmodelled = set(declared) - set(ROLES) - inert
    if unmodelled:
        raise Refuse(
            "build.rs names a table constant this gate does not model: "
            + ", ".join(sorted(unmodelled))
            + ".\n  Add it to ROLES with the job it does in emit_address_map, or the collision\n"
            "  scan runs over a partial view of the tables and reports a clean count it has not\n"
            "  earned."
        )
    missing = set(ROLES) - set(declared)
    if missing:
        raise Refuse(
            "build.rs no longer declares: " + ", ".join(sorted(missing)) + ".\n"
            "  The emission this gate reproduces has changed; re-read emit_address_map before\n"
            "  trusting anything below."
        )

    body = _one(
        r"fn detourable_pairs\(.*?\n\}\n", text, "the detourable_pairs body", re.S
    ).group(0)
    return {
        "paths": {
            name: os.path.normpath(
                os.path.join(os.path.dirname(path), declared[name])
            )
            for name in ROLES
        },
        "inert": sorted(inert),
        "min_insns": int(_one(r"MIN_VERIFIED_INSNS: u32 = (\d+)", text, "the insn floor").group(1)),
        "exhaustive": tuple(
            re.findall(
                r'"([^"]+)"',
                _one(
                    r"EXHAUSTIVE_VERDICTS: \[&str; \d+\] = \[([^\]]*)\]",
                    text,
                    "EXHAUSTIVE_VERDICTS",
                ).group(1),
            )
        ),
        # The second floor-exempt class: verdicts where the bodies differ and the patch site does
        # not. Read with `_one`, so a rename empties nothing quietly -- it stops the gate. A gate
        # that silently forgot this class would under-report the detour table, which is the safe
        # direction and still drift.
        "patch_site": tuple(
            re.findall(
                r'"([^"]+)"',
                _one(
                    r"PATCH_SITE_VERDICTS: \[&str; \d+\] = \[([^\]]*)\]",
                    text,
                    "PATCH_SITE_VERDICTS",
                ).group(1),
            )
        ),
        # The call-only class, and the only place the two maps take different rows from the same
        # table. Read with `_one` like the rest, so losing it stops the gate rather than quietly
        # reverting the model to the world before the split -- which is precisely the drift this
        # gate caught in itself on 2026-08-30, reporting `CALL modelled 497 vs generated 499`.
        "callable_only": tuple(
            re.findall(
                r'"([^"]+)"',
                _one(
                    r"CALLABLE_ONLY_VERDICTS: \[&str; \d+\] = \[([^\]]*)\]",
                    text,
                    "CALLABLE_ONLY_VERDICTS",
                ).group(1),
            )
        ),
        "entry_evidence": tuple(
            re.findall(
                r'"([^"]+)"',
                _one(
                    r"DETOURABLE_ENTRY_EVIDENCE: \[&str; \d+\] = \[([^\]]*)\]",
                    text,
                    "DETOURABLE_ENTRY_EVIDENCE",
                ).group(1),
            )
        ),
        # The prefix verdict, held to the floor. Spelled out in a match arm, not a constant --
        # which is exactly why another gate is stale on it today.
        "prefix_verdict": _one(
            r'\n\s+"([A-Z-]+)" => \{', body, "the prefix verdict match arm"
        ).group(1),
        "refuted_verdict": _one(
            r'fields\[\d+\] != "([A-Z-]+)"', text, "the refuted-verdict literal"
        ).group(1),
        "min_columns": int(_one(r"fields\.len\(\) < (\d+)", body, "the column floor").group(1)),
        "verdict_column": int(_one(r"match fields\[(\d+)\]", body, "the verdict column").group(1)),
        "insns_column": int(
            _one(r"fields\[(\d+)\]\.trim\(\)\.parse::<u32>", body, "the insn column").group(1)
        ),
        "entry_column": int(
            _one(
                r"DETOURABLE_ENTRY_EVIDENCE\.contains\(&fields\[(\d+)\]",
                body,
                "the entry-evidence column",
            ).group(1)
        ),
        "refuted_column": int(
            _one(r'fields\[(\d+)\] != "[A-Z-]+"', text, "the refuted-verdict column").group(1)
        ),
    }


# --------------------------------------------------------------------------------------------
# Reproducing emit_address_map
# --------------------------------------------------------------------------------------------


class Row:
    """One admitted pair, with the provenance a failure message needs to be actionable."""

    __slots__ = ("src", "dst", "table", "line", "note")

    def __init__(self, src, dst, table, line, note):
        self.src, self.dst, self.table, self.line, self.note = src, dst, table, line, note

    def where(self):
        rel = os.path.relpath(self.table, ROOT)
        return f"{rel}:{self.line}"


def _lines(path):
    try:
        with open(path, encoding="utf-8") as handle:
            text = handle.read()
    except FileNotFoundError:
        return []
    return [
        (number, line)
        for number, line in enumerate(text.splitlines(), 1)
        if line.strip() and not line.startswith("#")
    ]


def _verdict_rows(path, rules):
    """`detourable_pairs`: the rows a verdict table is allowed to carry."""
    out = []
    for number, line in _lines(path):
        fields = line.split("\t")
        if len(fields) < rules["min_columns"]:
            continue
        verdict = fields[rules["verdict_column"]]
        if verdict in rules["exhaustive"]:
            pass
        elif verdict in rules["patch_site"]:
            pass
        elif verdict == rules["prefix_verdict"]:
            try:
                compared = int(fields[rules["insns_column"]].strip())
            except ValueError:
                compared = 0
            if compared < rules["min_insns"]:
                continue
        else:
            continue
        if fields[rules["entry_column"]].strip() not in rules["entry_evidence"]:
            continue
        try:
            src = int(fields[0], 16) - BASE
            dst = int(fields[1], 16) - BASE
        except ValueError:
            continue
        note = fields[5].strip() if len(fields) > 5 else ""
        out.append(Row(src, dst, path, number, note))
    return out


def _callable_only_rows(path, rules):
    """`callable_only_pairs`: rows a verdict table gives the call map and no other.

    Its own reader beside `_verdict_rows`, mirroring the two separate functions in `build.rs` --
    the detour model must not be able to pick these up by sharing a code path with them.
    """
    out = []
    for number, line in _lines(path):
        fields = line.split("\t")
        if len(fields) < rules["min_columns"]:
            continue
        if fields[rules["verdict_column"]] not in rules["callable_only"]:
            continue
        if fields[rules["entry_column"]].strip() not in rules["entry_evidence"]:
            continue
        try:
            src = int(fields[0], 16) - BASE
            dst = int(fields[1], 16) - BASE
        except ValueError:
            continue
        note = fields[5].strip() if len(fields) > 5 else ""
        out.append(Row(src, dst, path, number, note))
    return out


def _refuted(path, rules):
    """Sources a verdict table positively disagrees with. Subtracted from both maps."""
    out = set()
    for _, line in _lines(path):
        fields = line.split("\t")
        column = rules["refuted_column"]
        if len(fields) <= column or fields[column] != rules["refuted_verdict"]:
            continue
        try:
            out.add(int(fields[0], 16) - BASE)
        except ValueError:
            continue
    return out


def _plain_rows(path, exclude):
    """`FUNCTION_MAP` / `DATA_MAP`: RVA pairs, admitted unless the source is already keyed."""
    out = []
    for number, line in _lines(path):
        fields = line.split("\t")
        if len(fields) < 2:
            continue
        try:
            src, dst = int(fields[0], 16), int(fields[1], 16)
        except ValueError:
            continue
        if src in exclude:
            continue
        note = fields[2].strip() if len(fields) > 2 else ""
        out.append(Row(src, dst, path, number, note))
    return out


def _finish(rows, held_back):
    """build.rs's `retain` / `sort_unstable` / `dedup_by_key(old)`, keeping provenance."""
    kept = sorted(
        (row for row in rows if row.src not in held_back),
        key=lambda row: (row.src, row.dst),
    )
    out, last = [], None
    for row in kept:
        if row.src != last:
            out.append(row)
            last = row.src
    return out


def emit(rules):
    """`{CALL: [Row], DETOUR: [Row]}` -- what `emit_address_map` writes, with provenance."""
    paths = rules["paths"]
    # Detour is taken from the detourable rows alone, and taken first. `list(call)` was right only
    # while the two seeds were the same set; since the call-only verdict exists they are not, and
    # copying `call` after the extend below would hand every call-only row a detour licence.
    detour = _verdict_rows(paths["VERIFIED_MAP"], rules) + _verdict_rows(
        paths["NEEDED_VERIFIED_MAP"], rules
    )
    call = _verdict_rows(paths["VERIFIED_MAP"], rules) + _callable_only_rows(
        paths["VERIFIED_MAP"], rules
    )

    seeded = {row.src for row in call}
    call += _plain_rows(paths["FUNCTION_MAP"], seeded)
    call += _plain_rows(paths["DATA_MAP"], {row.src for row in call})

    held_back = set()
    for _, line in _lines(paths["QUARANTINE"]):
        try:
            held_back.add(int(line.split("\t")[0].strip(), 16))
        except ValueError:
            continue
    held_back |= _refuted(paths["NEEDED_VERIFIED_MAP"], rules)
    held_back |= _refuted(paths["VERIFIED_MAP"], rules)

    return {CALL: _finish(call, held_back), DETOUR: _finish(detour, held_back)}


# --------------------------------------------------------------------------------------------
# The collision itself
# --------------------------------------------------------------------------------------------


class Collision:
    """`first -> address` and `address -> second`, both real moves."""

    __slots__ = ("first", "address", "second", "routes", "rows")

    def __init__(self, first, address, second):
        self.first, self.address, self.second = first, address, second
        self.routes = set()
        self.rows = {}

    def key(self):
        return (self.first, self.address, self.second)

    def label(self):
        return f"0x{self.first:x} -> 0x{self.address:x} -> 0x{self.second:x}"


def collisions(tables):
    """Every double-resolve triple, over every ordered pair of tables a caller might use.

    Both orders and both tables, because the resolvers are separate functions over separate
    tables: `resolve_game_address` reads the call map and `resolve_detour_address` the detour
    map. The path that bit resolved through one and then the other, so checking each table
    against itself would have missed it.
    """
    found = {}
    for first_name, first_rows in tables.items():
        for second_name, second_rows in tables.items():
            second = {row.src: row for row in second_rows}
            for row in first_rows:
                if row.src == row.dst:
                    continue  # did not move: resolution is idempotent here
                onward = second.get(row.dst)
                if onward is None or onward.src == onward.dst:
                    continue  # nothing keyed by the destination, or it is an identity row
                collision = found.setdefault(
                    (row.src, row.dst, onward.dst),
                    Collision(row.src, row.dst, onward.dst),
                )
                collision.routes.add(f"{first_name}>{second_name}")
                collision.rows.setdefault("first", row)
                collision.rows.setdefault("second", onward)
    return sorted(found.values(), key=lambda c: c.key())


# The MATCHER this replaced, frozen as a literal so the selftest's controls keep meaning what
# they say. Until 2026-08-30 the question "does anything claim this address" was asked as a text
# search for `const NAME: usize = 0x<addr>;`, and a miss was reported to the reader as
# "claimed by no feature: deleting it removes this collision at zero cost."
#
# It was wrong on a real address. 0xb0d400 is declared `MenuJobWait = 0x00b0d400` inside
# `#[repr(u32)] pub enum MenuTraceRva`, and reached as
# `pub const TITLE_MENU_JOB_WAIT_RVA: usize = MenuTraceRva::MenuJobWait as usize;` -- an enum
# DISCRIMINANT, so the shape this pattern demands never occurs, and its three live uses sit on the
# autoload path. Following this gate's own advice would have deleted a working feature's address.
# `check-stale-rva-calls.py` had been caught the same morning for the same reason, on the same
# spelling. Two tools, one root cause: a hex literal in one shape is not an address.
#
# Kept, not deleted, because the controls in `selftest` have to prove the new resolver sees what
# this could not. A control both matchers catch would pass on the broken gate and prove nothing.
LEGACY_CONST_PATTERN = r"const [A-Z0-9_]+: *usize *= *0x{rva:x}\b"


def legacy_names_the_address(rva, text):
    """The pre-fix test, verbatim, run over one blob of text instead of over `crates/`."""
    return re.findall(LEGACY_CONST_PATTERN.format(rva=rva), text, re.I)


def claims_on(rva):
    """Every symbol, alias and bare literal in `crates/` that claims this address.

    Delegates to `scripts/rva_symbols.py`, which is shared with `check-stale-rva-calls.py` rather
    than being a third dialect of the same regex. The result carries `proven_unclaimed`, which is
    not the same fact as "found nothing" and is the only thing that may license a deletion.
    """
    try:
        return rva_symbols.index().claims(rva)
    except (OSError, RecursionError) as failure:  # a broken walk must not read as a clean one
        print(f"    (could not resolve crates/ symbols: {failure})", file=sys.stderr)
        return None


def _already_translated_in(pairs, rva):
    """`already_translated_in` from `crates/er-game-base/src/game_build.rs`, on `(src, dst)` pairs.

    Reproduced rather than imported because it is Rust, and reproduced here rather than assumed
    because the whole point below is what the runtime would do with a row this gate suggests
    removing. Kept byte-for-byte equivalent to that function: a destination of some other row that
    is not itself the source of a move.
    """
    is_destination = any(dst == rva and src != rva for src, dst in pairs)
    is_source_of_a_move = any(src == rva and dst != rva for src, dst in pairs)
    return is_destination and not is_source_of_a_move


# What a genuine, first-time request for the collision address would get once row B is gone.
LOUD = "REFUSED"  # `resolve_on_running_build` logs address refused and returns None
SILENT = "HANDED BACK UNTRANSLATED"  # `already_translated_in` claims it; no log line at all
STILL_MAPPED = "STILL TRANSLATED"  # another row keys the same source


def deletion_failure_mode(tables, collision):
    """`{table: (kind, address)}` -- what asking for the collision address gets after row B goes.

    This is the question the old "AT ZERO COST" sentence assumed an answer to. Deleting a row is
    only cheap-if-wrong when being wrong is LOUD: a missing address costs a feature and says so.
    For this exact shape it is not. Row B's source is row A's destination, so once row B is deleted
    the source is a destination that no row claims as a source -- which is precisely the condition
    `already_translated_in` tests -- and the resolver hands the address straight back with no
    translation, no refusal and no log line. On 1.17 that address is a different function.

    So the gate computes the failure mode instead of assuming it, and prints it beside any
    suggestion to delete. `LOUD` would make a deletion recoverable from a log; `SILENT` means a
    wrong deletion looks exactly like the collision it replaced.
    """
    out = {}
    for name, rows in tables.items():
        pairs = [
            (row.src, row.dst)
            for row in rows
            if not (row.src == collision.address and row.dst == collision.second)
        ]
        if _already_translated_in(pairs, collision.address):
            out[name] = (SILENT, collision.address)
            continue
        onward = next((dst for src, dst in pairs if src == collision.address), None)
        out[name] = (STILL_MAPPED, onward) if onward is not None else (LOUD, None)
    return out


# --------------------------------------------------------------------------------------------
# Baseline
# --------------------------------------------------------------------------------------------


def read_baseline(path=BASELINE):
    out = {}
    if not os.path.exists(path):
        return out
    with open(path, encoding="utf-8") as handle:
        for line in handle:
            if line.startswith("#") or not line.strip():
                continue
            fields = line.rstrip("\n").split("\t")
            if len(fields) < 4:
                continue
            try:
                key = (int(fields[0], 16), int(fields[1], 16), int(fields[2], 16))
            except ValueError:
                continue
            out[key] = {
                "routes": {r for r in fields[3].split(",") if r},
                "note": fields[4] if len(fields) > 4 else "",
            }
    return out


# A baseline NOTE is also advice, and it was wrong for months. All three notes used to end
# "the row carries no feature: delete it from both ledgers" -- copied from the gate's own
# claimed-by-no-feature line, which was a text search for one spelling. The notes are printed on
# every run, so a stale one is read far more often than the code that produced it. This refuses a
# note that advises deletion unless the address is proven unclaimed right now.
#
# Two detectors, because the two phrases are refused for different reasons and one of them can
# never be earned. "Claimed by no feature" is a statement about `crates/`, which `rva_symbols` can
# settle. "At zero cost" is a statement about the whole system -- source, ledgers and the running
# game -- and nothing in this repo establishes it. It is also false in the specific direction that
# matters here: see `deletion_failure_mode`, where deleting one of these rows makes a later request
# for the address silently wrong rather than loudly refused.
DELETION_ADVICE = re.compile(
    r"carries no feature|claimed by no feature|safe to delete|deleting it removes", re.I
)
COSTLESS_CLAIM = re.compile(r"zero(?: feature)? cost|costs nothing|at no cost|free to delete", re.I)


def unearned_deletion_advice(known, baseline):
    """Baselined notes whose advice this gate cannot stand behind: `(collision, note, claims, why)`.

    Two grounds, and they are not the same fact:

    * a COSTLESS claim, refused unconditionally -- no check in this repo computes the cost of a
      deletion, and for a collision row the cost of being wrong is a silent misroute;
    * Deletion advice about an address that is not proven unclaimed -- "I found no reference" read
      as "there is no reference", which is the failure this whole file exists for.
    """
    out = []
    for collision in known:
        note = baseline[collision.key()]["note"]
        costless = COSTLESS_CLAIM.search(note)
        advises = DELETION_ADVICE.search(note)
        if not costless and not advises:
            continue
        claims = claims_on(collision.address)
        if costless:
            out.append(
                (
                    collision,
                    note,
                    claims,
                    "it claims a deletion is costless, which nothing here computes and which "
                    "deletion_failure_mode contradicts",
                )
            )
        elif claims is None or not claims.proven_unclaimed:
            out.append((collision, note, claims, "the address is not PROVEN unclaimed"))
    return out


def baseline_line(collision, note):
    return "\t".join(
        [
            f"0x{collision.first:x}",
            f"0x{collision.address:x}",
            f"0x{collision.second:x}",
            ",".join(sorted(collision.routes)),
            note,
        ]
    )


# --------------------------------------------------------------------------------------------
# The double-resolve tripwire
# --------------------------------------------------------------------------------------------

# The layers that own address resolution. A `pub` entry point here whose name says its argument is
# already resolved is the exact shape that reintroduces a second resolve at a call site where it
# looks reasonable -- `register_shared_hook_resolved`, deleted 2026-08-30. er-hook keeps
# `register_union_hook_resolved` private on purpose and says why in its own doc.
RESOLVE_OWNERS = ("crates/er-hook/src", "crates/er-game-base/src")
RESOLVED_FN = re.compile(
    r"^\s*(?P<vis>pub(?:\([^)]*\))?\s+)?(?:unsafe\s+)?(?:extern\s+\"[^\"]+\"\s+)?"
    r"fn\s+(?P<name>\w+_resolved)\b"
)


def resolved_entrypoints(root=ROOT, owners=RESOLVE_OWNERS):
    """Public `*_resolved` functions in the resolving layers. Empty is the expected state."""
    out = []
    for owner in owners:
        for path in sorted(glob.glob(os.path.join(root, owner, "**", "*.rs"), recursive=True)):
            with open(path, encoding="utf-8") as handle:
                for number, line in enumerate(handle, 1):
                    found = RESOLVED_FN.match(line)
                    if found and found.group("vis"):
                        out.append((os.path.relpath(path, root), number, found.group("name")))
    return out


# --------------------------------------------------------------------------------------------
# Cross-check against a table cargo really generated
# --------------------------------------------------------------------------------------------


def parse_generated(path):
    with open(path, encoding="utf-8") as handle:
        text = handle.read()
    out = {}
    for name, key in (
        ("VERIFIED_1162_TO_1170", CALL),
        ("DETOUR_SAFE_1162_TO_1170", DETOUR),
    ):
        body = re.search(rf"const {name}: \[\(u32, u32\); \d+\] = \[(.*?)\];", text, re.S)
        out[key] = (
            [
                (int(a, 16), int(b, 16))
                for a, b in re.findall(r"\((0x[0-9a-f]+), (0x[0-9a-f]+)\)", body.group(1))
            ]
            if body
            else []
        )
    return out


def newest_generated(rules):
    """A cargo-generated map, only if it is newer than every input that feeds it.

    A stale artifact would fail the comparison for a reason that has nothing to do with this
    gate -- a sibling edited a ledger and has not rebuilt -- so staleness is a skip, not a red.
    """
    candidates = glob.glob(
        os.path.join(ROOT, "target", "**", "build", "er-game-base-*", "out", "address_map_1170.rs"),
        recursive=True,
    )
    if not candidates:
        return None, "no cargo-generated map under target/"
    newest = max(candidates, key=os.path.getmtime)
    inputs = [BUILD_RS] + list(rules["paths"].values())
    freshest_input = max(
        (os.path.getmtime(p) for p in inputs if os.path.exists(p)), default=0
    )
    if os.path.getmtime(newest) <= freshest_input:
        return None, f"{os.path.relpath(newest, ROOT)} is older than its inputs (stale)"
    return newest, None


# --------------------------------------------------------------------------------------------
# Reporting
# --------------------------------------------------------------------------------------------


def describe(collision, out, tables=None):
    """Print one collision in full. `tables` lets it compute what deleting row B would do.

    Without `tables` the deletion-failure-mode paragraph is replaced by a line saying it could not
    be computed -- never by silence, because its absence is what let the old "at zero cost"
    sentence sound like a finished argument.
    """
    first, second = collision.rows["first"], collision.rows["second"]
    print(f"\n  COLLISION at 0x{collision.address:x}   ({', '.join(sorted(collision.routes))})", file=out)
    print(
        f"    row A  0x{first.src:x} -> 0x{first.dst:x}"
        f"   {first.where()}   {first.note or '(no constant recorded)'}",
        file=out,
    )
    print(
        f"    row B  0x{second.src:x} -> 0x{second.dst:x}"
        f"   {second.where()}   {second.note or '(no constant recorded)'}",
        file=out,
    )
    print(
        f"    Row A's DESTINATION 0x{collision.address:x} is row B's SOURCE. Resolving\n"
        f"    0x{first.src:x} once gives 0x{collision.address:x}, which is correct. Resolving that\n"
        f"    result again gives 0x{second.dst:x} -- a different function, returned with no error,\n"
        f"    no refusal and no log line, because from a bare address the table cannot tell a\n"
        f"    destination from a source.",
        file=out,
    )
    if f"{CALL}>{DETOUR}" in collision.routes or f"{DETOUR}>{DETOUR}" in collision.routes:
        print(
            f"    A detour taken on 0x{second.dst:x} writes five bytes into that unrelated body.",
            file=out,
        )
    self_refusal = f"refused at runtime 0x{second.src:x}"
    if self_refusal in second.note:
        print(
            f"    ROW B WAS ADDED TO SILENCE A REFUSAL OF ITS OWN SOURCE. Its provenance column\n"
            f"    says {second.note!r}. That refusal was the symptom of a double resolve --\n"
            f"    0x{second.src:x} was refused because it is a DESTINATION, and destinations are not\n"
            f"    keys. The row did not fix the refusal; it replaced a loud refusal with a silent\n"
            f"    wrong answer.",
            file=out,
        )
    claims = claims_on(second.src)
    if claims is None:
        print(
            f"    UNKNOWN whether anything claims 0x{second.src:x}: the symbol resolver could not\n"
            f"    run. Do not delete row B on this evidence.",
            file=out,
        )
        return
    if claims.declarations or claims.literals:
        print(f"    0x{second.src:x} IS CLAIMED, so row B carries a feature:", file=out)
        for decl in claims.declarations[:6]:
            uses = claims.uses.get(decl.symbol, [])
            print(
                f"      {decl.where()}  {decl.qualified}"
                f"  ({decl.form}, {len(uses)} use site(s) elsewhere)",
                file=out,
            )
        for alias, target in claims.aliases[:4]:
            print(f"      ...also reachable as `{alias}` (use {target} as {alias})", file=out)
        for literal in claims.literals[:4]:
            print(f"      {literal.where()}  bare literal {literal.text}", file=out)
        print(
            "    Deleting row B would cost that feature. Keep the single-resolve discipline\n"
            "    instead, or re-derive whichever of the two rows is wrong.",
            file=out,
        )
    elif claims.proven_unclaimed:
        print(
            f"    NOTHING in crates/ DECLARES 0x{second.src:x}, and THAT MUCH is PROVEN: all\n"
            f"    {claims.universe} address-capable declarations were evaluated, none is this\n"
            f"    address, and no bare literal of it occurs in code.",
            file=out,
        )
        print(
            f"    THAT IS NOT A LICENCE TO DELETE ROW B, and this gate will not write one. What\n"
            f"    the resolver reads is this repo's SOURCE; an address can still be asked for by a\n"
            f"    value the source never spells -- computed from another constant, read out of a\n"
            f"    live vtable, recovered from a call site. docs/recon/rva-1170-observed-refusals.txt\n"
            f"    exists because 42 of the 54 addresses the running game asked for on 2026-08-29\n"
            f"    were invisible to a declaration scan.",
            file=out,
        )
        if tables is None:
            print(
                f"    AND THE COST OF BEING WRONG WAS NOT COMPUTED: describe() was called without\n"
                f"    the tables, so what a later request for 0x{second.src:x} would get after a\n"
                f"    deletion is unknown here. Do not delete on this page alone.",
                file=out,
            )
        else:
            outcomes = deletion_failure_mode(tables, collision)
            summary = ", ".join(
                f"{name} map: {kind}" + (f" -> 0x{landing:x}" if landing is not None else "")
                for name, (kind, landing) in sorted(outcomes.items())
            )
            print(
                f"    IF ROW B WERE DELETED, a later first request for 0x{second.src:x} would be\n"
                f"    -- {summary}.",
                file=out,
            )
            if any(kind == SILENT for kind, _ in outcomes.values()):
                print(
                    f"    THAT IS THE SILENT DIRECTION, so a wrong deletion here is NOT cheap. With\n"
                    f"    row B gone, 0x{second.src:x} is a destination that no row claims as a\n"
                    f"    source -- exactly what `already_translated_in` tests -- so the resolver\n"
                    f"    hands it back unchanged with no translation, no refusal and no log line,\n"
                    f"    and on 1.17 that address is a DIFFERENT function. A wrong deletion looks\n"
                    f"    exactly like the collision it replaced.",
                    file=out,
                )
        print(
            f"    WHAT WOULD SETTLE IT is a RUN, not a scan: the DLL logs must show 0x{second.src:x}\n"
            f"    arriving only as some other row's ADDRESS TRANSLATED output and never as a first\n"
            f"    request of its own. Record that evidence in the baseline note, then decide.",
            file=out,
        )
    else:
        # "I FOUND NO REFERENCE" and "THERE IS NO REFERENCE" must not print the same sentence.
        # This branch is the first one, and it recommends nothing. The old gate collapsed the two
        # and told a reader to delete a row on a search that had simply not looked in the right
        # shape.
        print(
            f"    NOT PROVEN. No declaration, alias or bare literal for 0x{second.src:x} was\n"
            f"    FOUND, but {len(claims.residue)} of {claims.universe} address-capable\n"
            f"    declarations could not be evaluated, so one of them may be it. DO NOT DELETE\n"
            f"    ROW B on this evidence -- finish the proof by reading these:",
            file=out,
        )
        for decl in claims.residue[:20]:
            print(
                f"      {decl.where()}  {decl.qualified}: {decl.type_text} = "
                f"{' '.join(decl.expr.split())[:60]}",
                file=out,
            )
        if len(claims.residue) > 20:
            print(
                f"      ...and {len(claims.residue) - 20} more "
                f"(python3 scripts/rva_symbols.py --residue 0x{second.src:x})",
                file=out,
            )


def preamble(out):
    print(
        "\nWHY THIS IS FATAL. A translation table maps 1.16.2 -> 1.17 and is keyed by the 1.16.2\n"
        "side. When a row's DESTINATION is also another row's SOURCE, translating twice does not\n"
        "fail -- it succeeds and returns a third, unrelated function. Nothing errors, so the only\n"
        "symptom is a hook or a call landing somewhere it was never meant to. It is inert while\n"
        "every address is resolved exactly once; that discipline is a convention across six\n"
        "crates, it has been violated once already (register_shared_hook, 2026-08-30), and it\n"
        "cannot be seen when it is violated. The data condition can be seen, so it is checked.",
        file=out,
    )


def epilogue(out):
    print(
        "\nWHAT TO DO. In order of preference:\n"
        "  1. FIX THE DOUBLE RESOLVE AT THE CALL SITE, which is the defect itself and the only\n"
        "     remedy that generalises. A caller that does `game_rva(RVA)` and then hands the\n"
        "     RESULT to `MhHook::new` / `register_union_hook` resolves twice; those entry points\n"
        "     resolve internally, so they take the UNRESOLVED 1.16.2 address. Grep the run log for\n"
        "     two adjacent ADDRESS TRANSLATED lines where the second's source is the first's\n"
        "     destination -- that pair names the call site.\n"
        "  2. Only then consider dropping the row whose source is the other's destination, and\n"
        "     ONLY where the gate says above that nothing DECLARES it and that this is PROVEN.\n"
        "     PROVEN is a fact about `crates/`, not about the game: it does not cover an address\n"
        "     computed at runtime, and -- read the deletion-failure-mode lines above -- being\n"
        "     wrong about it is SILENT here, not loud. `NOT PROVEN` means the resolver found\n"
        "     nothing and could not read everything, which is not the same fact and is not grounds\n"
        "     for a deletion; `IS CLAIMED` names the symbol that would lose its address. An enum\n"
        "     discriminant is a declaration: 0xb0d400 is spelled `MenuJobWait = 0x00b0d400`, and\n"
        "     the shape `const NAME: usize = 0x..;` never occurs for it.\n"
        "  3. If both rows are load-bearing, keep them and record the collision in\n"
        f"     {os.path.relpath(BASELINE, ROOT)} with a note saying why -- it is then printed on\n"
        "     every run rather than forgotten.\n"
        "  4. Never 'fix' an ADDRESS REFUSED log line by adding a row for the refused address\n"
        "     without first checking whether it is already some row's destination. That is how\n"
        "     all three current collisions were created.\n"
        "Context for one address: python3 scripts/pdata-enclosing-function.py 1162:0x<va>\n",
        file=out,
    )


# --------------------------------------------------------------------------------------------
# Selftest
# --------------------------------------------------------------------------------------------

SYNTHETIC_BUILD_RS = '''
const VERIFIED_MAP: &str = "recon/verified.tsv";
const NEEDED_VERIFIED_MAP: &str = "recon/needed-verified.tsv";
const FUNCTION_MAP: &str = "recon/needed.tsv";
const DATA_MAP: &str = "recon/data.tsv";
const QUARANTINE: &str = "recon/quarantine.tsv";
const MIN_VERIFIED_INSNS: u32 = {floor};
const EXHAUSTIVE_VERDICTS: [&str; 2] = [{exhaustive}];
const PATCH_SITE_VERDICTS: [&str; 1] = [{patch_site}];
const CALLABLE_ONLY_VERDICTS: [&str; 1] = [{callable_only}];
const DETOURABLE_ENTRY_EVIDENCE: [&str; 2] = ["BOTH-ENTRIES", "NEITHER-ENTRY"];
fn refuted_sources(path: &Path) -> Vec<u32> {{
    if fields.len() < 3 || fields[2] != "DIVERGES" {{ continue; }}
}}
fn detourable_pairs(path: &Path) -> Vec<(u32, u32)> {{
    if fields.len() < 7 {{ continue; }}
    match fields[2] {{
        verdict if EXHAUSTIVE_VERDICTS.contains(&verdict) => {{}}
        verdict if PATCH_SITE_VERDICTS.contains(&verdict) => {{}}
        "{prefix}" => {{
            if fields[4].trim().parse::<u32>().unwrap_or(0) < MIN_VERIFIED_INSNS {{ continue; }}
        }}
        _ => continue,
    }}
    if !DETOURABLE_ENTRY_EVIDENCE.contains(&fields[6].trim()) {{ continue; }}
}}
'''

VERDICT_HEADER = "# 1.16.2 VA\t1.17 VA\tverdict\tratio\tinsns\thow\tentry\textent\n"


def _write(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(text)


def _synthetic_tree(
    scratch,
    needed_rows,
    floor=12,
    exhaustive='"BYTE-IDENTICAL", "IDENTICAL-WHOLE"',
    prefix="IDENTICAL",
    patch_site='"PATCH-SITE-IDENTICAL"',
    callable_only='"IDENTICAL-LEAF-NOPATCH"',
    verified_rows=(),
):
    """A whole miniature of what build.rs reads, so the gate is driven end to end offline."""
    build = os.path.join(scratch, "crate", "build.rs")
    _write(
        build,
        SYNTHETIC_BUILD_RS.format(
            floor=floor,
            exhaustive=exhaustive,
            prefix=prefix,
            patch_site=patch_site,
            callable_only=callable_only,
        ),
    )
    recon = os.path.join(scratch, "crate", "recon")
    _write(os.path.join(recon, "verified.tsv"), VERDICT_HEADER + "".join(verified_rows))
    _write(os.path.join(recon, "needed-verified.tsv"), VERDICT_HEADER + "".join(needed_rows))
    _write(os.path.join(recon, "needed.tsv"), "# 1.16.2 RVA\t1.17 RVA\tconstant\n")
    _write(os.path.join(recon, "data.tsv"), "# 1.16.2 RVA\t1.17 RVA\tconstant\n")
    _write(os.path.join(recon, "quarantine.tsv"), "# held back\n")
    return build


def _verdict(src, dst, verdict="BYTE-IDENTICAL", insns=99, note="SOME_RVA"):
    return f"0x{BASE + src:x}\t0x{BASE + dst:x}\t{verdict}\t1.000\t{insns}\t{note}\tBOTH-ENTRIES\tPDATA\n"


def selftest():
    """Prove the gate fires on the shape, stays quiet on the lookalikes, and reads live rules."""
    failures = []

    def check(name, got, want):
        if got != want:
            failures.append(f"{name}: got {got!r}, want {want!r}")

    with tempfile.TemporaryDirectory() as scratch:
        # The shape. A -> B and B -> C, both real moves.
        build = _synthetic_tree(
            os.path.join(scratch, "collide"),
            [_verdict(0x1000, 0x2000), _verdict(0x2000, 0x3000)],
        )
        found = collisions(emit(build_rules(build)))
        check("finds exactly one collision", len(found), 1)
        if found:
            check("names the triple", found[0].key(), (0x1000, 0x2000, 0x3000))
            check(
                "reports it on every table pairing that can reach it",
                sorted(found[0].routes),
                ["DETOUR>DETOUR"],
            )

        # A clean table. Two rows that move, sharing nothing.
        build = _synthetic_tree(
            os.path.join(scratch, "clean"),
            [_verdict(0x1000, 0x2000), _verdict(0x4000, 0x5000)],
        )
        check("clean table is clean", collisions(emit(build_rules(build))), [])

        # The LOOKALIKE that is not a collision. `X -> X` is a destination and a source at once,
        # and both answers are the same address, so a second resolve is idempotent.
        build = _synthetic_tree(
            os.path.join(scratch, "identity"),
            [_verdict(0x2000, 0x2000)],
        )
        check("an identity row alone is not a collision", collisions(emit(build_rules(build))), [])

        # The harder LOOKALIKE. A -> B where the only row keyed by B is `B -> B`: the second
        # resolve returns B, which is the right answer. Flagging it would bury the real ones.
        build = _synthetic_tree(
            os.path.join(scratch, "benign-chain"),
            [_verdict(0x1000, 0x2000), _verdict(0x2000, 0x2000)],
        )
        check(
            "a chain onto an identity row is not a collision",
            collisions(emit(build_rules(build))),
            [],
        )

        # The cross-table pairing, driven straight at the rule. `resolve_game_address` reads the
        # call map and `resolve_detour_address` the detour map, and the path that bit went through
        # one and then the other -- so a chain that starts in one table and lands in the other is
        # the real shape, and checking each table only against itself would miss it.
        crossed = {
            CALL: [Row(0x1000, 0x2000, "synthetic", 1, "")],
            DETOUR: [Row(0x2000, 0x3000, "synthetic", 1, "")],
        }
        check(
            "a chain from the CALL map into the DETOUR map is a collision",
            [c.key() for c in collisions(crossed)],
            [(0x1000, 0x2000, 0x3000)],
        )
        crossed_found = collisions(crossed)
        check(
            "and it is reported on the pairing that reaches it",
            sorted(crossed_found[0].routes) if crossed_found else "nothing found",
            ["CALL>DETOUR"],
        )
        # ...but a row that did not move still cannot start one, even when the other table keys
        # its address to somewhere else. Nothing was translated by the first lookup, so there is
        # no second translation; that shape is a CALL/DETOUR disagreement, which
        # `every_detour_row_agrees_with_the_call_map` in er-game-base already owns.
        check(
            "a row that did not move cannot start a double resolve",
            collisions(
                {
                    CALL: [Row(0x2000, 0x2000, "synthetic", 1, "")],
                    DETOUR: [Row(0x2000, 0x3000, "synthetic", 1, "")],
                }
            ),
            [],
        )

        # The rules are live, not copied -- mutation test one. Raise the floor above the row's
        # instruction count and the row must leave the table, taking the collision with it.
        build = _synthetic_tree(
            os.path.join(scratch, "floor"),
            [
                _verdict(0x1000, 0x2000, verdict="IDENTICAL", insns=20),
                _verdict(0x2000, 0x3000, verdict="IDENTICAL", insns=20),
            ],
            floor=12,
        )
        check("prefix rows above the floor are admitted", len(collisions(emit(build_rules(build)))), 1)
        build = _synthetic_tree(
            os.path.join(scratch, "floor-raised"),
            [
                _verdict(0x1000, 0x2000, verdict="IDENTICAL", insns=20),
                _verdict(0x2000, 0x3000, verdict="IDENTICAL", insns=20),
            ],
            floor=99,
        )
        check("raising MIN_VERIFIED_INSNS in build.rs drops them", collisions(emit(build_rules(build))), [])

        # Mutation test two, and the one a sibling already got wrong: shrink EXHAUSTIVE_VERDICTS
        # and the rows carrying the removed verdict must disappear. A hard-coded copy would not
        # notice, which is how a detour table got reported as 42 rows instead of 374.
        build = _synthetic_tree(
            os.path.join(scratch, "verdicts"),
            [
                _verdict(0x1000, 0x2000, verdict="IDENTICAL-WHOLE", insns=1),
                _verdict(0x2000, 0x3000, verdict="IDENTICAL-WHOLE", insns=1),
            ],
        )
        check("IDENTICAL-WHOLE is admitted while build.rs lists it", len(collisions(emit(build_rules(build)))), 1)
        build = _synthetic_tree(
            os.path.join(scratch, "verdicts-shrunk"),
            [
                _verdict(0x1000, 0x2000, verdict="IDENTICAL-WHOLE", insns=1),
                _verdict(0x2000, 0x3000, verdict="IDENTICAL-WHOLE", insns=1),
            ],
            exhaustive='"BYTE-IDENTICAL", "IDENTICAL-LEAF"',
        )
        check(
            "removing IDENTICAL-WHOLE from build.rs drops those rows",
            collisions(emit(build_rules(build))),
            [],
        )

        # Mutation test three. The prefix verdict is spelled in a match arm rather than a
        # constant, which is exactly why a sibling gate still compares against a stale
        # `"IDENTICAL"`. Rename it in build.rs and the rows carrying the new word must still be
        # admitted; a hard-coded copy would silently drop every one of them.
        build = _synthetic_tree(
            os.path.join(scratch, "renamed-prefix"),
            [
                _verdict(0x1000, 0x2000, verdict="PREFIXMATCH", insns=99),
                _verdict(0x2000, 0x3000, verdict="PREFIXMATCH", insns=99),
            ],
            prefix="PREFIXMATCH",
        )
        rules = build_rules(build)
        check("the prefix verdict is read from the match arm", rules["prefix_verdict"], "PREFIXMATCH")
        check("rows carrying the renamed prefix verdict are still admitted", len(collisions(emit(rules))), 1)

        # Refuse rather than guess. A table constant the model does not know about must stop the
        # run, not be silently skipped -- a partial view reporting zero collisions is the whole
        # defect wearing a green tick.
        build = _synthetic_tree(os.path.join(scratch, "unknown"), [])
        with open(build, "a", encoding="utf-8") as handle:
            handle.write('const FOURTH_MAP: &str = "recon/fourth.tsv";\n')
        try:
            build_rules(build)
            check("an unmodelled table refuses", "no refusal", "Refuse")
        except Refuse as refusal:
            check("the refusal names the constant", "FOURTH_MAP" in str(refusal), True)

        # ...unless build.rs itself says the constant is unread, which is what `let _ = X;` means.
        build = _synthetic_tree(os.path.join(scratch, "inert"), [])
        with open(build, "a", encoding="utf-8") as handle:
            handle.write('const FIFTH_MAP: &str = "recon/fifth.tsv";\nlet _ = FIFTH_MAP;\n')
        try:
            check("an inert constant is tolerated", build_rules(build)["inert"], ["FIFTH_MAP"])
        except Refuse as refusal:
            check("an inert constant is tolerated", f"refused: {refusal}", "no refusal")

        # A build.rs whose shape moved must refuse too, rather than quietly using stale rules.
        build = _synthetic_tree(os.path.join(scratch, "shapeless"), [])
        text = open(build, encoding="utf-8").read().replace("MIN_VERIFIED_INSNS: u32 =", "FLOOR =")
        _write(build, text)
        try:
            build_rules(build)
            check("a moved build.rs refuses", "no refusal", "Refuse")
        except Refuse:
            pass

        # The TRIPWIRE. A public `*_resolved` entry point in a resolving layer is the shape that
        # reintroduces the second resolve.
        owner = os.path.join(scratch, "tripwire", "crates", "er-hook", "src")
        _write(os.path.join(owner, "lib.rs"), "unsafe fn register_union_hook_resolved() {}\n")
        check(
            "a private *_resolved fn is fine",
            resolved_entrypoints(os.path.join(scratch, "tripwire"), RESOLVE_OWNERS),
            [],
        )
        _write(
            os.path.join(owner, "lib.rs"),
            "unsafe fn register_union_hook_resolved() {}\n"
            "pub unsafe fn register_shared_hook_resolved(t: usize) {}\n",
        )
        hits = resolved_entrypoints(os.path.join(scratch, "tripwire"), RESOLVE_OWNERS)
        check("a public *_resolved fn is caught", [name for _, _, name in hits], ["register_shared_hook_resolved"])

    # ------------------------------------------------------------------------------------
    # The claimed-by-no-feature test. This gate did not merely miss something; it passed and
    # recommended a destructive action on the strength of a search that had looked in one
    # spelling. Every control below is checked against `legacy_names_the_address` as well: a
    # control the old matcher also catches would pass on the broken gate and prove nothing.
    # ------------------------------------------------------------------------------------

    # Positive control -- an address declared only as an enum discriminant. This is the live case
    # (`TITLE_MENU_JOB_WAIT_RVA` / `MenuTraceRva::MenuJobWait`, 0xb0d400, on the autoload path)
    # that the old matcher called unclaimed while telling the reader deleting it cost nothing.
    enum_only = (
        "#[repr(u32)]\n"
        "pub enum MenuTraceRva {\n"
        "    TaskEnqueue = 0x007a7b60,\n"
        "    MenuJobWait = 0x00b0d400,\n"
        "}\n"
        "pub const TITLE_MENU_JOB_WAIT_RVA: usize = MenuTraceRva::MenuJobWait as usize;\n"
        "fn use_it(base: usize) -> usize { base + TITLE_MENU_JOB_WAIT_RVA }\n"
    )
    check(
        "the OLD matcher misses an enum-discriminant address (control is non-vacuous)",
        legacy_names_the_address(0xB0D400, enum_only),
        [],
    )
    with tempfile.TemporaryDirectory() as scratch:
        source = os.path.join(scratch, "crates", "er-title-flow", "src", "constants_moved.rs")
        _write(source, enum_only)
        synthetic = rva_symbols.Index.build(root=scratch)
        saved = rva_symbols.index
        rva_symbols.index = lambda root=None, _built=synthetic: _built
        try:
            claimed = claims_on(0xB0D400)
            check(
                "the NEW resolver finds it, and names the declaring symbols",
                sorted(d.qualified for d in claimed.declarations),
                ["MenuTraceRva::MenuJobWait", "TITLE_MENU_JOB_WAIT_RVA"],
            )
            check("...so it is not proven unclaimed", claimed.proven_unclaimed, False)

            # And the remediation line must change with it. The failure was not the matcher on its
            # own; it was the sentence the matcher's silence produced. So the sentence is asserted.
            import io

            spoken = io.StringIO()
            collision = Collision(0x1000, 0xB0D400, 0x3000)
            collision.rows["first"] = Row(0x1000, 0xB0D400, "synthetic", 1, "")
            collision.rows["second"] = Row(0xB0D400, 0x3000, "synthetic", 2, "")
            describe(collision, spoken)
            said = spoken.getvalue()
            check(
                "a claimed address is never called free to delete",
                bool(COSTLESS_CLAIM.search(said)),
                False,
            )
            check("...and the declaring symbol is named", "MenuTraceRva::MenuJobWait" in said, True)
            check("...as is the constant that reaches it", "TITLE_MENU_JOB_WAIT_RVA" in said, True)

            # An address nothing declares, in a tree the resolver fully understands: Proven, and
            # only here may the gate advise a deletion.
            unclaimed = claims_on(0x555000)
            check("an unclaimed address in a resolved tree is PROVEN", unclaimed.proven_unclaimed, True)
            spoken = io.StringIO()
            collision = Collision(0x1000, 0x555000, 0x3000)
            collision.rows["first"] = Row(0x1000, 0x555000, "synthetic", 1, "")
            collision.rows["second"] = Row(0x555000, 0x3000, "synthetic", 2, "")
            # Proven must be DISTINGUISHABLE from the other two branches (or the control is
            # vacuous) and must still not license a deletion -- the second correction, 2026-08-30.
            proven_tables = {
                CALL: [
                    Row(0x1000, 0x555000, "synthetic", 1, ""),
                    Row(0x555000, 0x3000, "synthetic", 2, ""),
                ],
                DETOUR: [
                    Row(0x1000, 0x555000, "synthetic", 1, ""),
                    Row(0x555000, 0x3000, "synthetic", 2, ""),
                ],
            }
            describe(collision, spoken, proven_tables)
            proven_said = spoken.getvalue()
            check("...and PROVEN is still said in those words", "is PROVEN" in proven_said, True)
            check("...distinguishably from NOT PROVEN", "NOT PROVEN" in proven_said, False)
            check(
                "...but PROVEN never becomes a costless claim",
                bool(COSTLESS_CLAIM.search(proven_said)),
                False,
            )
            check(
                "...nor a deletion licence",
                bool(DELETION_ADVICE.search(proven_said)),
                False,
            )
            check(
                "...and the SILENT failure mode of deleting the row is spelled out",
                SILENT in proven_said,
                True,
            )
            # Called without tables, the cost paragraph must be replaced by an admission, never
            # dropped: its silence is what made "at zero cost" sound finished.
            blind = io.StringIO()
            describe(collision, blind)
            check(
                "describe() without tables admits the cost was not computed",
                "WAS NOT COMPUTED" in blind.getvalue(),
                True,
            )
        finally:
            rva_symbols.index = saved

    # ...AND when the RESOLVER cannot read everything, the advice is withheld. Finding nothing in
    # a tree with an unevaluated declaration is "I did not see it", which must not print as "it is
    # not there" -- that collapse is the whole defect.
    with tempfile.TemporaryDirectory() as scratch:
        _write(
            os.path.join(scratch, "crates", "a", "src", "lib.rs"),
            "pub const KNOWN_RVA: usize = 0x111000;\n"
            "pub const OPAQUE_RVA: usize = some_helper(WHATEVER);\n",
        )
        murky = rva_symbols.Index.build(root=scratch)
        saved = rva_symbols.index
        rva_symbols.index = lambda root=None, _built=murky: _built
        try:
            import io

            unknown = claims_on(0x555000)
            check("an unreadable declaration keeps the answer unproven", unknown.proven_unclaimed, False)
            check("...even though nothing was found", unknown.found_nothing, True)
            spoken = io.StringIO()
            collision = Collision(0x1000, 0x555000, 0x3000)
            collision.rows["first"] = Row(0x1000, 0x555000, "synthetic", 1, "")
            collision.rows["second"] = Row(0x555000, 0x3000, "synthetic", 2, "")
            describe(collision, spoken)
            said = spoken.getvalue()
            check(
                "an unproven address is NEVER called free to delete",
                bool(COSTLESS_CLAIM.search(said)) or bool(DELETION_ADVICE.search(said)),
                False,
            )
            check("...and the gate says so in those words", "NOT PROVEN" in said, True)
            check("...naming what it could not read", "OPAQUE_RVA" in said, True)
        finally:
            rva_symbols.index = saved

    # The NOTE is advice too. A baselined note that says a row costs nothing to delete is refused
    # unless the address is proven unclaimed, because a note outlives the reasoning behind it and
    # is what a reader actually sees.
    noted = Collision(0x1000, 0xB0D400, 0x3000)
    check(
        "a note advising deletion of a CLAIMED address is refused",
        [c.label() for c, _, _, _ in unearned_deletion_advice(
            [noted],
            {noted.key(): {"routes": set(), "note": "carries no feature, delete at zero cost"}},
        )],
        [noted.label()],
    )
    check(
        "a note that advises nothing is left alone",
        unearned_deletion_advice(
            [noted], {noted.key(): {"routes": set(), "note": "both rows are load-bearing"}}
        ),
        [],
    )
    # ...AND A COSTLESS claim is refused even when the address *is* proven UNCLAIMED. Nothing in
    # this repo computes the cost of a deletion, and for this shape the cost of being wrong is a
    # silent misroute, so the phrase can never be earned. 0x555000 is unclaimed in the real tree.
    free = Collision(0x1000, 0x555000, 0x3000)
    check(
        "a costless claim is refused even on a PROVEN-unclaimed address",
        [c.label() for c, _, _, _ in unearned_deletion_advice(
            [free],
            {free.key(): {"routes": set(), "note": "nothing declares it, so this costs nothing"}},
        )],
        [free.label()],
    )
    check(
        "...and the refusal says which of the two grounds it is",
        [why for _, _, _, why in unearned_deletion_advice(
            [free],
            {free.key(): {"routes": set(), "note": "nothing declares it, so this costs nothing"}},
        )][0].startswith("it claims a deletion is costless"),
        True,
    )

    # The cost of being wrong, computed rather than assumed. `A -> B` and `B -> C`: delete row B
    # and B is a destination that no row sources, so `already_translated_in` claims it and the
    # resolver hands it back UNTRANSLATED. That is the silent direction, and it is why "at zero
    # cost" could never be said about one of these rows.
    shape = Collision(0x1000, 0x2000, 0x3000)
    rows = [Row(0x1000, 0x2000, "synthetic", 1, ""), Row(0x2000, 0x3000, "synthetic", 2, "")]
    check(
        "deleting row B makes a later request SILENT, not loud",
        deletion_failure_mode({CALL: rows, DETOUR: rows}, shape),
        {CALL: (SILENT, 0x2000), DETOUR: (SILENT, 0x2000)},
    )
    # The control that makes it non-vacuous: an address that is not any surviving row's
    # destination is genuinely refused, which is the loud direction the old advice assumed.
    lone = Collision(0x1000, 0x2000, 0x3000)
    check(
        "an address no surviving row lands on is REFUSED instead, which is loud",
        deletion_failure_mode({CALL: [Row(0x2000, 0x3000, "synthetic", 1, "")]}, lone),
        {CALL: (LOUD, None)},
    )
    # ...and a duplicate row keying the same source keeps the translation alive.
    check(
        "a second row on the same source keeps it translated",
        deletion_failure_mode(
            {
                CALL: [
                    Row(0x1000, 0x2000, "synthetic", 1, ""),
                    Row(0x2000, 0x3000, "synthetic", 2, ""),
                    Row(0x2000, 0x4000, "synthetic", 3, ""),
                ]
            },
            shape,
        ),
        {CALL: (STILL_MAPPED, 0x4000)},
    )
    # And the reproduction of `already_translated_in` must agree with the Rust on the case the
    # whole rule turns on: an address that is both a destination and a source is not claimed by
    # the shortcut, because translation has to win for real sources.
    check(
        "the shortcut declines on an address that is both a destination and a source",
        _already_translated_in([(0x1000, 0x2000), (0x2000, 0x3000)], 0x2000),
        False,
    )
    check(
        "...and claims one that is only a destination",
        _already_translated_in([(0x1000, 0x2000)], 0x2000),
        True,
    )
    check(
        "...and declines a row that did not move",
        _already_translated_in([(0x2000, 0x2000)], 0x2000),
        False,
    )

    # The live case, against the real tree rather than a fixture: 0xb0d400 is claimed today, and
    # the frozen legacy matcher still cannot see it. If this ever fails because the constant was
    # legitimately renamed or retired, pick another enum-discriminant address from
    # `crates/er-title-flow/src/constants_moved.rs` -- do not delete the control, which is the only
    # thing standing between this gate and the advice it used to give.
    live_claims = claims_on(0xB0D400)
    check(
        "the real tree still claims 0xb0d400 by enum discriminant",
        sorted({d.qualified for d in live_claims.declarations}),
        ["MenuTraceRva::MenuJobWait", "TITLE_MENU_JOB_WAIT_RVA"],
    )
    with open(
        os.path.join(ROOT, "crates", "er-title-flow", "src", "constants_moved.rs"),
        encoding="utf-8",
    ) as handle:
        check(
            "and the OLD matcher still misses it in the real file",
            legacy_names_the_address(0xB0D400, handle.read()),
            [],
        )

    # The CALL/DETOUR split, driven end to end on a synthetic tree. A call-only verdict must reach
    # one map and not the other, and the control beside it is a detourable row in the same file --
    # without that, a model which simply dropped the whole verified table would pass the first
    # assertion and be wrong about everything.
    with tempfile.TemporaryDirectory() as scratch:
        build = _synthetic_tree(
            scratch,
            [],
            verified_rows=[
                _verdict(0x1000, 0x2000, verdict="IDENTICAL-LEAF-NOPATCH"),
                _verdict(0x3000, 0x4000, verdict="BYTE-IDENTICAL"),
            ],
        )
        split = emit(build_rules(build))
        check(
            "a CALL-only row reaches the CALL map",
            0x1000 in {row.src for row in split[CALL]},
            True,
        )
        check(
            "...and is refused the DETOUR map",
            0x1000 in {row.src for row in split[DETOUR]},
            False,
        )
        check(
            "...while a detourable row in the same table reaches both",
            (
                0x3000 in {row.src for row in split[CALL]},
                0x3000 in {row.src for row in split[DETOUR]},
            ),
            (True, True),
        )
        # And the negative control on the rule rather than on the row: with the call-only
        # vocabulary spelled differently in build.rs, the same table admits the row nowhere. A
        # model that had hard-coded the verdict word would still admit it and pass above.
        renamed = _synthetic_tree(
            os.path.join(scratch, "renamed"),
            [],
            callable_only='"SOMETHING-ELSE"',
            verified_rows=[_verdict(0x1000, 0x2000, verdict="IDENTICAL-LEAF-NOPATCH")],
        )
        blind = emit(build_rules(renamed))
        check(
            "the CALL-only vocabulary is READ from build.rs, not spelled here",
            (
                0x1000 in {row.src for row in blind[CALL]},
                0x1000 in {row.src for row in blind[DETOUR]},
            ),
            (False, False),
        )

    # The real tree's rules must parse. A gate that only ever runs against its own fixtures is a
    # fixture.
    try:
        rules = build_rules()
        check("the real build.rs still declares every modelled table", sorted(rules["paths"]), sorted(ROLES))
        check("the prefix verdict is read, not assumed", rules["prefix_verdict"], "IDENTICAL")
        check(
            "the CALL-only verdict is read from the real build.rs",
            rules["callable_only"],
            ("IDENTICAL-LEAF-NOPATCH",),
        )
        check(
            "and it is disjoint from both detour lists",
            set(rules["callable_only"]) & (set(rules["exhaustive"]) | set(rules["patch_site"])),
            set(),
        )
    except Refuse as refusal:
        failures.append(f"the real build.rs no longer parses: {refusal}")

    for failure in failures:
        print(f"selftest FAILED -- {failure}", file=sys.stderr)
    if failures:
        return 1
    print("selftest: OK")
    return 0


# --------------------------------------------------------------------------------------------


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--strict", action="store_true", help="ignore the baseline; any collision fails")
    parser.add_argument("--list", action="store_true", help="describe every collision, baselined or not")
    parser.add_argument("--against", metavar="PATH", help="compare the model with a generated address map")
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()

    if args.selftest:
        return selftest()

    try:
        rules = build_rules()
    except Refuse as refusal:
        print(f"REFUSED: {refusal}", file=sys.stderr)
        return 2

    tables = emit(rules)
    print(
        f"crates/er-game-base/build.rs: CALL {len(tables[CALL])} rows, "
        f"DETOUR {len(tables[DETOUR])} rows"
        + (f" (inert, not read: {', '.join(rules['inert'])})" if rules["inert"] else "")
    )

    # Cross-check the model against a table cargo really wrote, when a fresh one exists. A
    # difference that does not change the collision verdict is a note; one that does is fatal,
    # because then the answer below is not about the table the build ships.
    generated_path, why_not = (args.against, None) if args.against else newest_generated(rules)
    if generated_path:
        generated = parse_generated(generated_path)
        modelled = {name: [(row.src, row.dst) for row in rows] for name, rows in tables.items()}
        differing = [name for name in (CALL, DETOUR) if modelled[name] != generated[name]]
        if differing:
            shadow = {
                name: [Row(src, dst, generated_path, 0, "") for src, dst in generated[name]]
                for name in (CALL, DETOUR)
            }
            # Routes count, not just the triples: which table carries a collision decides whether
            # it can misplace a detour, and it is what the baseline's escalation check compares.
            def fingerprint(rows):
                return [(c.key(), sorted(c.routes)) for c in collisions(rows)]

            same_verdict = fingerprint(shadow) == fingerprint(tables)
            print(
                f"  MODEL DRIFT against {shown(generated_path)}: "
                + ", ".join(f"{name} modelled {len(modelled[name])} vs generated {len(generated[name])}" for name in differing)
            )
            if not same_verdict:
                print(
                    "  and the drift CHANGES the collision set, so nothing below is trustworthy.\n"
                    "  Re-read emit_address_map in build.rs and fix this gate's model of it.",
                    file=sys.stderr,
                )
                return 2
            print("  (the collision set is identical either way, so the verdict below still holds)")
        else:
            print(f"  model matches {shown(generated_path)} exactly")
    elif why_not:
        print(f"  generated-map cross-check skipped: {why_not}")

    found = collisions(tables)
    baseline = read_baseline()
    known, novel, escalated = [], [], []
    for collision in found:
        recorded = baseline.get(collision.key())
        if recorded is None or args.strict:
            novel.append(collision)
        elif collision.routes - recorded["routes"]:
            escalated.append((collision, recorded))
        else:
            known.append(collision)
    stale = [key for key in baseline if key not in {c.key() for c in found}]

    if known:
        print(f"\n{len(known)} KNOWN collision(s), recorded in {os.path.relpath(BASELINE, ROOT)} and NOT fixed:")
        for collision in known:
            note = baseline[collision.key()]["note"]
            print(f"  {collision.label()}   {note}")
            if args.list:
                describe(collision, sys.stdout, tables)

    if stale:
        print(f"\nBASELINE STALE -- {len(stale)} recorded collision(s) no longer exist. Delete from")
        print(f"{os.path.relpath(BASELINE, ROOT)}:")
        for first, address, second in stale:
            print(f"  0x{first:x}\t0x{address:x}\t0x{second:x}")

    tripwire = resolved_entrypoints()
    failed = False

    unearned = unearned_deletion_advice(known, baseline)
    if unearned:
        print(
            f"\n{len(unearned)} baselined note(s) carry advice this gate cannot stand behind:",
            file=sys.stderr,
        )
        for collision, note, claims, why in unearned:
            print(
                f"\n  {collision.label()}\n    note says: {note}\n    refused because: {why}",
                file=sys.stderr,
            )
            if COSTLESS_CLAIM.search(note):
                for name, (kind, landing) in sorted(
                    deletion_failure_mode(tables, collision).items()
                ):
                    where = f" -> 0x{landing:x}" if landing is not None else ""
                    print(
                        f"    ...and on the {name} map a later request for "
                        f"0x{collision.address:x} after that deletion would be {kind}{where}.",
                        file=sys.stderr,
                    )
                continue
            if claims is None:
                print("    ...and the symbol resolver could not run to check it.", file=sys.stderr)
            elif claims.declarations or claims.literals:
                print(
                    f"    ...but 0x{collision.address:x} IS claimed: "
                    + ", ".join(sorted({d.qualified for d in claims.declarations}) or ["a bare literal"]),
                    file=sys.stderr,
                )
            else:
                print(
                    f"    ...and nothing claims it SO FAR AS THE RESOLVER COULD SEE, with "
                    f"{len(claims.residue)} declaration(s) it could not evaluate. Finding nothing "
                    "is not\n    finding it absent. Rewrite the note, or finish the proof "
                    f"(python3 scripts/rva_symbols.py --residue 0x{collision.address:x}).",
                    file=sys.stderr,
                )
        failed = True

    if novel:
        label = "collision(s)" if args.strict else "NEW collision(s), not in the baseline"
        print(f"\n{len(novel)} {label}:", file=sys.stderr)
        preamble(sys.stderr)
        for collision in novel:
            describe(collision, sys.stderr, tables)
        epilogue(sys.stderr)
        print("If both rows must stay, add these lines to " + os.path.relpath(BASELINE, ROOT) + ":", file=sys.stderr)
        for collision in novel:
            print("  " + baseline_line(collision, "<why both rows must stay>"), file=sys.stderr)
        failed = True

    for collision, recorded in escalated:
        print(
            f"\nESCALATED: the known collision {collision.label()} now also applies to "
            + ", ".join(sorted(collision.routes - recorded["routes"]))
            + f"\n  (it was recorded as {','.join(sorted(recorded['routes']))}). A collision that "
            "reaches the DETOUR map is\n  no longer only a wrong call target: MinHook writes five "
            "bytes at the wrong address.",
            file=sys.stderr,
        )
        describe(collision, sys.stderr, tables)
        failed = True

    if tripwire:
        print(
            "\nDOUBLE-RESOLVE TRIPWIRE: a PUBLIC function whose name says its argument is already\n"
            "resolved now exists in a layer that resolves addresses. That is the shape that made\n"
            "the collisions above reachable -- a caller resolves, then hands the result to\n"
            "something that resolves again, and a colliding address translates a second time to a\n"
            "different function with no error. Keep such helpers PRIVATE, as er-hook's\n"
            "`register_union_hook_resolved` is, and let one layer own the single resolve:",
            file=sys.stderr,
        )
        for path, number, name in tripwire:
            print(f"  {path}:{number}  {name}", file=sys.stderr)
        failed = True

    if failed:
        return 1
    print(
        f"\nno new translation collisions ({len(found)} known, {len(tables[CALL])} CALL rows, "
        f"{len(tables[DETOUR])} DETOUR rows) and no public *_resolved entry point."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
