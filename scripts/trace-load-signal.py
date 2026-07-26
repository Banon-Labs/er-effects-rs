#!/usr/bin/env python3
"""Extract the LOAD SIGNAL from an er_reload_trace_dll.dll trace log.

The trace is dominated by title-screen idle spam (title_native_ready) and
high-frequency task_enqueue/result_* churn. This filters those out and emits the
ordered native load-path function sequence with the b80(load phase)/ac0(save
slot)/c30(map id) RAM snapshot at each event, collapsing runs of the identical
(fn, b80, ac0, c30) tuple into a single line with a count. That makes a vanilla
trace directly diffable against a product trace to pin the missing native step.

Usage:
    scripts/trace-load-signal.py <trace.log> [--keep-spam] [--fn SUBSTR]
    zcat reference-captures/<sample>.gz | scripts/trace-load-signal.py -
"""
from __future__ import annotations

import argparse
import re
import sys

# Idle/churn events that swamp the signal. title_native_ready is pure title-screen
# spam; task_enqueue/result_* fire thousands of times per frame during menus.
SPAM = (
    "title_native_ready",
    "task_enqueue",
    "result_event_wrapper_builder",
    "result_event_handler",
    "result_action_builder",
    "menu_window_job_idle_ctor",
)

LINE_RE = re.compile(
    r"\+(?P<ms>\d+)ms\]\s+(?P<fn>[a-z0-9_]+)\s+ENTER\b.*?"
    r"b80=(?P<b80>-?\d+)\s+ac0=(?P<ac0>-?\d+)\s+c30=(?P<c30>0x[0-9a-f]+|-?\d+)"
)


def iter_lines(path: str):
    if path == "-":
        yield from sys.stdin
    else:
        with open(path, encoding="utf-8", errors="replace") as fh:
            yield from fh


def main() -> int:
    ap = argparse.ArgumentParser(description="Extract load signal from an er-reload-trace log.")
    ap.add_argument("trace", help="path to er-reload-trace.log, or - for stdin")
    ap.add_argument("--keep-spam", action="store_true", help="do not filter idle/churn events")
    ap.add_argument("--fn", default=None, help="only show events whose fn contains this substring")
    args = ap.parse_args()

    prev_key = None
    count = 0
    first_ms = None
    emitted = 0

    def flush():
        if prev_key is not None:
            fn, b80, ac0, c30 = prev_key
            xn = f" x{count}" if count > 1 else ""
            print(f"+{first_ms}ms {fn}  b80={b80} ac0={ac0} c30={c30}{xn}")

    for raw in iter_lines(args.trace):
        m = LINE_RE.search(raw)
        if not m:
            continue
        fn = m.group("fn")
        if not args.keep_spam and any(s in fn for s in SPAM):
            continue
        if args.fn and args.fn not in fn:
            continue
        key = (fn, m.group("b80"), m.group("ac0"), m.group("c30"))
        if prev_key is not None and prev_key == key:
            count += 1
            continue
        flush()
        prev_key = key
        first_ms = m.group("ms")
        count = 1
        emitted += 1
    flush()
    if emitted == 0:
        print("# no load-signal events matched", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
