#!/usr/bin/env python3
"""Derive semaphore gates from a passive controller-input trace.

Reads er-effects-input-trace.jsonl (written by the DLL's input-trace mode: pad-edge rows with
embedded semaphore snapshots + change-detected sem rows) and, for every button PRESS, answers:
"what semaphore state was the user waiting for before this press?"

For each press it reports:
  - the full semaphore snapshot at the press
  - the delta vs the semaphore snapshot at the PREVIOUS press (what changed while the user waited)
  - the LAST semaphore transition before the press and how long after it the press came
    (a short gap => that transition IS the gate the user was waiting on)
  - the wait time since the previous press

Output: human-readable report on stdout and, with --json OUT, a machine-usable gate list for
mapping the flow onto the zero-input self-drive stepper.

Usage:
  python3 scripts/analyze-input-trace.py [TRACE.jsonl] [--json OUT.json] [--presses-only]
Default trace path: the native-Windows game dir copy.
"""

import json
import sys

DEFAULT_TRACE = "/mnt/c/SteamLibrary/steamapps/common/ELDEN RING/Game/er-effects-input-trace.jsonl"

# Volatile fields excluded from gate deltas (they change every frame / are advisory context).
VOLATILE = {"stable_frames", "mms_blocks"}


def load_rows(path):
    rows = []
    with open(path, "r", encoding="utf-8", errors="replace") as f:
        for lineno, line in enumerate(f, 1):
            line = line.strip()
            if not line:
                continue
            try:
                rows.append(json.loads(line))
            except json.JSONDecodeError as exc:
                print(f"WARN: {path}:{lineno}: unparseable row ({exc})", file=sys.stderr)
    return rows


def sem_of(row):
    """Semaphore dict of a row: pad rows embed it under 'sem', sem/hb rows carry it inline."""
    if "sem" in row:
        return dict(row["sem"])
    return {
        k: v
        for k, v in row.items()
        if k not in ("t", "ms", "frame", "seq", "packet", "polls", "edges", "dropped")
    }


def sem_delta(prev, cur):
    out = {}
    for k, v in cur.items():
        if k in VOLATILE:
            continue
        if prev.get(k) != v:
            out[k] = {"from": prev.get(k), "to": v}
    return out


def main():
    args = [a for a in sys.argv[1:]]
    json_out = None
    presses_only = False
    if "--json" in args:
        i = args.index("--json")
        json_out = args[i + 1]
        del args[i : i + 2]
    if "--presses-only" in args:
        presses_only = True
        args.remove("--presses-only")
    path = args[0] if args else DEFAULT_TRACE

    rows = load_rows(path)
    if not rows:
        print(f"ERROR: no rows in {path}", file=sys.stderr)
        return 1

    sems = [r for r in rows if r.get("t") == "sem"]
    pads = [r for r in rows if r.get("t") == "pad"]
    presses = [r for r in pads if r.get("pressed")]
    hbs = [r for r in rows if r.get("t") == "hb"]
    print(
        f"trace: {path}\nrows: {len(rows)} total, {len(sems)} sem transitions, "
        f"{len(pads)} pad edges ({len(presses)} presses), {len(hbs)} heartbeats"
    )
    if hbs:
        last = hbs[-1]
        print(
            f"last heartbeat: ms={last.get('ms')} polls={last.get('polls')} "
            f"edges={last.get('edges')} dropped={last.get('dropped')}"
        )
        if last.get("dropped"):
            print("WARN: ring drops occurred; some edges were lost", file=sys.stderr)

    derived = []
    prev_press = None
    for p in presses:
        ms = p.get("ms", 0)
        cur_sem = sem_of(p)
        prev_sem = sem_of(prev_press) if prev_press else {}
        prev_ms = prev_press.get("ms", 0) if prev_press else 0
        # sem transitions strictly between the previous press and this press
        between = [s for s in sems if prev_ms < s.get("ms", 0) <= ms]
        last_transition = between[-1] if between else None
        entry = {
            "ms": ms,
            "frame": p.get("frame"),
            "pressed": p.get("pressed"),
            "released": p.get("released"),
            "wait_since_prev_press_ms": ms - prev_ms if prev_press else None,
            "delta_since_prev_press": sem_delta(prev_sem, cur_sem) if prev_press else None,
            "last_transition_before_press": (
                {
                    "ms": last_transition.get("ms"),
                    "gap_ms": ms - last_transition.get("ms", 0),
                    "changed": sem_delta(
                        sem_of(between[-2]) if len(between) > 1 else prev_sem,
                        sem_of(last_transition),
                    ),
                }
                if last_transition
                else None
            ),
            "sem_at_press": cur_sem,
        }
        derived.append(entry)
        prev_press = p

    for i, e in enumerate(derived):
        print(f"\n=== press #{i}: {e['pressed']} at +{e['ms']}ms (frame {e['frame']}) ===")
        if e["wait_since_prev_press_ms"] is not None:
            print(f"  waited {e['wait_since_prev_press_ms']}ms since previous press")
        if e["delta_since_prev_press"]:
            print("  semaphores that changed while waiting:")
            for k, d in e["delta_since_prev_press"].items():
                print(f"    {k}: {d['from']} -> {d['to']}")
        elif e["delta_since_prev_press"] is not None:
            print("  no gate-field changes since previous press (pure timing/intent)")
        lt = e["last_transition_before_press"]
        if lt:
            print(
                f"  last transition {lt['gap_ms']}ms before press: "
                + ", ".join(f"{k}:{d['from']}->{d['to']}" for k, d in lt["changed"].items())
            )
        if not presses_only:
            s = e["sem_at_press"]
            keys = (
                "focused menu_top menu_opt menu_prof prof_cursor opt_tab in_world player ig_d8 bc4 "
                "quickload_phase fresh_deser mms_step load_done native_loadscreen msgbox_builds"
            ).split()
            print("  state at press: " + " ".join(f"{k}={s.get(k)}" for k in keys if k in s))

    if json_out:
        with open(json_out, "w", encoding="utf-8") as f:
            json.dump({"trace": path, "presses": derived}, f, indent=2)
        print(f"\nwrote {json_out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
