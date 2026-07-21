#!/usr/bin/env python3
"""TOOL 1 of the real oracle (user 2026-07-20): build the load1 boot IMPRINT from a known-good run.

Input: a `telemetry-timeseries.jsonl` recorded by capture-samechar-3x.py during a SUCCESSFUL clean
load1 boot (each line = one poll: {"t_ms", ...oracle_* semaphore fields...}). Output: an `imprint.json`
holding the ORDERED sequence of discrete-semaphore TRANSITIONS with their inter-transition timing, plus
key milestone times. This is the ground-truth reference the comparator (TOOL 2, oracle-compare.py)
checks a live run against, so the oracle can say -- with certainty and a stack-trace-like line -- exactly
where a run left the known-good path.

Multiple known-good timeseries can be merged (--merge) to learn which transitions are order-STABLE vs
run to run NON-DETERMINISTIC, and per-transition timing spread. A single input gives a v1 imprint (the
observed order + gaps); non-determinism needs >=2 inputs.

Usage:
  python3 scripts/oracle-imprint.py <timeseries.jsonl> [more.jsonl ...] -o imprint.json
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

# Discrete state semaphores whose value CHANGES mark a boot step. Continuous/noisy fields (fps,
# frame_ms, play_time_ms, havok_pos, per-frame counters, loading-bar frame/permille) are excluded from
# the transition sequence -- they are tracked as milestones/ranges instead, not step boundaries.
DISCRETE_FIELDS = [
    "system_quit_continue_confirm_fresh_deser_count",
    "oracle_system_step_state",
    "oracle_system_step_label",
    "oracle_player_present",
    "oracle_char_name",
    "oracle_player_render_ready",
    "oracle_chr_draw_group_enabled",
    "oracle_chr_render_group_enabled",
    "oracle_chr_enable_render",
    "oracle_now_loading",
    "oracle_fake_loading_any_visible",
    "oracle_loading_bar_enabled",
    "oracle_loading_bar_current_terminal",
    "oracle_loading_screen_close_sent",
    "oracle_load_in_progress_b80",
    "oracle_stepfinish_request_code",
    "oracle_stepfinish_mms_state",
    "oracle_stepfinish_finalize_substate_12a",
    "oracle_stepfinish_warmup",
    "oracle_stepfinish_testnet_stepper_present",
    "oracle_csremo_present",
    "oracle_csremo_remoman_present",
    "oracle_csremo_remo_pending",
    "oracle_play_time_live",
    "oracle_saved_map_c30",
    "sq_repro_state",
]

# Epoch/driver markers kept in rows (for --load-epoch slicing) but EXCLUDED from the transition
# sequence, so a load2 run compares against a load1 imprint on GAME semaphores alone. MUST match
# oracle_common.EXCLUDE_FROM_TRANSITIONS so the imprinter and comparator agree.
_EXCLUDE_FROM_TRANSITIONS = {
    "system_quit_continue_confirm_fresh_deser_count",
    "sq_repro_state",
}


def load_rows(path: Path) -> list[dict]:
    rows = []
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            rows.append(json.loads(line))
        except json.JSONDecodeError:
            continue
    rows.sort(key=lambda r: r.get("t_ms", 0))
    return rows


def extract_transitions(rows: list[dict]) -> list[dict]:
    """Ordered list of discrete-semaphore value changes across the boot."""
    last: dict[str, object] = {}
    transitions: list[dict] = []
    for r in rows:
        t = r.get("t_ms", 0)
        for f in DISCRETE_FIELDS:
            if f in _EXCLUDE_FROM_TRANSITIONS or f not in r:
                continue
            v = r.get(f)
            if f not in last:
                last[f] = v
                # first observed value = a transition from "unknown" (None) so the sequence has a start
                transitions.append({"t_ms": t, "field": f, "from": None, "to": v})
                continue
            if v != last[f]:
                transitions.append({"t_ms": t, "field": f, "from": last[f], "to": v})
                last[f] = v
    transitions.sort(key=lambda x: x["t_ms"])
    # inter-transition gap
    for i, tr in enumerate(transitions):
        tr["gap_ms"] = tr["t_ms"] - (transitions[i - 1]["t_ms"] if i else 0)
    return transitions


def milestones(rows: list[dict]) -> dict:
    """First time key world-readiness conditions hold (for a fast human/agent summary)."""

    def first(pred) -> int | None:
        for r in rows:
            try:
                if pred(r):
                    return r.get("t_ms")
            except Exception:  # noqa: BLE001
                continue
        return None

    def i(r, k, d=None):
        v = r.get(k)
        return v if isinstance(v, int) else d

    return {
        "player_present": first(lambda r: bool(r.get("oracle_player_present"))),
        "char_name_set": first(
            lambda r: r.get("oracle_char_name") not in (None, "", "_")
        ),
        "mms_reached_18": first(lambda r: i(r, "oracle_stepfinish_mms_state") == 18),
        "mms_settled_-1": first(lambda r: i(r, "oracle_stepfinish_mms_state") == -1),
        "now_loading_cleared": first(
            lambda r: i(r, "oracle_now_loading", 1) == 0
            and bool(r.get("oracle_player_present"))
        ),
        "play_time_live": first(lambda r: bool(r.get("oracle_play_time_live"))),
        "request_code_0_inworld": first(
            lambda r: i(r, "oracle_stepfinish_request_code") == 0
            and bool(r.get("oracle_player_present"))
        ),
    }


def build(paths: list[Path], load_epoch: int | None = None) -> dict:
    per_run = []
    for p in paths:
        rows = load_rows(p)
        if load_epoch is not None:
            # Slice to one load epoch (deser==N) so a per-phase imprint (e.g. load1 boot) is not
            # polluted by later loads' transitions in the same timeseries.
            rows = [
                r
                for r in rows
                if r.get("system_quit_continue_confirm_fresh_deser_count") == load_epoch
            ]
        if not rows:
            continue
        trs = extract_transitions(rows)
        per_run.append(
            {
                "source": str(p),
                "rows": len(rows),
                "terminal_t_ms": rows[-1].get("t_ms", 0),
                "transitions": trs,
                "milestones": milestones(rows),
            }
        )
    if not per_run:
        raise SystemExit("no usable timeseries rows in any input")

    base = per_run[0]
    imprint = {
        "version": 1,
        "n_runs": len(per_run),
        "sources": [r["source"] for r in per_run],
        "terminal_t_ms": base["terminal_t_ms"],
        "milestones": base["milestones"],
        "transitions": base["transitions"],
    }
    # NON-DETERMINISM (needs >=2 runs): mark transitions whose ORDER or timing varies across runs.
    if len(per_run) >= 2:
        # signature = (field, to-value); compare the ordered signature list across runs.
        def sig(run):
            return [(tr["field"], json.dumps(tr["to"])) for tr in run["transitions"]]

        base_sig = sig(base)
        stable = True
        for r in per_run[1:]:
            if sig(r) != base_sig:
                stable = False
                break
        imprint["order_stable_across_runs"] = stable
        # timing spread per transition index (min/max gap across runs, where lengths match)
        spreads = []
        for idx in range(len(base["transitions"])):
            gaps = [
                r["transitions"][idx]["gap_ms"]
                for r in per_run
                if idx < len(r["transitions"])
            ]
            spreads.append(
                {"idx": idx, "gap_min_ms": min(gaps), "gap_max_ms": max(gaps)}
            )
        imprint["timing_spread"] = spreads
    else:
        imprint["order_stable_across_runs"] = None  # unknown from a single run
        imprint["note"] = "single run: order/timing non-determinism NOT yet characterized (need >=2)"
    return imprint


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("timeseries", nargs="+", type=Path)
    ap.add_argument("-o", "--out", type=Path, required=True)
    ap.add_argument(
        "--load-epoch",
        type=int,
        default=None,
        help="slice to one load epoch (deser==N): 0=load1 boot, 1=load2, ... (default: all rows)",
    )
    a = ap.parse_args()
    imprint = build(a.timeseries, a.load_epoch)
    a.out.write_text(json.dumps(imprint, indent=2), encoding="utf-8")
    trs = imprint["transitions"]
    print(f"imprint -> {a.out}")
    print(f"  runs={imprint['n_runs']} transitions={len(trs)} terminal={imprint['terminal_t_ms']}ms")
    print("  milestones:")
    for k, v in imprint["milestones"].items():
        print(f"    {k}: {v} ms")
    print("  first 25 boot transitions:")
    for tr in trs[:25]:
        print(f"    +{tr['t_ms']:>7}ms (gap {tr['gap_ms']:>6}) {tr['field']} {tr['from']!r} -> {tr['to']!r}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
