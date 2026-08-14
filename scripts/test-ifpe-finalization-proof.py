#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
import json
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location(
    "ifpe_finalization_proof", HERE / "check-ifpe-finalization-proof.py"
)
assert SPEC and SPEC.loader
MOD = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MOD)


def write_run(root: Path, *, passing: bool) -> None:
    terminal = {
        "oracle_mms_next_step_4c": 18,
        "oracle_foreign_input_unsuppressed_events": 0,
        "oracle_input_suppression_reads": 500,
        "oracle_msgbox_total_builds": 0,
        "oracle_blocking_modal_present": False,
        "oracle_native_controls_enabled": passing,
        "oracle_chr_ctrl_lua_event_flags": 0x60 if passing else 0x40,
        "oracle_chr_ctrl_disable_move": False,
        "oracle_l2_target_block_present": 1,
        "oracle_own_load_wbr_max_phase": "0xa",
        "oracle_now_loading": 1,
        "oracle_fake_loading_visible": 0,
        "oracle_loading_screen_close_sent_hits": 1,
    }
    (root / "er-effects-telemetry.json").write_text(
        json.dumps(terminal), encoding="utf-8"
    )
    readiness = {
        "ready": passing,
        "reason": "world_stable" if passing else "process_exited_before_ready",
        "telemetry": terminal,
    }
    (root / "readiness-result.json").write_text(
        json.dumps(readiness), encoding="utf-8"
    )

    if passing:
        rows = [
            {
                "t_ms": 0,
                "oracle_stepfinish_request_code": 1,
                "oracle_stepfinish_mms_state": 18,
                "oracle_stepfinish_finalize_substate_12a": 0,
                "oracle_chr_draw_group_enabled": False,
                "oracle_can_move": False,
                "oracle_player_present": True,
                "oracle_player_render_ready": True,
                "oracle_harness_move_verdict": 0,
                "oracle_supplied_movement_input_frames": 0,
                "oracle_did_move_frames": 0,
                "oracle_havok_pos": [0.0, 0.0, 0.0],
            },
            {
                "t_ms": 500,
                "oracle_stepfinish_request_code": 1,
                "oracle_stepfinish_mms_state": 18,
                "oracle_stepfinish_finalize_substate_12a": 0,
                "oracle_chr_draw_group_enabled": False,
                "oracle_can_move": True,
                "oracle_player_present": True,
                "oracle_player_render_ready": True,
                "oracle_harness_move_verdict": 0,
                "oracle_supplied_movement_input_frames": 1,
                "oracle_did_move_frames": 0,
                "oracle_havok_pos": [0.0, 0.0, 0.0],
            },
            {
                "t_ms": 1000,
                "oracle_stepfinish_request_code": 1,
                "oracle_stepfinish_mms_state": 18,
                "oracle_stepfinish_finalize_substate_12a": 0,
                "oracle_chr_draw_group_enabled": False,
                "oracle_can_move": True,
                "oracle_player_present": True,
                "oracle_player_render_ready": True,
                "oracle_harness_move_verdict": 1,
                "oracle_supplied_movement_input_frames": 30,
                "oracle_did_move_frames": 20,
                "oracle_havok_pos": [0.2, 0.0, 0.0],
            },
        ]
    else:
        rows = [
            {
                "t_ms": t_ms,
                "oracle_stepfinish_request_code": 1,
                "oracle_stepfinish_mms_state": 18,
                "oracle_stepfinish_finalize_substate_12a": 0,
                "oracle_chr_draw_group_enabled": False,
                "oracle_can_move": False,
                "oracle_player_present": True,
                "oracle_player_render_ready": True,
                "oracle_harness_move_verdict": 0,
                "oracle_supplied_movement_input_frames": 0,
                "oracle_did_move_frames": 0,
                "oracle_havok_pos": [0.0, 0.0, 0.0],
            }
            for t_ms in range(0, 65_001, 500)
        ]
    (root / "telemetry-timeseries.jsonl").write_text(
        "".join(json.dumps(row) + "\n" for row in rows), encoding="utf-8"
    )


def main() -> int:
    with tempfile.TemporaryDirectory(prefix="ifpe-finalization-red-") as tmp:
        root = Path(tmp)
        write_run(root, passing=False)
        failures = MOD.evaluate(root)
        text = "\n".join(failures)
        assert "exact ifpe stall reproduced for 65.0s" in text
        assert "native controls never enabled" in text
        assert "movement proof failed" in text
        assert "RAM/native readiness semaphore" in text

    with tempfile.TemporaryDirectory(prefix="ifpe-finalization-green-") as tmp:
        root = Path(tmp)
        write_run(root, passing=True)
        failures = MOD.evaluate(root)
        assert failures == [], failures

    with tempfile.TemporaryDirectory(prefix="ifpe-finalization-movement-stop-") as tmp:
        root = Path(tmp)
        write_run(root, passing=True)
        readiness = json.loads((root / "readiness-result.json").read_text(encoding="utf-8"))
        readiness.update(ready=False, reason="process_exited_before_ready")
        (root / "readiness-result.json").write_text(
            json.dumps(readiness), encoding="utf-8"
        )
        (root / "monitor-report.md").write_text(
            "result: **MOVEMENT_PROVEN**\n", encoding="utf-8"
        )
        failures = MOD.evaluate(root)
        assert failures == [], failures

    print("ifpe finalization replay gate self-test: PASS")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
