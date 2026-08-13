#!/usr/bin/env python3
"""Decide whether a Seamless session state's dwell is a FRAME COUNT, a WALL CLOCK, or neither.

Reads `er-invasion-warp-dll.log` and looks at the transition lines the DLL emits:

    local-invasion: session state 0x0e (unreversed) -> 0x11 (unreversed) -- held 612 ticks / 10203ms (~59 fps)

The interesting one is `0x11`, Seamless's no-match retry wait. Whether that wait is counted in
FRAMES or in SECONDS decides whether raising the frame rate would shorten it -- the difference
between "run the game faster while hunting" being a real lever and being void.

WHY THIS SCRIPT EXISTS RATHER THAN AN EYEBALL
---------------------------------------------
An earlier reading of "600 ticks, nine times, zero variance" was taken from a heartbeat that fires
every 600 ticks unconditionally -- it measured the logger's own period and was mistaken for a
measurement of the game. Eyeballing a column of numbers is exactly how that happens.

THE METHOD: whichever quantity the timer counts is the one it holds constant, while the other is
forced to absorb every wobble in frame rate. So the two columns are compared on RELATIVE spread
and the tighter one names the mechanism. This works even when the frame rate barely moves -- a
small fps spread shrinks both columns' variation without changing the ratio between them, which
is the quantity the inference actually rests on.

Measured live 2026-08-06 on six `0x11` dwells: wall clock 15003ms +/-0.22%, ticks 844 +/-2.64%.
The clock is ~12x tighter, and the tick range a fixed 15.0s interval predicts (812-875) matches
the observed range (814-876). Seamless's retry is a wall-clock timer; frame rate cannot shorten
it.

Usage:
    python3 scripts/invasion-dwell-verdict.py <log> [--state 0x11]
    python3 scripts/invasion-dwell-verdict.py --selftest
"""

from __future__ import annotations

import argparse
import re
import statistics
import sys
from dataclasses import dataclass

# `-- held 612 ticks / 10203ms (~59 fps)`, with the fps part optional (it is omitted for
# intervals too short to divide).
DWELL_RE = re.compile(
    r"session state (?:\(first read\)|(0x[0-9a-f]+)[^-]*?) -> (0x[0-9a-f]+)"
    r".*?held (\d+) ticks / (\d+)ms(?: \(~(\d+) fps\))?"
)

# The discriminator is the RATIO of the two columns' relative spread, not the frame-rate spread.
#
# Whichever quantity the timer is really counting is the one held constant; the other is forced to
# absorb every wobble in frame rate. So compare coefficient of variation between the columns: the
# tighter column names the mechanism. One column must be this many times tighter than the other
# before the call is made.
#
# This supersedes an earlier rule that demanded a large frame-rate spread before deciding. That
# asked the wrong question and produced a false INCONCLUSIVE on decisive data: six live 0x11
# dwells at only a 1.08x fps spread nevertheless pinned wall clock, because the wall clock held to
# 0.22% while ticks moved 2.64% -- and the tick range predicted by a 15.0s clock (812-875) matched
# the observed range (814-876) almost exactly. A small fps spread does not weaken the inference; it
# only shrinks both numbers, leaving the RATIO between them intact.
CV_RATIO_MARGIN = 3.0

# Below this, a column counts as flat -- within sampling noise of "the same value every time".
# The state is polled once per tick, so even a perfectly constant quantity carries about one tick
# of quantisation error either way.
CV_FLAT = 0.005

# Above this, a column is not merely wobbling but genuinely unconstrained. Both columns loose
# together means the wait is not a fixed quantity of either kind.
CV_LOOSE = 0.20


@dataclass(frozen=True)
class Dwell:
    """One measured stay in a state, as the DLL reported it.

    THE ATTRIBUTION IS THE EASY THING TO GET WRONG, so it is spelled out here. A log line reads

        session state 0x11 (unreversed) -> 0x0d SEARCHING -- held 816 ticks / 14990ms

    and the `held` figure is how long the state on the LEFT was occupied -- the DLL stamps the
    interval since the previous transition, which is the interval during which it sat in `0x11`.
    So `state` below is the LEFT-hand state, the one being left; `next_state` is where it went.

    Grouping by `next_state` instead is silently wrong rather than obviously wrong: it still
    produces plausible per-state numbers, just for the wrong states. It cost a live run's analysis
    on 2026-08-06, where "state 0x11" reported 7-9 tick dwells that actually belonged to 0x0e,
    while the real 0x11 dwells were ~815 ticks sitting on the lines that LEAVE 0x11.
    """

    state: str
    """The state that was held for `ticks`/`ms` -- the LEFT-hand side of the transition."""
    next_state: str
    """Where it went afterwards. Carried for context; never what a dwell is grouped by."""
    ticks: int
    ms: int

    @property
    def fps(self) -> float:
        """Ticks per second across this dwell. Zero when the interval cannot be divided."""
        return (self.ticks * 1000.0 / self.ms) if self.ms else 0.0


def parse(text: str) -> list[Dwell]:
    """Every dwell in a log, in order. Lines without a `held` clause are ignored.

    The DLL's first-read line has no previous state and no `held` clause, so it never reaches
    here; a line that somehow carried one without a left-hand state is dropped rather than
    attributed to a placeholder, since a dwell whose owner is unknown cannot be grouped.
    """
    out: list[Dwell] = []
    for line in text.splitlines():
        m = DWELL_RE.search(line)
        if m and m.group(1):
            out.append(Dwell(m.group(1), m.group(2), int(m.group(3)), int(m.group(4))))
    return out


def _cv(values: list[float]) -> float:
    """Coefficient of variation: spread relative to size. 0.0 means every value is identical.

    Relative rather than absolute because the two columns are in different units -- milliseconds
    and ticks -- and the whole method rests on comparing how tightly each is held.
    """
    mean = statistics.fmean(values)
    return (statistics.pstdev(values) / mean) if mean > 0 else float("inf")


def verdict(dwells: list[Dwell]) -> tuple[str, str]:
    """Classify a set of dwells in ONE state. Returns (verdict, the reasoning behind it).

    Whichever quantity the timer counts is the one it holds constant; the other absorbs every
    wobble in frame rate. So the tighter column names the mechanism, and the strength of the
    inference is the RATIO between the two columns' relative spread -- not how much the frame
    rate happened to move. See [`CV_RATIO_MARGIN`].
    """
    if len(dwells) < 2:
        return "INCONCLUSIVE", f"only {len(dwells)} dwell(s); need at least 2 to compare"

    usable = [d for d in dwells if d.ms > 0 and d.ticks > 0]
    if len(usable) < 2:
        return "INCONCLUSIVE", "not enough dwells long enough to measure"

    ticks = [float(d.ticks) for d in usable]
    ms = [float(d.ms) for d in usable]
    rates = [d.fps for d in usable]
    cv_t, cv_ms = _cv(ticks), _cv(ms)

    detail = (
        f"n={len(usable)}; wall clock {statistics.fmean(ms):.0f}ms +/-{cv_ms * 100:.2f}%, "
        f"ticks {statistics.fmean(ticks):.0f} +/-{cv_t * 100:.2f}%, "
        f"fps {min(rates):.0f}-{max(rates):.0f}"
    )

    # Both loose: not a fixed quantity of either kind.
    if cv_t > CV_LOOSE and cv_ms > CV_LOOSE:
        return (
            "EVENT-DRIVEN",
            f"{detail} -- NEITHER column is held, so the wait is not a fixed quantity of either "
            f"kind. It most likely ends on an external event (a server response), in which case "
            f"no local frame-rate or timer lever shortens it.",
        )

    # Both flat: the frame rate never moved enough to force the columns apart.
    if cv_t < CV_FLAT and cv_ms < CV_FLAT:
        return (
            "INCONCLUSIVE",
            f"{detail} -- both columns are flat, so the frame rate never moved enough to make "
            f"the two hypotheses disagree. Re-measure across a frame-rate change.",
        )

    if cv_ms * CV_RATIO_MARGIN < cv_t:
        predicted = [statistics.fmean(ms) * r / 1000.0 for r in rates]
        return (
            "WALL CLOCK",
            f"{detail} -- the clock is {cv_t / max(cv_ms, 1e-9):.1f}x tighter than the tick "
            f"count, and ticks rise with frame rate exactly as a fixed "
            f"{statistics.fmean(ms) / 1000:.1f}s interval predicts "
            f"({min(predicted):.0f}-{max(predicted):.0f} predicted vs "
            f"{min(ticks):.0f}-{max(ticks):.0f} observed). Raising the frame rate CANNOT shorten "
            f"this wait.",
        )
    if cv_t * CV_RATIO_MARGIN < cv_ms:
        return (
            "FRAME COUNTER",
            f"{detail} -- the tick count is {cv_ms / max(cv_t, 1e-9):.1f}x tighter than the "
            f"clock, so the wait is a fixed number of frames and the wall-clock time moves "
            f"inversely with frame rate. Raising the frame rate DOES shorten it.",
        )
    return (
        "INCONCLUSIVE",
        f"{detail} -- neither column is decisively tighter than the other "
        f"(ratio {max(cv_t, cv_ms) / max(min(cv_t, cv_ms), 1e-9):.1f}x, need "
        f"{CV_RATIO_MARGIN:.0f}x). Collect more dwells, ideally across a frame-rate change.",
    )


def report(dwells: list[Dwell], state: str) -> int:
    """Print the dwells for one state and the verdict. Returns a process exit code."""
    picked = [d for d in dwells if d.state == state]
    print(f"=== state {state}: {len(picked)} dwell(s) of {len(dwells)} transitions total")
    if not picked:
        print(f"  no dwells recorded for {state}.")
        print("  If the log has transition lines but none carry a `held ...` clause, it predates")
        print("  the per-transition stamp and cannot answer this. Re-run on a current DLL.")
        return 2
    for i, d in enumerate(picked, 1):
        rate = f"{d.fps:6.1f} fps" if d.fps else "    -- fps"
        print(f"  {i:2d}. held {d.ticks:6d} ticks {d.ms:7d} ms  {rate}   then -> {d.next_state}")
    what, why = verdict(picked)
    print(f"\nVERDICT: {what}\n  {why}")
    return 0


def selftest() -> int:
    """Prove the verdict logic on synthetic logs before it is trusted on a real one."""
    failures = []

    def check(name, got, want):
        if got != want:
            failures.append(f"{name}: got {got!r}, wanted {want!r}")

    # A frame counter: same ticks, wall clock doubles when the frame rate halves.
    frame = [Dwell("0x11", "0x0d", 600, 10_000), Dwell("0x11", "0x0d", 601, 20_033)]
    check("frame counter", verdict(frame)[0], "FRAME COUNTER")

    # A clock: same wall clock, tick count halves with the frame rate.
    clock = [Dwell("0x11", "0x0d", 600, 10_000), Dwell("0x11", "0x0d", 300, 10_010)]
    check("wall clock", verdict(clock)[0], "WALL CLOCK")

    # Neither held: the wait ends on something external.
    event = [
        Dwell("0x15", "0x22", 600, 10_000),
        Dwell("0x15", "0x22", 130, 4_300),
        Dwell("0x15", "0x22", 2_400, 41_000),
    ]
    check("event driven", verdict(event)[0], "EVENT-DRIVEN")

    # THE IMPORTANT ONE. Constant ticks at a CONSTANT frame rate must NOT read as a frame counter:
    # that is exactly the reading that produced the original wrong claim.
    steady = [Dwell("0x11", "0x0d", 600, 10_000), Dwell("0x11", "0x0d", 600, 10_000)]
    check("steady fps is not evidence", verdict(steady)[0], "INCONCLUSIVE")

    check("single sample", verdict(frame[:1])[0], "INCONCLUSIVE")

    # THE REAL LIVE DATA, and the case the previous rule got WRONG. These six dwells span only a
    # 1.08x frame-rate spread, which the old "needs a big fps spread" gate rejected as
    # inconclusive -- yet they pin wall clock decisively, because the clock is ~12x tighter than
    # the tick count. Pinned verbatim so the threshold can never drift back to refusing them.
    live_0x11 = [
        Dwell("0x11", "0x0d", 816, 14_990),
        Dwell("0x11", "0x0d", 814, 15_043),
        Dwell("0x11", "0x0d", 850, 15_032),
        Dwell("0x11", "0x0d", 848, 14_940),
        Dwell("0x11", "0x0d", 876, 15_017),
        Dwell("0x11", "0x0d", 859, 14_996),
    ]
    what, why = verdict(live_0x11)
    check("live 0x11 is wall clock", what, "WALL CLOCK")
    if "CANNOT shorten" not in why:
        failures.append("live 0x11 verdict must say the frame rate cannot shorten it")

    # The same six dwells must NOT be readable as a frame counter under any reordering -- the
    # statistic is order-independent, and a verdict that flipped on ordering would be noise.
    check("order independent", verdict(list(reversed(live_0x11)))[0], "WALL CLOCK")

    # THE REGRESSION THAT PROMPTED THE REWRITE. A dwell must be attributed to the state being
    # LEFT, not the one being entered. Grouping the other way is silently wrong -- it still
    # yields plausible-looking per-state numbers, for the wrong states -- so it is pinned with
    # real log text rather than a constructed object.
    attributed = parse(
        "local-invasion: session state 0x11 (unreversed) -> 0x0d SEARCHING "
        "-- held 816 ticks / 14990ms (~54 fps)"
    )
    check("one dwell parsed", len(attributed), 1)
    if attributed:
        check("dwell belongs to the state LEFT", attributed[0].state, "0x11")
        check("destination recorded separately", attributed[0].next_state, "0x0d")
        # Reported under 0x11, and NOT under 0x0d.
        check("grouped under the held state", report(attributed, "0x11"), 0)
        check("not grouped under the destination", report(attributed, "0x0d"), 2)

    # The parser must survive the real line shape, including the unreversed-state annotations.
    line = (
        "er-invasion-warp-dll: local-invasion: session state 0x0e (unreversed) -> 0x11 "
        "(unreversed) -- held 612 ticks / 10203ms (~59 fps)"
    )
    parsed = parse(line)
    check("parses a real line", len(parsed), 1)
    if parsed:
        check("parses ticks", parsed[0].ticks, 612)
        check("parses ms", parsed[0].ms, 10_203)
        # `0x0e -> 0x11`: the 612 ticks were spent in 0x0e, which is what the dwell describes.
        check("parses the held state", parsed[0].state, "0x0e")
        check("parses the destination", parsed[0].next_state, "0x11")

    # A driven-by-us line still parses -- our own cancels are transitions too.
    driven = parse(
        "local-invasion: session state 0x15 (unreversed) -> 0x22 CANCELLING -- held 8 ticks / "
        "133ms (~60 fps) (driven by us: cancel)"
    )
    check("parses a driven line", len(driven), 1)

    # A pre-instrumentation line must be ignored rather than parsed as a zero dwell.
    old = parse("local-invasion: session state 0x0e (unreversed) -> 0x11 (unreversed)")
    check("ignores un-stamped lines", len(old), 0)

    for f in failures:
        print(f"FAIL {f}")
    print(f"selftest: {'PASS' if not failures else str(len(failures)) + ' FAILURE(S)'}")
    return 1 if failures else 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("log", nargs="?", help="path to er-invasion-warp-dll.log")
    ap.add_argument("--state", default="0x11", help="session state to analyse (default: 0x11)")
    ap.add_argument("--selftest", action="store_true", help="verify the verdict logic and exit")
    args = ap.parse_args()

    if args.selftest:
        return selftest()
    if not args.log:
        ap.error("a log path is required (or --selftest)")
    with open(args.log, encoding="utf-8", errors="replace") as fh:
        dwells = parse(fh.read())
    return report(dwells, args.state)


if __name__ == "__main__":
    sys.exit(main())
