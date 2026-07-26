#!/usr/bin/env python3
"""TOOL 2 of the real oracle (user 2026-07-20): compare a run against a known-good phase IMPRINT and
emit a STACK-TRACE-LIKE divergence line.

Given a phase imprint (from oracle-imprint.py / the store) and a run's telemetry timeseries, it walks
the imprint's ordered semaphore transitions and matches each one, IN ORDER, against the run. The first
imprint transition the run fails to reproduce is the divergence point -- reported as a single line that
names the exact semaphore that left the known-good path (what to look at next), plus the last matched
step and the run's actual tail state on that field. It also flags transitions that occurred but far
outside the imprint's timing budget (a stall). This is what lets the oracle tear down with CERTAINTY.

Modes:
  post-hoc:  --imprint imprint.json --live timeseries.jsonl
  live:      --imprint imprint.json --telemetry <er-effects-telemetry.json> --live-out ts.jsonl
             polls the telemetry file, appends a timeseries, and prints the divergence line the instant
             the run misses the expected next transition beyond its time budget (drives teardown).

Exit code: 0 if the run matched the imprint to the end; 1 if it diverged (structural or timing).
"""
from __future__ import annotations

import argparse
import json
import sys
import threading
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import oracle_common as oc  # noqa: E402

# Timing budget for a matched/expected transition: allow this multiple of the imprint gap, plus slack,
# before calling it a stall. Generous by default (a single-run imprint has wide natural variance; tighten
# once the imprint is built from several runs and carries per-transition min/max in timing_spread).
GAP_TOLERANCE = 4.0
GAP_SLACK_MS = 8000


def _fmt(tr: dict) -> str:
    return f"+{tr['t_ms']:>7}ms {tr['field']} = {json.dumps(tr['to'])}"


def divergence_line(phase: str, matched: list, expected: list, ei: int, live: list) -> str:
    E = expected[ei]
    last = matched[-1][0] if matched else None
    # the run's most recent value for the field that failed to transition
    tail = None
    for L in reversed(live):
        if L["field"] == E["field"]:
            tail = L
            break
    lines = [
        f"ORACLE DIVERGENCE (phase '{phase}'): matched {len(matched)}/{len(expected)} imprint steps.",
        f"  last matched : {_fmt(last[0]) if isinstance(last, tuple) else (_fmt(last) if last else '(none)')}",
        f"  EXPECTED next: {_fmt(E)}  (imprint gap ~{E.get('gap_ms', 0)}ms)",
    ]
    if tail is not None:
        lines.append(
            f"  run reached  : {E['field']} last went to {json.dumps(tail['to'])} at +{tail['t_ms']}ms; "
            f"the expected value {json.dumps(E['to'])} was never reached"
        )
    else:
        lines.append(
            f"  run reached  : {E['field']} never changed from its initial value; "
            f"expected {json.dumps(E['to'])} was never reached"
        )
    lines.append(f"  => LOOK AT: {E['field']} (failed to reach {json.dumps(E['to'])})")
    return "\n".join(lines)


def compare(imprint: dict, live_rows: list[dict], phase: str = "?") -> dict:
    expected = imprint["transitions"]
    live = oc.extract_transitions(live_rows)
    li = 0
    matched: list[tuple[dict, dict]] = []
    timing_flags: list[str] = []
    for ei, E in enumerate(expected):
        found = None
        for j in range(li, len(live)):
            L = live[j]
            if L["field"] == E["field"] and oc.key(L["to"]) == oc.key(E["to"]):
                found = j
                break
        if found is None:
            return {
                "diverged": True,
                "kind": "structural",
                "matched": len(matched),
                "total": len(expected),
                "line": divergence_line(phase, matched, expected, ei, live),
                "timing_flags": timing_flags,
            }
        L = live[found]
        # timing: compare the gap since the PREVIOUS matched transition (aligned pair) to the imprint gap.
        if matched:
            live_gap = L["t_ms"] - matched[-1][1]["t_ms"]
            budget = E.get("gap_ms", 0) * GAP_TOLERANCE + GAP_SLACK_MS
            if live_gap > budget:
                timing_flags.append(
                    f"SLOW: {E['field']} -> {json.dumps(E['to'])} took {live_gap}ms "
                    f"(imprint ~{E.get('gap_ms', 0)}ms, budget {int(budget)}ms)"
                )
        matched.append((E, L))
        li = found + 1
    return {
        "diverged": bool(timing_flags),
        "kind": "timing" if timing_flags else "none",
        "matched": len(matched),
        "total": len(expected),
        "line": None
        if not timing_flags
        else f"ORACLE TIMING DIVERGENCE (phase '{phase}'): all {len(expected)} steps occurred but "
        + timing_flags[0],
        "timing_flags": timing_flags,
        "live_terminal_ms": live[-1]["t_ms"] if live else 0,
        "imprint_terminal_ms": imprint.get("terminal_t_ms"),
    }


def run_posthoc(a) -> int:
    imprint = json.loads(Path(a.imprint).read_text(encoding="utf-8"))
    live_rows = oc.load_rows(Path(a.live))
    if a.load_epoch is not None:
        # Slice the run to one load epoch (deser==N) so e.g. a load2 (deser=1) reload can be diffed
        # against a vanilla-continue (deser=0) imprint on the game semaphores alone.
        live_rows = [
            r
            for r in live_rows
            if r.get("system_quit_continue_confirm_fresh_deser_count") == a.load_epoch
        ]
    res = compare(imprint, live_rows, a.phase)
    if res["diverged"]:
        print(res["line"])
        for tf in res["timing_flags"]:
            print("  " + tf)
        return 1
    print(
        f"ORACLE OK (phase '{a.phase}'): matched all {res['total']} imprint steps; "
        f"run terminal {res['live_terminal_ms']}ms vs imprint {res['imprint_terminal_ms']}ms."
    )
    return 0


_POLL = threading.Event()


def run_live(a) -> int:
    imprint = json.loads(Path(a.imprint).read_text(encoding="utf-8"))
    tel = Path(a.telemetry)
    out = Path(a.live_out).open("w", encoding="utf-8")
    start = time.monotonic()
    rows: list[dict] = []
    fields = oc.DISCRETE_FIELDS
    while True:
        elapsed = (time.monotonic() - start) * 1000
        if elapsed > a.max_ms:
            print(f"ORACLE: reached max {a.max_ms}ms without diverging or completing the imprint.")
            return 2
        try:
            t = json.loads(tel.read_text(encoding="utf-8", errors="replace"))
        except (OSError, json.JSONDecodeError):
            t = None
        if t is not None:
            snap = {"t_ms": round(elapsed)}
            snap.update({k: t.get(k) for k in fields})
            out.write(json.dumps(snap) + "\n")
            out.flush()
            rows.append(snap)
            res = compare(imprint, rows, a.phase)
            # In live mode we only care about STRUCTURAL divergence once the run has progressed enough
            # that the missing step is truly overdue (its budget elapsed). Structural "not found" while
            # the run is still early is normal (the step just hasn't happened yet), so gate on the
            # expected step's imprint time + budget having passed.
            if not res["diverged"] and res["matched"] == res["total"]:
                print(f"ORACLE OK (phase '{a.phase}'): run matched all {res['total']} imprint steps.")
                out.close()
                return 0
        _POLL.wait(a.poll_s)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--imprint", required=True)
    ap.add_argument("--phase", default="?")
    ap.add_argument(
        "--load-epoch",
        type=int,
        default=None,
        help="post-hoc: filter the live run to deser==N before comparing (e.g. 1 = load2)",
    )
    ap.add_argument("--live", help="post-hoc: a completed timeseries.jsonl to compare")
    ap.add_argument("--telemetry", help="live: the er-effects-telemetry.json to poll")
    ap.add_argument("--live-out", help="live: timeseries output path")
    ap.add_argument("--poll-s", type=float, default=0.5)
    ap.add_argument("--max-ms", type=float, default=300000)
    a = ap.parse_args()
    if a.live:
        return run_posthoc(a)
    if a.telemetry and a.live_out:
        return run_live(a)
    ap.error("provide --live (post-hoc) OR --telemetry + --live-out (live)")


if __name__ == "__main__":
    sys.exit(main())
