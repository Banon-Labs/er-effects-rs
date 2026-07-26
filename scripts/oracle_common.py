"""Shared logic for the real-oracle tools (imprinter + comparator), user 2026-07-20.

A telemetry timeseries is a list of per-poll dicts ({"t_ms", ...oracle_* fields...}). The oracle models
a boot/load PHASE as an ordered sequence of DISCRETE-SEMAPHORE TRANSITIONS (a tracked field's value
changing). Continuous/noisy fields (fps, frame_ms, play_time_ms, havok_pos, per-frame counters, the
loading-bar frame/permille) are deliberately NOT step boundaries and are excluded here.
"""
from __future__ import annotations

import json
from pathlib import Path

# Discrete state semaphores whose value CHANGE marks a step. Keep imprinter + comparator identical.
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


def load_rows(path: Path) -> list[dict]:
    rows = []
    for line in Path(path).read_text(encoding="utf-8", errors="replace").splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            rows.append(json.loads(line))
        except json.JSONDecodeError:
            continue
    rows.sort(key=lambda r: r.get("t_ms", 0))
    return rows


def key(v: object) -> str:
    """Canonical comparison key for a transition's to/from value (order-insensitive JSON)."""
    return json.dumps(v, sort_keys=True)


# Epoch / driver markers: kept in the rows (the imprinter slices by deser) but EXCLUDED from the
# transition sequence, so a load2 (deser=1) run can be compared against a load1 (deser=0) imprint on
# the GAME semaphores alone -- the deser value and the sq-repro driver state are phase keys, not steps.
EXCLUDE_FROM_TRANSITIONS = {
    "system_quit_continue_confirm_fresh_deser_count",
    "sq_repro_state",
}


def extract_transitions(rows: list[dict]) -> list[dict]:
    """Ordered list of discrete-semaphore value changes across the timeseries."""
    last: dict[str, object] = {}
    transitions: list[dict] = []
    for r in rows:
        t = r.get("t_ms", 0)
        for f in DISCRETE_FIELDS:
            if f in EXCLUDE_FROM_TRANSITIONS or f not in r:
                continue
            v = r.get(f)
            if f not in last:
                last[f] = v
                transitions.append({"t_ms": t, "field": f, "from": None, "to": v})
                continue
            if v != last[f]:
                transitions.append({"t_ms": t, "field": f, "from": last[f], "to": v})
                last[f] = v
    transitions.sort(key=lambda x: x["t_ms"])
    for i, tr in enumerate(transitions):
        tr["gap_ms"] = tr["t_ms"] - (transitions[i - 1]["t_ms"] if i else 0)
    return transitions
