#!/usr/bin/env python3
"""Turn an `er-invasion-warp-dll.log` into a live-top-up verdict.

The question a world-map run answers is narrow: did a legacy dungeon's real invasion points reach
the map WITHOUT a world re-entry? That is a specific ORDERING in the log -- a `TOP-UP claimed` line
falling between two `WorldMapViewModel ctor` lines, rather than one explained by a ctor. Reading
that ordering by eye out of a 30k-line log is how earlier runs got mis-summarised (a run was
reported as "no top-up fired" when the interesting question was which gate refused), so the
ordering is asserted here instead of eyeballed.

Also prints the decline reasons verbatim. Every refusal path in `top_up_live_pins` /
`restyle_live_pins` logs one, deduped in-process, so a run that shows nothing on screen still says
why in one line.

Usage:
    python3 scripts/invasion-topup-timeline.py [LOG_PATH]
    python3 scripts/invasion-topup-timeline.py --selftest

`LOG_PATH` defaults to the game-directory log, resolved from `ER_GAME_DIR` when set so the tool is
not pinned to one user's Steam layout.
"""

from __future__ import annotations

import os
import re
import sys

DEFAULT_GAME_DIR = os.path.join(
    os.path.expanduser("~"),
    ".local/share/Steam/steamapps/common/ELDEN RING/Game",
)
LOG_NAME = "er-invasion-warp-dll.log"


def default_log_path() -> str:
    return os.path.join(os.environ.get("ER_GAME_DIR", DEFAULT_GAME_DIR), LOG_NAME)


# Every line that changes the top-up's state, in the order the DLL can emit them.
EVENTS: list[tuple[str, re.Pattern[str]]] = [
    ("ctor", re.compile(r"WorldMapViewModel ctor #(\d+)")),
    ("reserved", re.compile(r"reserved (\d+)/(\d+) dormant row")),
    ("harvest", re.compile(r"read (\d+) newly resident map\(s\)")),
    ("merge", re.compile(r"(\d+) legacy invasion point\(s\) across (\d+) map\(s\) -> (\d+) separable")),
    # Matches both wordings: the line said "fresh block(s)" before the count was corrected to be
    # per-POINT. A matcher pinned to one wording made this tool report "NO top-up claimed anything"
    # on a run whose log said it claimed 9 of 9 -- the tool lying is worse than no tool.
    ("claimed", re.compile(r"TOP-UP claimed (\d+) of (\d+) fresh (?:block|POINT)")),
    ("declined", re.compile(r"top-up declined -- (.*)$")),
    ("restyle_declined", re.compile(r"restyle declined.*?-- (.*)$")),
    ("restyled", re.compile(r"restyled LIVE pins -- (\d+) of (\d+) repainted")),
    # The evidence for the four-descriptor icon fix: how many repainted rows' own param disagreed
    # with the icon written, plus a handful of (entity_id, block, wanted, found) samples.
    ("icon_mismatch", re.compile(r"(\d+) of the repainted rows' own BonfireWarpParam")),
    # Whether the re-colour reached the rows a live top-up claimed. Those live in the DORMANT span,
    # and walking only the injected span is why marking a harvested dungeon repainted exactly one
    # row -- the whole-dungeon marker the top-up had already hidden -- and nothing visible.
    ("dormant_walk", re.compile(r"(\d+) claimed dormant row\(s\) were walked")),
    ("marked", re.compile(r"(?:MARKED|EXCLUDED) (0x[0-9a-f]+)[^-]*-- now (\d+) chosen, (\d+) excluded")),
    ("tiers", re.compile(r"pin tiers chosen=(\d+) untouched=(\d+) excluded=(\d+)")),
    ("exhausted", re.compile(r"top-up ran out of dormant rows")),
    ("unowned", re.compile(r"dormant row (\d+) does not point into our param slab")),
    ("confirm", re.compile(r"invasion pin entity_id=(\S+) -> LOCAL warp to")),
    # Seamless session surface. The lobby key decides which lobby-list query you are in; the
    # rejects tell you what that query actually offered. Correlating them is what separates
    # "the password changed my key" from "the password changed who can invade me" -- two claims
    # that look identical if you only watch one of them.
    ("lobby_key", re.compile(r"LOBBY KEY = ([0-9A-Fa-f]+)")),
    ("reject", re.compile(r"REJECT (0x[0-9a-f]+) \((\w+)\)")),
    ("search", re.compile(r"you started a search")),
]


def classify(line: str) -> list[tuple[str, str]]:
    """EVERY pattern that matches, not just the first.

    One log line carries several facts. The restyle summary states the repaint counts AND how many
    claimed dormant rows were walked AND the param-disagreement count. Returning only the first
    match meant `restyled` swallowed the line and the dormant-coverage check silently saw nothing --
    so the tool reported "this log predates the fix" about a log that contained the fix's own
    output. A reader that can only see one fact per line is a reader that invents absences.
    """
    return [(name, m.group(0)) for name, pattern in EVENTS if (m := pattern.search(line))]


def timeline_of(lines: list[str]) -> list[tuple[int, str, str]]:
    events = []
    for index, line in enumerate(lines):
        for name, text in classify(line):
            events.append((index, name, text))
    return events


def verdict_lines(events: list[tuple[int, str, str]]) -> list[str]:
    """The one thing the run is being read for, as plain sentences."""
    ctors = [index for index, kind, _ in events if kind == "ctor"]
    claims = [(index, text) for index, kind, text in events if kind == "claimed"]
    out: list[str] = []
    if not ctors:
        out.append("NO world map was ever built -- the run never reached a loaded world.")
        return out
    if not claims:
        reasons = [text for _, kind, text in events if kind == "declined"]
        out.append(f"NO top-up claimed anything across {len(ctors)} world entr(ies).")
        for reason in dict.fromkeys(reasons) or [
            "(no decline logged either -- a SILENT refusal path remains, which is a defect)"
        ]:
            out.append(f"  declined: {reason}")
        return out
    for index, text in claims:
        before = sum(1 for c in ctors if c < index)
        later = [c for c in ctors if c > index]
        if before == 0:
            out.append(f"line {index}: claimed before any world map was built -- suspicious. {text}")
        elif later:
            out.append(
                f"line {index}: MARKERS APPEARED WITHOUT A WORLD RE-ENTRY "
                f"(after world entry #{before}, before #{before + 1}). {text}"
            )
        else:
            out.append(
                f"line {index}: markers appeared without a world re-entry "
                f"(after world entry #{before}, no later entry). {text}"
            )
    return out


def selftest() -> int:
    entered = "er-invasion-warp-dll: map-hooks: WorldMapViewModel ctor #1 this=0x1"
    claimed = "er-invasion-warp-dll: map-inject: TOP-UP claimed 8 of 9 fresh POINT(s)"
    declined = "er-invasion-warp-dll: map-inject: top-up declined -- nothing new to show"

    cases: list[tuple[str, list[str], str]] = [
        ("no world", ["boot"], "never reached a loaded world"),
        ("declined only", [entered, declined], "NO top-up claimed anything"),
        ("silent refusal", [entered], "SILENT refusal path remains"),
        ("claim between entries", [entered, claimed, entered], "WITHOUT A WORLD RE-ENTRY"),
        ("claim after last entry", [entered, claimed], "no later entry"),
    ]
    # A single line carrying several facts must yield ALL of them.
    multi = (
        "er-invasion-warp-dll: map-inject: restyled LIVE pins -- 10 of 476 repainted at generation "
        "12, 0 of 476 span row(s) REFUSED ... 9 claimed dormant row(s) were walked alongside the "
        "injected span"
    )
    kinds = {kind for _, kind, _ in timeline_of([multi])}
    if not {"restyled", "dormant_walk"} <= kinds:
        print(f"FAIL multi-fact line: got {sorted(kinds)}, expected restyled AND dormant_walk")
        return 1

    # Both historical wordings must match, or the tool silently reports the opposite of the truth.
    cases.append(
        (
            "legacy 'fresh block' wording",
            [entered, "er-invasion-warp-dll: TOP-UP claimed 9 of 9 fresh block(s)", entered],
            "WITHOUT A WORLD RE-ENTRY",
        )
    )
    failures = 0
    for name, lines, expected in cases:
        got = "\n".join(verdict_lines(timeline_of(lines)))
        if expected not in got:
            print(f"FAIL {name}: expected {expected!r} in:\n{got}")
            failures += 1
    # A claim explained by its own world entry must NOT read as the feature working.
    got = "\n".join(verdict_lines(timeline_of([claimed, entered])))
    if "suspicious" not in got:
        print(f"FAIL claim-before-any-entry: expected 'suspicious' in:\n{got}")
        failures += 1
    print(f"selftest: {len(cases) + 2 - failures}/{len(cases) + 2} passed")
    return 1 if failures else 0


def main(argv: list[str]) -> int:
    if "--selftest" in argv:
        return selftest()
    path = argv[1] if len(argv) > 1 else default_log_path()
    try:
        with open(path, encoding="utf-8", errors="replace") as handle:
            lines = handle.read().splitlines()
    except OSError as exc:
        print(f"cannot read {path}: {exc}")
        return 2

    events = timeline_of(lines)
    print(f"{path}\n{len(lines)} log lines, {len(events)} state events\n")
    for index, kind, text in events:
        print(f"{index:>7}  {kind:<17} {text[:160]}")
    print("\n=== verdict ===")
    for line in verdict_lines(events):
        print(line)

    # A repaint whose count repeats the previous pass, on rows whose tier did NOT change, is the
    # signature of a write the engine ignores. A repaint that follows a mark and is NOT repeated is
    # the feature working.
    repaints = [text for _, kind, text in events if kind == "restyled"]
    marks = [text for _, kind, text in events if kind == "marked"]
    tiers = [text for _, kind, text in events if kind == "tiers"]
    print(f"\nmarks: {len(marks)}, repaint passes: {len(repaints)}")
    for text in marks:
        print(f"  {text}")
    if tiers:
        print(f"  last injection tiers: {tiers[-1]}")
    # --- Seamless session summary -------------------------------------------------------------
    keys = [text for _, kind, text in events if kind == "lobby_key"]
    rejects = [text for _, kind, text in events if kind == "reject"]
    if keys or rejects:
        print("\n=== seamless session ===")
        distinct = list(dict.fromkeys(keys))
        for key in distinct:
            print(f"  {key}")
        if len(distinct) > 1:
            print(
                f"  the key CHANGED {len(distinct) - 1} time(s) during this run -- it re-derives on "
                "a jittered schedule, so a comparison against another machine must use the key that "
                "was live when the offers arrived, not just the first one"
            )
        blocks = [re.search(r"0x[0-9a-f]+", r).group(0) for r in rejects if re.search(r"0x[0-9a-f]+", r)]
        unique_blocks = list(dict.fromkeys(blocks))
        print(f"  offers rejected: {len(rejects)} from {len(unique_blocks)} distinct destination(s)")
        if len(unique_blocks) > 3:
            print(
                "  offers came from MANY distinct places, which is what a shared/public matchmaking "
                "namespace looks like -- not a private one gated on a secret both parties typed"
            )
        if keys and not rejects:
            print("  a key was derived but NO offer arrived -- supply, not filtering, was the limit")

    walks = [text for _, kind, text in events if kind == "dormant_walk"]
    if walks:
        print(f"  dormant coverage: {walks[-1]}")
        claims = [text for _, kind, text in events if kind == "claimed"]
        if all(w.startswith("0 ") for w in walks) and marks and claims:
            # NOT a fault on its own. A top-up paints each row's tier at CLAIM time from the
            # config as it stands then, so rows claimed AFTER a mark are already correct and have
            # nothing to repaint. Zero coverage only matters when a mark lands after the claim --
            # which shows up as a repaint pass that walked zero while claims already existed.
            # A claim only survives until the next world entry: the constructor rebuilds the row
            # list and resets the claim counter. So the question is never "did any claim precede
            # this mark" -- it is "was a claim still LIVE in the current ViewModel". Comparing
            # against the first claim in the whole run reports a miss for the ordinary case of
            # marking after travelling, which is what most marks are.
            def claims_live_at(position: int) -> bool:
                last_ctor = max(
                    (i for i, kind, _ in events if kind == "ctor" and i < position), default=-1
                )
                return any(
                    kind == "claimed" and last_ctor < i < position for i, kind, _ in events
                )

            stale = [i for i, kind, _ in events if kind == "marked" and claims_live_at(i)]
            if stale:
                print(
                    f"  a mark at line {stale[0]} landed while a top-up claim was still live in "
                    "the same ViewModel, yet no repaint walked a claimed dormant row -- those "
                    "markers would not have changed on screen. Worth a look."
                )
            else:
                print(
                    "  zero coverage is correct here: every mark followed a world entry that reset "
                    "the claim counter, and rows claimed later are painted at claim time"
                )
    elif repaints:
        print(
            "  no dormant-coverage clause in the repaint line -- this log predates the fix that "
            "walks claimed dormant rows."
        )
    if marks and not repaints:
        print(
            "  A MARK WAS RECORDED BUT NO REPAINT PASS RAN -- the live restyle never fired, so the "
            "map could only change on the next world entry."
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
