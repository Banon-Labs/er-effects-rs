#!/usr/bin/env python3
"""Print a run's character-switch timeline, and whether each switch actually tore the old world down.

WHY THIS EXISTS. A session that switches characters twice produces a 2 MB debug log, and the
difference between a switch that worked and one that did nothing is a handful of lines scattered
through it. Worse, the failure is an ABSENCE -- switch #2 arms, and then the return-title REQUEST,
the bc4 force, the final functor and `WORLD LOST` simply never appear -- so reading forwards for
"the error" finds nothing and the run reads as fine. This prints the events in order and then the
three gates that decide whether a NEXT switch can tear down at all, so the absence is visible
beside the counters that explain it.

THE GATES. `system_quit_quickload_return_title_request_count`,
`system_quit_direct_return_title_chain_submit_count` and
`system_quit_return_title_final_functor_call_count` are run-global one-shots the teardown runs
behind (`== 0` gates and a `compare_exchange(0, 1)`). They are handed back at the end of every
committed switch. Finding all three at 1 with a second switch armed IS the no-teardown failure --
the old character stays standing, no loading screen appears, and the quit menu reopens over a
destroyed ProfileSelect.

Usage:
  python3 scripts/er-switch-timeline.py <run-dir>        # e.g. ~/.cache/er-me3-runs/br-...
  python3 scripts/er-switch-timeline.py --selftest
"""

from __future__ import annotations

import json
import pathlib
import re
import sys
import tempfile

LOG_NAME = "er-quickload-autoload-debug.log"
TELEMETRY_NAME = "er-quickload-telemetry.json"

# Every line that marks a step of the switch, in the order a healthy switch emits them. Kept as one
# alternation rather than a list of substrings so the scan is a single pass over a multi-megabyte
# file.
EVENT = re.compile(
    r"(switch-trigger"
    r"|armed product Continue autoload"
    r"|title owner appeared"
    r"|direct return-title chain"
    r"|forced return-title bc4"
    r"|native return-title REQUEST fired"
    r"|final-functor"
    r"|restored bc4"
    r"|WORLD LOST"
    r"|reload-fd4io:"
    r"|gaitem-reset:"
    r"|own-load-feed:"
    r"|own-load-switch-reload:"
    r"|re-armed for the next load"
    r"|requestwait-tick"
    r"|movemap-init-block"
    r"|slot activation ARMED"
    r"|skip restore real windows)"
)

# The DLL's own line prefix: `[+NNNNms] YYYY-MM-DD HH:MM:SS:ms dll:XXXXXXXX <body>`.
PREFIX = re.compile(r"^\[\+(\d+)ms\] \S+ \S+ dll:\S+ (.*)$")

GATES = (
    "system_quit_quickload_return_title_request_count",
    "system_quit_direct_return_title_chain_submit_count",
    "system_quit_return_title_final_functor_call_count",
)
CONTEXT = (
    "system_quit_quickload_phase",
    "oracle_world_lost_to_title",
    "oracle_char_name",
    "oracle_player_present",
    "oracle_saved_map_c30",
)


def timeline(log_text: str) -> list[tuple[float, str]]:
    """(seconds, body) for every switch event, skipping the log's own repeat-collapsing lines."""
    out = []
    for line in log_text.splitlines():
        if not EVENT.search(line) or line.startswith("repeat:") or " repeat: " in line:
            continue
        match = PREFIX.match(line)
        if match:
            out.append((int(match.group(1)) / 1000.0, match.group(2)))
    return out


def report(run_dir: pathlib.Path) -> int:
    log = run_dir / LOG_NAME
    if not log.is_file():
        print(f"no {LOG_NAME} in {run_dir}", file=sys.stderr)
        return 2
    events = timeline(log.read_text(encoding="utf-8", errors="replace"))
    for seconds, body in events:
        print(f"{seconds:8.1f}s  {body[:190]}")
    arms = sum(1 for _, body in events if "slot activation ARMED" in body or "switch-trigger #" in body)
    teardowns = sum(1 for _, body in events if "WORLD LOST" in body)
    rearms = sum(1 for _, body in events if "re-armed for the next load" in body)
    print(f"\nswitches armed={arms}  world teardowns={teardowns}  re-arms={rearms}")

    telemetry = run_dir / TELEMETRY_NAME
    if telemetry.is_file():
        data = json.loads(telemetry.read_text(encoding="utf-8", errors="replace"))
        print("\n--- the gates a NEXT switch runs behind (all three must be handed back) ---")
        for key in GATES:
            print(f"  {key} = {data.get(key)}")
        print("--- context ---")
        for key in CONTEXT:
            print(f"  {key} = {data.get(key)}")
    return 0


def selftest() -> int:
    """A synthetic two-switch log: the first tears down, the second arms and does nothing."""
    sample = "\n".join(
        [
            "[+1000ms] 2026-09-05 00:00:01:00 dll:abcd1234 system-quit-dup: ProfileSelect slot activation ARMED cursor=1 bound=10 row->slot=1",
            "[+1200ms] 2026-09-05 00:00:01:20 dll:abcd1234 system-quit-quickload: native return-title REQUEST fired 0x14067b1f0",
            "[+4000ms] 2026-09-05 00:00:04:00 dll:abcd1234 WORLD LOST #1: c30 0x1c000000 -> 0xa010000",
            "[+4100ms] 2026-09-05 00:00:04:10 dll:abcd1234 own-load-switch-reload: picked slot 1 mounted",
            "[+4200ms] 2026-09-05 00:00:04:20 dll:abcd1234 system-quit-quickload: System->Quit->Load Character re-armed for the next load source=x",
            "[+9000ms] 2026-09-05 00:00:09:00 dll:abcd1234 unrelated chatter that must not appear",
            "[+9500ms] 2026-09-05 00:00:09:50 dll:abcd1234 system-quit-dup: ProfileSelect slot activation ARMED cursor=0 bound=1 row->slot=0",
        ]
    )
    events = timeline(sample)
    assert len(events) == 6, f"expected 6 events, got {len(events)}: {events}"
    assert events[0][0] == 1.0 and "slot activation ARMED" in events[0][1]
    assert all("unrelated chatter" not in body for _, body in events)
    assert sum(1 for _, body in events if "WORLD LOST" in body) == 1

    with tempfile.TemporaryDirectory() as tmp:
        run_dir = pathlib.Path(tmp)
        (run_dir / LOG_NAME).write_text(sample, encoding="utf-8")
        (run_dir / TELEMETRY_NAME).write_text(
            json.dumps({key: 1 for key in GATES}), encoding="utf-8"
        )
        assert report(run_dir) == 0
    print("er-switch-timeline: selftest OK")
    return 0


def main() -> int:
    args = sys.argv[1:]
    if not args or args[0] in ("-h", "--help"):
        print(__doc__)
        return 0 if args else 2
    if args[0] == "--selftest":
        return selftest()
    return report(pathlib.Path(args[0]).expanduser())


if __name__ == "__main__":
    sys.exit(main())
