#!/usr/bin/env python3
"""Fail-closed replay gate for the requestCode=1 / MoveMapStep=18 finalization blocker.

Usage: python3 scripts/check-ifpe-finalization-proof.py RUN_DIR
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

STALL_MIN_MS = 60_000
MAX_SAMPLE_GAP_MS = 2_000
MOVE_NOISE_FRAMES = 3


def load_json(path: Path) -> dict:
    try:
        value = json.loads(path.read_text(encoding="utf-8", errors="strict"))
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise ValueError(f"cannot read {path}: {exc}") from exc
    if not isinstance(value, dict):
        raise ValueError(f"{path} is not a JSON object")
    return value


def load_rows(path: Path) -> list[dict]:
    try:
        lines = path.read_text(encoding="utf-8", errors="strict").splitlines()
    except (OSError, UnicodeError) as exc:
        raise ValueError(f"cannot read {path}: {exc}") from exc
    rows: list[dict] = []
    for lineno, line in enumerate(lines, 1):
        if not line.strip():
            continue
        try:
            row = json.loads(line)
        except json.JSONDecodeError as exc:
            raise ValueError(f"{path}:{lineno}: invalid JSON: {exc}") from exc
        if not isinstance(row, dict):
            raise ValueError(f"{path}:{lineno}: telemetry row is not a JSON object")
        rows.append(row)
    rows.sort(key=lambda row: as_int(row.get("t_ms"), 0))
    if not rows:
        raise ValueError(f"{path} has no telemetry rows")
    return rows


def as_int(value: object, default: int = -1) -> int:
    if isinstance(value, bool):
        return int(value)
    return value if isinstance(value, int) else default


def terminal_snapshot(run_dir: Path) -> tuple[dict, dict]:
    telemetry = load_json(run_dir / "er-effects-telemetry.json")
    readiness = load_json(run_dir / "readiness-result.json")
    captured = readiness.get("telemetry")
    if isinstance(captured, dict):
        telemetry.update(captured)
    # The generic readiness watcher can publish a post-teardown snapshot after the decisive movement
    # monitor has already stopped the process. Its schema/value set may therefore be stale relative to
    # the RAM sample that caused MOVEMENT_PROVEN. The monitor's final ordered timeseries row is the
    # exact terminal oracle sample; let it override the generic watcher snapshot.
    rows = load_rows(run_dir / "telemetry-timeseries.jsonl")
    telemetry.update(rows[-1])
    return telemetry, readiness


def exact_stall_row(row: dict) -> bool:
    return (
        as_int(row.get("oracle_stepfinish_request_code")) == 1
        and as_int(row.get("oracle_stepfinish_mms_state")) == 18
        and row.get("oracle_chr_draw_group_enabled") is False
        and row.get("oracle_can_move") is False
    )


def longest_exact_stall_ms(rows: list[dict]) -> int:
    longest = 0
    start: int | None = None
    previous: int | None = None
    for row in rows:
        now = as_int(row.get("t_ms"), 0)
        if exact_stall_row(row):
            if start is None or previous is None or now - previous > MAX_SAMPLE_GAP_MS:
                start = now
            previous = now
            longest = max(longest, now - start)
        else:
            start = None
            previous = None
    return longest


def max_horizontal_delta(rows: list[dict]) -> float:
    positions: list[tuple[float, float]] = []
    for row in rows:
        # Ignore load/spawn teleports. Only movement after the harness has supplied same-stage input is
        # objective evidence that the input caused a position delta.
        if as_int(row.get("oracle_supplied_movement_input_frames"), 0) <= 0:
            continue
        value = row.get("oracle_havok_pos")
        if isinstance(value, list) and len(value) >= 3:
            try:
                positions.append((float(value[0]), float(value[2])))
            except (TypeError, ValueError):
                continue
    if not positions:
        return 0.0
    origin_x, origin_z = positions[0]
    return max(
        ((x - origin_x) ** 2 + (z - origin_z) ** 2) ** 0.5
        for x, z in positions
    )


def evaluate(run_dir: Path) -> list[str]:
    telemetry, readiness = terminal_snapshot(run_dir)
    rows = load_rows(run_dir / "telemetry-timeseries.jsonl")
    failures: list[str] = []

    stall_ms = longest_exact_stall_ms(rows)
    terminal_next = as_int(telemetry.get("oracle_mms_next_step_4c"))
    if stall_ms >= STALL_MIN_MS and terminal_next == 18:
        failures.append(
            "exact ifpe stall reproduced for "
            f"{stall_ms / 1000:.1f}s: requestCode=1, MoveMapStep state/next=18, "
            "draw_group=false, can_move=false"
        )

    # 1.16.2 static proof: MoveMap state 18 / finalize substate 0 / requestCode 1 is the
    # intentional resident-world window. Leaving 18 enters Cleanup/Finish (world teardown), so it is
    # not a readiness requirement. The red gate is the resident window with its native control
    # predicates still false and no objective movement.
    resident_rows = [
        row
        for row in rows
        if as_int(row.get("oracle_stepfinish_request_code")) == 1
        and as_int(row.get("oracle_stepfinish_mms_state")) == 18
        and as_int(row.get("oracle_stepfinish_finalize_substate_12a"), 0) == 0
        and row.get("oracle_player_present") is True
        and row.get("oracle_player_render_ready") is True
    ]
    if not resident_rows:
        failures.append(
            "native resident-world window was not proven: expected requestCode=1, "
            "MoveMap=18/finalize=0, player present and rendered"
        )

    controls_enabled = telemetry.get("oracle_native_controls_enabled")
    lua_flags = as_int(telemetry.get("oracle_chr_ctrl_lua_event_flags"))
    disable_move = telemetry.get("oracle_chr_ctrl_disable_move")
    if controls_enabled is not True:
        failures.append(
            "native controls never enabled: "
            f"enabled={controls_enabled!r} luaEventFlags=0x{lua_flags & 0xff:02x} "
            f"disableMove={disable_move!r} "
            f"taskRegistration={telemetry.get('oracle_mms_task_registration_4b8')!r} "
            f"controlInput={telemetry.get('oracle_mms_control_enable_4ba')!r} "
            f"pause={telemetry.get('oracle_mms_pause_game_128')!r} "
            f"disable348={telemetry.get('oracle_mms_disable_tasks_348')!r} "
            f"globalDisable={telemetry.get('oracle_mms_global_tasks_disabled')!r} "
            "(requires luaEventFlags&0x60==0x60, !disableMove, and the native MoveMap task path)"
        )

    target_present = as_int(telemetry.get("oracle_l2_target_block_present"))
    block_phase = telemetry.get("oracle_own_load_wbr_max_phase")
    if target_present != 1 or block_phase not in ("0xa", 10):
        failures.append(
            "target world block was not proven resident: "
            f"target_present={target_present} max_phase={block_phase!r}"
        )

    load_done = as_int(telemetry.get("oracle_now_loading"))
    cover_visible = telemetry.get("oracle_fake_loading_visible")
    close_hits = as_int(telemetry.get("oracle_loading_screen_close_sent_hits"), 0)
    if load_done != 1 or cover_visible not in (False, 0) or close_hits <= 0:
        failures.append(
            "native loading surface did not complete: "
            f"load_done={load_done} fake_cover_visible={cover_visible!r} close_hits={close_hits}"
        )

    verdict = max(
        (as_int(row.get("oracle_harness_move_verdict")) for row in rows),
        default=-1,
    )
    supplied = max(
        (as_int(row.get("oracle_supplied_movement_input_frames"), 0) for row in rows),
        default=0,
    )
    moved = max(
        (
            max(
                as_int(row.get("oracle_did_move_frames"), 0),
                as_int(row.get("oracle_move_probe_moved_frames"), 0),
            )
            for row in rows
        ),
        default=0,
    )
    position_delta = max_horizontal_delta(rows)
    if (
        verdict != 1
        or supplied <= 0
        or moved <= MOVE_NOISE_FRAMES
        or position_delta < 0.05
    ):
        failures.append(
            f"movement proof failed: verdict={verdict} supplied={supplied} moved={moved} "
            f"horizontal_delta={position_delta:.3f}"
        )

    unsuppressed = as_int(
        telemetry.get("oracle_foreign_input_unsuppressed_events")
    )
    suppression_reads = as_int(telemetry.get("oracle_input_suppression_reads"), 0)
    if unsuppressed != 0 or suppression_reads <= 0:
        failures.append(
            "foreign-input suppression proof failed: "
            f"unsuppressed={unsuppressed} suppression_reads={suppression_reads}"
        )

    msgbox_builds = as_int(telemetry.get("oracle_msgbox_total_builds"))
    blocking_modal = telemetry.get("oracle_blocking_modal_present")
    if msgbox_builds != 0 or blocking_modal is not False:
        failures.append(
            "zero-MessageBoxDialog proof failed: "
            f"builds={telemetry.get('oracle_msgbox_total_builds')!r} "
            f"blocking_modal={blocking_modal!r}"
        )

    monitor_report = run_dir / "monitor-report.md"
    monitor_result = (
        monitor_report.read_text(encoding="utf-8", errors="replace")
        if monitor_report.is_file()
        else ""
    )
    watcher_world_stable = (
        readiness.get("ready") is True and readiness.get("reason") == "world_stable"
    )
    movement_oracle_stop = "result: **MOVEMENT_PROVEN**" in monitor_result
    if not watcher_world_stable and not movement_oracle_stop:
        failures.append(
            "run did not terminate from a RAM/native readiness semaphore: "
            f"ready={readiness.get('ready')!r} reason={readiness.get('reason')!r} "
            f"movement_oracle_stop={movement_oracle_stop}"
        )
    return failures


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        print(f"usage: {Path(argv[0]).name} RUN_DIR", file=sys.stderr)
        return 2
    run_dir = Path(argv[1]).resolve()
    try:
        failures = evaluate(run_dir)
    except ValueError as exc:
        print(f"IFPE FINALIZATION PROOF: FAIL\n- {exc}")
        return 1
    if failures:
        print("IFPE FINALIZATION PROOF: FAIL")
        for failure in failures:
            print(f"- {failure}")
        return 1
    print(
        "IFPE FINALIZATION PROOF: PASS -- native finalization, draw-group, readiness, "
        "movement, modal, and input-contamination gates passed"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
