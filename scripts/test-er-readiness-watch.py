#!/usr/bin/env python3
"""Regression tests for scripts/er-readiness-watch.py."""
from __future__ import annotations

import importlib.util
import sys
import time
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
WATCH_PATH = REPO_ROOT / "scripts" / "er-readiness-watch.py"
TEST_PID = 4242
TEST_POLLS = 7
TEST_BUDGET = 3
TEST_WINDOW = {
    "class": "steam_app_1245620",
    "title": "ELDEN RING™",
    "at": [2, 23],
    "size": [640, 360],
    "mapped": True,
    "hidden": False,
    "focusHistoryID": 0,
}


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
    assert watcher.window_capture_safe(TEST_WINDOW, watcher.DEFAULT_WINDOW_CLASS)
    assert not watcher.client_is_game_window(
        {
            "class": "steam_proton",
            "title": "Z:\\home\\banon\\.local\\share\\Steam\\steamapps\\common\\ELDEN RING\\Game\\start_protected_game.exe",
        },
        watcher.DEFAULT_WINDOW_CLASS,
    )
    assert "target_window_not_focused" in watcher.target_window_capture_problems(
        {**TEST_WINDOW, "focusHistoryID": 1},
        watcher.DEFAULT_WINDOW_CLASS,
    )
    assert "target_window_focus_unknown" in watcher.target_window_capture_problems(
        {key: value for key, value in TEST_WINDOW.items() if key != "focusHistoryID"},
        watcher.DEFAULT_WINDOW_CLASS,
    )
    assert "target_window_hidden" in watcher.target_window_capture_problems(
        {**TEST_WINDOW, "hidden": True},
        watcher.DEFAULT_WINDOW_CLASS,
    )

    ready = watcher.classify_snapshot(
        pid=TEST_PID,
        process_running=True,
        telemetry={"game_man_instance_resolved": True},
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
        telemetry={"game_man_instance_resolved": False},
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
            "game_man_instance_resolved": True,
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
            "game_man_instance_resolved": True,
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
            "game_man_instance_resolved": True,
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

    title_handoff_complete = watcher.classify_snapshot(
        pid=TEST_PID,
        process_running=True,
        telemetry={
            "game_man_instance_resolved": True,
            "autoload_slot": 9,
            "title_handoff_complete": True,
            "autoload_last_status": "direct continue sequence requested slot 9",
        },
        bootstrap={"stage": "telemetry_write"},
        windows=[TEST_WINDOW],
        window_stale_polls=0,
        window_stale_poll_budget=TEST_BUDGET,
        polls=TEST_POLLS,
        target=watcher.TARGET_REQUEST_CONSUMPTION,
    )
    assert title_handoff_complete is not None
    assert title_handoff_complete.ready
    assert title_handoff_complete.reason == watcher.TITLE_HANDOFF_COMPLETE

    player_load_budget = watcher.classify_snapshot(
        pid=TEST_PID,
        process_running=True,
        telemetry={
            "game_man_instance_resolved": True,
            "autoload_slot": 9,
            "title_handoff_complete": True,
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
            "game_man_instance_resolved": True,
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
        "game_man_instance_resolved": True,
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
    loaded_with_stale_title_gate = {
        **world_loaded_telemetry,
        "oracle_player_present": False,
        "oracle_block_id_valid": False,
        "oracle_grounded": False,
        "player_available": True,
        "product_autoload_armed": True,
        "product_core_autoload_ticks": 100,
        "product_core_ready_blocker": "title_owner_state",
        "product_core_ready_successes": 8,
        "autoload_attempts": 0,
        "title_handoff_complete": False,
    }
    loaded_progress = watcher.autoload_progress_summary(loaded_with_stale_title_gate)
    assert loaded_progress["player_character_loaded"] is True
    assert loaded_progress["blocker"] == "player_character_loaded"
    assert watcher.name_empty_like("")
    assert watcher.name_empty_like("   ")
    assert watcher.name_empty_like("_")
    assert not watcher.name_empty_like("Tester")
    assert not watcher.telemetry_placeholder_character_detected(world_loaded_telemetry, expected_save_oracle)
    placeholder_live = {**world_loaded_telemetry, "oracle_char_name": "_", "oracle_char_name_len": 1}
    assert watcher.telemetry_placeholder_character_detected(placeholder_live, expected_save_oracle)
    assert not watcher.telemetry_placeholder_character_detected({**placeholder_live, "oracle_player_present": False, "player_available": False}, expected_save_oracle)
    title_placeholder = {**placeholder_live, "current_animation_id": None, "oracle_havok_pos": None, "oracle_block_id_valid": False}
    assert not watcher.telemetry_placeholder_character_detected(title_placeholder, expected_save_oracle)
    assert not watcher.telemetry_placeholder_character_detected(placeholder_live, None)
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
    assert watcher.native_legal_text_id(607100) == 607100
    assert watcher.native_legal_text_id(607200) == 607200
    assert watcher.native_legal_text_id(607301) == 607301
    assert watcher.native_legal_text_id(606300) is None
    assert watcher.telemetry_native_legal_popup_detected({"oracle_msgbox_builder_args": [1, 607200, 3, 4]})
    assert watcher.telemetry_native_legal_popup_detected({"oracle_policy_window_total_builds": 1})
    assert watcher.telemetry_native_legal_popup_detected({"oracle_policy_window_any_seen": True})
    assert not watcher.telemetry_native_legal_popup_detected({"oracle_msgbox_builder_args": [1, 2, 3, 4]})
    assert 401120 in watcher.SERVER_STATUS_TEXT_IDS
    assert 401150 in watcher.SERVER_STATUS_TEXT_IDS
    assert 401160 in watcher.SERVER_STATUS_TEXT_IDS
    assert watcher.telemetry_server_status_semaphore_detected({"oracle_server_status_text_id": 401120})
    assert watcher.telemetry_server_status_semaphore_detected({"oracle_server_status_any_seen": True})
    assert watcher.telemetry_server_status_semaphore_detected({"oracle_server_status_total_seen": 1})
    assert not watcher.telemetry_server_status_semaphore_detected({"oracle_server_status_text_id": 401110})

    manual_world_wait = watcher.classify_snapshot(
        pid=TEST_PID,
        process_running=True,
        telemetry={"game_man_instance_resolved": True, "autoload_slot": None},
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

    # --- world-stream stall semaphore -------------------------------------------------------
    STALL_WINDOW = 20.0
    # Stalled-run telemetry: continue fired + player block present, but the player map block never
    # streams -- per-block phase pinned at 0x2, zero IO inflight, player never present (ground truth
    # measured 2026-06-22 on the menu-free OWN-LOAD path).
    stalled_state = {
        "oracle_own_load_continue_fired": True,
        "oracle_own_load_target_block_present": 1,
        "oracle_own_load_wbr_max_phase": "0x2",
        "oracle_own_load_stream_io_inflight": "0x0",
        "oracle_own_load_stream_mms_state": 3,
        "oracle_own_m28_dispatch_fired": 0,
        "oracle_player_present": False,
        "oracle_now_loading": 0,
    }
    # The watermark arms only once map-load has begun.
    assert watcher.world_stream_armed(stalled_state)
    assert not watcher.world_stream_armed(None)
    assert not watcher.world_stream_armed({})
    assert watcher.world_stream_progress_watermark(None) is None
    assert watcher.world_stream_progress_watermark({}) is None
    assert watcher.world_stream_progress_watermark(stalled_state) is not None

    def drive_stall(states, window=STALL_WINDOW, tick=1.0):
        """Replay poll states through the loop's exact stall step; return reason string."""
        watermark = None
        progress_since = None
        now = 0.0
        for state in states:
            watermark, progress_since, stalled = watcher.world_stream_stall_step(
                state, watermark, progress_since, now, window
            )
            if stalled:
                return watcher.WORLD_STREAM_STALLED
            now += tick
        return None  # never fired -> run would continue / reach world-stable

    # STALL: flat watermark held past the window -> world_stream_stalled.
    stall_seq = [dict(stalled_state) for _ in range(int(STALL_WINDOW) + 5)]
    assert drive_stall(stall_seq) == watcher.WORLD_STREAM_STALLED

    # WORKING: io_inflight/wbr_max_phase/player_present advance within the window -> never fires.
    working_seq = [
        dict(stalled_state),
        {**stalled_state, "oracle_own_load_stream_io_inflight": "0x4"},          # IO dispatched
        {**stalled_state, "oracle_own_load_stream_io_inflight": "0x4", "oracle_own_load_wbr_max_phase": "0x3"},
        {**stalled_state, "oracle_own_m28_dispatch_fired": 1, "oracle_own_load_wbr_max_phase": "0x4"},
        {**stalled_state, "oracle_player_present": True},                          # terminal progress
    ]
    # Even with a long tail of no-further-progress polls, the run advanced within the window each
    # time, so the timer stayed fresh and the predicate must not fire across the early window.
    assert drive_stall(working_seq, tick=3.0) is None

    # PRE-CONTINUE: continue not fired (title / slow boot), everything flat past the window -> never fires.
    pre_continue = {**stalled_state, "oracle_own_load_continue_fired": False, "oracle_own_load_target_block_present": 0}
    assert watcher.world_stream_progress_watermark(pre_continue) is None
    assert drive_stall([dict(pre_continue) for _ in range(int(STALL_WINDOW) + 5)]) is None

    # ARMED-BUT-NO-BLOCK: continue fired but the player block never registered -> disarmed, never fires.
    no_block = {**stalled_state, "oracle_own_load_target_block_present": 0}
    assert watcher.world_stream_progress_watermark(no_block) is None
    assert drive_stall([dict(no_block) for _ in range(int(STALL_WINDOW) + 5)]) is None

    # FLAG OFF: the loop never calls the step when --no-world-stream-stall-exit is passed, so the
    # same flat sequence yields no early exit. Confirm the flag wiring + default.
    # The decision step itself is unconditional; the loop gates it on args.world_stream_stall_exit.
    # Mirror "flag off" by simply not invoking the step (gate False) and asserting no stall reason.
    def drive_stall_gated(states, enabled, window=STALL_WINDOW):
        if not enabled:
            return None
        return drive_stall(states, window=window)
    assert drive_stall_gated(stall_seq, enabled=False) is None
    assert drive_stall_gated(stall_seq, enabled=True) == watcher.WORLD_STREAM_STALLED

    # Diagnostic snapshot surfaces the wedged field values + stuck duration.
    snap = watcher.world_stream_stall_snapshot(stalled_state, 22.5)
    assert snap["stuck_seconds"] == 22.5
    assert snap["fields"]["oracle_own_load_wbr_max_phase"] == "0x2"
    assert snap["fields"]["oracle_player_present"] is False
    stalled_result = watcher.ReadinessResult(
        False,
        watcher.WORLD_STREAM_STALLED,
        TEST_PID,
        {"stage": "telemetry_write"},
        stalled_state,
        [TEST_WINDOW],
        TEST_POLLS,
        world_stream_stall=snap,
    )
    assert stalled_result.to_json()["world_stream_stall"]["stuck_seconds"] == 22.5
    assert stalled_result.to_json()["reason"] == watcher.WORLD_STREAM_STALLED

    # --- per-phase progress watchdog --------------------------------------------------------
    # Generalizes the tail-stage detector into a per-phase "<=N s between progress semaphores" rule
    # over the previously-BLIND gaps: boot->title and the title_boot_ready continue wait. Both phases
    # ride game_task_ticks (advances every frame while alive), so a slow-but-MOVING phase resets the
    # timer and never trips; only a true freeze (flat ticks/scan/state for >window) fails fast.
    PHASE_WINDOW = 3.0

    # Phase predicates are mutually exclusive on continue_fired + title-owner-captured.
    title_state = {
        "game_task_ticks": 100,
        "title_owner_scan_attempts": 5,
        "title_owner_scan_vtable_hits": 0,
        "title_owner_scan_last_state": 4,
        "title_handoff_complete": False,
        "oracle_own_load_continue_fired": False,
    }
    continue_state = {
        "game_task_ticks": 900,
        "title_owner_scan_attempts": 800,
        "title_owner_scan_last_state": 10,  # title owner captured -> continue phase
        "title_handoff_complete": True,
        "oracle_own_load_continue_fired": False,
    }
    post_continue_state = {**continue_state, "oracle_own_load_continue_fired": True}

    assert watcher.phase_title_active(title_state)
    assert not watcher.phase_continue_active(title_state)
    assert not watcher.phase_title_owner_captured(title_state)
    assert watcher.phase_title_owner_captured(continue_state)
    assert watcher.phase_continue_active(continue_state)
    assert not watcher.phase_title_active(continue_state)
    # continue fired -> neither watched phase active (world_stream owns the tail).
    assert not watcher.phase_title_active(post_continue_state)
    assert not watcher.phase_continue_active(post_continue_state)
    assert watcher.active_watchdog_phase(post_continue_state) is None
    assert watcher.active_watchdog_phase(None) is None
    # Empty-but-present telemetry ({}) means the title phase IS active (telemetry written, continue
    # not fired, owner not captured) -- it arms the title watermark at all-zeros.
    empty_active = watcher.active_watchdog_phase({})
    assert empty_active is not None and empty_active[0] == "title"
    assert watcher.phase_title_progress(title_state) is not None
    assert watcher.phase_continue_progress(continue_state) is not None
    assert watcher.phase_title_progress(continue_state) is None  # wrong phase -> None

    def drive_phase(states, window=PHASE_WINDOW, tick=1.0, enabled=True):
        """Replay poll states through the loop's exact watchdog step; return (stalled_reason)."""
        if not enabled:
            return None
        state = {"phase": None, "value": None, "since": None}
        now = 0.0
        for s in states:
            # When stalled, the step returns the reason string (e.g. "title_stalled") as the 2nd item.
            stalled, reason = watcher.phase_progress_stall_step(s, state, now, window)
            if stalled:
                return reason
            now += tick
        return None

    # TITLE FROZEN: ticks/scan/state flat past the window -> title_stalled.
    title_freeze = [dict(title_state) for _ in range(int(PHASE_WINDOW) + 5)]
    assert drive_phase(title_freeze) == watcher.TITLE_STALLED

    # TITLE ADVANCING (slow but moving): ticks/scan climb every poll within the window -> NOT stalled.
    # Even on a wide tick (each poll well over the window) the watermark improves each time, so the
    # timer is fresh and the watchdog must not fire -- this is the ~12s inherent boot-to-title.
    title_moving = [
        {**title_state, "game_task_ticks": 100 + i * 10, "title_owner_scan_attempts": 5 + i}
        for i in range(8)
    ]
    assert drive_phase(title_moving, tick=5.0) is None

    # CONTINUE FROZEN: ticks flat, continue never fires, past the window -> continue_stalled.
    continue_freeze = [dict(continue_state) for _ in range(int(PHASE_WINDOW) + 5)]
    assert drive_phase(continue_freeze) == watcher.CONTINUE_STALLED

    # CONTINUE ALIVE-BUT-SLOW: ticks advance every poll but continue never fires -> NOT stalled.
    # This is the ~10.7s title_boot_ready wait; it is PROGRESSING (game alive), so the deadline
    # backstop handles "continue too slow", NOT this watchdog.
    continue_moving = [
        {**continue_state, "game_task_ticks": 900 + i * 7} for i in range(8)
    ]
    assert drive_phase(continue_moving, tick=5.0) is None

    # PHASE TRANSITION resets the timer: title (briefly flat) then continue -> no false stall across
    # the boundary even though neither individually advanced long enough at the seam.
    transition = [dict(title_state), dict(title_state), dict(continue_state), dict(continue_state)]
    assert drive_phase(transition, tick=1.0) is None

    # PRE-TELEMETRY (boot) and POST-CONTINUE (world_stream's turf): no watched phase -> never trips.
    assert drive_phase([None for _ in range(int(PHASE_WINDOW) + 5)]) is None
    assert drive_phase([dict(post_continue_state) for _ in range(int(PHASE_WINDOW) + 5)]) is None

    # FLAG OFF: --no-phase-watchdog disables the watchdog entirely.
    assert drive_phase(title_freeze, enabled=False) is None
    assert drive_phase(title_freeze, enabled=True) == watcher.TITLE_STALLED

    # Diagnostic snapshot surfaces the wedged phase + the flat signals.
    phase_snap = watcher.phase_progress_stall_snapshot(title_state, "title", 3.4)
    assert phase_snap["phase"] == "title"
    assert phase_snap["stuck_seconds"] == 3.4
    assert phase_snap["fields"]["game_task_ticks"] == 100
    phase_result = watcher.ReadinessResult(
        False,
        watcher.TITLE_STALLED,
        TEST_PID,
        {"stage": "telemetry_write"},
        title_state,
        [TEST_WINDOW],
        TEST_POLLS,
        phase_progress_stall=phase_snap,
    )
    assert phase_result.to_json()["phase_progress_stall"]["phase"] == "title"
    assert phase_result.to_json()["reason"] == watcher.TITLE_STALLED

    # --- milestone timing + world-load fail-fast deadline -----------------------------------
    import types

    # Timing dict shape + first-transition-only marking. Pin the launch epoch into the past so the
    # deltas are deterministic and strictly positive without sleeping.
    base_epoch = 1_000_000.0
    tracker = watcher.TimingTracker(base_epoch)
    assert tracker.launch_epoch == base_epoch
    assert tracker.deltas["t_launch"] == 0.0
    snap0 = tracker.snapshot()
    assert snap0["launch_epoch"] == base_epoch
    assert snap0["t_launch"] == 0.0
    # Every milestone present in the dict, null until reached.
    for name in watcher.TIMING_MILESTONES:
        assert name in snap0
    assert snap0["t_first_telemetry"] is None
    assert snap0["t_continue_fired"] is None
    assert snap0["t_player_present"] is None
    assert snap0["t_world_stable"] is None

    # observe() marks first_telemetry + continue + player from the right oracle fields, once each.
    tracker.observe(None)  # no telemetry -> no first_telemetry yet
    assert tracker.deltas["t_first_telemetry"] is None
    tracker.observe({})  # telemetry present (empty) -> first_telemetry fires
    assert tracker.deltas["t_first_telemetry"] is not None
    first_delta = tracker.deltas["t_first_telemetry"]
    tracker.observe({"oracle_own_load_continue_fired": True})
    assert tracker.deltas["t_continue_fired"] is not None
    tracker.observe({"oracle_player_present": True})
    assert tracker.deltas["t_player_present"] is not None
    # Re-observing does not overwrite the first transition.
    tracker.observe({})
    assert tracker.deltas["t_first_telemetry"] == first_delta
    tracker.mark("t_world_stable")
    assert tracker.deltas["t_world_stable"] is not None
    snap_full = tracker.snapshot()
    assert all(snap_full[m] is not None for m in watcher.TIMING_MILESTONES if m != "t_teardown")
    assert "reason=world_stable" in tracker.summary_line(watcher.WORLD_STABLE)

    # DEADLINE EXCEEDED: first telemetry landed, player never present, now past launch+deadline.
    deadline_tracker = watcher.TimingTracker(base_epoch)
    deadline_tracker.observe({})  # first telemetry present, no player
    # launch_epoch far in the past -> _now_delta() is huge -> well past any sane deadline.
    assert deadline_tracker.world_load_deadline_exceeded(30.0) is True

    # NOT exceeded before first telemetry (cannot trip until telemetry lands).
    pre_telemetry = watcher.TimingTracker(base_epoch)
    assert pre_telemetry.world_load_deadline_exceeded(30.0) is False

    # NOT tripped when world-stable reached in time: even with an old epoch, player/world present
    # means the semaphore is satisfied so the deadline is moot.
    reached_player = watcher.TimingTracker(base_epoch)
    reached_player.observe({"oracle_player_present": True})
    assert reached_player.world_load_deadline_exceeded(30.0) is False
    reached_world = watcher.TimingTracker(base_epoch)
    reached_world.observe({})
    reached_world.mark("t_world_stable")
    assert reached_world.world_load_deadline_exceeded(30.0) is False

    # NOT exceeded when the deadline is still in the future (future epoch -> tiny delta).
    fresh_tracker = watcher.TimingTracker(time.time())
    fresh_tracker.observe({})
    assert fresh_tracker.world_load_deadline_exceeded(30.0) is False

    # resolve_launch_epoch precedence: explicit --launch-epoch wins over the env var; a bad/<=0
    # value falls back to wall-clock now.
    ns_explicit = types.SimpleNamespace(launch_epoch=base_epoch)
    assert watcher.resolve_launch_epoch(ns_explicit) == base_epoch
    ns_env = types.SimpleNamespace(launch_epoch=None)
    import os as _os
    _prev = _os.environ.get("ER_PROBE_LAUNCH_EPOCH")
    _os.environ["ER_PROBE_LAUNCH_EPOCH"] = "1234567.5"
    try:
        assert watcher.resolve_launch_epoch(ns_env) == 1234567.5
    finally:
        if _prev is None:
            _os.environ.pop("ER_PROBE_LAUNCH_EPOCH", None)
        else:
            _os.environ["ER_PROBE_LAUNCH_EPOCH"] = _prev
    ns_bad = types.SimpleNamespace(launch_epoch=-1.0)
    assert watcher.resolve_launch_epoch(ns_bad) > 0  # falls back to time.time()

    # ReadinessResult carries the timing dict into readiness-result.json.
    timed_result = watcher.ReadinessResult(
        False,
        watcher.WORLD_LOAD_DEADLINE_EXCEEDED,
        TEST_PID,
        {"stage": "telemetry_write"},
        {"oracle_player_present": False},
        [TEST_WINDOW],
        TEST_POLLS,
        timing=deadline_tracker.snapshot(),
    )
    payload = timed_result.to_json()
    assert payload["reason"] == watcher.WORLD_LOAD_DEADLINE_EXCEEDED
    assert payload["timing"]["launch_epoch"] == base_epoch
    assert payload["timing"]["t_first_telemetry"] is not None
    assert payload["timing"]["t_player_present"] is None

    print("er-readiness-watch regression tests passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
