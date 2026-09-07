#!/usr/bin/env python3
"""Forbid comparing a live stack address against a raw 1.16.2 RVA.

Why
===
Every other stale-address class in this workspace announces itself. A detour goes through
`er-hook`, which logs `HOOK REFUSED`. A call goes through `er_game_base::mem::game_rva`, which
logs `ADDRESS REFUSED`. `scripts/check-stale-rva-calls.py` ratchets the ones that do not.

Call sites have no such moment. `trace_first_game_caller_rva()` and
`callstack_contains_game_rva(start, end)` take a return address off the live stack with
`RtlCaptureStackBackTrace`, subtract the module base, and compare the result against a 1.16.2
constant. Nothing is resolved, so nothing can be refused: on a build that moved the code the
comparison simply never matches. No log line of any kind is produced, and the feature behind it
is silently dead.

Measured, 2026-08-30. Nine such comparisons existed. Two shipped user-visible features:

  * the Load Character / Load Character from File / Load Build from URL rows were never cloned
    onto the System>Quit tab, because `SYSTEM_QUIT_DUPLICATE_TARGET_RETURN_RVA` (0x958a20) never
    matched a 1.17 frame;
  * the title FadeIn suppression never fired, because
    `TITLE_GFX_VISIBLE_TITLE_FADEIN_CALLER_RVA` (0x744e02) never matched either.

Both addresses are mid-function, so neither could ever appear in the 1.16.2 -> 1.17 map: that map
is keyed on `.pdata` function starts, and `scripts/select-needed-1170-rows.py` could not see them.
They were invisible to every tool the migration has.

The rule
========
Name the containing function and add the offset at the use site, then resolve through
`er_game_base::game_build::resolve_call_site_rva` (or `resolve_call_site_band` for a window):

    // before -- unmappable, and silent when it stops matching
    const FOO_CALLER_RVA: usize = 0x744e02;
    if caller_rva == FOO_CALLER_RVA { ... }

    // after -- the function is mappable, the offset rides along, a refusal is logged
    const FOO_FN_RVA: usize = 0x744dd0;
    const FOO_CALL_OFFSET: usize = 0x32;
    if resolve_call_site_rva(FOO_FN_RVA, FOO_CALL_OFFSET, "FOO_FN_RVA") == Some(caller_rva) { ... }

`scripts/derive-callsite-1170.py <addr>` prints the evidence for the split: the `.pdata` record
containing the address, the map's pair for that function, and the callee each image's `E8` reaches
at the same offset.

Why not just put the return address in the map
==============================================
Because a verdict-table row licenses a detour. `DETOURABLE_ENTRY_EVIDENCE` in
`er-game-base/build.rs` accepts `NEITHER-ENTRY` -- deliberately, for a pair that sits the same
distance before a Ghidra-named entry in both images -- so a row for a mid-function address would
be accepted into `DETOUR_SAFE_1162_TO_1170` and MinHook would then write five bytes into the
middle of a live function. Resolving the function and adding the offset in Rust keeps the offset
out of every table.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
CRATES_DIR = REPO_ROOT / "crates"

# A hex literal, with the `_` separators this workspace uses in RVA constants.
HEX = r"0x[0-9a-fA-F_]+"
# A named 1.16.2 address constant. `_RVA` is this workspace's own convention, which is what makes
# the set findable; the bound suffixes (`_MIN`, `_MAX`, ...) are part of the name, not a separate
# thing, so `GX_CMD_QUEUE_WRAPPER_RVA_MIN` has to match too.
RVA_NAME = r"[A-Z][A-Z0-9_]*RVA[A-Z0-9_]*"
# One argument of the callstack predicates: either a bare hex literal or a named RVA constant,
# optionally with arithmetic hung off it (`FOO_RVA as usize + 0x360`, `FOO_RVA.saturating_sub(W)`).
STALE_ARG = rf"(?:{HEX}|{RVA_NAME})"

FORBIDDEN = (
    (
        "callstack_contains_game_rva() compared against a raw 1.16.2 address -- resolve the "
        "containing function with er_game_base::game_build::resolve_call_site_band first",
        re.compile(rf"\bcallstack_contains_game_rva\s*\(\s*{STALE_ARG}\b"),
    ),
    (
        "stack_producer_rva() given a raw 1.16.2 band -- resolve it with "
        "er_game_base::game_build::resolve_call_site_band first",
        re.compile(rf"\bstack_producer_rva\s*\(\s*{STALE_ARG}\b"),
    ),
    (
        "a live caller RVA compared against a raw 1.16.2 constant -- resolve the containing "
        "function with er_game_base::game_build::resolve_call_site_rva first",
        re.compile(rf"\b\w*caller_rva\w*\s*(?:==|!=)\s*{STALE_ARG}\b"),
    ),
    (
        "a raw 1.16.2 constant compared against a live caller RVA -- resolve the containing "
        "function with er_game_base::game_build::resolve_call_site_rva first",
        re.compile(rf"\b{STALE_ARG}\s*(?:==|!=)\s*\w*caller_rva\w*\b"),
    ),
    (
        "a live caller RVA range-tested against raw 1.16.2 constants -- resolve the containing "
        "function with er_game_base::game_build::resolve_call_site_band first",
        re.compile(rf"\(\s*{STALE_ARG}\s*\.\.=?\s*{STALE_ARG}\s*\)\s*\.contains\s*\(\s*&?\w*caller_rva"),
    ),
)

# `AV_GAME_TEXT_RVA_MIN`/`_MAX` are 0x1000 and 0x4000000: where `.text` begins and a generous
# upper bound. They are not a 1.16.2 fact -- they are "is this address plausibly game code at
# all" -- so they mean the same thing on every build and are the one legitimate raw comparison.
EXEMPT_CONSTANTS = ("AV_GAME_TEXT_RVA_MIN", "AV_GAME_TEXT_RVA_MAX")


# How many following lines to fold into the probe for one source line.
#
# Not cosmetic. `rustfmt` wraps a predicate whose arguments do not fit, and the constant then lands
# on the next line:
#
#     let first_row_call = callstack_contains_game_rva(
#         SYSTEM_QUIT_DUPLICATE_TARGET_RETURN_RVA
#             .saturating_sub(SYSTEM_QUIT_DUPLICATE_CALLER_WINDOW_BYTES),
#
# A line-at-a-time scanner sees `callstack_contains_game_rva(` with nothing after it and passes.
# Measured against the pre-change tree: line-at-a-time caught 4 of the 9 real sites; folding four
# following lines catches all 9. The formatter's own wrapping was hiding more than half of them.
JOIN_LINES = 4


def scan_text(path_label: str, text: str) -> list[str]:
    """Every stale live-address comparison in `text`, as reportable lines."""
    findings: list[str] = []
    lines = text.split("\n")
    for index, line in enumerate(lines):
        stripped = line.lstrip()
        # Comments and doc comments describe the rule (this file's own message included);
        # only real code is a violation.
        if stripped.startswith("//"):
            continue
        # The probe stops at the first comment line, so prose below a call cannot create a hit.
        probe_parts = [stripped]
        for follower in lines[index + 1 : index + 1 + JOIN_LINES]:
            follower = follower.lstrip()
            if follower.startswith("//"):
                break
            probe_parts.append(follower)
        probe = " ".join(probe_parts)
        if any(name in probe for name in EXEMPT_CONSTANTS):
            continue
        for description, pattern in FORBIDDEN:
            match = pattern.search(probe)
            # Anchored to this line: the match must start within the first line's own text, so a
            # call is reported once, at the line that opens it, rather than once per line above it.
            if match and match.start() < len(stripped):
                findings.append(f"{path_label}:{index + 1}: {description}\n    {stripped[:140]}")
                break
    return findings


def scan_repo() -> list[str]:
    findings: list[str] = []
    for path in sorted(CRATES_DIR.rglob("*.rs")):
        findings.extend(
            scan_text(
                str(path.relative_to(REPO_ROOT)),
                path.read_text(encoding="utf-8", errors="replace"),
            )
        )
    return findings


def selftest() -> int:
    """Drive the scanner over synthetic sources, so the gate is never trusted on its own say-so.

    The bad fixture is the real set this gate was written for: every line below is one of the
    nine comparisons that existed on 2026-08-30, transcribed.
    """
    failures: list[str] = []

    bad = "\n".join(
        (
            "    if dialog >= 0x10000 && callstack_contains_game_rva(0x7a3000, 0x7a4000) {",
            "    let a = callstack_contains_game_rva(SYSTEM_QUIT_DUPLICATE_TARGET_RETURN_RVA, x);",
            "    let b = callstack_contains_game_rva(RESULT_ACTION_BUILDER_RVA as usize, y);",
            "    let (p, s) = stack_producer_rva(GX_CMD_QUEUE_WRAPPER_RVA_MIN..GX_CMD_QUEUE_WRAPPER_RVA_MAX);",
            "    if caller_rva == TITLE_GFX_VISIBLE_TITLE_FADEIN_CALLER_RVA && visible != 0 {",
            "    let kind = if caller_rva == 0x0076432c {",
            "    if MENU_CONTINUE_IDLE_INSERT_CALLER_RVA == caller_rva {",
            "    if (LO_RVA..HI_RVA).contains(&caller_rva) {",
        )
    )
    hits = scan_text("fixture.rs", bad)
    if len(hits) != 8:
        failures.append(f"expected 8 violations in the bad fixture, got {len(hits)}: {hits}")

    # The form the FORMATTER produces, and the reason `JOIN_LINES` exists. A line-at-a-time
    # scanner passes this, and five of the nine real sites looked exactly like it.
    wrapped = "\n".join(
        (
            "    let first_row_call = callstack_contains_game_rva(",
            "        SYSTEM_QUIT_DUPLICATE_TARGET_RETURN_RVA",
            "            .saturating_sub(SYSTEM_QUIT_DUPLICATE_CALLER_WINDOW_BYTES),",
            "        SYSTEM_QUIT_DUPLICATE_TARGET_RETURN_RVA + SYSTEM_QUIT_DUPLICATE_CALLER_WINDOW_BYTES,",
            "    );",
        )
    )
    hits = scan_text("fixture.rs", wrapped)
    if len(hits) != 1:
        failures.append(f"expected 1 violation in the wrapped fixture, got {len(hits)}: {hits}")
    elif not hits[0].startswith("fixture.rs:1:"):
        failures.append(f"a wrapped call must be reported at the line that OPENS it: {hits[0]}")

    good = "\n".join(
        (
            "    let site = resolve_call_site_rva(FOO_FN_RVA, FOO_CALL_OFFSET, \"FOO_FN_RVA\");",
            "    if site == Some(caller_rva) && visible != 0 {",
            "    let band = resolve_call_site_band(FOO_FN_RVA, 0, 0x360, \"FOO_FN_RVA\");",
            "    band.is_some_and(|b| callstack_contains_game_rva(b.start, b.end));",
            "    let (p, s) = stack_producer_rva(band);",
            "    if !(AV_GAME_TEXT_RVA_MIN..AV_GAME_TEXT_RVA_MAX).contains(&rva) { continue; }",
            "    // if caller_rva == SOME_RVA is the rule being explained, not a comparison",
            "    /// `caller_rva == 0x744e02` in a doc comment is prose, not code",
        )
    )
    hits = scan_text("fixture.rs", good)
    if hits:
        failures.append(f"clean fixture must not report violations, got {hits}")

    # The gate has to see the real repo as clean, or it is enforcing nothing.
    live = scan_repo()
    if live:
        failures.append(
            "the repo itself must be clean for this gate to mean anything; found: " + str(live)
        )

    for failure in failures:
        print(f"selftest FAILED: {failure}", file=sys.stderr)
    if failures:
        return 1
    print("check-no-stale-callsite-rva selftest: ok")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--selftest",
        action="store_true",
        help="run the scanner against synthetic fixtures instead of the repo",
    )
    args = parser.parse_args()
    if args.selftest:
        return selftest()

    findings = scan_repo()
    if findings:
        print("a live stack address compared against a raw 1.16.2 RVA:", file=sys.stderr)
        for finding in findings:
            print(f"  {finding}", file=sys.stderr)
        print(
            "\nThis is the one stale-address class that produces NO log line when it goes wrong: "
            "no detour is installed and no address is resolved, so nothing is refused -- the "
            "comparison just stops matching and the feature behind it dies in silence. Name the "
            "containing function and add the offset at the use site, then resolve through "
            "er_game_base::game_build::resolve_call_site_rva / resolve_call_site_band. Run "
            "`python3 scripts/derive-callsite-1170.py <addr>` for the evidence that the split is "
            "correct.",
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
