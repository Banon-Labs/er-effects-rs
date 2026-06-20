#!/usr/bin/env python3
"""Regression tests for scripts/er-readiness-watch.py."""
from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
WATCH_PATH = REPO_ROOT / "scripts" / "er-readiness-watch.py"
TEST_PID = 4242
TEST_POLLS = 7
TEST_BUDGET = 3
TEST_WINDOW = {"class": "steam_app_1245620", "title": "ELDEN RING™"}


def load_watcher():
    spec = importlib.util.spec_from_file_location("er_readiness_watch", WATCH_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"failed to load {WATCH_PATH}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def main() -> int:
    watcher = load_watcher()

    assert watcher.client_is_game_window(TEST_WINDOW, watcher.DEFAULT_WINDOW_CLASS)
    assert not watcher.client_is_game_window(
        {
            "class": "steam_proton",
            "title": "Z:\\home\\banon\\.local\\share\\Steam\\steamapps\\common\\ELDEN RING\\Game\\start_protected_game.exe",
        },
        watcher.DEFAULT_WINDOW_CLASS,
    )

    ready = watcher.classify_snapshot(
        pid=TEST_PID,
        process_running=True,
        telemetry={"game_man_available": True},
        bootstrap=None,
        windows=[],
        window_stale_polls=0,
        window_stale_poll_budget=TEST_BUDGET,
        polls=TEST_POLLS,
    )
    assert ready is not None
    assert ready.ready
    assert ready.reason == watcher.READY_REASON

    exited = watcher.classify_snapshot(
        pid=TEST_PID,
        process_running=False,
        telemetry=None,
        bootstrap=None,
        windows=[],
        window_stale_polls=0,
        window_stale_poll_budget=TEST_BUDGET,
        polls=TEST_POLLS,
    )
    assert exited is not None
    assert not exited.ready
    assert exited.reason == watcher.PROCESS_EXITED

    no_bootstrap_waiting = watcher.classify_snapshot(
        pid=TEST_PID,
        process_running=True,
        telemetry=None,
        bootstrap=None,
        windows=[TEST_WINDOW],
        window_stale_polls=1,
        window_stale_poll_budget=TEST_BUDGET,
        polls=TEST_POLLS,
    )
    assert no_bootstrap_waiting is None

    no_bootstrap = watcher.classify_snapshot(
        pid=TEST_PID,
        process_running=True,
        telemetry=None,
        bootstrap=None,
        windows=[TEST_WINDOW],
        window_stale_polls=TEST_BUDGET,
        window_stale_poll_budget=TEST_BUDGET,
        polls=TEST_POLLS,
    )
    assert no_bootstrap is not None
    assert not no_bootstrap.ready
    assert no_bootstrap.reason == watcher.WINDOW_WITHOUT_BOOTSTRAP

    waiting_for_task = watcher.classify_snapshot(
        pid=TEST_PID,
        process_running=True,
        telemetry=None,
        bootstrap={"stage": "game_task_thread_started"},
        windows=[TEST_WINDOW],
        window_stale_polls=TEST_BUDGET - 1,
        window_stale_poll_budget=TEST_BUDGET,
        polls=TEST_POLLS,
    )
    assert waiting_for_task is None

    no_task = watcher.classify_snapshot(
        pid=TEST_PID,
        process_running=True,
        telemetry=None,
        bootstrap={"stage": "game_task_thread_started"},
        windows=[TEST_WINDOW],
        window_stale_polls=TEST_BUDGET,
        window_stale_poll_budget=TEST_BUDGET,
        polls=TEST_POLLS,
    )
    assert no_task is not None
    assert not no_task.ready
    assert no_task.reason == watcher.WINDOW_WITHOUT_TASK

    telemetry_without_game_man = watcher.classify_snapshot(
        pid=TEST_PID,
        process_running=True,
        telemetry={"game_man_available": False},
        bootstrap={"stage": "telemetry_write"},
        windows=[TEST_WINDOW],
        window_stale_polls=TEST_BUDGET,
        window_stale_poll_budget=TEST_BUDGET,
        polls=TEST_POLLS,
    )
    assert telemetry_without_game_man is not None
    assert not telemetry_without_game_man.ready
    assert telemetry_without_game_man.reason == watcher.TELEMETRY_WITHOUT_GAME_MAN

    autoload_requested = watcher.classify_snapshot(
        pid=TEST_PID,
        process_running=True,
        telemetry={
            "game_man_available": True,
            "autoload_slot": 9,
            "autoload_last_status": "direct continue sequence requested slot 9",
        },
        bootstrap={"stage": "telemetry_write"},
        windows=[TEST_WINDOW],
        window_stale_polls=0,
        window_stale_poll_budget=TEST_BUDGET,
        polls=TEST_POLLS,
        target=watcher.TARGET_AUTOLOAD_REQUEST,
    )
    assert autoload_requested is not None
    assert autoload_requested.ready
    assert autoload_requested.reason == watcher.AUTOLOAD_REQUESTED

    autoload_budget = watcher.classify_snapshot(
        pid=TEST_PID,
        process_running=True,
        telemetry={
            "game_man_available": True,
            "autoload_slot": 9,
            "autoload_attempts": TEST_BUDGET,
            "autoload_last_status": "waiting for title bootstrap/save activity before direct continue queue",
        },
        bootstrap={"stage": "telemetry_write"},
        windows=[TEST_WINDOW],
        window_stale_polls=0,
        window_stale_poll_budget=TEST_BUDGET,
        polls=TEST_POLLS,
        target=watcher.TARGET_AUTOLOAD_REQUEST,
        autoload_attempt_budget=TEST_BUDGET,
    )
    assert autoload_budget is not None
    assert not autoload_budget.ready
    assert autoload_budget.reason == watcher.AUTOLOAD_ATTEMPT_BUDGET_REACHED

    post_request_budget = watcher.classify_snapshot(
        pid=TEST_PID,
        process_running=True,
        telemetry={
            "game_man_available": True,
            "autoload_slot": 9,
            "game_task_ticks": TEST_BUDGET,
            "autoload_last_status": "direct continue sequence requested slot 9",
        },
        bootstrap={"stage": "telemetry_write"},
        windows=[TEST_WINDOW],
        window_stale_polls=0,
        window_stale_poll_budget=TEST_BUDGET,
        polls=TEST_POLLS,
        target=watcher.TARGET_REQUEST_CONSUMPTION,
        post_request_tick_budget=TEST_BUDGET,
    )
    assert post_request_budget is not None
    assert not post_request_budget.ready
    assert post_request_budget.reason == watcher.POST_REQUEST_TICK_BUDGET_REACHED

    title_bootstrap_seen = watcher.classify_snapshot(
        pid=TEST_PID,
        process_running=True,
        telemetry={
            "game_man_available": True,
            "autoload_slot": 9,
            "title_bootstrap_seen": True,
            "autoload_last_status": "direct continue sequence requested slot 9",
        },
        bootstrap={"stage": "telemetry_write"},
        windows=[TEST_WINDOW],
        window_stale_polls=0,
        window_stale_poll_budget=TEST_BUDGET,
        polls=TEST_POLLS,
        target=watcher.TARGET_REQUEST_CONSUMPTION,
    )
    assert title_bootstrap_seen is not None
    assert title_bootstrap_seen.ready
    assert title_bootstrap_seen.reason == watcher.TITLE_BOOTSTRAP_SEEN

    player_load_budget = watcher.classify_snapshot(
        pid=TEST_PID,
        process_running=True,
        telemetry={
            "game_man_available": True,
            "autoload_slot": 9,
            "title_bootstrap_seen": True,
            "game_task_ticks": TEST_BUDGET,
            "autoload_last_status": "direct map load requested slot 9",
        },
        bootstrap={"stage": "telemetry_write"},
        windows=[TEST_WINDOW],
        window_stale_polls=0,
        window_stale_poll_budget=TEST_BUDGET,
        polls=TEST_POLLS,
        target=watcher.TARGET_PLAYER_LOAD,
        post_request_tick_budget=TEST_BUDGET,
    )
    assert player_load_budget is not None
    assert not player_load_budget.ready
    assert player_load_budget.reason == watcher.PLAYER_LOAD_TICK_BUDGET_REACHED
    loading_screen_player_data = watcher.classify_snapshot(
        pid=TEST_PID,
        process_running=True,
        telemetry={
            "game_man_available": True,
            "autoload_slot": 9,
            "player_seen": True,
            "player_available": True,
            "oracle_player_present": True,
            "oracle_block_id_valid": False,
        },
        bootstrap={"stage": "telemetry_write"},
        windows=[TEST_WINDOW],
        window_stale_polls=0,
        window_stale_poll_budget=TEST_BUDGET,
        polls=TEST_POLLS,
        target=watcher.TARGET_PLAYER_LOAD,
    )
    assert loading_screen_player_data is None

    world_loaded_telemetry = {
        "game_man_available": True,
        "player_seen": True,
        "oracle_player_present": True,
        "oracle_block_id_valid": True,
        "oracle_now_loading": 1,
        "oracle_load_in_progress_b80": 0,
        "oracle_grounded": True,
        "oracle_saved_map_c30": "0xa010000",
        "game_save_slot": 0,
        "oracle_char_name": "Tester",
        "oracle_char_name_len": 6,
        "oracle_char_level": 9,
        "oracle_char_current_hp": 522,
        "oracle_char_stats": [15, 10, 11, 14, 13, 9, 9, 7],
        "current_animation_id": watcher.DEFAULT_EXPECTED_ANIMATION_ID,
        "oracle_msgbox_postload_builds": 0,
        "oracle_postload_modal_seen": False,
        "oracle_blocking_modal_present": False,
        "game_task_ticks": TEST_POLLS,
    }
    expected_save_oracle = {
        "source_path": "/tmp/ER0000.sl2",
        "slot": 0,
        "decoded_fields": {
            "name": "Tester",
            "name_len": 6,
            "level": 9,
            "health": 522,
            "stats": [15, 10, 11, 14, 13, 9, 9, 7],
            "saved_map_c30": 0x0A010000,
        },
    }
    assert watcher.telemetry_world_loaded(world_loaded_telemetry, expected_save_oracle, watcher.DEFAULT_EXPECTED_ANIMATION_ID)
    assert watcher.oracle_summary(world_loaded_telemetry, expected_save_oracle, watcher.DEFAULT_EXPECTED_ANIMATION_ID)["character_name"] == "Tester"
    assert watcher.name_empty_like("")
    assert watcher.name_empty_like("   ")
    assert watcher.name_empty_like("_")
    assert not watcher.name_empty_like("Tester")
    mismatched_save = {**expected_save_oracle, "decoded_fields": {**expected_save_oracle["decoded_fields"], "name": "Bonky Bean"}}
    assert not watcher.telemetry_world_loaded(world_loaded_telemetry, mismatched_save, watcher.DEFAULT_EXPECTED_ANIMATION_ID)
    mismatched_slot = {**expected_save_oracle, "slot": 1}
    assert not watcher.telemetry_world_loaded(world_loaded_telemetry, mismatched_slot, watcher.DEFAULT_EXPECTED_ANIMATION_ID)
    assert not watcher.telemetry_world_loaded({**world_loaded_telemetry, "oracle_char_name": "_", "oracle_char_name_len": 1}, expected_save_oracle, watcher.DEFAULT_EXPECTED_ANIMATION_ID)
    assert not watcher.telemetry_world_loaded({**world_loaded_telemetry, "oracle_char_name": "   ", "oracle_char_name_len": 3}, expected_save_oracle, watcher.DEFAULT_EXPECTED_ANIMATION_ID)
    assert not watcher.telemetry_world_loaded({**world_loaded_telemetry, "current_animation_id": 999}, expected_save_oracle, watcher.DEFAULT_EXPECTED_ANIMATION_ID)
    assert not watcher.telemetry_world_loaded({**world_loaded_telemetry, "oracle_msgbox_postload_builds": 1}, expected_save_oracle, watcher.DEFAULT_EXPECTED_ANIMATION_ID)
    assert watcher.telemetry_world_tick(world_loaded_telemetry, 0) == TEST_POLLS
    selector_loaded_telemetry = {
        **world_loaded_telemetry,
        "oracle_grounded": False,
        "oracle_now_loading": 1,
        "oracle_char_level": 9,
        "oracle_char_current_hp": 522,
        "oracle_char_name": "Tester",
        "oracle_char_name_len": 6,
        "oracle_char_stats": [15, 10, 11, 14, 13, 9, 9, 7],
        "oracle_chr_model_ins_present": True,
        "oracle_chr_ctrl_present": True,
        "oracle_chr_onscreen": True,
        "oracle_chr_render_group_enabled": True,
        "oracle_chr_enable_render": True,
        "oracle_player_render_ready": False,
        "oracle_blocking_modal_present": False,
    }
    assert watcher.telemetry_world_loaded(selector_loaded_telemetry)
    selector_loaded_telemetry["oracle_blocking_modal_present"] = True
    assert not watcher.telemetry_world_loaded(selector_loaded_telemetry)
    world_loaded_telemetry["oracle_load_in_progress_b80"] = 1
    assert not watcher.telemetry_world_loaded(world_loaded_telemetry)
    world_loaded_telemetry["oracle_load_in_progress_b80"] = 0
    stable = watcher.ReadinessResult(
        True,
        watcher.WORLD_STABLE,
        TEST_PID,
        {"stage": "telemetry_write"},
        world_loaded_telemetry,
        [TEST_WINDOW],
        TEST_POLLS,
        world_stable_samples=3,
    )
    assert stable.to_json()["world_stable_samples"] == 3
    loading_tip_text = "Critical Hits You can also perform a critical hit when near a stance-broken enemy Next"
    loading_matches = [pattern.pattern for pattern in watcher.LOADING_SCREEN_OCR_PATTERNS if pattern.search(loading_tip_text)]
    assert len(loading_matches) >= 2
    torch_tip_text = "Using Torches Raise your torch to see further into dark spaces"
    torch_matches = [pattern.pattern for pattern in watcher.LOADING_SCREEN_OCR_PATTERNS if pattern.search(torch_tip_text)]
    assert torch_matches
    eula_text = "END USER LICENSE AGREEMENT Please read this Software License Agreement Accept Decline"
    assert watcher.legal_popup_ocr_matches(eula_text)
    assert watcher.legal_popup_ocr_matches("Terms of Service and Privacy Policy")
    assert not watcher.legal_popup_ocr_matches("ELDEN RING Continue Load Game System")

    manual_world_wait = watcher.classify_snapshot(
        pid=TEST_PID,
        process_running=True,
        telemetry={"game_man_available": True, "autoload_slot": None},
        bootstrap={"stage": "telemetry_write"},
        windows=[TEST_WINDOW],
        window_stale_polls=0,
        window_stale_poll_budget=TEST_BUDGET,
        polls=TEST_POLLS,
        target=watcher.TARGET_WORLD_STABLE,
    )
    assert manual_world_wait is None

    no_telemetry = watcher.classify_snapshot(
        pid=TEST_PID,
        process_running=True,
        telemetry=None,
        bootstrap={"stage": "telemetry_write"},
        windows=[TEST_WINDOW],
        window_stale_polls=TEST_BUDGET,
        window_stale_poll_budget=TEST_BUDGET,
        polls=TEST_POLLS,
    )
    assert no_telemetry is not None
    assert not no_telemetry.ready
    assert no_telemetry.reason == watcher.WINDOW_WITHOUT_TELEMETRY

    print("er-readiness-watch regression tests passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
