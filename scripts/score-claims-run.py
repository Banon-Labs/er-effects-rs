#!/usr/bin/env python3
"""Score one product runtime run's artifact directory against the oracles a claim set needs.

Why this exists. A settlement run is judged per claim, not in aggregate -- "the run passed" settles
nothing. That judgement needs three things read together, and reading them by hand invites reading
only the flattering one:

  1. What the run actually produced. An artifact that is absent is not a zero. A telemetry JSON that
     never appeared would make every "oracle read 0" below a statement about a file that is not
     there, so the file listing is printed first.
  2. The DLL'S own refusal lines. After a game-version bump a stale address is a `HOOK REFUSED` /
     `ADDRESS REFUSED` log line plus a silently-zeroed counter, not a crash. A counter sitting at 0
     beside a refusal for the address that feeds it is a refused hook, not a disproven feature --
     so refusals are printed before the oracle values, never after.
  3. The named ORACLES, grouped by the claim group that needs them, printed as `<ABSENT>` when the
     key is not in the JSON at all. "Absent" and "0" are different verdicts, and this repo has
     already been burned by a field a crate published but telemetry never emitted.

The `er-quickload-autoload-debug.log` is default-OFF: `append_autoload_debug` returns before any I/O
unless the marker file `er-quickload-autoload-debug.txt` exists in the game directory. A run staged
without it produces an empty log, and every log-line oracle below is silence rather than evidence --
which is why a missing log is called out explicitly instead of counting as "zero refusals".

Usage:
    python3 scripts/score-claims-run.py <artifact-dir>

Read-only: it opens the run's artifacts and prints. It never writes, never launches, never kills.
"""

import collections
import json
import os
import re
import sys

AD = sys.argv[1] if len(sys.argv) > 1 else "."


def p(n):
    return os.path.join(AD, n)


print("=== ARTIFACTS PRESENT ===")
for f in sorted(os.listdir(AD)):
    fp = p(f)
    print(f"  {os.path.getsize(fp):>12,}  {f}")

log = p("er-quickload-autoload-debug.log")
print("\n=== REFUSALS / CONFIG / DRIVE (from the DLL's own log) ===")
if os.path.exists(log) and os.path.getsize(log) > 0:
    c = collections.Counter()
    samples = collections.defaultdict(list)
    pats = {
        "HOOK REFUSED": re.compile(r"HOOK REFUSED"),
        "ADDRESS REFUSED": re.compile(r"ADDRESS REFUSED"),
        "failed to resolve": re.compile(r"failed to resolve"),
        "MessageBoxDialog": re.compile(r"MessageBoxDialog"),
        "save-override verdict": re.compile(r"save-override: (DEFAULT-USER-SAVE|ENFORCED|no usable)"),
        "runtime-config: loaded": re.compile(r"runtime-config: loaded"),
        "configured autoload slot": re.compile(r"configured autoload slot"),
        "loadgame-builder": re.compile(r"loadgame-builder"),
        "switch-trigger": re.compile(r"switch-trigger"),
        "sq-repro": re.compile(r"sq-repro:"),
    }
    with open(log, encoding="utf-8", errors="replace") as f:
        for line in f:
            for k, r in pats.items():
                if r.search(line):
                    c[k] += 1
                    if len(samples[k]) < 4:
                        samples[k].append(line.rstrip()[:240])
    for k in pats:
        print(f"  {k:28} {c[k]}")
        for s in samples[k][:3]:
            print(f"      {s}")
else:
    print("  !! NO AUTOLOAD DEBUG LOG. `append_autoload_debug` is gated on the game-directory")
    print("     marker `er-quickload-autoload-debug.txt`; without it every log-line oracle is")
    print("     SILENCE, not zero, and 'no refusals' cannot be claimed from this run.")

tel = p("er-quickload-telemetry.json")
print("\n=== ORACLES ===")
GROUPS = {
    "R1 identity / world-ready": [
        "oracle_char_name", "oracle_char_level", "oracle_player_present",
        "oracle_player_render_ready", "oracle_play_time_live", "oracle_play_time_ms",
        "oracle_play_time_advanced_ms", "oracle_saved_map_c30", "oracle_chr_draw_group_enabled",
        "profile_slot_active_bytes_qword", "oracle_privacy_policy_gate", "oracle_save_redirect_mode",
        "oracle_system_step_label", "oracle_system_step_state", "oracle_stepfinish_mms_state",
        "oracle_msgbox_total_builds", "oracle_blocking_modal_present",
        "simulated_button_presses_total", "oracle_menu_item_update_hits", "game_task_ticks",
        "oracle_boot_view_milestone_mask", "oracle_total_world_loads", "oracle_current_load_index",
        "oracle_load_count_mismatches",
    ],
    "R2 switch / reload": [
        "system_quit_continue_confirm_fresh_deser_count",
        "system_quit_continue_confirm_fresh_deser_done",
        "oracle_current_load_epoch", "oracle_can_move", "oracle_supplied_movement_input_frames",
        "oracle_harness_move_verdict", "oracle_worldreswait_hold_engaged",
        "oracle_worldreswait_released_on_settle", "oracle_worldreswait_gate_calls",
        "oracle_common_finalize_count", "oracle_outgoing_teardown_done",
        "oracle_stepfinish_finalize_substate_12a", "oracle_switch_menu_job_present",
        "oracle_menu_window_finalize_guards", "oracle_ownership_ledger_violations",
        "oracle_profile_spare_orphans_deleted", "oracle_ownership_spared_outstanding",
        "system_quit_return_till_final_functor_call_count",
        "system_quit_return_title_final_functor_call_count",
        "oracle_switch_reload_drain_waits", "oracle_msgbox_builds_since_switch_arm",
        "oracle_profileselect_table_repairs", "system_quit_profile_load_activate_count",
    ],
    "R3 loading cover / portrait": [
        "oracle_boot_view_semantic_releases", "oracle_boot_view_stop_reason",
        "oracle_boot_view_backstop_releases", "oracle_boot_view_release_before_confirm",
        "oracle_boot_view_release_held_for_confirm", "oracle_native_ls_exposure_frames",
        "oracle_native_ls_covered_frames", "oracle_ls_portrait_w", "oracle_ls_portrait_h",
        "oracle_ls_portrait_slot", "oracle_ls_portrait_rejected_publishes",
        "oracle_portrait_display_frames_last_window", "oracle_portrait_publish_clean",
        "oracle_portrait_published_slot_mismatches", "oracle_loading_bg_portrait_gx_nonblack",
        "oracle_loading_bg_portrait_is_checker", "oracle_loading_bg_portrait_rgba_version",
        "oracle_depth_key_fresh", "oracle_depth_key_applied", "oracle_portrait_mask_stale_reuse",
        "oracle_portrait_render_drive_hits", "oracle_present_hook_hits",
        "oracle_portrait_target_kicks", "oracle_portrait_coherent_read_ok",
        "oracle_portrait_depth_from_chain", "oracle_portrait_depth_from_bfs",
        "oracle_boot_view_nonfade_draw_during_fade", "oracle_profile_cam_apply_calls",
        "oracle_gx_cmdqueue_reserves",
    ],
    "R4 ProfileSelect / System>Quit UI": [
        "oracle_stats_text_slot_cache_state", "oracle_stats_text_slot_decoded",
        "oracle_stats_text_settext_subs", "oracle_profile_player_name_push_attempts",
        "oracle_profile_player_name_push_failures", "oracle_profile_05_010_runtime_edit_armed",
        "oracle_profile_05_010_runtime_edit_serves", "oracle_profile_05_010_runtime_edit_failures",
        "oracle_system_quit_grid_cols", "oracle_system_quit_grid_rows",
        "oracle_system_quit_navigable_cells", "oracle_system_quit_item_count",
        "oracle_system_quit_row_refused_disagreement_count",
        "system_quit_load_build_url_action_count", "system_quit_load_build_url_editor_open_count",
    ],
    "R5 fps / frame timing": [
        "oracle_fps", "oracle_frame_ms", "oracle_flip_task_delta", "oracle_flip_fixed_spf",
        "oracle_present_qpc_delta_us", "oracle_present_sync_interval",
        "oracle_present_refresh_per_present_x100", "oracle_gpu_frame_us",
        "oracle_gpu_frame_samples", "oracle_gpu_frame_state",
    ],
}
if os.path.exists(tel):
    d = json.load(open(tel, encoding="utf-8", errors="replace"))
    for g, keys in GROUPS.items():
        print(f"\n-- {g}")
        for k in keys:
            print(f"   {k:52} = {json.dumps(d.get(k, '<ABSENT>'))[:110]}")
    print(f"\n  (telemetry carries {len(d)} keys total)")
else:
    print("  (no telemetry json -- nothing to score)")

ts = p("telemetry-timeseries.jsonl")
if os.path.exists(ts):
    rows = [json.loads(line) for line in open(ts) if line.strip()]
    print(f"\n=== AGENT-SIDE TIMESERIES: {len(rows)} samples ===")
    ep = collections.Counter(r.get("oracle_current_load_epoch") for r in rows)
    print("  samples per load epoch:", dict(ep))

rep = p("report.json")
if os.path.exists(rep):
    # The deleted `capture-samechar-3x.py --report` wrote MARKDOWN despite the `.json` name callers pass.
    # Print whatever it is rather than crashing on a JSON parse -- the report is the run's verdict
    # and a scorer that dies on it is worse than useless.
    print("\n=== WATCHER REPORT ===")
    raw = open(rep, encoding="utf-8", errors="replace").read()
    try:
        print(json.dumps(json.loads(raw), indent=1)[:4000])
    except json.JSONDecodeError:
        print(raw[:4000])
