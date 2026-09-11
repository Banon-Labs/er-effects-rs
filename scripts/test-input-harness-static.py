#!/usr/bin/env python3
"""Static regressions for the input-harness in-world menu drive path."""

from __future__ import annotations

import re
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
READINESS_WATCH = REPO_ROOT / "scripts/er-readiness-watch.py"


def test_pad_inject_stamps_the_pad_device_not_the_padmaps_cs_ingame_pad() -> None:
    """The virtual-key array is a field of `FD4::FD4PadDevice`, reached through padDevices.

    This assertion used to pin the defect, twice over. Until 2026-08-31 it required the padMaps
    TypeID tree-walk -- `CS_INGAME_PAD_TYPEID_RVAS`, `for target in targets`, and a write through
    the `CS::CSInGamePad_UserInput1` the walk returned. That object does not own the array. The
    only function that writes it (1.16.2 0x1426634a0, `mov byte [rcx+rdx*2+0x88],1`, bound
    `cmp eax,0x50` on id-1000) has exactly four call sites -- 0x140240e70, 0x140241130,
    0x140e321b0, 0x140e32470 -- and every one of them computes `rcx` as
    `*(manager + 0x18 + dev*8)`, i.e. `FD4PadManager::padDevices[dev]`. `FD4PadManager::Init`
    fills that array with `HeapAlloc(0x3c0)` + `FD4PadDevice::FD4PadDevice` +
    `FD4PadDevice::vftable`; the CSInGamePad merely holds the device at its own +0x10.

    The tree-walk was not merely useless. The CSInGamePad is `HeapAlloc(0x98)` = 152 bytes, so
    `0x88 + (id-1000)*2` leaves the object at id 1008 and every id above wrote past the end of a
    live game allocation. It never fired only because the TypeID needles are `.data` RVAs with no
    1.17 mapping, so the search matched nothing -- a reason it was never observed, not a reason it
    was safe.
    """
    src = (REPO_ROOT / "crates/er-input-harness/src/pad_inject.rs").read_text()
    assert "const PAD_MGR_DEVICES_18_OFFSET: usize = 0x18;" in src
    assert "const PAD_DEVICES_COUNT_40_OFFSET: usize = 0x40;" in src
    assert "const VK_ARRAY_88_OFFSET: usize = 0x88;" in src
    assert "rd(manager + PAD_MGR_DEVICES_18_OFFSET + dev * 8)" in src, (
        "the direct stamp no longer resolves the device the way the game's own writer does"
    )
    # The bound the game itself checks `dev` against, so a garbage count cannot walk off the struct.
    assert "rd(manager + PAD_DEVICES_COUNT_40_OFFSET)" in src
    # ...and the disproved route must not come back. These are the exact tokens the tree-walk needed.
    for gone in (
        "CS_INGAME_PAD_TYPEID_RVAS",
        "0x3d5df27",
        "for target in targets",
        "PADMAPS_88_OFFSET",
    ):
        assert gone not in src, (
            f"the padMaps CSInGamePad tree-walk is back ({gone}). It writes the wrong object and "
            "overruns it above id 1007; if new evidence says otherwise, rewrite this gate against "
            "that evidence rather than deleting it"
        )


def test_pad_inject_says_so_when_it_cannot_resolve_a_device() -> None:
    """A drive that resolves nothing must say so, exactly once.

    This carries forward the concern of a sibling assertion that this file lost when the padMaps
    tree-walk went away (it required the TypeID needles to be resolved rather than `base + rva`,
    because every `.data` global moved on 1.17 and an unresolved needle matched no node). That
    specific check is unsatisfiable now -- there are no needles -- but the failure it was written
    for is not about needles: the drive went inert for six weeks with no fault, no refusal line and
    no counter moving, and silence is indistinguishable from "the game ignored the input".

    `game_data_addr` still answers 0 for a refusal, and 0 still fails the read; what changed is
    that the refusal now reaches the log instead of an early `return`.
    """
    src = (REPO_ROOT / "crates/er-input-harness/src/pad_inject.rs").read_text()
    assert 'game_data_addr(\n        base,\n        FD4_PAD_MANAGER_RVA,\n        "FD4_PAD_MANAGER_RVA",\n    )' in src, (
        "the manager global must be RESOLVED for the running build, not `base + rva`"
    )
    assert 'report_inert("GLOBAL_FD4PadManager did not resolve or read back")' in src
    assert 'report_inert("no usable padDevices entry under the manager")' in src
    assert "INERT_LOGGED\n        .compare_exchange(0, 1" in src, (
        "the inert line must be one-shot; a per-frame repeat is what gets logging deleted again"
    )


def test_pad_inject_records_why_the_owner_is_fd4paddevice() -> None:
    """The RE that settled the owner must stay written down, not just applied.

    A future pass that reads only the code sees `manager + 0x18` and no reason to prefer it over
    the accessor at +0x48; that is exactly how the 2026-07-23 "correction" happened.
    """
    prose = _prose((REPO_ROOT / "crates/er-input-harness/src/pad_inject.rs").read_text())
    assert "FD4::FD4PadDevice" in prose
    assert "0x1426634a0" in prose and "0x142665cb0" in prose
    assert "HeapAlloc(0x3c0)" in prose
    assert "HeapAlloc(0x98)" in prose


def test_pad_inject_direct_stamp_writes_are_enabled() -> None:
    src = (REPO_ROOT / "crates/er-input-harness/src/pad_inject.rs").read_text()
    assert "BISect: write disabled" not in src
    assert "BISECT: write disabled" not in src
    assert "crate::win32::write_u8(cached + off, val)" in src
    assert "crate::win32::write_u8(device + off, val)" in src


def _prose(src: str) -> str:
    """Comment prose with markers stripped and whitespace collapsed.

    The assertions below are about what the file says, not how it is wrapped. An earlier version
    matched a literal `"all ids\n/// 1000..1080 ..."` including the line break, so re-flowing the
    paragraph -- which is what happened when the retired raw-pad cluster was deleted 2026-08-21 and
    the finding moved onto `set_vk_id` -- broke the gate while the documented fact was intact and in
    fact more detailed. A gate that fails on rewrapping teaches people to rewrite the gate.
    """
    return re.sub(r"\s+", " ", re.sub(r"^\s*(///|//!|//)", " ", src, flags=re.M))


def test_pad_inject_id_map_todo_is_burned_down_without_speculative_ids() -> None:
    src = (REPO_ROOT / "crates/er-input-harness/src/pad_inject.rs").read_text()
    prose = _prose(src)
    assert "TO" + "DO(id-map)" not in src
    # The negative finding must stay written down: the full 1000..1080 sweep produced no response.
    # Without it, a later pass re-runs a sweep this repo has already paid for.
    assert "1000..1080" in prose
    assert "reproducible job/flags/tab/return-title response" in prose
    # ...and the swept range itself must still be the one the sentence claims was swept.
    assert "const VK_ID_MIN: u32 = 1000;" in src
    assert "const VK_ID_MAX: u32 = 1080;" in src
    # The old `PadButton::TabRight => 0` assertion is deliberately gone. It pinned one arm of a
    # `PadButton -> vk id` map whose every variant returned 0 -- a placeholder holding no recovered
    # id, deleted 2026-08-21. The concern it guarded (a speculative id being written in) cannot
    # recur through a map that no longer exists; what guards it now is the sentence above.
    # Match the type, not the word: the surviving doc comment explains that a typed `PadButton`
    # wrapper used to sit in front of this raw-id API and why it went. Forbidding the string would
    # forbid recording the removal, which is the opposite of what this repo wants kept.
    assert "enum PadButton" not in src and "PadButton::" not in src, (
        "A PadButton id map reappeared. If a real id->action map was recovered, rewrite this gate "
        "to assert the recovered ids against their evidence -- do not delete the check."
    )


def test_input_harness_manifest_names_actual_hook_layer() -> None:
    manifest = (REPO_ROOT / "crates/er-input-harness/Cargo.toml").read_text()
    assert "FD4PadDevice::poll" not in manifest
    assert "DLUID virtual-key builders/writer" in manifest
    assert "0x240e70/0x241130/0x26634a0" in manifest


def test_boot_autoload_mms18_can_force_stuck_testnet_step() -> None:
    hooks = (REPO_ROOT / "crates/er-quickload/src/experiments/startup_hooks/quit_menu/system_quit_hooks.rs").read_text()
    assert "let boot_epoch = epoch == 0;" in hooks
    assert "if boot_epoch {" in hooks
    assert "mms_state == MOVEMAPSTEP_STEP_MOVEMAP_INDEX" in hooks
    assert "fin == 0" in hooks
    assert "MOVEMAPSTEP_TESTNETSTEP_WRAPPER_108_OFFSET" in hooks
    assert "EZ_CHILD_STEP_REQUEST_FINISH_RVA" in hooks
    assert "request_finish(wrapper)" in hooks
    assert "ORACLE_RELIABLE_MMS_PTR.load(Ordering::SeqCst)" in hooks
    assert "SWITCH_ORACLE_REQUEST_CODE.load(Ordering::SeqCst)" in hooks
    assert "No reload committed yet -> this is load1; never force" not in hooks
    telemetry = (REPO_ROOT / "crates/er-quickload/src/telemetry/runtime_oracles/write_telemetry.rs").read_text()
    assert 'oracle_testnet_ff_stuck_frames' in telemetry
    assert 'oracle_testnet_ff_fired_epoch' in telemetry
    oracle = (REPO_ROOT / "crates/er-quickload/src/telemetry/runtime_oracles/write_oracle.rs").read_text()
    assert 'oracle_mms_next_step_4c' in oracle
    assert 'oracle_mms_done_flag_50' in oracle
    assert 'oracle_mms_advance_gate_lo_4b8' in oracle
    assert 'oracle_mms_advance_gate_hi_4b9' in oracle


def test_continue_and_boot_view_timing_oracles_exist() -> None:
    # The whole module, not just its root file. `counters` became a directory when the loading
    # cover's statics moved into `counters/loading_cover.rs`, and reading only `counters.rs` then
    # asserted a declaration was missing from a module that still declares it -- a red gate about
    # where a static lives rather than about whether it exists.
    counters_root = REPO_ROOT / "crates/er-telemetry-core/src/counters.rs"
    counters = counters_root.read_text() + "".join(
        child.read_text()
        for child in sorted((REPO_ROOT / "crates/er-telemetry-core/src/counters").glob("*.rs"))
    )
    assert "pub static BOOT_VIEW_PUMP_STOP_MS" in counters
    assert "pub static BOOT_VIEW_DARK_GAP_FAILURES" in counters
    assert "pub static BOOT_VIEW_PRESENT_FULL_CLEAR_HITS" in counters
    assert "pub static BOOT_VIEW_PRESENT_COVER_FAILURES" in counters
    assert "pub static BOOT_VIEW_PRE_WORLD_STOP_FAILURES" in counters
    assert "pub static BOOT_VIEW_TELEMETRY_HANDOFF_STAMPS" in counters
    assert "pub static BOOT_VIEW_NATIVE_GFX_FADE_HOLD_HITS" in counters
    assert "pub static BOOT_VIEW_NATIVE_GFX_FADE_HOLD_COMPLETE_MS" in counters
    assert "pub static LOADING_SCREEN_UPDATE_LAST_MS" in counters
    assert "pub static LOADING_SCREEN_GFX_FADEOUT_HOOK_INSTALLED" in counters
    assert "pub static LOADING_SCREEN_GFX_FADEOUT_HITS" in counters
    assert "pub static LOADING_SCREEN_GFX_FADEOUT_FIRST_MS" in counters
    assert "pub static LOADING_SCREEN_GFX_FADEOUT_LAST_MS" in counters
    assert "pub static LOADING_SCREEN_CLOSE_SENT_FIRST_MS" in counters
    assert "pub static OWN_LOAD_FORCED_CONTINUE_HANDOFF_MS" in counters
    assert "pub static TFC_FORCED_CONTINUE_HANDOFF_MS" in counters

    product_core = (REPO_ROOT / "crates/er-quickload/src/experiments/mod/product_core_own_stepper.rs").read_text()
    assert "pub(crate) fn mark_own_load_forced_continue_handoff()" in product_core
    assert "OWN_LOAD_FORCED_CONTINUE_HANDOFF_MS.compare_exchange" in product_core
    assert "pub(crate) fn mark_tfc_forced_continue_handoff()" in product_core
    assert "TFC_FORCED_CONTINUE_HANDOFF_MS.compare_exchange" in product_core

    present = (REPO_ROOT / "crates/er-quickload/src/experiments/present_overlay.rs").read_text()
    assert "fn set_boot_view_pump_stop_reason" in present
    assert "BOOT_VIEW_PUMP_STOP_MS.store" in present

    # The oracle emission is a 29-line spine plus per-subsystem include files (the split that
    # brought write_game_module_oracles.rs under the 3200-line limit on 2026-08-30). Read the
    # whole directory rather than one filename: what these assertions care about is that the
    # field is emitted somewhere in the emission, not which file it lives in. Pinning the
    # filename made a pure refactor look like a deleted oracle.
    game_oracles = "\n".join(
        sorted(
            p.read_text()
            for p in (REPO_ROOT / "crates/er-quickload/src/telemetry/runtime_oracles").rglob("*.rs")
        )
    )
    assert '"oracle_boot_view_pump_stop_ms"' in game_oracles
    assert '"oracle_boot_view_dark_gap_failures"' in game_oracles
    assert '"oracle_boot_view_missed_handoff_failures"' in game_oracles
    assert '"oracle_boot_view_telemetry_handoff_stamps"' in game_oracles
    assert '"oracle_boot_view_present_full_clear_hits"' in game_oracles
    assert '"oracle_boot_view_present_cover_failures"' in game_oracles
    assert '"oracle_boot_view_pre_world_stop_failures"' in game_oracles
    assert '"oracle_boot_view_native_gfx_fade_hold_hits"' in game_oracles
    assert '"oracle_boot_view_native_gfx_fade_hold_complete_ms"' in game_oracles
    assert '"oracle_loading_screen_gfx_fadeout_hook_installed"' in game_oracles
    assert '"oracle_loading_screen_gfx_fadeout_hits"' in game_oracles
    assert '"oracle_loading_screen_gfx_fadeout_first_ms"' in game_oracles
    assert '"oracle_loading_screen_gfx_fadeout_last_ms"' in game_oracles
    assert '"oracle_loading_screen_update_last_ms"' in game_oracles
    assert '"oracle_loading_screen_close_sent_first_ms"' in game_oracles
    assert "BOOT_VIEW_HANDOFF_SEEN_MS" in game_oracles
    assert ".compare_exchange(0, now_ms" in game_oracles
    readiness_watch = READINESS_WATCH.read_text()
    assert "BOOT_VIEW_DARK_GAP_FAILURE" in readiness_watch
    assert "telemetry_boot_view_dark_gap_failure_detected" in readiness_watch
    assert "oracle_boot_view_missed_handoff_failures" in readiness_watch
    assert "oracle_boot_view_present_cover_failures" in readiness_watch
    assert "boot_view_present_cover_failure" in readiness_watch
    assert "boot_view_pre_world_stop_failure" in readiness_watch
    boot_progress = (REPO_ROOT / "crates/er-quickload/src/experiments/gpu_readback/boot_progress.rs").read_text()
    present_overlay = (REPO_ROOT / "crates/er-quickload/src/experiments/present_overlay.rs").read_text()
    assert "BOOT_VIEW_EPOCH_WORLD_LIVE.load(Ordering::SeqCst) == cur" in present_overlay
    assert "cur != 0" in present_overlay
    assert "crate::experiments::is_native_windows() && idx >= 8" not in boot_progress
    assert "let can_move_handoff" in boot_progress
    assert "boot_view_cover_release_ready(can_move_handoff)" in boot_progress
    assert "boot_view_player_render_ready" in boot_progress
    assert "BOOT_VIEW_RELEASE_FADE_MS" in boot_progress
    assert "BOOT_VIEW_NATIVE_GFX_FADEOUT_HOLD_MS" in boot_progress
    assert "BOOT_VIEW_NATIVE_LOADING_QUIET_HOLD_MS" in boot_progress
    assert "LOADING_SCREEN_GFX_FADEOUT_LAST_MS.load" in boot_progress
    assert "LOADING_SCREEN_UPDATE_LAST_MS.load" in boot_progress
    assert "native_gfx_hold_pending" in boot_progress
    assert "holding opaque cover through native loading fade/quiet window" in boot_progress
    assert "composite_boot_release_fade_frame" in boot_progress
    assert "IN_WORLD_REACHED.load(Ordering::SeqCst) == IN_WORLD_REACHED_YES)" not in boot_progress
    assert "CAN_MOVE_CONFIRMED" in game_oracles
    assert "MOVE_PROBE_EPOCH" in game_oracles
    assert "is_render_group_enabled" in game_oracles
    telemetry = (REPO_ROOT / "crates/er-quickload/src/telemetry/runtime_oracles/write_telemetry.rs").read_text()
    assert 'oracle_own_load_forced_continue_handoff_ms' in telemetry
    assert 'oracle_tfc_forced_continue_handoff_ms' in telemetry


def main() -> int:
    tests = [
        test_pad_inject_stamps_the_pad_device_not_the_padmaps_cs_ingame_pad,
        test_pad_inject_records_why_the_owner_is_fd4paddevice,
        test_pad_inject_says_so_when_it_cannot_resolve_a_device,
        test_pad_inject_direct_stamp_writes_are_enabled,
        test_pad_inject_id_map_todo_is_burned_down_without_speculative_ids,
        test_input_harness_manifest_names_actual_hook_layer,
        test_boot_autoload_mms18_can_force_stuck_testnet_step,
        test_continue_and_boot_view_timing_oracles_exist,
    ]
    for test in tests:
        test()
    print("input harness static checks passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
