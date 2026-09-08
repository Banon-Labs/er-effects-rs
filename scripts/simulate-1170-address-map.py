#!/usr/bin/env python3
"""What would the generated 1.16.2 -> 1.17 address map contain if a ledger row changed?

`crates/er-game-base/build.rs` turns four TSV ledgers into two tables -- `VERIFIED_1162_TO_1170`
(may be CALLed or read) and `DETOUR_SAFE_1162_TO_1170` (may additionally be hooked). The rules it
applies are not obvious: most verdicts that fall short of a detour are dropped from both tables
rather than merely denied one, `CALLABLE_ONLY_VERDICTS` is the single exception that reaches the
call map alone, and one `DIVERGES` row is subtracted from both tables across ledgers.

So "what does this ledger edit actually do" cannot be answered by reading the diff, and answering
it by rebuilding means a cross-compile per scenario -- and mutating a tracked ledger to measure.
This re-implements `emit_address_map` and counts, which makes a verdict-policy decision measurable
before it is taken.

It is not trusted on its own say-so. `--against <address_map_1170.rs>` compares the simulation to
a table cargo really generated and fails on any difference, so a drift in build.rs's rules shows
up as a red comparison rather than as a confident wrong number.

Usage
    python3 scripts/simulate-1170-address-map.py
    python3 scripts/simulate-1170-address-map.py --verified <alternative verified.tsv>
    python3 scripts/simulate-1170-address-map.py --against target/.../out/address_map_1170.rs
"""

import argparse
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RECON = os.path.join(ROOT, "docs", "recon")
BASE = 0x140000000
BUILD_RS = os.path.join(ROOT, "crates", "er-game-base", "build.rs")


def build_rs_rules(path=BUILD_RS):
    """The admission rules read out of `build.rs`, not transcribed from it.

    Transcribing them is how this tool would lie. Measured 2026-08-30, within an hour of the first
    version being written: a sibling added the verdicts `IDENTICAL-WHOLE` and `IDENTICAL-LEAF`, and
    a hard-coded copy of the old list silently dropped 295 rows and reported the detour map as 42
    instead of 374 -- a confident number, wrong by nine-fold, out of a tool whose whole job is to be
    trusted about counts. Parsing the source cannot drift; `--against` still proves the rest.
    """
    text = open(path, encoding="utf-8").read()
    floor = re.search(r"MIN_VERIFIED_INSNS: u32 = (\d+)", text)
    exhaustive = re.search(r"EXHAUSTIVE_VERDICTS: \[&str; \d+\] = \[([^\]]*)\]", text)
    patch_site = re.search(r"PATCH_SITE_VERDICTS: \[&str; \d+\] = \[([^\]]*)\]", text)
    # Required, not optional-with-a-default. A missing list read as an empty one would silently
    # simulate the world as it was before the call-only verdict existed -- the same class of
    # confident wrong number the hard-coded verdict list produced within an hour of this file
    # being written. Failing closed makes a build.rs rename a red tool, not a quiet undercount.
    callable_only = re.search(r"CALLABLE_ONLY_VERDICTS: \[&str; \d+\] = \[([^\]]*)\]", text)
    entry = re.search(r"DETOURABLE_ENTRY_EVIDENCE: \[&str; \d+\] = \[([^\]]*)\]", text)
    if not (floor and exhaustive and patch_site and callable_only and entry):
        sys.exit(f"cannot read the admission rules out of {path}; its shape changed")
    return (
        int(floor.group(1)),
        tuple(re.findall(r'"([^"]+)"', exhaustive.group(1))),
        tuple(re.findall(r'"([^"]+)"', patch_site.group(1))),
        tuple(re.findall(r'"([^"]+)"', callable_only.group(1))),
        tuple(re.findall(r'"([^"]+)"', entry.group(1))),
    )


(
    MIN_VERIFIED_INSNS,
    EXHAUSTIVE_VERDICTS,
    PATCH_SITE_VERDICTS,
    CALLABLE_ONLY_VERDICTS,
    DETOURABLE_ENTRY_EVIDENCE,
) = build_rs_rules()
# The property the CALL/DETOUR split rests on, checked in the tool that measures the split. A
# verdict in both lists would make this simulation agree with a build.rs that licenses the hook it
# means to refuse -- agreeing with the bug rather than exposing it.
_overlap = set(CALLABLE_ONLY_VERDICTS) & (set(EXHAUSTIVE_VERDICTS) | set(PATCH_SITE_VERDICTS))
if _overlap:
    sys.exit(f"build.rs puts {sorted(_overlap)} in CALLABLE_ONLY_VERDICTS and in a detour list")

VERIFIED = os.path.join(RECON, "rva-map-1162-to-1170.verified.tsv")
FUNCTION_MAP = os.path.join(RECON, "rva-map-1162-to-1170.needed.tsv")
DATA_MAP = os.path.join(RECON, "rva-map-1162-to-1170.data.tsv")
NEEDED_VERIFIED = os.path.join(RECON, "rva-map-1162-to-1170.needed-verified.tsv")
QUARANTINE = os.path.join(RECON, "rva-1170-quarantine.tsv")


def rows(path):
    try:
        with open(path, encoding="utf-8") as handle:
            text = handle.read()
    except FileNotFoundError:
        return []
    return [line for line in text.splitlines() if line.strip() and not line.startswith("#")]


def detourable_pairs(path):
    """Rows good enough to carry a detour -- and, from the verified table, to seed the call map."""
    out = []
    for line in rows(path):
        fields = line.split("\t")
        if len(fields) < 7:
            continue
        if fields[2] in EXHAUSTIVE_VERDICTS:
            pass
        elif fields[2] in PATCH_SITE_VERDICTS:
            # Bodies differ, patch site does not. Floor-exempt for the same reason as above.
            pass
        elif fields[2] == "IDENTICAL":
            try:
                compared = int(fields[4].strip())
            except ValueError:
                compared = 0
            if compared < MIN_VERIFIED_INSNS:
                continue
        else:
            continue
        if fields[6].strip() not in DETOURABLE_ENTRY_EVIDENCE:
            continue
        try:
            out.append((int(fields[0], 16) - BASE, int(fields[1], 16) - BASE))
        except ValueError:
            continue
    return out


def callable_only_pairs(path):
    """Rows a verdict table admits to the call map and to nothing else.

    Its own function, mirroring `build.rs::callable_only_pairs` -- which is also its own function
    there, and for the same reason: the detour set must be reachable only through
    `detourable_pairs`, in both the build and the tool that predicts it.
    """
    out = []
    for line in rows(path):
        fields = line.split("\t")
        if len(fields) < 7 or fields[2] not in CALLABLE_ONLY_VERDICTS:
            continue
        if fields[6].strip() not in DETOURABLE_ENTRY_EVIDENCE:
            continue
        try:
            out.append((int(fields[0], 16) - BASE, int(fields[1], 16) - BASE))
        except ValueError:
            continue
    return out


def refuted_sources(path):
    out = []
    for line in rows(path):
        fields = line.split("\t")
        if len(fields) < 3 or fields[2] != "DIVERGES":
            continue
        try:
            out.append(int(fields[0], 16) - BASE)
        except ValueError:
            continue
    return out


def quarantined(path):
    out = []
    for line in rows(path):
        try:
            out.append(int(line.split("\t")[0], 16))
        except ValueError:
            continue
    return out


def emit(verified=VERIFIED):
    """`(call_rows, detour_rows)`, each sorted and deduplicated by source RVA."""
    # `detour` is taken from `detourable_pairs` before the callable-only rows join `call`. The
    # old `list(call)` was correct only while the two seeds were the same set; they are not, and
    # copying `call` after the extend below would hand every call-only row a detour.
    detour = detourable_pairs(verified) + detourable_pairs(NEEDED_VERIFIED)
    call = detourable_pairs(verified) + callable_only_pairs(verified)
    seeded = {old for old, _ in call}
    for path, exclude in ((FUNCTION_MAP, seeded), (DATA_MAP, None)):
        known = exclude if exclude is not None else {old for old, _ in call}
        for line in rows(path):
            fields = line.split("\t")
            if len(fields) < 2:
                continue
            try:
                old, new = int(fields[0], 16), int(fields[1], 16)
            except ValueError:
                continue
            if old not in known:
                call.append((old, new))
    held_back = (
        set(quarantined(QUARANTINE))
        | set(refuted_sources(NEEDED_VERIFIED))
        | set(refuted_sources(verified))
    )

    def finish(pairs):
        kept = sorted(pair for pair in pairs if pair[0] not in held_back)
        out, last = [], None
        for pair in kept:
            if pair[0] != last:
                out.append(pair)
                last = pair[0]
        return out

    return finish(call), finish(detour)


def parse_generated(path):
    """The two tables out of a cargo-generated `address_map_1170.rs`."""
    with open(path, encoding="utf-8") as handle:
        text = handle.read()
    tables = {}
    for name in ("VERIFIED_1162_TO_1170", "DETOUR_SAFE_1162_TO_1170"):
        body = re.search(rf"const {name}: \[\(u32, u32\); \d+\] = \[(.*?)\];", text, re.S)
        tables[name] = (
            [
                (int(a, 16), int(b, 16))
                for a, b in re.findall(r"\((0x[0-9a-f]+), (0x[0-9a-f]+)\)", body.group(1))
            ]
            if body
            else []
        )
    return tables["VERIFIED_1162_TO_1170"], tables["DETOUR_SAFE_1162_TO_1170"]


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--verified", default=VERIFIED, help="verified ledger to simulate with")
    parser.add_argument("--against", metavar="PATH", help="compare with a generated address map")
    args = parser.parse_args()

    call, detour = emit(args.verified)
    print(f"{args.verified}\n  CALL {len(call)}  DETOUR {len(detour)}")

    if args.against:
        want_call, want_detour = parse_generated(args.against)
        bad = 0
        for name, got, want in (
            ("CALL", call, want_call),
            ("DETOUR", detour, want_detour),
        ):
            if got != want:
                bad += 1
                only_got = set(got) - set(want)
                only_want = set(want) - set(got)
                print(
                    f"  {name} MISMATCH: simulated {len(got)}, generated {len(want)}; "
                    f"{len(only_got)} only simulated, {len(only_want)} only generated"
                )
                for old, new in sorted(only_got)[:10]:
                    print(f"    only simulated 0x{old:x} -> 0x{new:x}")
                for old, new in sorted(only_want)[:10]:
                    print(f"    only generated 0x{old:x} -> 0x{new:x}")
            else:
                print(f"  {name} matches the generated table exactly ({len(got)} rows)")
        return 1 if bad else 0
    return 0


if __name__ == "__main__":
    sys.exit(main())
