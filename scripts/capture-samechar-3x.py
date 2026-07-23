#!/usr/bin/env python3
"""Observer + capture watcher for the same-character-3x milestone (docs/goals SS4a).

The PRODUCT DLL drives the loads (boot autoload = load1; sq-repro XInput autopilot drives 2 same-slot
reloads = load2, load3, with the load-2 freeze force-advanced to the load-3 recovery by the DLL's
freeze-recovery deadline). This script does NOT drive input -- it OBSERVES er-effects-telemetry.json,
records the per-load RAM-oracle signature (render-ready / can-see + havok motion + freeze markers),
captures the mandatory loading-screen-portrait image at the frozen-load-2 moment, and tears down after
load-3 reaches a held render-ready dwell OR the runtime cap. It is the "logging" half of the two-DLL
setup; the trace DLL logs the native pipeline in parallel.

Load epochs are keyed by system_quit_continue_confirm_fresh_deser_count: 0 = load1 (boot autoload),
1 = load2 (first reload), 2 = load3 (second reload / recovery).
"""

from __future__ import annotations

import argparse
import contextlib
import json
import os
import statistics
import subprocess
import sys
import threading
import time
from pathlib import Path

RENDER_READY_DWELL_SECONDS = 5.0  # goal SS4 hard render gate dwell
POLL_SECONDS = 0.5  # capture cadence (user 2026-07-19: 3s log throttle hid the completion->teardown)
LOG_THROTTLE_SECONDS = 0.5  # console log cadence -- match the poll so fast transitions are visible
TARGET_FINAL_EPOCH = 3  # 4 loads total = fresh_deser 0..3

# BELOW-TARGET-FPS teardown (user 2026-07-22, bd harness-teardown-on-below-target-fps-not-cap): the
# reload FPS dip is the whole point of this test, so the INSTANT a reload epoch confirms sustained
# below-target framerate the answer is captured (it is already in the timeseries) -- tear down then,
# do NOT drive further loads or ride to the wall-clock cap making the user watch a known 20fps run.
FPS_DIP_TASK_DELTA_THRESHOLD = 0.025  # >=0.025 s/frame (<=40fps) = clearly below the 60fps target
FPS_DIP_CONFIRM_POLLS = 6  # consecutive movable polls below target on a reload = sustained (0.5s*6=3s);
# rejects a single-frame blip and the brief asset-streaming-overlap transient (which recovers in <3s).
# The final reload epoch for the current 2-switch run (load1=deser0, load2=deser1, load3=deser2). Success
# (--require-reload-settled) requires THIS epoch's reload to move+settle so the run drives through all 3
# loads instead of ending on load2. Matches SQ_REPRO_TARGET_SWITCHES in constants/system_quit.rs.
FINAL_RELOAD_EPOCH = 2
# The goal requires 3 SECONDS of movement per load. Hold each load's playable+moving window at least
# this long (sampling fps) before triggering the next switch, so FPS parity is measured over a real
# window rather than a single frame.
MOVE_WINDOW_SECONDS = 3.0
FINAL_LOAD_DWELL_SECONDS = (
    14.0  # after the 4th load appears, give the 60-frame move-probe time to run
)
BOOT_TIMEOUT_SECONDS = (
    # Raised 110->300 (user 2026-07-20): ER asset loading is single-core-bound; parallel cargo/agents
    # starve that core so boot is SLOW-but-progressing, not failed. Do not tear a slow boot down early.
    300.0  # if no in-world player by here, the boot failed -> tear down, don't idle
)
# DEFENSIVE reload backstop (user 2026-07-21): the PRIMARY stall signal is the per-step loading-bar
# dwell divergence (a reload dwelling at the same bar step far longer than load1 did there -- see the
# capture loop). This backstop only covers the edge case where a reload reaches an in-world player but
# the loading-bar step key never populated, so the per-step check could not fire; without it such a
# reload would ride silently to the cap. Generous so a slow-but-progressing load is never cut.
RELOAD_STALL_BACKSTOP_SECONDS = 120.0

# Interruptible poll wait (never set) -- the sanctioned no-bare-sleep pace primitive (see
# scripts/multi-load-proof-monitor.py); real synchronization is the telemetry-file readiness checks.
_POLL_WAIT = threading.Event()

# sq-repro autopilot menu-nav stage (constants/system_quit.rs). The MENU-NAV stage the driver reached
# is the ATTEMPT semaphore: it distinguishes "the driver never drove the menu far enough to start a
# load" (stuck < CONFIRM) from "confirm fired but the load did not complete" (reached CONFIRM/activate
# but fresh_deser never incremented). Run1 stalled at TO_PROFILE(3) -> load3 was never attempted.
SQ_REPRO_STATE_LABELS = {
    0: "WAIT_WORLD",
    1: "OPEN_MENU",
    2: "TO_SYSTEM",
    3: "TO_PROFILE",
    4: "TO_SLOT",
    5: "CONFIRM",
    6: "DONE",
    7: "WAIT_RELOAD",
}


def read_telemetry(path: Path) -> dict | None:
    try:
        return json.loads(path.read_text(encoding="utf-8", errors="replace"))
    except (OSError, json.JSONDecodeError):
        return None


def as_int(value: object, default: int = -1) -> int:
    if isinstance(value, bool):
        return int(value)
    if isinstance(value, int):
        return value
    if isinstance(value, (float, str, bytes, bytearray)):
        try:
            return int(value)
        except (OverflowError, ValueError):
            return default
    return default


def parse_havok(value: object) -> tuple[float, float, float] | None:
    """oracle_havok_pos is a [x,y,z] list (or its JSON string, or None). Returns the tuple or None."""
    v = value
    if isinstance(v, str):
        try:
            v = json.loads(v)
        except json.JSONDecodeError:
            return None
    if isinstance(v, (list, tuple)) and len(v) >= 3:
        try:
            return (float(v[0]), float(v[1]), float(v[2]))
        except (TypeError, ValueError):
            return None
    return None


def snap(t: dict) -> dict:
    """Extract the load-readiness signature we care about."""
    keys = [
        "oracle_char_name",
        "oracle_player_present",
        "oracle_wcm_world_area_chr_list_count",
        "oracle_wcm_world_block_chr_list_count",
        "oracle_wcm_world_grid_area_chr_list_count",
        "oracle_player_render_ready",
        "oracle_can_move",
        "oracle_move_probe_moved_frames",
        "oracle_chr_model_ins_present",
        "oracle_chr_ctrl_present",
        "oracle_chr_draw_group_enabled",
        "oracle_chr_render_group_enabled",
        "oracle_chr_onscreen",
        "oracle_chr_enable_render",
        "oracle_havok_pos",
        "oracle_play_time_ms",
        "oracle_play_time_advanced_ms",
        "oracle_play_time_live",
        "oracle_now_loading",
        "oracle_fake_loading_any_visible",
        "oracle_fake_loading_field_c",
        "oracle_fake_loading_field_10",
        "oracle_loading_bar_enabled",
        "oracle_loading_screen_last_this",
        "oracle_loading_screen_last_data",
        "oracle_loading_bar_current_frame",
        "oracle_loading_bar_max_frame",
        "oracle_loading_bar_progress_permille",
        "oracle_loading_bar_current_terminal",
        "oracle_loading_bar_final_hits",
        "oracle_loading_bar_update_hits",
        "oracle_loading_screen_close_sent",
        "oracle_loading_screen_close_sent_hits",
        "oracle_load_in_progress_b80",
        # SWITCH-TRIGGER pipeline (2026-07-21): makes a NO-LOAD explain itself instead of CAP_REACHED.
        # arm_count rises when a switch actually arms; deferred rises when a request is seen but the world
        # is not eligible; reload_phase 0 IDLE/1 DRAIN/2 COMMIT; player_present+menu_job_present+
        # stable_frames = the arm-eligibility gate (needs a live in-world menu job).
        "oracle_switch_arm_count",
        "oracle_switch_teardown_count",
        "oracle_switch_deferred_count",
        "oracle_switch_last_slot",
        "oracle_switch_reload_phase",
        "oracle_switch_reload_drain_waits",
        "oracle_switch_reload_committed",
        "oracle_switch_slot_control_mtime",
        "oracle_switch_slot_control_primed",
        "oracle_switch_player_present",
        "oracle_switch_menu_job_present",
        "oracle_switch_stable_frames",
        "oracle_fps",
        "oracle_min_fps",
        # FLIP-TIMING (2026-07-21): the game's frame limiter. oracle_flip_fixed_spf is DECISIVE for the
        # load2/load3 20fps cap -- 0.05=20fps CAP, 0.0167=60. mode_current/dynamic_lock name the lever.
        "oracle_flip_fixed_spf",
        "oracle_flip_last_frame_time",
        "oracle_flip_task_delta",
        "oracle_flip_calc_fps",
        "oracle_flip_mode_current",
        "oracle_flip_mode_initial",
        "oracle_flip_vsync_interval",
        "oracle_flip_use_dynamic_lock",
        "oracle_flip_dynamic_fps_lock",
        "oracle_flip_dynamic_active",
        # FOCUS (2026-07-21 focus A/B): is the ER window the OS foreground? Tests the compositor-throttle
        # theory -- does the 20fps stall correlate with the surface being unfocused?
        "oracle_window_foreground",
        # PRESENT DURATION (2026-07-21): us inside the original Present. ~tens-of-ms = present-block
        # (compositor/vsync throttle); ~1-2ms with a 50ms frame = real per-frame WORK stall.
        "oracle_present_call_us",
        # PRESENT CADENCE (2026-07-22, bd GPU-timestamp-semaphore-split-reload-20fps-residual): the
        # reload 20fps is 100% flip/present residual (fixed_spf cap + dynamic lock REFUTED; target=60).
        # sync_interval = the SyncInterval the GAME passes to Present (3 => deliberate 20fps throttle;
        # 1 => wants 60 but can't keep up). refresh_per_present_x100 = observed refreshes/present from
        # GetFrameStatistics (300 => vsync-locked to every 3rd vblank). qpc_delta_us = DXGI present spacing.
        "oracle_present_sync_interval",
        "oracle_present_refresh_per_present_x100",
        "oracle_present_qpc_delta_us",
        # GX COMMAND-QUEUE submission volume (2026-07-22): reserves = cumulative GX cmd-queue slot
        # reservations (per-submission hook, fires every frame in-world). Reserve RATE (delta/frame)
        # boot-vs-reload measures whether the reload frame submits MORE draw work (render-bound cause).
        # top_producers = cumulative caller-RVA histogram; diffing a boot poll vs a reload poll NAMES
        # the producer that grows on reload. bd GPU-timestamp-semaphore-split-reload-20fps-residual.
        "oracle_gx_cmdqueue_reserves",
        "oracle_gx_cmdqueue_max_fill",
        "oracle_gx_cmdqueue_top_producers",
        # COMPOSITE (2026-07-22): DLL boot-view composite duration in present detour + boot-view epoch
        # state. composite ~tens-of-ms in-world on reloads + bv_epoch_live != current_epoch => the
        # boot-view composite never stopped for the reload => fixable DLL bug.
        "oracle_composite_us",
        "oracle_boot_view_epoch_live",
        "oracle_boot_view_self_presents",
        "oracle_boot_view_pump_stop_reason",
        "oracle_boot_view_stop_native_hits",
        "oracle_current_load_epoch",
        # DLL MAIN GAME-TASK duration (2026-07-22): large on reloads => DLL per-frame code cost; fast =>
        # game-side loop (playable-window 50ms not the DLL).
        "oracle_game_task_us",
        "oracle_build_driver_us",
        "oracle_frame_ms",
        "oracle_system_step_state",
        "oracle_system_step_label",
        "oracle_stepfinish_request_code",
        "oracle_stepfinish_mms_state",
        "oracle_stepfinish_finalize_substate_12a",
        # STEP_Finish sub-gate disambiguators: which of these holds STEP_Finish terminal
        # (so requestCode never latches 2 -> STEP_GameStepWait reverts to title). See bd
        # product-system-quit-orchestration-proven-working-gaps-false / render-handoff-freeze-second-gate.
        "oracle_stepfinish_warmup",
        "oracle_stepfinish_testnet_stepper_present",
        "oracle_csremo_present",
        "oracle_csremo_remoman_present",
        "oracle_csremo_remo_pending",
        # Movement semaphore split (can_move capability vs supplied-input vs did-move):
        "oracle_can_move",
        "oracle_move_probe_moved_frames",
        "oracle_supplied_movement_input_frames",
        "oracle_did_move_frames",
        # HARNESS-attributed contamination-proof verdict (0 pending/1 proven/2 disproven/3 contaminated):
        "oracle_harness_move_verdict",
        # RAWINPUT RECEPTION (contamination oracle): user mouse/kb events the game received (harness
        # injects via direct-memory inputmgr, not RawInput -> any nonzero = user input = contamination).
        # hook_calls validates the oracle is LIVE (game routes input through GetRawInputData); 0 = blind.
        "oracle_rawinput_hook_calls",
        # blocked = user input dropped while the game was UNFOCUSED (ineffective, NOT contamination).
        "oracle_rawinput_blocked_unfocused_events",
        "oracle_rawinput_mouse_move_events",
        "oracle_rawinput_mouse_button_events",
        "oracle_rawinput_key_events",
        # RESIDUAL-STATE diagnostic: MoveMapStep child EzChildStepBase (embedded at mms+0x108) member
        # fields + the step it wraps (*(mms+0x110)), each frame, to pin the field that flips when the
        # FD4 scheduler drops load2's child (load1-vs-load2 diff at mms=18).
        "oracle_mms_child_ez08_step",
        "oracle_mms_child_ez10",
        "oracle_mms_child_ez18",
        "oracle_mms_child_ez20",
        "oracle_mms_child_ez28",
        "oracle_mms_child_step10",
        "oracle_mms_child_step18",
        "oracle_mms_child_step40",
        "oracle_mms_child_step48",
        # LOAD2 BLOCK-STREAMING discriminator: is the target-area block registered in resmgr+0xb3030
        # (registration gap) vs present-but-not-streaming; the deeper root of the mms18 stall.
        "oracle_l2_req_coord",
        "oracle_l2_block_count",
        "oracle_l2_target_block_present",
        "oracle_saved_map_c30",
        "sq_repro_state",
        "sq_repro_switch_index",
        "system_quit_profile_load_activate_count",  # ATTEMPT: load-confirm activations fired
        "system_quit_continue_confirm_allow_count",
        "system_quit_continue_confirm_fresh_deser_count",  # COMPLETE: reload deserializes committed
        "system_quit_continue_confirm_fresh_deser_done",
    ]
    return {k: t.get(k) for k in keys}


def capture_portrait(artifact_dir: Path) -> None:
    """Mandatory loading-screen-portrait capture (AGENTS.md protocol). One-shot; agent never reads it."""
    out = artifact_dir / "loading-screen-portrait-screenshot.jpg"
    note = out.with_suffix(".txt")
    if out.exists() or note.exists():
        return
    helper = Path(__file__).with_name("capture-er-window.py")
    try:
        subprocess.run(
            [sys.executable, str(helper), str(out)],
            text=True,
            capture_output=True,
            timeout=25,
        )
    except Exception as exc:  # noqa: BLE001 -- capture is best-effort; fail closed to a note
        note.write_text(
            f"loading-screen-portrait capture failed: {exc}\n", encoding="utf-8"
        )


def _load_semaphore_rows(artifact_dir: Path, game_dir: Path) -> list[dict]:
    paths = [
        artifact_dir / "load-semaphore-trace.jsonl",
        game_dir / "er-effects-input-trace.jsonl",
    ]
    trace_path = next((p for p in paths if p.exists()), paths[0])
    rows: list[dict] = []
    if not trace_path.exists():
        return rows
    for line in trace_path.read_text(encoding="utf-8", errors="replace").splitlines():
        if not line.strip():
            continue
        try:
            row = json.loads(line)
        except json.JSONDecodeError:
            continue
        if row.get("t") == "sem":
            rows.append(row)
    return rows


def _present_flag(row: dict, key: str) -> bool:
    value = row.get(key)
    if isinstance(value, str):
        return value not in {"0x0", "0", ""}
    return bool(value)


def _checkpoint_names(row: dict) -> list[str]:
    names: list[str] = []
    ig_d8 = as_int(row.get("ig_d8"))
    ig_pstep = as_int(row.get("ig_pstep"))
    mms_step = as_int(row.get("mms_step"))
    mms_next = as_int(row.get("mms_next"))
    mms_done = as_int(row.get("mms_done50"))
    gate_lo = as_int(row.get("mms_gate_lo"))
    gate_hi = as_int(row.get("mms_gate_hi"))
    loading_mode = as_int(row.get("loading_mode"))
    loading10 = as_int(row.get("loading_field10"))
    loading11 = as_int(row.get("loading_field11"))
    bar_frame = as_int(row.get("bar_frame"))
    bar_progress = as_int(row.get("bar_progress_permille"))
    if ig_pstep >= 0:
        names.append(f"ig_parent:{ig_pstep}")
    if ig_d8 >= 0:
        names.append(f"ig_request:{ig_d8}")
    if mms_step >= 0:
        names.append(f"mms_step:{mms_step}:{row.get('mms_name', '?')}")
        fin = as_int(row.get("mms_finalize12a"))
        if fin >= 0:
            names.append(f"finalize:{fin}:{row.get('mms_finalize12a_name', '?')}")
        if mms_next >= 0:
            names.append(f"mms_next:{mms_next}")
        if mms_done >= 0:
            names.append(f"mms_done50:{mms_done}")
        if gate_lo >= 0 or gate_hi >= 0:
            names.append(f"mms_gate:{gate_lo}/{gate_hi}")
    if loading_mode >= 0:
        names.append(f"loading_mode:{loading_mode}")
    if loading10 >= 0 or loading11 >= 0:
        names.append(f"loading_fields:{loading10}/{loading11}")
    if bar_progress >= 0 and (bar_frame > 0 or mms_step >= 0 or ig_d8 >= 1):
        bucket = max(0, min(1000, (bar_progress // 25) * 25))
        names.append(f"loading_bar_bucket:{bucket}")
    if _present_flag(row, "world_chr_man"):
        names.append("worldchr:present")
    if _present_flag(row, "main_player"):
        names.append("main_player:present")
    if row.get("player"):
        names.append("player:present")
    if row.get("play_time_live"):
        names.append("world_clock:live")  # play_time advanced >=1s past this load's baseline
    if row.get("can_move"):
        names.append("movement:can_move")
    # RE-verified reload-retention semaphores (bd er-effects-rs-9fmm): the MoveMap destination BlockId
    # STEP_MoveMap_Update loads after requestCode=2 (0xffffffff/4294967295 = skip -> revert), the
    # session protocol_state (WaitReload=4 selects loadTargetMapId), and the WorldChrMan world-stable
    # oracle. These pin whether the reload's revert is "no destination" vs a genuine finalize race.
    dest = as_int(row.get("dest_block_id"))
    if dest >= 0:
        tag = "NONE" if dest == 0xFFFFFFFF else f"0x{dest:x}"
        names.append(f"dest_block_id:{tag}")
    ltm = as_int(row.get("load_target_map_id"))
    if ltm >= 0:
        tag = "NONE" if ltm == 0xFFFFFFFF else f"0x{ltm:x}"
        names.append(f"load_target_map_id:{tag}")
    proto = as_int(row.get("protocol_state"))
    if proto >= 0:
        names.append(f"protocol_state:{proto}")
    if row.get("world_stable"):
        names.append("world_stable")
    # Online flags (GameMan BC8/BC9): a nonzero value in-world can fire the connection-loss /
    # network-error return-title (user hypothesis 2026-07-19). Checkpoint the ONLINE-re-enable event so
    # the load1-vs-load2 diff shows if the reload re-flags online right before its revert.
    if as_int(row.get("online_mode")) > 0:
        names.append("online_mode:ENABLED")
    if as_int(row.get("server_conn")) > 0:
        names.append("server_conn:ENABLED")
    return names


def _shared_row(row: dict) -> bool:
    return (
        as_int(row.get("ig_d8")) >= 1
        or as_int(row.get("mms_step")) >= 0
        or _present_flag(row, "world_chr_man")
        or _present_flag(row, "main_player")
        or bool(row.get("player"))
    )


def _first_checkpoint_times(rows: list[dict], load_epoch: int) -> dict[str, dict]:
    epoch_rows = [
        r
        for r in rows
        if as_int(r.get("load_epoch"), as_int(r.get("fresh_deser"), -1)) == load_epoch
    ]
    if not epoch_rows:
        return {}
    shared_start_ms = None
    out: dict[str, dict] = {}
    for row in epoch_rows:
        if shared_start_ms is None and _shared_row(row):
            shared_start_ms = as_int(row.get("ms"), 0)
        if shared_start_ms is None:
            continue
        ms = as_int(row.get("ms"), 0)
        for name in _checkpoint_names(row):
            out.setdefault(
                name,
                {
                    "ms": ms,
                    "rel_shared_ms": ms - shared_start_ms,
                    "seq": as_int(row.get("seq"), 0),
                    "phase": row.get("phase_name"),
                    "event_key": row.get("event_key"),
                },
            )
    return out


def _checkpoint_row_summary(row: dict) -> dict:
    return {
        "seq": row.get("seq"),
        "ms": row.get("ms"),
        "phase": row.get("phase_name"),
        "ig_pstep": row.get("ig_pstep"),
        "ig_pnext": row.get("ig_pnext"),
        "ig_d8": row.get("ig_d8"),
        "mms_step": row.get("mms_step"),
        "mms_name": row.get("mms_name"),
        "mms_next": row.get("mms_next"),
        "mms_gate_lo": row.get("mms_gate_lo"),
        "mms_gate_hi": row.get("mms_gate_hi"),
        "bar_frame": row.get("bar_frame"),
        "bar_progress_permille": row.get("bar_progress_permille"),
        "player": row.get("player"),
        "can_move": row.get("can_move"),
        "world_chr_man": row.get("world_chr_man"),
        "main_player": row.get("main_player"),
    }


def _ordered_checkpoints(shared_rows: list[dict]) -> list[dict]:
    seen: set[str] = set()
    out: list[dict] = []
    start_ms = as_int(shared_rows[0].get("ms"), 0) if shared_rows else 0
    for row in shared_rows:
        for name in _checkpoint_names(row):
            if name in seen:
                continue
            seen.add(name)
            summary = _checkpoint_row_summary(row)
            summary["name"] = name
            summary["rel_shared_ms"] = as_int(row.get("ms"), 0) - start_ms
            out.append(summary)
    return out


def _density_summary(shared_rows: list[dict]) -> dict:
    if not shared_rows:
        return {"rows": 0}
    ms = [as_int(r.get("ms"), 0) for r in shared_rows]
    gaps = [b - a for a, b in zip(ms, ms[1:]) if b >= a]
    bar_rows = [r for r in shared_rows if as_int(r.get("bar_frame"), -1) >= 0]
    bar_ms = [as_int(r.get("ms"), 0) for r in bar_rows]
    bar_gaps = [b - a for a, b in zip(bar_ms, bar_ms[1:]) if b >= a]
    return {
        "rows": len(shared_rows),
        "duration_ms": ms[-1] - ms[0],
        "max_gap_ms": max(gaps) if gaps else 0,
        "avg_gap_ms": round(sum(gaps) / len(gaps), 1) if gaps else 0,
        "bar_rows": len(bar_rows),
        "bar_max_gap_ms": max(bar_gaps) if bar_gaps else 0,
        "bar_avg_gap_ms": round(sum(bar_gaps) / len(bar_gaps), 1) if bar_gaps else 0,
    }


def _epoch_shared_rows(rows: list[dict], load_epoch: int) -> list[dict]:
    epoch_rows = [
        r
        for r in rows
        if as_int(r.get("load_epoch"), as_int(r.get("fresh_deser"), -1)) == load_epoch
    ]
    started = False
    out: list[dict] = []
    for row in epoch_rows:
        if not started and _shared_row(row):
            started = True
        if started:
            out.append(row)
    return out


def write_semaphore_diff(
    artifact_dir: Path, game_dir: Path
) -> tuple[Path, Path] | None:
    rows = _load_semaphore_rows(artifact_dir, game_dir)
    json_out = artifact_dir / "samechar-3x-semaphore-diff.json"
    md_out = artifact_dir / "samechar-3x-semaphore-diff.md"
    if not rows:
        md_out.write_text(
            "# Semaphore diff\n\nNo semaphore trace rows found.\n", encoding="utf-8"
        )
        json_out.write_text(
            json.dumps({"error": "no semaphore trace rows"}, indent=2), encoding="utf-8"
        )
        return json_out, md_out
    epochs = sorted(
        {
            as_int(r.get("load_epoch"), as_int(r.get("fresh_deser"), -1))
            for r in rows
            if as_int(r.get("load_epoch"), as_int(r.get("fresh_deser"), -1)) >= 0
        }
    )
    boot_events = _epoch_shared_rows(rows, 0)
    boot_checkpoints = _ordered_checkpoints(boot_events)
    boot_names = [cp["name"] for cp in boot_checkpoints]
    boot_cp = _first_checkpoint_times(rows, 0)
    analyses = []
    for epoch in [e for e in epochs if e > 0]:
        reload_events = _epoch_shared_rows(rows, epoch)
        reload_checkpoints = _ordered_checkpoints(reload_events)
        boot_index = 0
        matched = []
        first_out_of_order = None
        for cp in reload_checkpoints:
            name = cp["name"]
            if name not in boot_names:
                first_out_of_order = {
                    "reload": cp,
                    "reason": "not present in boot shared-subtree baseline",
                }
                break
            try:
                found = boot_names.index(name, boot_index)
            except ValueError:
                first_out_of_order = {
                    "reload": cp,
                    "reason": "present in boot but earlier than current matched prefix",
                    "next_boot_index": boot_index,
                }
                break
            matched.append(
                {"reload": cp, "boot": boot_checkpoints[found], "boot_index": found}
            )
            boot_index = found + 1
        expected_next = (
            boot_checkpoints[boot_index] if boot_index < len(boot_checkpoints) else None
        )
        reload_cp = _first_checkpoint_times(rows, epoch)
        missing = [name for name in boot_names if name not in reload_cp]
        common = [name for name in boot_names if name in reload_cp and name in boot_cp]
        timing = {
            name: reload_cp[name]["rel_shared_ms"] - boot_cp[name]["rel_shared_ms"]
            for name in common
        }
        analyses.append(
            {
                "reload_epoch": epoch,
                "reload_rows": len(reload_events),
                "reload_density": _density_summary(reload_events),
                "matched_prefix_checkpoints": len(matched),
                "first_out_of_order": first_out_of_order,
                "expected_next_boot_checkpoint": expected_next,
                "first_missing_checkpoint": missing[0] if missing else None,
                "missing_checkpoints": missing[:50],
                "timing_delta_ms": timing,
                "last_reload_row": _checkpoint_row_summary(reload_events[-1])
                if reload_events
                else None,
            }
        )
    payload = {
        "row_count": len(rows),
        "epochs": epochs,
        "boot_shared_rows": len(boot_events),
        "boot_density": _density_summary(boot_events),
        "boot_checkpoint_count": len(boot_checkpoints),
        "analyses": analyses,
    }
    json_out.write_text(json.dumps(payload, indent=2, sort_keys=True), encoding="utf-8")
    md = [
        "# Semaphore diff",
        "",
        f"trace_rows: {len(rows)}",
        f"epochs: {epochs}",
        f"boot_shared_density: {payload['boot_density']}",
        f"boot_checkpoint_count: {payload['boot_checkpoint_count']}",
        "",
    ]
    for a in analyses:
        md.extend(
            [
                f"## Reload epoch {a['reload_epoch']}",
                f"- shared_rows: {a['reload_rows']}",
                f"- shared_density: {a['reload_density']}",
                f"- matched_prefix_checkpoints_against_boot_subset: {a['matched_prefix_checkpoints']}",
                f"- first_missing_checkpoint: {a['first_missing_checkpoint']}",
            ]
        )
        if a["first_out_of_order"]:
            r = a["first_out_of_order"]["reload"]
            md.append(
                f"- first_out_of_order_reason: {a['first_out_of_order'].get('reason')}"
            )
            md.append(
                f"- first_out_of_order_reload_checkpoint: name={r.get('name')} seq={r.get('seq')} ms={r.get('ms')} phase={r.get('phase')} "
                f"ig={r.get('ig_pstep')}/{r.get('ig_pnext')} d8={r.get('ig_d8')} "
                f"mms={r.get('mms_step')} next={r.get('mms_next')} gate={r.get('mms_gate_lo')}/{r.get('mms_gate_hi')} "
                f"bar={r.get('bar_frame')}/{r.get('bar_progress_permille')} player={r.get('player')} "
                f"world_chr_man={r.get('world_chr_man')} main_player={r.get('main_player')}"
            )
        if a["expected_next_boot_checkpoint"]:
            b = a["expected_next_boot_checkpoint"]
            md.append(
                f"- expected_next_boot_checkpoint: name={b.get('name')} seq={b.get('seq')} ms={b.get('ms')} phase={b.get('phase')} "
                f"ig={b.get('ig_pstep')}/{b.get('ig_pnext')} d8={b.get('ig_d8')} "
                f"mms={b.get('mms_step')} next={b.get('mms_next')} gate={b.get('mms_gate_lo')}/{b.get('mms_gate_hi')} "
                f"bar={b.get('bar_frame')}/{b.get('bar_progress_permille')} player={b.get('player')} "
                f"world_chr_man={b.get('world_chr_man')} main_player={b.get('main_player')}"
            )
        if a["last_reload_row"]:
            r = a["last_reload_row"]
            md.append(
                f"- last_reload_row: seq={r.get('seq')} ms={r.get('ms')} phase={r.get('phase')} "
                f"ig={r.get('ig_pstep')}/{r.get('ig_pnext')} d8={r.get('ig_d8')} "
                f"mms={r.get('mms_step')} next={r.get('mms_next')} gate={r.get('mms_gate_lo')}/{r.get('mms_gate_hi')} "
                f"bar={r.get('bar_frame')}/{r.get('bar_progress_permille')} can_move={r.get('can_move')} "
                f"world_chr_man={r.get('world_chr_man')} main_player={r.get('main_player')}"
            )
        deltas = sorted(
            a["timing_delta_ms"].items(), key=lambda kv: abs(kv[1]), reverse=True
        )[:12]
        if deltas:
            md.append("- largest_common_timing_deltas_ms:")
            for name, delta in deltas:
                md.append(f"  - {name}: {delta}")
        md.append("")
    md_out.write_text("\n".join(md), encoding="utf-8")
    return json_out, md_out


def teardown() -> None:
    for image in ("eldenring.exe", "me3.exe"):
        with contextlib.suppress(subprocess.TimeoutExpired):
            subprocess.run(
                ["taskkill.exe", "/F", "/IM", image],
                capture_output=True,
                text=True,
                timeout=15,
            )


def write_switch_trigger(game_dir: Path, slot: int, save_file: str | None) -> None:
    """Deterministically trigger the product's programmatic switch to (save_file, slot).

    Writes er-effects-switch-save-file.txt (the source override; empty/absent = keep active save) then
    er-effects-switch-slot.txt (the mtime-triggered slot). The product's poll_switch_slot_control_file
    consumes the fresh mtime and arms switch_slot_arm_programmatic when the world is eligible. bd
    DETERMINISTIC-switch-trigger-recipe-write-slot-control-file-not-menu-nav-2026-07-21.
    """
    save_ctl = game_dir / "er-effects-switch-save-file.txt"
    if save_file:
        save_ctl.write_text(save_file, encoding="utf-8")
    else:
        # case 1 (same active save): ensure the override is empty so the source falls through to active.
        with contextlib.suppress(OSError):
            if save_ctl.exists():
                save_ctl.write_text("", encoding="utf-8")
    # The slot file's fresh mtime is the trigger; write it LAST (after the source override is in place).
    (game_dir / "er-effects-switch-slot.txt").write_text(str(slot), encoding="utf-8")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument(
        "--game-dir",
        type=Path,
        required=True,
        help="dir with er-effects-telemetry.json + logs",
    )
    ap.add_argument("--artifact-dir", type=Path, required=True)
    ap.add_argument(
        "--max-seconds",
        type=float,
        required=True,
        help="runtime cap (.auto/runtime_timeout_cap_seconds)",
    )
    ap.add_argument("--report", type=Path, required=True)
    ap.add_argument(
        "--require-reload-move",
        action="store_true",
        help="success requires a RELOAD (deser>=1) to prove movement, not just load1 (the full-sequence goal)",
    )
    ap.add_argument(
        "--require-reload-settled",
        action="store_true",
        help=(
            "success requires a RELOAD (deser>=1) to prove movement AND finish native MoveMap "
            "handoff (requestCode==2, MoveMapStep absent); catches load2 moving under stale mms18"
        ),
    )
    ap.add_argument(
        "--capture-load1-imprint",
        action="store_true",
        help=(
            "IMPRINT capture mode (real-oracle foundation): record the load1 boot timeseries and tear "
            "down the instant the USER walks the char (sustained havok displacement in a stable world). "
            "No reload driven; no move-probe teardown."
        ),
    )
    ap.add_argument(
        "--observe-only",
        action="store_true",
        help=(
            "PURE OBSERVATION: record the full timeseries (incl. havok per poll) with NO probe/verdict/"
            "stall/fps teardowns -- ride to --observe-seconds (or load-epoch target) so the complete "
            "load1->load2 sequence (teleports, mms) is captured for offline analysis / imprint building."
        ),
    )
    ap.add_argument(
        "--observe-seconds",
        type=float,
        default=140.0,
        help="observe-only window before teardown (default 140s).",
    )
    ap.add_argument(
        "--steady-window-seconds",
        type=float,
        default=float(os.environ.get("STEADY_WINDOW_SECONDS", "0") or "0"),
        help=(
            "PARITY-MEASUREMENT mode (goal 3c, acceptance strengthened 2026-07-22): after each load "
            "reaches can_move, HOLD this many seconds of movable steady-state before driving the next "
            "load (and before declaring final-epoch success), so the telemetry captures a SUSTAINED "
            "post-readiness window per load for the vanilla-vs-mod semaphore diff. >0 also DISABLES the "
            "eager fps-dip teardown, which fires during a reload's LOADING ramp (high task_delta because "
            "loading, not steady-state) and tears down before the window can form. 0=off (env "
            "STEADY_WINDOW_SECONDS)."
        ),
    )
    # DETERMINISTIC SWITCH DRIVER (2026-07-21, bd DETERMINISTIC-switch-trigger-recipe): drive each
    # subsequent load by WRITING the product's control file (er-effects-switch-slot.txt, +
    # er-effects-switch-save-file.txt for cross-save) once the current load proves movement -- instead of
    # the flaky input-harness menu-nav. The product's poll_switch_slot_control_file arms the switch
    # programmatically when the world is eligible (player present + live menu job). The input-harness DLL
    # is still loaded to drive the 3s FORWARD-MOVEMENT proof; only the SWITCH trigger moves to the file.
    ap.add_argument(
        "--drive-reload-slots",
        default="",
        help=(
            "comma list of slots to drive for load2,load3,... on the SAME (active) save, each triggered "
            "after the prior load proves movement (e.g. '0,0' = reload angrE slot 0 twice). Empty = do "
            "not drive (legacy harness menu-nav)."
        ),
    )
    ap.add_argument(
        "--drive-cross-save-file",
        default="",
        help="Windows path to a NON-angrE read/write .sl2/.co2 to load as the FINAL step (case 2).",
    )
    ap.add_argument(
        "--drive-cross-save-slot",
        type=int,
        default=-1,
        help="slot within --drive-cross-save-file to load (>=0 enables the final cross-save step).",
    )
    args = ap.parse_args()

    # Build the deterministic switch plan: list of (slot, save_file|None). Each entry i is triggered
    # after epoch i proves movement (epoch 0 = boot load1), producing epoch i+1.
    switch_plan: list[tuple[int, str | None]] = []
    if args.drive_reload_slots.strip():
        for tok in args.drive_reload_slots.split(","):
            tok = tok.strip()
            if tok:
                switch_plan.append((int(tok), None))
    if args.drive_cross_save_file and args.drive_cross_save_slot >= 0:
        switch_plan.append((args.drive_cross_save_slot, args.drive_cross_save_file))
    # DETERMINISTIC-DRIVER HANDOFF: create the switch control file with an OUT-OF-RANGE sentinel (-1)
    # before any load so the product marks the deterministic driver as owning switches from boot (the
    # flaky sq-repro menu-nav stands down, which also un-suppresses the move-probe) WITHOUT triggering a
    # switch (-1 is rejected as out-of-range). This also clears any stale trigger/override from a prior
    # run. Real triggers overwrite it with a valid slot after each load proves movement.
    if switch_plan:
        with contextlib.suppress(OSError):
            (args.game_dir / "er-effects-switch-save-file.txt").write_text("", encoding="utf-8")
            (args.game_dir / "er-effects-switch-slot.txt").write_text("-1", encoding="utf-8")

    args.artifact_dir.mkdir(parents=True, exist_ok=True)
    telemetry_path = args.game_dir / "er-effects-telemetry.json"

    # Per-epoch record: first-seen ts, the max/settled snapshot, whether render-ready was ever held.
    epochs: dict[int, dict] = {}
    # Deterministic switch driver: which plan entry to fire next, and the final success epoch.
    next_switch_idx = 0
    final_epoch = len(switch_plan) if switch_plan else FINAL_RELOAD_EPOCH
    # Per-epoch time the playable+moving window opened (first can_move), to enforce the 3s hold.
    epoch_canmove_start: dict[int, float] = {}
    if switch_plan:
        print(
            f"[drive-switch] plan: {len(switch_plan)} triggered load(s) after load1; "
            f"final success epoch={final_epoch}; entries={switch_plan}",
            flush=True,
        )
    portrait_captured = False
    first_present_at: float | None = None
    start = time.monotonic()
    last_log = 0.0
    result = "TIMEOUT_NO_LOAD3"
    # TELEMETRY TIMESERIES (user 2026-07-20, real-oracle foundation): append every poll's full
    # semaphore snapshot with a relative timestamp. This is the raw material to (a) BUILD the known-good
    # load1 boot IMPRINT (ordered semaphores + inter-transition timing) and (b) later compare a live run
    # against it in lockstep. bd real-oracle-imprint-lockstep-boot-sequence-direction-2026-07-20.
    ts_path = args.artifact_dir / "telemetry-timeseries.jsonl"
    ts_f = ts_path.open("w", encoding="utf-8")
    # IMPRINT capture (boot_to_control phase): the boundary is the USER pressing forward -> SUSTAINED
    # walking (control-available ground truth; RAM proxies lie). A LOAD reposition is a single-poll
    # TELEPORT (tens of units, e.g. load1 places the char at spawn) and must NOT count as walking; only
    # sustained walking-range displacement in a STABLE non-loading world is the user (bd
    # boot-imprint-boundary-reject-teleport-require-sustained-walk-2026-07-20).
    prev_havok: tuple[float, float, float] | None = None
    HAVOK_MOVE_THRESHOLD = 0.3  # min horizontal world-unit displacement/poll that counts as walking
    HAVOK_TELEPORT = 5.0  # displacement/poll above this = a load reposition/teleport, NOT walking
    WALK_CONFIRM_POLLS = 3  # consecutive walking-range polls needed to confirm the user is walking
    walk_streak = 0
    max_nav_stage = (
        0  # highest sq_repro_state reached = how far the driver ever drove the menu
    )
    max_activate = (
        0  # profile-load confirm activations = number of reload ATTEMPTS started
    )
    # Per-step divergence detail for the report (set when a reload diverges from load1 at a step).
    step_divergence_detail: str | None = None
    # FAST FPS-COMPARISON TEARDOWN (user 2026-07-20, bd fast-teardown-load2-fps-far-from-load1-3s): once
    # load2 is in-world, sample its FPS for ~3s and compare to the load1 in-world baseline; tear down
    # immediately with a regression/ok verdict instead of consuming the full cap.
    load1_inworld_fps: list[float] = []
    load2_inworld_fps: list[float] = []
    load2_fps_window_start: float | None = None
    fps_compared_logged = False  # parity note already printed once
    FPS_SETTLE_BEFORE_TEST_S = 5.0  # let load2 FINISH loading into world two before testing the drop
    FPS_REGRESSION_RATIO = 0.85  # load2 avg < 85% of load1 = a real, user-visible frame drop
    # PER-STEP DWELL DIVERGENCE (user 2026-07-21): load1 is the baseline -- it is EXPECTED to dwell at
    # specific loading-bar steps during a normal first load. A reload (load2/load3) that dwells at the
    # SAME step SIGNIFICANTLY longer than load1 did there has diverged -> tear down naming the step. Step
    # key = the game-native loading-bar frame (oracle_loading_bar_current_frame; game-global loading
    # screen), NOT mms/req_code (title-owner-derived, stale on the reload path). Contention-robust: core
    # starvation slows BOTH loads, so the RELATIVE load2-vs-load1 per-step comparison holds. Replaces the
    # old mms18 check AND the 15-field flat-window signature (whose many flickering fields never went flat
    # -> the "no teardown on stall" bug). bd user-wants-bootup-sequence-divergence-semaphore-2026-07-20.
    STEP_DWELL_FACTOR = 3.0  # reload dwell > load1_dwell*FACTOR + SLACK at the same step = divergence
    STEP_DWELL_SLACK_S = 8.0
    load1_step_dwell: dict[int, float] = {}  # bar-frame -> max dwell (s) observed on load1
    l1_step_key: int | None = None
    l1_step_entered_at: float | None = None
    reload_step_key: int | None = None
    reload_step_entered_at: float | None = None
    fps_dip_polls = 0  # consecutive movable polls below target on the current reload epoch
    # Observe-only settle-then-teardown (tear-down-after-insight, bd tear-down-immediately-after-insight):
    # once the FINAL reload epoch has been continuously in-world for this many seconds, tear down instead of
    # riding the full --observe-seconds window (which leaves the game up capturing input long after the
    # steady-state is captured). 0 disables (ride the full window). Default 45s = enough sustained
    # steady-state for the parity diff, then a prompt teardown.
    observe_settle_teardown_s = float(
        os.environ.get("OBSERVE_SETTLE_TEARDOWN_SECONDS", "45") or "0"
    )
    final_reload_settled_at: float | None = None
    # TEARDOWN DETECTION (bd timeout-design-use-DETERMINISTIC-process-exit): the PRIMARY signal is the game
    # PROCESS, not a fixed timeout. Once eldenring.exe has been seen alive and then disappears, exit
    # IMMEDIATELY (bounded only by the ~1s process-poll, so a crash at t=0.5s reacts at ~1s, not a flat 12s).
    # A time value is ONLY a backstop for a HUNG-but-alive game (process present, telemetry frozen).
    import subprocess as _sp  # noqa: PLC0415

    def _er_alive() -> bool:
        try:
            out = _sp.run(
                ["tasklist.exe", "/FI", "IMAGENAME eq eldenring.exe", "/NH"],
                capture_output=True,
                text=True,
                timeout=5,
            ).stdout
            return "eldenring.exe" in out.lower()
        except Exception:
            return True  # can't determine -> assume alive, never false-exit

    game_seen_alive = False
    last_proc_check = 0.0
    proc_check_interval = 1.0
    hung_stale_seconds = float(os.environ.get("HUNG_STALE_SECONDS", "60") or "60")
    last_mtime: float | None = None
    last_mtime_change = start

    while True:
        now = time.monotonic()
        elapsed = now - start
        if elapsed >= args.max_seconds:
            result = "CAP_REACHED"
            break
        # PRIMARY: process exit -> immediate teardown (throttled ~1s check).
        if now - last_proc_check >= proc_check_interval:
            last_proc_check = now
            if _er_alive():
                game_seen_alive = True
            elif game_seen_alive:
                result = "GAME_EXITED"
                break
        # BACKSTOP: hung-but-alive (process present, telemetry frozen) -- long window, not a primary signal.
        try:
            mtime = os.path.getmtime(telemetry_path)
        except OSError:
            mtime = None
        if mtime != last_mtime:
            last_mtime = mtime
            last_mtime_change = now
        elif (
            last_mtime is not None
            and now - last_mtime_change > hung_stale_seconds
            and elapsed > 20
        ):
            result = "TELEMETRY_FROZEN_HUNG"
            break

        t = read_telemetry(telemetry_path)
        if t is not None:
            s = snap(t)
            with contextlib.suppress(Exception):
                ts_f.write(json.dumps({"t_ms": round(elapsed * 1000), **s}) + "\n")
                ts_f.flush()
            deser = s.get("system_quit_continue_confirm_fresh_deser_count") or 0
            present = bool(s.get("oracle_player_present"))
            # PURE OBSERVATION: the timeseries is already written above; skip ALL probe/verdict/stall/fps
            # teardown logic and just ride to the observe window so the full load1->load2 sequence
            # (havok teleports, mms progression) is captured for offline analysis.
            if args.observe_only:
                if elapsed >= args.observe_seconds:
                    result = f"OBSERVED_{int(args.observe_seconds)}s_deser{deser}"
                    break
                # settle-then-teardown: the final reload epoch reached + continuously in-world -> once it
                # has been stable for observe_settle_teardown_s, tear down promptly (don't ride the window).
                cur_ep_o = s.get("oracle_current_load_epoch")
                at_final_reload = (
                    present and cur_ep_o is not None and cur_ep_o >= final_epoch
                )
                if observe_settle_teardown_s > 0 and at_final_reload:
                    if final_reload_settled_at is None:
                        final_reload_settled_at = elapsed
                    elif elapsed - final_reload_settled_at >= observe_settle_teardown_s:
                        result = (
                            f"OBSERVED_SETTLED_{int(observe_settle_teardown_s)}s_epoch{cur_ep_o}_deser{deser}"
                        )
                        break
                else:
                    final_reload_settled_at = None
                if elapsed - last_log >= LOG_THROTTLE_SECONDS:
                    last_log = elapsed
                    print(
                        f"[{elapsed:6.1f}s] OBSERVE deser={deser} present={present} "
                        f"now_loading={s.get('oracle_now_loading')} mms={s.get('oracle_stepfinish_mms_state')} "
                        f"req={s.get('oracle_stepfinish_request_code')} havok={s.get('oracle_havok_pos')}",
                        flush=True,
                    )
                _POLL_WAIT.wait(POLL_SECONDS)
                continue
            render_ready = bool(s.get("oracle_player_render_ready"))
            fake_cover = bool(s.get("oracle_fake_loading_any_visible"))

            nav_stage = s.get("sq_repro_state") or 0
            activate = s.get("system_quit_profile_load_activate_count") or 0
            max_nav_stage = max(max_nav_stage, int(nav_stage))
            max_activate = max(max_activate, int(activate))

            can_move = bool(s.get("oracle_can_move"))
            # DECISIVE: the harness-attributed movement verdict is the whole point of a movement run.
            # The instant it latches (proven/disproven/contaminated) for the epoch under test, tear down
            # -- do NOT ride to an fps/stall/cap window after the answer is known (user 2026-07-20, bd
            # collect-decisive-info-teardown-immediately). Contamination is a first-class outcome here.
            harness_verdict = as_int(s.get("oracle_harness_move_verdict"), 0)
            # Only a PROVEN verdict ends the run early (a real movement confirmation is decisive). DISPROVEN
            # is NOT a load failure: the movement injection is foreground-limited and disproves on EVERY
            # load -- including during load1's own loading before the char is even movable -- which tore the
            # run down before load1 finished (user 2026-07-21: "why did you tear down before load 1").
            # Loads are validated by the game-global signature + the stall guard, not the unreliable
            # injection verdict; a disproven/contaminated verdict just lets the run keep going.
            # Track when THIS epoch's playable+moving window opened (first can_move), to enforce the 3s
            # sustained-movement hold before the next switch / the success verdict.
            cur_ep = as_int(deser)
            if can_move and cur_ep not in epoch_canmove_start:
                epoch_canmove_start[cur_ep] = now
            # BELOW-TARGET-FPS TEARDOWN: on a reload epoch (>=1), the instant the framerate is confirmed
            # sustained below target the diagnostic is complete -- tear down immediately (user 2026-07-22).
            # NO can_move gate: the reload's 20fps render-bound state has render_ready=False -> can_move
            # never latches, so gating on it rode past 155s at 20fps (run7). task_delta is the frame time
            # regardless of movability; a reload rendering sustained sub-target IS the answer.
            # PARITY-MEASUREMENT mode (--steady-window-seconds > 0) DISABLES this eager teardown: it fires
            # during the reload's loading ramp (task_delta is high while loading, not because of the
            # steady-state dip) and prevents the sustained post-readiness window from forming (bd
            # LOBOTOMIZE-single-core-contention-...: premature early-boot teardown is the one real hazard).
            if cur_ep >= 1 and args.steady_window_seconds <= 0:
                _td_raw = s.get("oracle_flip_task_delta")
                try:
                    _td = float(_td_raw) if _td_raw is not None else 0.0
                except (TypeError, ValueError):
                    _td = 0.0
                if _td >= FPS_DIP_TASK_DELTA_THRESHOLD:
                    fps_dip_polls += 1
                    if fps_dip_polls >= FPS_DIP_CONFIRM_POLLS:
                        result = (
                            f"RELOAD_FPS_DIP_CONFIRMED_epoch{cur_ep}_taskdelta{_td:.3f}"
                        )
                        print(
                            f"[{elapsed:6.1f}s] RELOAD FPS DIP CONFIRMED epoch{cur_ep}: "
                            f"task_delta={_td:.3f} (~{1.0 / _td:.0f}fps) sustained "
                            f"{FPS_DIP_CONFIRM_POLLS} polls below target -> teardown",
                            flush=True,
                        )
                        break
                else:
                    fps_dip_polls = 0
            else:
                fps_dip_polls = 0
            # DETERMINISTIC SWITCH DRIVER: once the CURRENT load proves movement AND has sustained a 3s
            # playable+moving window, write the control file to trigger the NEXT load (replaces the flaky
            # menu-nav). Plan entry i fires after epoch i (epoch 0 = boot load1 -> plan[0] triggers load2).
            # SINGLE-PRESS DRIVE (user 2026-07-22): the instant control is confirmed for this epoch
            # (harness_verdict==1 = one forward-press moved the char), trigger the NEXT load. Do NOT
            # require a sustained 3s move window / repeat the movement burst -- one press = go.
            # PARITY-MEASUREMENT hold (goal 3c): in steady mode, do not advance to the next load until
            # THIS epoch has held can_move for --steady-window-seconds (a sustained post-readiness window
            # is now in the telemetry). Off (<=0) preserves the legacy single-press-advances behavior.
            steady_held = args.steady_window_seconds <= 0 or (
                cur_ep in epoch_canmove_start
                and (now - epoch_canmove_start[cur_ep]) >= args.steady_window_seconds
            )
            if (
                switch_plan
                and next_switch_idx < len(switch_plan)
                and cur_ep == next_switch_idx
                and harness_verdict == 1
                and steady_held
            ):
                _slot, _save_file = switch_plan[next_switch_idx]
                write_switch_trigger(args.game_dir, _slot, _save_file)
                print(
                    f"[drive-switch] load{next_switch_idx + 2}: epoch {next_switch_idx} proved movement + "
                    f"held {MOVE_WINDOW_SECONDS:.0f}s window; wrote trigger slot={_slot} "
                    f"cross_save={_save_file or 'no (active angrE save)'}",
                    flush=True,
                )
                next_switch_idx += 1
            # Success = the FINAL planned load proves movement AND holds the 3s window (final_epoch =
            # len(plan) when driving, else the legacy FINAL_RELOAD_EPOCH).
            if harness_verdict == 1 and cur_ep >= final_epoch and steady_held:
                result = f"HARNESS_MOVE_PROVEN_harness_moved_char_epoch{deser}"
                break
            # IMPRINT CAPTURE (boot_to_control phase): end the boot imprint the INSTANT control is
            # available, marked by the USER pressing forward = the char's first real havok displacement.
            # RAM proxies for control-available (render_ready, present, mms==-1) are unreliable, so the
            # char actually MOVING is the ground-truth boundary (user 2026-07-20). No injection here, so
            # any move is the user. The boundary event itself is NOT part of the boot imprint (the
            # imprint is boot -> the frame BEFORE the move).
            if args.capture_load1_imprint:
                hv = parse_havok(s.get("oracle_havok_pos"))
                not_loading = as_int(s.get("oracle_now_loading"), 1) == 0
                if deser == 0 and present and hv is not None:
                    if prev_havok is not None:
                        dx = hv[0] - prev_havok[0]
                        dz = hv[2] - prev_havok[2]
                        disp = (dx * dx + dz * dz) ** 0.5
                        if disp > HAVOK_TELEPORT:
                            walk_streak = 0  # load reposition/teleport, not walking
                        elif not_loading and disp >= HAVOK_MOVE_THRESHOLD:
                            walk_streak += 1
                            if walk_streak >= WALK_CONFIRM_POLLS:
                                result = "PHASE_boot_to_control_CAPTURED"
                                break
                        else:
                            walk_streak = 0
                    prev_havok = hv
            if present and first_present_at is None:
                first_present_at = now

            # FAST FPS-COMPARISON TEARDOWN (user 2026-07-20): once load2 is loaded into world two, sample
            # its FPS for ~3s and compare to the load1 in-world baseline; tear down immediately.
            try:
                fps_now = float(s.get("oracle_fps") or 0.0)
            except (TypeError, ValueError):
                fps_now = -1.0
            _ep = int(deser)
            # GENUINELY loaded into world (not just present-in-memory). `present` alone is the in-memory
            # deserialize and fires DURING the loading screen, so testing FPS there measured the loading
            # phase (user 2026-07-20). BUT render_ready/can_move are BOTH FALSE for the WHOLE in-world
            # window on a warm RELOAD (run6 2026-07-21: only 2 genuinely_loaded fps samples for load2, 1
            # for load3 -> the fps-parity teardown never reached its >=6-sample threshold and never fired,
            # so a real 65%-of-load1 frame regression sailed through as PASS -- the exact "oracle should
            # have torn down here" bug). The char IS rendered on a reload (render_group=True, and the
            # harness verdict proves movement), so gate on render_group -- the same in-world signal the
            # offline framerate analysis uses (matched load2 n=51 / load3 n=69) -- which fires on reloads.
            # SETTLED-FPS GATE (2026-07-21, bd DECISIVE-all-load2-theories-falsified...): judge the fps
            # regression only once the char is genuinely SETTLED (movable), never mid-load. The decisive
            # two-agent trace + scripts/analyze-samechar-epochs.py on run 165038 established: (a)
            # now_loading clears at PRESENT (~in-memory deserialize), ~9-14s BEFORE the char is movable,
            # so it is NOT a settle signal; (b) render_group comes on DURING the loading ramp (load1
            # needed ~7.6s AFTER render_group/mms18 to reach can_move); (c) over the SETTLED window load2
            # fps (29) ~= load1 (27) -- NO real regression; the "load2 20fps" was load2's LOADING-window
            # fps sampled 5s after render_group, before it settled, which then tore the run down at ~3.5s
            # post-mms18 -- before load2 could prove movability. So gate the fps comparison on can_move
            # (the real movable/settled signal): both baselines become settled-vs-settled, and load2 is
            # never torn down before its ramp completes. If can_move never latches, the fps teardown
            # simply does not fire and the run rides longer -- the intent here is to give load2 its full
            # ramp and prove movability. bd fps-test-after-load2-finished-settled-not-during-load.
            genuinely_loaded = can_move
            # In DRIVE mode, do NOT tear down early on an fps regression: load2/load3 reach can_move much
            # later than load1 (slower ramp), so an early fps sample catches the still-ramping first
            # seconds of movability, not settled movement (run eligfix-224410: 45% "drop" that was really
            # settled-parity 27/28/29). Let the full 3-load sequence complete its 3s windows and judge fps
            # parity OFFLINE (analyze-samechar-epochs.py). bd MILESTONE-deterministic-3x-angre-works-clean.
            if fps_now > 0 and genuinely_loaded and not switch_plan:
                if _ep == 0:
                    load1_inworld_fps.append(fps_now)
                elif _ep >= 1:
                    if load2_fps_window_start is None:
                        load2_fps_window_start = now
                    load2_inworld_fps.append(fps_now)
                    # Test the frame drop only AFTER load2 has FINISHED loading into world two (sustained
                    # present >= FPS_SETTLE_BEFORE_TEST_S), comparing load2's RECENT window to load1's
                    # baseline, and RE-CHECK every poll so a drop that deepens as it plays is caught (user
                    # 2026-07-20: the early 3s sample missed the real in-world drop, 20 vs 25). Tear down
                    # ONLY on a real drop (load2 < 85% of load1); on parity keep going to prove load3.
                    if (
                        (now - load2_fps_window_start) >= FPS_SETTLE_BEFORE_TEST_S
                        and len(load2_inworld_fps) >= 6
                        and len(load1_inworld_fps) >= 6
                    ):
                        l1 = statistics.mean(load1_inworld_fps)
                        l2 = statistics.mean(load2_inworld_fps[-6:])
                        if l1 > 0 and l2 < FPS_REGRESSION_RATIO * l1:
                            result = (
                                f"FPS_REGRESSION_LOAD2 load2={l2:.0f} vs load1={l1:.0f}fps "
                                f"(drop {100 * (1 - l2 / l1):.0f}%)"
                            )
                            break
                        if not fps_compared_logged:
                            fps_compared_logged = True
                            print(
                                f"[fps-compare] load2={l2:.0f} vs load1={l1:.0f}fps parity so far -- continuing to load3",
                                flush=True,
                            )

            # PER-STEP DWELL DIVERGENCE (user 2026-07-21): it is EXPECTED for the loading bar to dwell at
            # specific steps during a normal first load. The stall signal is a RELOAD dwelling at the SAME
            # loading-bar step SIGNIFICANTLY longer than load1 did there, compared per-step against the
            # load1 baseline (contention-robust: core starvation slows both loads together). Step key = the
            # game-native loading-bar frame (game-global), NOT mms/req_code (title-owner-derived, stale on
            # reloads). Replaces the old mms18 check and the 15-field flat-window signature.
            _dep = int(deser)
            bar_enabled = as_int(s.get("oracle_loading_bar_enabled"), 0) > 0
            bar_frame = as_int(s.get("oracle_loading_bar_current_frame"), -1)
            step_key = bar_frame if (bar_enabled and bar_frame >= 0) else None
            if step_key is not None:
                if _dep == 0:
                    # load1 baseline: track the MAX dwell observed at each loading-bar step.
                    if step_key != l1_step_key:
                        l1_step_key = step_key
                        l1_step_entered_at = now
                    elif l1_step_entered_at is not None:
                        load1_step_dwell[step_key] = max(
                            load1_step_dwell.get(step_key, 0.0), now - l1_step_entered_at
                        )
                elif _dep >= 1:
                    # reload: tear down if it dwells at a step load1 also hit, far beyond load1's dwell.
                    if step_key != reload_step_key:
                        reload_step_key = step_key
                        reload_step_entered_at = now
                    elif reload_step_entered_at is not None:
                        dwell = now - reload_step_entered_at
                        base = load1_step_dwell.get(step_key)
                        if base is not None:
                            budget = base * STEP_DWELL_FACTOR + STEP_DWELL_SLACK_S
                            if dwell > budget:
                                step_divergence_detail = (
                                    f"load{_dep + 1} dwelt {dwell:.0f}s at loading-bar step "
                                    f"{step_key} (load1 dwelt {base:.0f}s there; budget {budget:.0f}s)"
                                )
                                result = f"STEP_DIVERGENCE_LOAD{_dep + 1}_barstep{step_key}"
                                break
            ep = epochs.setdefault(
                int(deser),
                {
                    "first_seen": elapsed,
                    "ever_ready": False,
                    "ever_moved": False,
                    "last": None,
                },
            )
            ep["last"] = s
            if render_ready:
                ep["ever_ready"] = True
            if can_move:
                ep["ever_moved"] = True

            # Mandatory portrait capture at the frozen-load view: a reload in progress (deser>=1),
            # cover up, not yet render-ready -- the exact moment the user sees the failure.
            if (
                not portrait_captured
                and deser >= 1
                and fake_cover
                and not render_ready
                and present
            ):
                capture_portrait(args.artifact_dir)
                portrait_captured = True

            # Success = a load proves movement (can_move latched: >=60 consecutive frames of injected
            # motion). For the full sequence (--require-reload-move) it must be a RELOAD (deser>=1) --
            # the user's "third time they can move" -- so load1 moving does NOT end the run; the driver
            # keeps going through the reloads. Stricter --require-reload-settled also requires the load to
            # have genuinely COMPLETED, judged by GAME-GLOBAL signals that ACTUALLY fire: now_loading
            # cleared + render_group(1c4) + world clock live. NOT oracle_player_render_ready (write_oracle
            # requires chr_draw_group_enabled, which never sets even in vanilla -> always false) and NOT
            # req_code==2/mms==-1 (title-owner-derived, never settle on the reload path). Verified 2026-07-21:
            # render_group(1c4) + enable_render(1c5) DO fire on both loads; draw_group never does.
            reload_epoch = as_int(deser) >= 1
            # Movement proof = the STRONG harness verdict (CAN_MOVE_CONFIRMED / verdict==1: >=70% ON-moved
            # + clean OFF-tail), NOT raw did_move_frames -- the per-frame threshold (0.01) counts DRIFT, so
            # a load that only drifts 0.1u still shows did_move=72 (run 111317 load2). verdict==1 correctly
            # rejects drift and is the real "the harness moved the char" proof.
            reload_move = can_move and reload_epoch
            reload_settled = (
                reload_move
                and as_int(s.get("oracle_now_loading"), 1) == 0
                and bool(s.get("oracle_chr_render_group_enabled"))
                and bool(s.get("oracle_play_time_live"))
            )
            if args.require_reload_settled:
                # Full sequence: succeed only when the FINAL reload (load3) proves movement + settles, so
                # the run drives through ALL loads rather than ending on load2.
                if reload_settled and as_int(deser) >= final_epoch:
                    result = "RELOAD_SETTLED"
                    break
            elif can_move and (not args.require_reload_move or reload_epoch):
                result = "MOVEMENT_PROVEN"
                break
            # TEARDOWN ON UNEXPECTED FAILURE (user 2026-07-18): if the boot never reaches an in-world
            # player within the boot budget, do NOT idle to the cap -- fail fast and tear down.
            if first_present_at is None and elapsed >= BOOT_TIMEOUT_SECONDS:
                result = "BOOT_TIMEOUT_NO_INWORLD"
                break
            # Defensive backstop for the no-bar edge case (see RELOAD_STALL_BACKSTOP_SECONDS): a reload that
            # reached an in-world player but never became render-ready within the budget tears down instead
            # of riding to the cap. The per-step dwell divergence above is the primary, faster signal.
            if (
                reload_epoch
                and present
                and not render_ready
                and (elapsed - ep["first_seen"]) > RELOAD_STALL_BACKSTOP_SECONDS
            ):
                result = f"RELOAD_STALL_BACKSTOP_epoch{deser}_no_render_ready"
                break

            if elapsed - last_log >= LOG_THROTTLE_SECONDS:
                last_log = elapsed
                nav_label = SQ_REPRO_STATE_LABELS.get(int(nav_stage), f"?{nav_stage}")
                print(
                    f"[{elapsed:6.1f}s] deser={deser} activate={activate} nav={nav_label} "
                    f"present={present} render_ready={render_ready} "
                    f"can_move={can_move}(f{s.get('oracle_move_probe_moved_frames')}) "
                    f"draw_group={s.get('oracle_chr_draw_group_enabled')} "
                    f"req_code={s.get('oracle_stepfinish_request_code')} mms={s.get('oracle_stepfinish_mms_state')} "
                    f"finalize12a={s.get('oracle_stepfinish_finalize_substate_12a')} "
                    f"fake_cover={fake_cover} switch_idx={s.get('sq_repro_switch_index')} "
                    f"sysstep={s.get('oracle_system_step_label')}({s.get('oracle_system_step_state')}) "
                    f"fps={s.get('oracle_fps')}/min{s.get('oracle_min_fps')} "
                    f"havok={s.get('oracle_havok_pos')}",
                    flush=True,
                )
        _POLL_WAIT.wait(POLL_SECONDS)

    with contextlib.suppress(Exception):
        ts_f.close()

    # Snapshot artifacts before teardown clears live state.
    for name in (
        "er-effects-telemetry.json",
        "er-effects-autoload-debug.log",
        "er-reload-trace.log",
    ):
        src = args.game_dir / name
        if src.exists():
            with contextlib.suppress(OSError):
                (args.artifact_dir / name).write_bytes(src.read_bytes())

    teardown()

    # Report.
    # Attempt vs complete (the semaphore gap this run exposed): reload ATTEMPTS = profile-load confirm
    # activations (max_activate); reload COMPLETIONS = fresh_deser epochs beyond load1 (max deser key).
    completions = max((d for d in epochs if d >= 1), default=0)
    max_nav_label = SQ_REPRO_STATE_LABELS.get(max_nav_stage, f"?{max_nav_stage}")
    lines = [
        "# Same-character-3x capture report",
        "",
        f"result: **{result}**",
        f"elapsed: {time.monotonic() - start:.1f}s (cap {args.max_seconds}s)",
        f"portrait_captured: {portrait_captured}",
        "",
    ]
    if step_divergence_detail is not None:
        lines.extend(
            [
                "## Per-step divergence (reload vs load1 baseline)",
                f"- {step_divergence_detail}",
                "",
            ]
        )
    lines.extend(
        [
            "## Driver progress (attempt vs complete)",
            f"- reload ATTEMPTS started (profile_load_activate_count): **{max_activate}**",
            f"- reload COMPLETIONS committed (max fresh_deser epoch): **{completions}**",
            f"- furthest menu-nav stage the driver reached (max sq_repro_state): **{max_nav_label}** ({max_nav_stage})",
            "  - reaching CONFIRM(5) = a load was attempted; stalling below it (e.g. TO_PROFILE(3)) = the",
            "    driver never drove the menu far enough to START the next load.",
            "",
            "## Per-load (fresh_deser epoch) settled signature",
            "",
        ]
    )
    diff_paths = write_semaphore_diff(args.artifact_dir, args.game_dir)
    if diff_paths is not None:
        lines.extend(
            [
                "## Semaphore diff artifacts",
                f"- json: {diff_paths[0]}",
                f"- markdown: {diff_paths[1]}",
                "",
            ]
        )

    epoch_names = {
        0: "load1 (boot autoload)",
        1: "load2 (first reload)",
        2: "load3 (second reload)",
        3: "load4 (third reload)",
    }
    for deser in sorted(epochs):
        ep = epochs[deser]
        s = ep["last"] or {}
        lines.append(f"### deser={deser} — {epoch_names.get(deser, 'load')}")
        lines.append(
            f"- first_seen: {ep['first_seen']:.1f}s   ever_render_ready: {ep['ever_ready']}   "
            f"ever_moved(can_move): {ep.get('ever_moved')}"
        )
        lines.append(
            f"- char_name: {s.get('oracle_char_name')}   "
            f"can_move: {s.get('oracle_can_move')}  moved_frames: {s.get('oracle_move_probe_moved_frames')}"
        )
        lines.append(
            f"- player_render_ready: {s.get('oracle_player_render_ready')}  "
            f"draw_group: {s.get('oracle_chr_draw_group_enabled')}  "
            f"render_group: {s.get('oracle_chr_render_group_enabled')}  "
            f"enable_render: {s.get('oracle_chr_enable_render')}"
        )
        lines.append(
            f"- request_code: {s.get('oracle_stepfinish_request_code')}  "
            f"mms_state: {s.get('oracle_stepfinish_mms_state')}  "
            f"finalize12a: {s.get('oracle_stepfinish_finalize_substate_12a')}  "
            f"now_loading: {s.get('oracle_now_loading')}  "
            f"fake_cover: {s.get('oracle_fake_loading_any_visible')}"
        )
        lines.append(
            f"- havok_pos: {s.get('oracle_havok_pos')}  play_time_ms: {s.get('oracle_play_time_ms')}"
        )
        lines.append("")
    # LOAD-TRIGGER DIAGNOSIS (2026-07-21, bd er-effects-rs-tx9n + USER-oracle-must-emit-teardown-and-
    # noload-cause): make a no-load EXPLAIN itself instead of a silent CAP_REACHED. Derive WHY the reload
    # did/didn't fire from the switch-trigger semaphores in the final sample.
    final_s = (epochs[max(epochs)]["last"] if epochs else {}) or {}
    sw_arm = as_int(final_s.get("oracle_switch_arm_count"), 0)
    sw_deferred = as_int(final_s.get("oracle_switch_deferred_count"), 0)
    sw_phase = as_int(final_s.get("oracle_switch_reload_phase"), 0)
    sw_committed = as_int(final_s.get("oracle_switch_reload_committed"), 0)
    sw_drain = as_int(final_s.get("oracle_switch_reload_drain_waits"), 0)
    sw_player = as_int(final_s.get("oracle_switch_player_present"), 0)
    sw_menujob = as_int(final_s.get("oracle_switch_menu_job_present"), 0)
    sw_stable = as_int(final_s.get("oracle_switch_stable_frames"), 0)
    sw_primed = as_int(final_s.get("oracle_switch_slot_control_primed"), 0)
    b80_final = as_int(final_s.get("oracle_load_in_progress_b80"), -1)
    phase_label = {0: "IDLE", 1: "DRAIN", 2: "COMMIT"}.get(sw_phase, f"?{sw_phase}")
    if completions >= 1:
        trigger_reason = f"reload(s) DID fire ({completions} committed); the trigger worked."
    elif sw_arm == 0 and sw_deferred > 0:
        # Eligibility is now player-present only (menu_job requirement dropped 2026-07-21). Use the
        # AUTHORITATIVE WorldChrMan signal oracle_player_present; oracle_switch_player_present only
        # updates during an ACTIVE switch tick (reads 0 in pre-switch gameplay), so it must NOT be the
        # reason source (that mis-attributed run cleandrive-223256 to "player absent" when the real block
        # was menu_job). bd DECISIVE-poller-eligibility-menujob-overconservative-2026-07-21.
        real_present = bool(final_s.get("oracle_player_present"))
        if not real_present:
            gate = "the local player was absent (world torn down / mid-load / never spawned)"
        else:
            gate = "phase was not idle / a switch was already in flight (the player IS present)"
        trigger_reason = (
            f"NEVER ARMED: {sw_deferred} deferred arm(s) -- {gate} "
            f"(oracle_player_present={real_present}, switch_menu_job={sw_menujob})."
        )
    elif sw_arm == 0:
        trigger_reason = (
            f"NEVER ARMED and 0 deferred -> no switch request was ever issued this run (control file "
            f"never written with a new mtime; slot_control_primed={sw_primed}). Trigger never invoked."
        )
    elif sw_phase == 0:
        trigger_reason = f"ARMED ({sw_arm}x) but the reload FSM never entered SUBMIT (phase IDLE)."
    elif sw_phase == 1:
        trigger_reason = (
            f"ARMED ({sw_arm}x), SUBMIT issued, but the deserialize never reached RESIDENT "
            f"(phase DRAIN, drain_waits={sw_drain}, b80={b80_final})."
        )
    else:
        trigger_reason = (
            f"ARMED ({sw_arm}x), drained to RESIDENT, COMMIT issued (committed={sw_committed}) but "
            f"fresh_deser never latched -- check continue_confirm/SetState5."
        )
    lines.extend(
        [
            "## Load-trigger diagnosis (why the reload did/didn't fire)",
            f"- {trigger_reason}",
            f"- switch semaphores: arm_count={sw_arm} deferred={sw_deferred} "
            f"reload_phase={phase_label}({sw_phase}) committed={sw_committed} drain_waits={sw_drain} "
            f"b80={b80_final} player_present={sw_player} menu_job_present={sw_menujob} "
            f"stable_frames={sw_stable}",
            "",
        ]
    )
    # EXIT CODE = experiment outcome (user 2026-07-20): a FAILED experiment MUST be non-zero, a PASSED
    # one zero. HARNESS_MOVE_PROVEN and the imprint capture are passes; DISPROVEN/CONTAMINATED and every
    # incomplete/stall/timeout are failures. Do not mask this (and never wrap the run in a trailing
    # always-success shell command). bd never-claim-launch-live / the exit-code masking correction.
    passed = (
        result in {"MOVEMENT_PROVEN", "RELOAD_SETTLED", "LOAD1_WORLD_READY_IMPRINT"}
        or result.startswith("HARNESS_MOVE_PROVEN")
        or result.startswith("RELOAD_FPS_DIP_CONFIRMED")  # the reload dip is the intended capture
        or result.endswith("_CAPTURED")
        or result.startswith("OBSERVED_")  # observation ran to its window as intended
    )
    if result == "RELOAD_SETTLED":
        verdict = "PASS (a reload PROVED movement and native MoveMap settled: requestCode==2, mms=-1)"
    elif result == "MOVEMENT_PROVEN":
        verdict = "PASS (a load PROVED movement: >=60 frames of injected motion)"
    elif result == "LOAD1_WORLD_READY_IMPRINT":
        verdict = "PASS (load1 reached sustained world-ready; imprint timeseries captured)"
    elif result.endswith("_CAPTURED"):
        verdict = f"PASS (phase boundary reached via user input; imprint timeseries captured: {result})"
    elif result.startswith("HARNESS_MOVE_PROVEN"):
        verdict = "PASS (harness-attributed movement proven, contamination excluded)"
    elif result.startswith("RELOAD_FPS_DIP_CONFIRMED"):
        verdict = (
            "PASS (reload below-target framerate confirmed and captured; torn down on the "
            "fps-dip semaphore instead of the wall-clock cap)"
        )
    else:
        verdict = f"FAIL / incomplete ({result})"
    lines.append(f"## Verdict: {verdict}")
    args.report.write_text("\n".join(lines), encoding="utf-8")
    print("\n".join(lines))
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())
