//! Destination for the ~900 telemetry atomic counters/latches being inverted
//! out of the product's `experiments/*` + `constants/*` trees.
//!
//! Ownership inversion (in progress): today these atomics are defined in the
//! product and telemetry merely mirrors them through `crate::*` glob imports.
//! The target state is that they are defined here (`pub` statics) and the
//! product write-sites reference `er_telemetry_core::counters::X`, so telemetry never
//! reaches up into product for state.
//!
//! This module currently holds only the counters that the standalone read-side
//! tick needs; the bulk migration (own_load / move_probe / rawinput / profile /
//! depth families) lands file-group by file-group per the plan's Step 3.
//!
//! A family that has grown coherent enough to name lives in its own file under `counters/` and is
//! re-exported here with a glob, so a consumer still spells every name
//! `er_telemetry_core::counters::<name>`. That is what keeps this file under the hard limit
//! `scripts/check-rust-file-sizes.py` enforces; put the next family in a new submodule rather than
//! at the bottom of this one.

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, AtomicUsize};

mod build_url_portrait;
mod cover_predicates;
mod input_injection;
mod loading_cover;
mod picked_save_source;
mod portrait_equipment;
/// The System>Quit panel's portrait fields, kept out of this file because it is already past the
/// hard limit `scripts/check-rust-file-sizes.py` enforces. The glob keeps every consumer's spelling.
mod quit_face_portrait;
mod save_flow;
mod save_picker;

pub use build_url_portrait::*;
pub use cover_predicates::{cover_owns_current_loading_screen, title_visual_suppression_active};
pub use input_injection::*;
pub use loading_cover::*;
pub use picked_save_source::*;
pub use portrait_equipment::*;
pub use quit_face_portrait::*;
pub use save_flow::*;
pub use save_picker::*;

/// Number of standalone read-side ticks that have executed (proves the game-thread
/// callback is live in the telemetry-only DLL). Owned here from the start.
pub static STANDALONE_TICKS: AtomicU64 = AtomicU64::new(0);

// ---- migrated group: present_overlay (23 counters) ----
pub static PRESENT_ORIG: AtomicUsize = AtomicUsize::new(0);
pub static PRESENT1_ORIG: AtomicUsize = AtomicUsize::new(0);
pub static PRESENT_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static PRESENT_HOOK_HITS: AtomicUsize = AtomicUsize::new(0);
/// Microseconds spent inside the last original IDXGISwapChain::Present/Present1 call (measured in the
/// present detour). Discriminates a present-block (compositor/vsync throttle => ~40ms) from a real
/// CPU/GPU per-frame work stall (present fast ~1-2ms but the frame is still 50ms). bd
/// focus-AB-falsifies-unfocused-throttle...next-present-duration-2026-07-21.
pub static PRESENT_CALL_LAST_US: AtomicUsize = AtomicUsize::new(0);
/// The `SyncInterval` argument the game passes to its own Present(this, SyncInterval, Flags) call,
/// latched in the present detour. Decisive for the reload 20fps: SyncInterval=3 => the game deliberately
/// requests present-every-3rd-vblank (a 20fps loading/low-priority throttle); =1 while frames are still
/// 3 vblanks apart => the game requests 60 but the GPU cannot keep up (render-bound). 0 = no-vsync.
/// bd GPU-timestamp-semaphore-split-reload-20fps-residual-2026-07-22.
pub static PRESENT_SYNC_INTERVAL_LAST: AtomicUsize = AtomicUsize::new(usize::MAX);
/// From IDXGISwapChain::GetFrameStatistics on the game swapchain: display-refreshes elapsed per present,
/// x100 (ratio ΔSyncRefreshCount/ΔPresentCount). ~300 (=3.00) on a 20fps flip-model reload means the
/// swapchain is vsync-locked to every 3rd vblank; ~100 (=1.00) means one present per vblank. 0 = no
/// stats yet / DISJOINT. Companion to PRESENT_SYNC_INTERVAL_LAST (requested) -- this is the observed
/// cadence. bd GPU-timestamp-semaphore-split-reload-20fps-residual-2026-07-22.
pub static PRESENT_REFRESH_PER_PRESENT_X100: AtomicUsize = AtomicUsize::new(0);
/// Wall-clock microseconds between the last two GetFrameStatistics SyncQPCTime samples (present-to-present
/// spacing straight from DXGI, independent of our Instant timing). ~49920 on the pinned reload frame.
pub static PRESENT_QPC_DELTA_US: AtomicUsize = AtomicUsize::new(0);
/// Per-frame GPU-busy time in MICROSECONDS: the median-of-recent span between two D3D12 TIMESTAMP
/// queries the DLL injects onto the game's ID3D12CommandQueue -- Start on the first ExecuteCommandLists
/// after a present, end at the top of the Present detour (before the original Present). Excludes the
/// vsync/flip present-wait (that happens inside the original Present, after the end stamp), so a large
/// value == render-bound (GPU genuinely busy ~50ms) while a small value with a 50ms frame == a
/// present/vblank throttle. This is the goal-doc §3.3 `gpu_frame_us` oracle, splitting the reload-20fps
/// residual into GPU-render vs present-wait. bd er-effects-rs-03ma /
/// switch-reload-framerate-parity-acceptance.md.
pub static GPU_FRAME_US_LAST: AtomicUsize = AtomicUsize::new(0);
/// Count of successful GPU-timestamp readbacks (each = one resolved START/END pair). Emitted as
/// `oracle_gpu_frame_samples` so a `gpu_frame_us == 0` is attributable: 0 samples == the oracle never
/// produced (queue not latched / D3D12 setup failed / not under Wine), not "GPU is instant".
pub static GPU_FRAME_ORACLE_SAMPLES: AtomicUsize = AtomicUsize::new(0);
/// GPU-timestamp oracle lifecycle state (emitted `oracle_gpu_frame_state`): 0=not started,
/// 1=game device + query heap/list/readback created, 2=game ExecuteCommandLists hooked + queue latched,
/// 3=producing (at least one full start..END pair resolved). Distinguishes where setup stopped when
/// `gpu_frame_us` stays 0.
pub static GPU_FRAME_ORACLE_STATE: AtomicUsize = AtomicUsize::new(0);
/// Internal previous-sample state for the GetFrameStatistics deltas (not emitted): last PresentCount,
/// last SyncRefreshCount, last SyncQPCTime (QPC ticks, low bits).
pub static PRESENT_STATS_PREV_PRESENT_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static PRESENT_STATS_PREV_SYNC_REFRESH: AtomicUsize = AtomicUsize::new(0);
pub static PRESENT_STATS_PREV_QPC: AtomicU64 = AtomicU64::new(0);
/// Microseconds spent in the DLL's boot-view composite (composite_on_game_swapchain) in the present
/// detour, before the original Present. If this is ~tens of ms in-world on reloads it is the per-frame
/// work stall (present_call_us is fast but the composite is invisible to it, yet counts in the
/// present-to-present frame time). bd present-fast-work-stall...dll-bootview-composite-2026-07-22.
pub static COMPOSITE_LAST_US: AtomicUsize = AtomicUsize::new(0);
/// Microseconds spent in the DLL's main recurring game-task body (FrameBegin) last frame. Splits a
/// DLL per-frame code cost (large on reloads => our bug) from a game-side loop cost (fast => game/env).
/// bd correction-scan-fix-didnt-recover...suspect-moveprobe-2026-07-22.
pub static GAME_TASK_LAST_US: AtomicUsize = AtomicUsize::new(0);
/// Free-running count of main recurring game-task bodies entered, readable from any thread.
///
/// `EffectsState::game_task_ticks` already counts this, but it lives behind the state mutex and is
/// only observable through a telemetry write the game task itself performs -- so it can answer "how
/// many ticks happened" only for as long as the task is alive to report it, which is precisely when
/// the question is uninteresting. A thread that needs to know whether the game task is still running
/// (the boot picker, which blocks for as long as a user browses) cannot use it: taking the mutex is
/// the one thing that can block forever if the task froze while holding it.
///
/// Measured need, run pr109-boot-oscancel-20260730-110704: the task reached tick 60 at +16.9s and
/// then stopped for the remaining 17s of the run. Nothing in the telemetry said so -- the file simply
/// stopped changing, which is indistinguishable from a file nobody looked at.
pub static GAME_TASK_TICKS_TOTAL: AtomicUsize = AtomicUsize::new(0);
/// Microseconds in the DLL build-driver FrameBegin task (maybe_register_stats_panel_textures +
/// force_profile_render_tick) last frame -- the last untimed DLL per-frame task. bd
/// sweep-DIAG-cheap-last-dll-suspect-is-build-driver-2026-07-22.
pub static BUILD_DRIVER_LAST_US: AtomicUsize = AtomicUsize::new(0);
pub static GAME_PRESENT_HOOKED: AtomicUsize = AtomicUsize::new(0);
pub static GAME_SWAPCHAIN_FIND_TRIES: AtomicUsize = AtomicUsize::new(0);
pub static PRESENT_RESOLVED_ADDR: AtomicUsize = AtomicUsize::new(0);
pub static PRESENT1_RESOLVED_ADDR: AtomicUsize = AtomicUsize::new(0);
pub static GAME_SWAPCHAIN: AtomicUsize = AtomicUsize::new(0);
pub static GAME_BASE: AtomicUsize = AtomicUsize::new(0);
pub static PRESENT_FIND_STAGE: AtomicUsize = AtomicUsize::new(0);
pub static PRESENT_FIND_CANDIDATE: AtomicUsize = AtomicUsize::new(0);
pub static PRESENT_FIND_CANDIDATE_VT: AtomicUsize = AtomicUsize::new(0);
pub static PRESENT_FIND_GOT8: AtomicUsize = AtomicUsize::new(0);
pub static PRESENT_FIND_GOT22: AtomicUsize = AtomicUsize::new(0);
pub static PRESENT_FIND_VT_MODULE_KIND: AtomicUsize = AtomicUsize::new(0);
pub static PRESENT_FIND_STREAK: AtomicUsize = AtomicUsize::new(0);
pub static PRESENT_FIND_LAST_CANDIDATE: AtomicUsize = AtomicUsize::new(0);
pub static PRESENT_ACCEPT_PATH: AtomicUsize = AtomicUsize::new(0);
pub static PRESENT_BACKBUFFER_FORMAT: AtomicUsize = AtomicUsize::new(0);
pub static PRESENT_COMPOSITE_EARLY_SKIPS: AtomicUsize = AtomicUsize::new(0);

// ---- migrated group: portrait_lookat, portrait_semaphores, return_title, anti_debug, stats_panel_text, stats_panel_background, tpf_textures, portrait_camera, gaitem_restore, loading_cover, switch_liveness, player_correctness, software_breakpoints (399 counters) ----
pub static PROFILE_LOOKAT_APPLY_CALLS: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_LOOKAT_HEAD_IDX: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static PROFILE_LOOKAT_NECK_IDX: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static PROFILE_LOOKAT_SPINE2_IDX: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static PROFILE_LOOKAT_BONE_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_LOOKAT_BONES_DUMPED_MASK: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_READBACK_SOME: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_READBACK_CHECKER: AtomicUsize = AtomicUsize::new(0);
// PROFILE_READBACK_DEFERRED_SOME / _NONBLACK removed 2026-08-31: the H2-vs-H3 deferred-readback
// diagnostic that wrote them was deleted (see lookat_bone_hooks.rs, "has been removed now that the
// ..."), but both were still printed every `lookat-phase-sweep` line as `defer_some=`/`defer_nonblack=`.
// The 2026-08-31 settlement run emitted six such lines reading 0, which is indistinguishable from
// "the deferred readback ran and found nothing". er-effects-rs counter census.
pub static PROFILE_CHECKER_DUMPED: AtomicBool = AtomicBool::new(false);
pub static PROFILE_PERFRAME_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_PERFRAME_HOOK_HITS: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_ANIM_BOUND_RENDERER: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_ANIM_BOUND_LOC: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_FACEDATA_NEQ_TICKS: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_DRIVE_TICKS: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_KICK_SLOT_KEY: AtomicUsize = AtomicUsize::new(0);
/// Times the LoadGame job builder (0x140826510) was asked to build for a slot other than the
/// explicit boot selection (user pick first, configured autoload slot second), and we redirected
/// it to that selection. Nonzero means the save container's persisted last-used slot
/// (`CSMenuSystemSaveLoad+0x1200`) would have loaded the wrong character. Was 1 in the 2026-08-03
/// picker repro (stored 2 vs pick 0) and the 2026-08-13 configured-slot repro (stored 9 vs config 0).
pub static LOADGAME_BUILDER_SLOT_OVERRIDES: AtomicUsize = AtomicUsize::new(0);
/// The native slot the last override replaced, u32-packed. Together with the explicit boot slot
/// this identifies exactly which character the game was about to load instead.
pub static LOADGAME_BUILDER_LAST_NATIVE_SLOT: AtomicUsize = AtomicUsize::new(usize::MAX);
/// The slot this loading-screen window committed its portrait to, +1 (0 == not yet committed).
/// Latched at the window's first slot resolution and held until the window closes, so the face on
/// screen cannot change character mid-load. See `er_loading_portrait_core::portrait_window_target_slot`.
pub static PORTRAIT_WINDOW_TARGET_SLOT: AtomicUsize = AtomicUsize::new(0);
/// Times the freshly-resolved target disagreed with what this window already committed to, i.e.
/// retargets that were suppressed. Each one is a mid-load face change the user did not see.
/// Nonzero proves the latch is load-bearing; it was 1 in the 2026-08-02 21:05 repro (slot 0 -> 9).
pub static PORTRAIT_WINDOW_RETARGETS_SUPPRESSED: AtomicUsize = AtomicUsize::new(0);
/// Times a window latch adopted from a guess was promoted to the user's explicit pick.
///
/// The window latch exists so a committed face cannot change mid-load, but it was committing to
/// whatever the boot autoload guessed before the user had picked anything -- and then refusing the
/// pick as a "mid-window retarget". Measured 2026-08-26: latched slot 0 at +1061ms with
/// `picker=None b78=None ac0=-1`, user picked slot 1 eighteen minutes later, retarget suppressed,
/// and the loading screen showed slot 0's character. Exactly one promotion per window is possible
/// (a latch that came from the pick never yields), so this counts windows the pick rescued.
pub static PORTRAIT_WINDOW_TARGET_PICK_PROMOTIONS: AtomicUsize = AtomicUsize::new(0);
/// Whether this window's latched portrait target came from the user's explicit pick (1) or from a
/// guess (0). Reset with `PORTRAIT_WINDOW_TARGET_SLOT` on window close.
pub static PORTRAIT_WINDOW_TARGET_FROM_PICK: AtomicUsize = AtomicUsize::new(0);
/// Which source this window's latch rests on, as `PortraitSlotSource::rank()`: 0 = not committed,
/// 1 = `GameMan.save_slot` (ac0), 2 = the `GameMan+0xb78` load-request register, 3 = the user's
/// explicit pick. Reset with `PORTRAIT_WINDOW_TARGET_SLOT` on window close.
///
/// `PORTRAIT_WINDOW_TARGET_FROM_PICK` is the top rank collapsed to a bit, and that collapse is
/// what hid bd `er-effects-rs-fmy6`: a latch taken off a stale `ac0` and a latch taken off the
/// user's pick were the same value, so the window refused the load request that superseded it.
/// The rank is what the yield rule compares; the boolean is kept because it is a published oracle.
pub static PORTRAIT_WINDOW_TARGET_SOURCE: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_KICK_RENDERER: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_LAST_CONFIRMED_SLOT: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_SLOT_FLIP_CANDIDATE: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_SLOT_FLIP_STREAK: AtomicUsize = AtomicUsize::new(0);
pub static DEPTH_KEY_CACHED: AtomicUsize = AtomicUsize::new(0);
pub static DEPTH_KEY_NOMASK: AtomicUsize = AtomicUsize::new(0);
pub static DEPTH_RB_INFLIGHT_SKIPS: AtomicUsize = AtomicUsize::new(0);
pub static DEPTH_RB_FIND_FAILS: AtomicUsize = AtomicUsize::new(0);
pub static DEPTH_RB_DIMS_MISMATCHES: AtomicUsize = AtomicUsize::new(0);
pub static DEPTH_RB_NOGAP: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_PUMP_BLOCK_R: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_PUMP_BLOCK_VTABLE: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_PUMP_BLOCK_OFF: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_PUMP_BLOCK_OFF_RESOURCE: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_PUMP_BLOCK_MULTI: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_ANIM_BIND_STATE: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_ANIM_BIND_ATTEMPTS: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_ANIM_BOUND_ID: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_ANIM_HANDLE_BEFORE: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_ANIM_HANDLE: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_ANIM_SENTINEL: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_MOTION_METRIC_LAST: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_MOTION_METRIC_MAX: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_LUMA_FLICKER_LAST: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_LUMA_FLICKER_MAX: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_DRAW_TASK_CTX: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_IN_OUR_DRIVE: AtomicBool = AtomicBool::new(false);
pub static PROFILE_RENDERER_TEARDOWN_FENCE: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_DRIVE_FENCE_SKIPS: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_DRIVE_CLOTH_SKIPS: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_TEARDOWN_FENCE_WAITS: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_TEARDOWN_FENCE_TIMEOUTS: AtomicUsize = AtomicUsize::new(0);
/// Window mark for [`PROFILE_RENDER_DRIVE_HITS`] -- the render-thread tick that owns the RT->SRV
/// copy, the readback and the publish attempt. This is the tick that actually feeds the publish
/// path, so its per-window delta is the honest "did the portrait pipeline run this window" number.
/// Distinct from [`PROFILE_DRIVE_FRAMES_WINDOW`], which counts the separate pose drive (model
/// update task + per-frame push) and is gated behind `off_resources_ready`.
pub static PROFILE_RENDER_DRIVE_HITS_WINDOW_MARK: AtomicUsize = AtomicUsize::new(0);
/// Window mark for [`PORTRAIT_PUMP_BLOCK_OFF_RESOURCE`]. Its per-window delta is the attribution for
/// a zero pose-drive count: the offscreen nest had a null native GX resource wrapper, so the pose
/// drive was skipped deliberately (to avoid the FUN_141e90290 rcx=0x20 AV) rather than the head
/// having "frozen early". Without this delta beside it, `animated 0` reads as a freeze.
pub static PORTRAIT_PUMP_BLOCK_OFF_RESOURCE_WINDOW_MARK: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_PUBLISH_CLEAN_WINDOW_MARK: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_PUBLISH_SKIPPED_TORN_WINDOW_MARK: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_PUBLISH_SKIPPED_UNKEYED_WINDOW_MARK: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_MULTI_MODEL_PUBLISH_SKIPS_WINDOW_MARK: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_RT_PIN_SWITCHES_WINDOW_MARK: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_DRIVE_FENCE_SKIPS_WINDOW_MARK: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_COLOR_FROM_BUNDLE_WINDOW_MARK: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_COLOR_FROM_SCAN_WINDOW_MARK: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_DEPTH_FROM_CHAIN_WINDOW_MARK: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_DEPTH_FROM_BFS_WINDOW_MARK: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_PUBLISH_SKIPPED_UNPAIRED_WINDOW_MARK: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_PUBLISH_SKIPPED_LOWMASK: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_PUBLISH_SKIPPED_LOWMASK_WINDOW_MARK: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_WINDOW_FIRST_KEYED_DISPLAY: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static PROFILE_WINDOW_FIRST_KEYED_DISPLAY_LAST: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_SLOT_NAMES_DUMPED: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_READBACK_CHECKER_WINDOW_MARK: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_TEAR_EMA: AtomicUsize = AtomicUsize::new(0);
// PROFILE_ALPHA0_CLEARS removed 2026-08-31: the scene-alpha clear it counted is compiled off
// (lookat_bone_hooks.rs keeps `portrait_alpha0_clear` alive only through a `let _ =` discard), so
// `oracle_portrait_alpha0_clears` emitted 0 forever. Note the discard `let _ = &PROFILE_ALPHA0_CLEARS`
// is exactly what made the by-reference write rule read this as a live counter.
pub static PROFILE_MODEL_PARTS_DUMPED: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_DRAW_TASK_CTX_LOGGED: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_PERFRAME_MODEL_DRAWS: AtomicUsize = AtomicUsize::new(0);
// PROFILE_PERFRAME_SPARED_DRAWS removed 2026-08-31: no writer, yet printed twice per
// `lookat-phase-sweep` line as `spared[... draws=]`. PROFILE_PERFRAME_MODEL_DRAWS (written) is the
// live per-frame draw counter; the spared-model variant counted a draw path that no longer exists.
pub static PROFILE_SPARED_MODEL_OK: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_GX_QUEUE_SAMPLES: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_GX_QUEUE_NONEMPTY: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_GX_POOL_FREE_MIN: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static PROFILE_GX_POOL_FREE_LAST: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_GX_POOL_USED_MASK: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_LOOKAT_YAW_BITS: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_LOOKAT_PITCH_BITS: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_LOOKAT_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_LOOKAT_HOOK_HITS: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_LOOKAT_RENDER_DRIVES: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_LOOKAT_REALTIME: AtomicBool = AtomicBool::new(false);
pub static PROFILE_LOOKAT_PHASE_DIAG_COUNTER: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_LOOKAT_DRAW_FRAME: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_LOOKAT_RT_SAMPLES: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_LOOKAT_RT_NONBLACK: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_LOOKAT_RT_CHANGED: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_LOOKAT_RT_RGB_MAX: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_LOOKAT_RT_ALPHA_MAX: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_RT_CONTENT_DUMPED: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_SRV_DUMPED: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_RT_SRV_COPIES: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_RT_SRV_COPIES_WINDOW: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_RB_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_RB_WAIT_US_SUM: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_RB_DESWIZZLE_US_SUM: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_RB_MASK_US_SUM: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_RB_MASK_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_PIPELINE_GEN: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_RT_SRV_COPY_DIAGGED: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_CONTENT_EXCL_DUMPED: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_BAKE_RGBA_CAPTURED: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_HAVE_KEYED_FRAME: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_PUBLISH_SKIPPED_UNKEYED: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_PORTRAIT_RETARGETS: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_TEAR_SCORE_LAST: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_TEAR_SCORE_MAX: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_TEAR_SCORE_CLEAN_MIN: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static PROFILE_PUBLISH_SKIPPED_TORN: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_PUBLISH_CLEAN: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_WINDOW_PUBLISH_FAILURES: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_WINDOW_PUBLISH_FAIL_CAUSE: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_PUBLISH_CLEAN_WINDOW: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_WINDOW_PUBLISH_FAIL_LATCHED: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_LAST_SKIP_CLASS: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_DRIVE_FRAMES_WINDOW: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_DISPLAY_FRAMES_WINDOW: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_DRIVE_FRAMES_WINDOW_LAST: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_DISPLAY_FRAMES_WINDOW_LAST: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_LOOKAT_RT_LASTHASH: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_LOOKAT_RT_LASTSLOT: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static PROFILE_LOOKAT_SELFTEST_ON: AtomicBool = AtomicBool::new(false);
pub static PORTRAIT_RENDER_WINDOW_DONE: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_SCALEFORM_BIND_OBSERVER_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_SCALEFORM_BIND_OBSERVER_HITS: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_SCALEFORM_BIND_OBSERVER_SYSTEX_HITS: AtomicUsize = AtomicUsize::new(0);
// TITLE_PROFILE_VISIBLE_SURFACE_BIND_REWRITES removed 2026-08-31 with its three
// TITLE_PROFILE_VISIBLE_SURFACE_BIND_LAST_* siblings in constants/anti_debug.rs: the visible-surface
// bind rewrite they were declared for was never implemented, so all four read 0 forever while five
// oracles emitted them -- one of which scripts/er-readiness-watch.py copied into the
// loading-screen-portrait event JSON.
pub static LS_PORTRAIT_LAST_W: AtomicUsize = AtomicUsize::new(0);
pub static LS_PORTRAIT_LAST_H: AtomicUsize = AtomicUsize::new(0);
pub static LS_PORTRAIT_LAST_NEUTRAL_PCT: AtomicUsize = AtomicUsize::new(0);
pub static LS_PORTRAIT_TOO_SMALL_SEEN_VERSION: AtomicUsize = AtomicUsize::new(0);
pub static LS_PORTRAIT_NEUTRAL_LEAK_SEEN_VERSION: AtomicUsize = AtomicUsize::new(0);
pub static LS_PORTRAIT_REJECTED_PUBLISHES: AtomicUsize = AtomicUsize::new(0);
/// Identity tag of the currently-published loading-portrait head (bd er-effects-rs-dpf6 Phase 1):
/// slot+1 (0 = no published head) and the FNV-1a64 hash of the slot's ProfileSummary character name
/// UTF-16 units (0 = unknown). Written next to the bridge on every publish; cleared with the bridge.
pub static LS_PORTRAIT_PUBLISHED_SLOT: AtomicUsize = AtomicUsize::new(0);
pub static LS_PORTRAIT_PUBLISHED_NAME_HASH: AtomicUsize = AtomicUsize::new(0);
/// Published-vs-loaded semaphore (bd er-effects-rs-qoqc defect 6 / er-effects-rs-91zb). The
/// pre-existing identity semaphore compared our target slot against the currently-resident
/// character, which is silent about the failure that actually reached the screen: on 2026-08-02
/// slot 9's face was published and displayed for 29.7s while slot 5 loaded, and every oracle said
/// ok. These compare what was published against the slot whose load actually completed, asserted
/// at every loading-window close (`PORTRAIT-LOADWIN VERDICT`). Both must stay 0.
pub static PORTRAIT_PUBLISHED_SLOT_MISMATCHES: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_PUBLISHED_NAME_HASH_MISMATCHES: AtomicUsize = AtomicUsize::new(0);
/// Number of loading windows whose published-vs-loaded identity was actually checked. A run with
/// 0 mismatches and 0 checks proved nothing -- read this before believing the two counters above.
pub static PORTRAIT_PUBLISHED_IDENTITY_CHECKS: AtomicUsize = AtomicUsize::new(0);
/// The slot whose fresh deserialize completed, as slot+1 (0 = none this process yet). Written at
/// each `SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_DONE = 1` site, all of which know their slot.
/// This is the "which character actually loaded" ground truth the publish check compares against;
/// `GameMan.save_slot` is not (both the game and our own code write it for other reasons).
pub static SYSTEM_QUIT_FRESH_DESER_DONE_SLOT: AtomicUsize = AtomicUsize::new(0);
/// Name-hash of the slot the portrait pipeline currently targets, stamped on the game thread at the
/// per-slot build kick (the consume worker may not read game memory, so it copies this atomic into
/// `LS_PORTRAIT_PUBLISHED_NAME_HASH` at publish). 0 = unknown/never kicked this window.
pub static PORTRAIT_TARGET_NAME_HASH: AtomicUsize = AtomicUsize::new(0);
/// Boot-view-epoch ms of the last switch confirm (RETARGET); consumed (swap 0) by the first publish
/// after it to compute `PORTRAIT_CONFIRM_TO_PUBLISH_MS_LAST`. 0 = no confirm pending.
pub static PORTRAIT_CONFIRM_MS: AtomicUsize = AtomicUsize::new(0);
/// ms from the last switch confirm (RETARGET) to the next portrait publish (version bump); keeps the
/// last measured value (oracle_portrait_confirm_to_publish_ms). 0 = never measured.
pub static PORTRAIT_CONFIRM_TO_PUBLISH_MS_LAST: AtomicUsize = AtomicUsize::new(0);
/// Same-identity bridge holds across an own-menu-switch rearm (bd er-effects-rs-dpf6 Phase 3): the
/// incoming slot+name-hash matched the published head, so the window reset kept the bridge.
pub static PORTRAIT_BRIDGE_SAME_IDENTITY_HOLDS: AtomicUsize = AtomicUsize::new(0);
/// The outstanding provisional bridge hold, as slot+1 (0 = none). A hold is taken at the switch
/// rearm on a name-hash comparison whose two operands both come from the same ProfileSummary
/// record, so it cannot detect that the record itself is wrong (2026-08-22, see
/// `same_identity_bridge_hold`). It is therefore recorded as provisional and stays that way until
/// something independent resolves it: this window's own publish clears it (proof), a
/// face-fingerprint mismatch revokes it (refutation), and reaching the next rearm still set means
/// neither ever happened.
pub static PORTRAIT_BRIDGE_HOLD_PROVISIONAL: AtomicUsize = AtomicUsize::new(0);
/// Provisional holds revoked by the record-vs-preview face fingerprint -- the one portrait identity
/// signal that compares the record against a source outside itself. A revocation drops the held head
/// and the frozen crop envelope, so the window shows no portrait rather than the previous
/// character's. `> 0` is a defect signal about the record, not a healthy safety check firing: an
/// intact record cannot produce one (bd k979 -- do not gate on this being non-zero).
pub static PORTRAIT_BRIDGE_HOLD_REVOCATIONS: AtomicUsize = AtomicUsize::new(0);
/// Provisional holds that reached the next switch rearm having neither published nor been revoked:
/// a whole loading window rode a held head that nothing ever confirmed. That is the shape of the
/// 2026-08-22 `displayed-stale` window (65 frames displayed, 0 published, 0 captured). The hold is
/// refused a second window when this fires, so a stale head can own at most one.
pub static PORTRAIT_BRIDGE_HOLD_UNPROVEN: AtomicUsize = AtomicUsize::new(0);
pub static LOADING_BG_PORTRAIT_IS_CHECKER: AtomicUsize = AtomicUsize::new(0);
pub static LOADING_BG_PORTRAIT_DIMS: AtomicUsize = AtomicUsize::new(0);
pub static LOADING_BG_PORTRAIT_FORMAT: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_CAM_FACE_YAW_LATCHED_MASK: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_RENDER_DRIVE_HITS: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_LOADSCREEN_REBUILT: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_LOADSCREEN_TABLE_BUILDS: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_LOADSCREEN_TABLE_OWNED: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_TARGET_KICKS: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_FOREIGN_MODELS_MAX: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_MULTI_MODEL_PUBLISH_SKIPS: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_TABLE_EMPTY_STREAK: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_TABLE_WAS_POPULATED: AtomicUsize = AtomicUsize::new(0);
pub static LOADING_BG_PORTRAIT_SPARED_RENDERER: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_SPARE_CANDIDATE: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_SPARE_CANDIDATE_MODEL: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_RENDERER_TEARDOWN_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_SELECT_TABLE_DIAG_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_SELECT_TABLE_DIAG_LAST: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_SELECT_TABLE_REPAIR_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_SELECT_TABLE_GUARD_SKIP_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_SELECT_TABLE_GUARD_SKIP_LAST: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_RENDERER_SPARE_HITS: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_HOLD_WAIT_TICKS: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_REFRESH_KICKED: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_RENDER_SEMAPHORE_STATE: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_RENDER_SEMAPHORE_LOGGED: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_SLOT_DUMP_MASK: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_FORCE_TICK_COUNTER: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_LOADSCREEN_FEED_TICKS: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_REAL_SLOT_KICK_LOGGED: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_SIZE_PATCHED: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_CHILD_FINISH_TRACE_ORIG: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_CHILD_FINISH_TRACE_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_CHILD_FINISH_TRACE_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_QUICKLOAD_NATIVE_QUIT_ACTION_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_DISABLE_SAVE_MENU_CLEAR_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_SAVE_GATE_DIAG_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SWITCH_ORACLE_TICK: AtomicUsize = AtomicUsize::new(0);
pub static SWITCH_ORACLE_STABLE_FRAMES: AtomicUsize = AtomicUsize::new(0);
pub static SWITCH_ORACLE_MAX_STABLE_FRAMES: AtomicUsize = AtomicUsize::new(0);
pub static SWITCH_ORACLE_TRACKED_SLOT: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static SWITCH_ORACLE_MMS_STEP: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static RELOAD_DRAIN_B80_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_STABLE_PROOF_EPOCH: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static SYSTEM_QUIT_RELOAD_FINALIZE_DONE_EPOCH: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static SWITCH_ORACLE_PLAYER_PRESENT: AtomicUsize = AtomicUsize::new(0);
pub static SWITCH_ORACLE_MENU_JOB_PRESENT: AtomicUsize = AtomicUsize::new(0);
/// Consecutive ticks on which `SYSTEM_QUIT_QUICKLOAD_PHASE` has been stuck at
/// `TITLE_OWNER_SEEN` while the world is demonstrably up.
///
/// The latch this exists to break (measured 2026-09-04, run br-20260904-181251-0586). The only
/// path back to `PHASE_IDLE` is the post-finish stable-proof block, and it is gated on
/// `phase >= AUTOLOAD_HANDOFF (4)`. A switch that reaches `TITLE_OWNER_SEEN (3)` and is then torn
/// down never advances to 4, so it can never reach that reset: `active_switch` stays true for the
/// rest of the process and the load-job Run guard never lifts. Observed effect -- `Load Character
/// from File` becomes a silent no-op FOREVER: the row resolves, the picker opens, the ProfileSelect
/// activation is allowed, and then the log says `forwarding native (load-job Run remains guarded)`
/// while two why-not lines spin for the rest of the session naming `active_switch=true(phase=3)`.
/// Meanwhile switch-oracle reported a perfectly healthy world: `player=true ig_d8=1 pstep=7/7`.
///
/// Phase 3 means "the title owner appeared, handing off to the product Continue autoload". With the
/// player present and the InGameStep resting in-world, the title owner is long gone and that handoff
/// has either completed or died -- either way the latch is stale, so this counts how long that
/// contradiction has held rather than acting on a single frame.
pub static SWITCH_PHASE_TITLE_OWNER_SEEN_STALE_TICKS: AtomicUsize = AtomicUsize::new(0);
/// Number of times the stale `TITLE_OWNER_SEEN` latch above was actually broken. Stays 0 on a
/// healthy session; a non-zero value means a switch was torn down and the recovery caught it.
pub static SWITCH_PHASE_TITLE_OWNER_SEEN_STALE_RESETS: AtomicUsize = AtomicUsize::new(0);
pub static SWITCH_ORACLE_MMS_INIT_HITS: AtomicUsize = AtomicUsize::new(0);
pub static SWITCH_ORACLE_MMS_FINISH_HITS: AtomicUsize = AtomicUsize::new(0);
pub static MOVEMAPSTEP_STEP_MOVEMAP_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static MOVEMAPSTEP_STEP_MOVEMAP_ORIG: AtomicUsize = AtomicUsize::new(0);
pub static INGAMESTEP_STEP_MOVEMAP_UPDATE_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static INGAMESTEP_STEP_MOVEMAP_UPDATE_ORIG: AtomicUsize = AtomicUsize::new(0);
pub static INGAMESTEP_MOVEMAP_UPDATE_DEFER_TICKS: AtomicUsize = AtomicUsize::new(0);
pub static INGAMESTEP_MOVEMAP_UPDATE_DEFER_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static RELOAD_B73_HOLD_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static RELOAD_ENDING_LATCH_HOLD_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_BC4_FORCE_READY_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_LOAD3_FINALIZE_CLEAR_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_TEARDOWN_SAVEREQ_CLEAR_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SWITCH_WORLDRES_NULL_STREAK: AtomicUsize = AtomicUsize::new(0);
pub static SWITCH_WORLDRES_REBUILD_TRIED: AtomicUsize = AtomicUsize::new(0);
pub static SWITCH_WORLDRES_REBUILD_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_QUICKLOAD_TITLE_OWNER_SEEN_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_QUICKLOAD_AUTOLOAD_HANDOFF_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_QUICKLOAD_LS10_REARM_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_QUICKLOAD_LS11_CLEAR_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_QUICKLOAD_MMS4B8_HOLD_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_QUICKLOAD_MMS18_NEXT_HOLD_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_QUICKLOAD_MMS18_TIMER_HOLD_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_QUICKLOAD_MMS244_HOLD_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_QUICKLOAD_LAST_TITLE_OWNER: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_LAST_DIALOG: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_LAST_BOUND: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_TOP_HIDE_ARMED_LIST: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_TOP_HIDE_ARMED_DIALOG: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_TOP_HIDE_TOP_WINDOW: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_TOP_HIDE_PROFILE_WINDOW: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_TOP_HIDE_LIST: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_TOP_HIDE_TOP_MENU_ID: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static SYSTEM_QUIT_TOP_HIDE_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_TOP_RESTORE_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_DUPLICATE_LAST_COUNT_BEFORE: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_DUPLICATE_LAST_COUNT_AFTER: AtomicUsize = AtomicUsize::new(0);
pub static C30_WRITER_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static C30_WRITER_LOG_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static DISMISS_WRITE_LOG: AtomicUsize = AtomicUsize::new(0);
pub static ANTI_ANTIDEBUG_APPLIED: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_ANIM_DIAG_CALLS: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_NATIVE_MENU_VISUAL_SUPPRESSED_BUILDS: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_PAB_INFORMATION_VISUAL_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_PAB_INFORMATION_VISUAL_BUILDS: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_CUSTOM_COVER_PROFILE_SOURCE_SAMPLE_CALLS: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_CUSTOM_COVER_PROFILE_SELECT_BUILDS: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_CUSTOM_COVER_BLACK_BUILDS: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_CUSTOM_COVER_RUN_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_CUSTOM_COVER_RUN_RECURSION: AtomicUsize = AtomicUsize::new(0);
// TITLE_CUSTOM_COVER_RUN_CALLS removed 2026-08-31. Its sole writer
// (title_custom_cover_menu_window_run_hook) was never codegen'd -- the same reason its siblings
// TITLE_CUSTOM_COVER_RUN_LAST_* are pinned to literals in oracles_title_visuals.rs. It was the fifth
// and-term of `oracle_title_loaded_character_portrait_rendered`, whose second and third terms are
// already pinned `false`/`0`, so that oracle was structurally incapable of being true and has been
// removed with it rather than left emitting a permanent `false`.
pub static PAB_RUN_POST_CALLS: AtomicUsize = AtomicUsize::new(0);
// TITLE_OVERLAY_COVER_* removed 2026-07-31: six counters with zero writers, read once each to emit
// oracles for the unbuilt custom title render surface (er-effects-rs-trp). A permanently-0 counter
// cannot be distinguished from a feature that ran and did nothing, so they reported an absent
// feature as a failing one. Re-add with writers at the real render site when trp lands.
// Loading-screen observer install state (2026-08-30). These are not booleans any more: 0 = not
// attempted yet, 1 = installed, 2 = permanently refused, 3 = created and queued, waiting on
// MH_ApplyQueued. The third value is the one that had to exist. Five observers used to share one
// install flag, so a counter reading 0 could equally mean "installed and the game never called it"
// or "never installed at all", and the run where four detours were created but never applied read
// exactly like a quiet loading screen. See `install_now_loading_helper_observer_hooks`.
//
// `NOW_LOADING_HELPER_HOOKS_INSTALLED` is the aggregate, and it is also the caller's poll gate:
// it stays 0 for as long as any one observer is still worth retrying, then latches 1 (at least one
// observer live) or 2 (all five terminal, none live). Callers must keep treating non-zero as "stop
// calling the installer".
pub static NOW_LOADING_HELPER_HOOKS_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static NOW_LOADING_HELPER_CTOR_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static NOW_LOADING_HELPER_UPDATE_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static SCALEFORM_LABEL_GOTO_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static NOW_LOADING_HELPER_CTOR_HITS: AtomicUsize = AtomicUsize::new(0);
pub static NOW_LOADING_HELPER_UPDATE_HITS: AtomicUsize = AtomicUsize::new(0);
pub static LOADING_SCREEN_UPDATE_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static LOADING_SCREEN_UPDATE_HITS: AtomicUsize = AtomicUsize::new(0);
pub static LOADING_SCREEN_UPDATE_LAST_MS: AtomicUsize = AtomicUsize::new(0);
pub static LOADING_SCREEN_GFX_FADEOUT_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
/// Fade-outs played on the loading screen'S own clip (`LOADING_SCREEN_LAST_THIS +
/// LOADING_SCREEN_FADEOUT_CLIP_OFFSET`). This is the signal that decides whether the custom cover
/// may start its release fade, so it has to mean the game's loading screen and nothing else.
///
/// It did not, until 2026-09-05. The `Scaleform label goto` detour stamped here for any movie
/// whose timeline hit a label containing "fadeout", and 98 of the 106 vanilla menu `.gfx` files
/// carry one -- including `02_000_ingametop.gfx`, the pause menu. In run br-20260905-221201-969c
/// all 129 stamps were foreign: not one `this` matched the loading screen's clip, so the cover was
/// being held open by ordinary menu transitions and stayed up ~15s after the world was playable.
pub static LOADING_SCREEN_GFX_FADEOUT_HITS: AtomicUsize = AtomicUsize::new(0);
/// Fade-out labels the same detour saw on some other movie and refused to count above.
///
/// Two jobs, both of which the narrowed counter alone cannot do. It is the detour's LIVENESS
/// proof: `LOADING_SCREEN_GFX_FADEOUT_HITS == 0` on its own cannot tell "the loading screen never
/// faded" from "the hook is dead", and a nonzero value here settles it. And it is the size of the
/// noise that used to be counted as the loading screen's fade -- the number that was 129 in the
/// reproducing run.
pub static LOADING_SCREEN_GFX_FADEOUT_FOREIGN_HITS: AtomicUsize = AtomicUsize::new(0);
/// Armed at every own-menu switch: this cover window must span two native loading screens, not one.
///
/// The flow the game does not NATIVELY do (user report 2026-09-05, measured on
/// br-20260905-234626-ce9a). A System->Quit->Load Character switch shows the native loading plate
/// twice -- once while the outgoing world is torn down, once while the incoming character loads --
/// and the two are distinguishable in the game's own bar:
///
/// ```text
///   REARM   +164497ms   epoch 1
///   screen 1 (0xb2710080)  frame 1/500 for its whole life, FINISH at frame 1/500   <- the unload
///   screen 2 (0xab408c80)  frame 61 -> 421 -> FINISH at frame 500/500              <- the load
/// ```
///
/// The cover is supposed to own the screen from the first plate coming up to the second fading out.
/// It did not: it released 682 ms after the arm, 350 ms before screen 1 even opened, and the user
/// watched 12.9 s of bare native loading screen. Same shape on the third switch (689 ms, 8.2 s bare).
///
/// So the release is gated on the character load's own screen having finished. That was first
/// written as `LOADING_SCREEN_CLOSE_SENT_HITS >= 2` -- the ordinal -- which the user's actual
/// ProfileSelect path (one plate, already the load's) could never satisfy; it now reads
/// `LOADING_SCREEN_COMPLETED_CLOSE_HITS >= 1`, the same instant on the two-plate shape above and a
/// reachable one on the single-plate shape. 0 = boot (one screen only, no gate).
pub static BOOT_VIEW_RELEASE_REQUIRE_SECOND_SCREEN: AtomicUsize = AtomicUsize::new(0);
/// Frames the release was held because the second native loading screen had not finished yet. The
/// direct measure of the defect above: it was 0 on every switch of br-20260905-234626-ce9a, because
/// nothing was holding.
pub static BOOT_VIEW_RELEASE_HELD_FOR_SECOND_SCREEN: AtomicUsize = AtomicUsize::new(0);
/// Mirror of `LOADING_SCREEN_COMPLETED_CLOSE_HITS` as the gate last read it: Completed native
/// loading screens (gauge at 500/500 when they finished) in this cover window. 0 = nothing has
/// loaded a world yet, only teardown plates; 1 = the character load's plate has faded, which is the
/// moment the cover is allowed to let go.
pub static BOOT_VIEW_NATIVE_SCREENS_SEEN: AtomicUsize = AtomicUsize::new(0);
/// Boot-view-epoch ms of the last clean portrait publish, i.e. the last frame on which the head the
/// user is looking at actually changed. 0 = none published in this window.
///
/// Why a TIMESTAMP and not another count. The window-reset line already reports how many frames were
/// published; what it cannot say is when the last one landed, and that is the whole question behind
/// "does the portrait animate for as long as it is on screen". On br-20260906-000112-021a the
/// portrait-motion oracle samples every ~4.5 s and its last line for the switch window is +114405ms
/// while the cover did not stop until +118876ms -- so the final 4.5 s, including the entire release
/// fade, had no evidence either way. Paired with `BOOT_VIEW_STOP_MS` this turns that blind spot into
/// a subtraction.
pub static PORTRAIT_LAST_PUBLISH_MS: AtomicUsize = AtomicUsize::new(0);
/// Boot-view-epoch ms of the last frame the portrait draw tick ran. Distinguishes "the pipeline
/// stopped being driven" from "it ran and had nothing new to publish", which are different bugs.
pub static PORTRAIT_LAST_DRAW_TICK_MS: AtomicUsize = AtomicUsize::new(0);
pub static LOADING_SCREEN_GFX_FADEOUT_FIRST_MS: AtomicUsize = AtomicUsize::new(0);
pub static LOADING_SCREEN_GFX_FADEOUT_LAST_MS: AtomicUsize = AtomicUsize::new(0);
pub static KNOWLEDGE_TIP_REFRESH_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static KNOWLEDGE_TIP_SUPPRESSED_HITS: AtomicUsize = AtomicUsize::new(0);
pub static KNOWLEDGE_TIP_ADVANCE_ENABLED_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static KNOWLEDGE_TIP_ADVANCE_SUPPRESSED_HITS: AtomicUsize = AtomicUsize::new(0);
pub static SCALEFORM_DESC_ADVANCE_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static SCALEFORM_DESC_PROVIDER_NULL_HITS: AtomicUsize = AtomicUsize::new(0);
pub static LOADING_SCREEN_BAR_ENABLED: AtomicUsize = AtomicUsize::new(0);
pub static LOADING_SCREEN_BAR_CURRENT_FRAME: AtomicUsize = AtomicUsize::new(0);
pub static LOADING_SCREEN_BAR_MAX_FRAME: AtomicUsize = AtomicUsize::new(0);
pub static LOADING_SCREEN_BAR_PROGRESS_PERMILLE: AtomicUsize = AtomicUsize::new(0);
pub static LOADING_SCREEN_BAR_FINAL_HITS: AtomicUsize = AtomicUsize::new(0);
pub static LOADING_SCREEN_CLOSE_SENT: AtomicUsize = AtomicUsize::new(0);
pub static LOADING_SCREEN_CLOSE_SENT_HITS: AtomicUsize = AtomicUsize::new(0);
pub static LOADING_SCREEN_CLOSE_SENT_FIRST_MS: AtomicUsize = AtomicUsize::new(0);
/// Native loading screens that finished with the gauge at its terminal frame, this cover window.
///
/// The discriminator `LOADING_SCREEN_CLOSE_SENT_HITS` is missing. That one counts plates; this one
/// counts plates that filled. The distinction is already written down in
/// `BOOT_VIEW_RELEASE_REQUIRE_SECOND_SCREEN`'s own table -- the unload plate finishes at
/// `frame 1/500`, the character load's finishes at `frame 500/500` -- but the gate read the count
/// rather than the frame, so it could only express "the second one" and not "the one that loaded a
/// world".
///
/// What that cost, measured on this run (er-quickload-autoload-debug.log, 2026-09-06). The user's
/// ProfileSelect switch path shows exactly one plate, not two: `loadscreen_builds` went 1 -> 2 and
/// 2 -> 3 across the two switches, and each window's single finish reported `frame=500/500`. So
/// `screens < 2` held forever, `world_handoff=false` in every decision line of both windows, and
/// the cover came down only on the 35 s FPS bail -- `cover_window_ms=35005` at +82240ms and
/// `cover_window_ms=35017` at +263339ms, i.e. 15.3 s and 18.9 s after the bar filled, with the
/// world audible behind it the whole time.
///
/// Counting completed plates instead is the same answer where the old gate worked (on the two-plate
/// run br-20260905-235624-a149 the unload's `frame=1/500` finish does not count and the load's
/// `frame=500/500` finish does, so the release lands at the identical moment) and a reachable one
/// where it deadlocked.
pub static LOADING_SCREEN_COMPLETED_CLOSE_HITS: AtomicUsize = AtomicUsize::new(0);
pub static FAKE_LOADING_SCREEN_SAMPLE_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static FAKE_LOADING_SCREEN_VISIBLE_SAMPLES: AtomicUsize = AtomicUsize::new(0);
pub static RENDER_LOADING_LAYER_SAMPLE_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static RENDER_LOADING_LAYER_NONNULL_SAMPLES: AtomicUsize = AtomicUsize::new(0);
pub static RENDER_LOADING_LAYER_LAST_SLOTS_MASK: AtomicUsize = AtomicUsize::new(0);
pub static RENDER_LOADING_LAYER_VISIBLE_SLOTS_MASK: AtomicUsize = AtomicUsize::new(0);
pub static LOADING_BG_PORTRAIT_GX_KEPT: AtomicUsize = AtomicUsize::new(0);
pub static LOADING_BG_PORTRAIT_GX_CAPTURE_HITS: AtomicUsize = AtomicUsize::new(0);
pub static LOADING_BG_PORTRAIT_NONBLACK: AtomicUsize = AtomicUsize::new(0);
pub static LOADING_BG_PORTRAIT_RGBA_VERSION: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_LIVE_FEED_LOGGED: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_STATS_PUSH_IN_PROGRESS: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_STATS_ROW_POPULATES: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_STATS_SETTEXT_SUBS: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_STATS_PUSH_FAILURES: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_PLAYER_NAME_PUSH_ATTEMPTS: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_PLAYER_NAME_SETTEXT_SUBS: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_PLAYER_NAME_PUSH_FAILURES: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_SLOT_NAMES_DECODED: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_STATS_PUSH_STALE_SKIPS: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_STATS_PUSH_STALE_LAST_COMP: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_STATS_PUSH_STALE_LAST_VT: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_ROW_POPULATE_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_SLOT_STATS_CACHE_STATE: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_SLOT_STATS_DECODED: AtomicUsize = AtomicUsize::new(0);
/// Bitmask (bit N = save slot N) of slots the per-slot cache named but could not decode stats for.
///
/// This is the semaphore for a Load Character row that renders its header and nothing else. The
/// two caches already disagreed in the log (`9/10 slots decoded, 10/10 names decoded`, run
/// `br-20260901-161521-9f7d`) and no oracle carried the disagreement, so a row with a name, no
/// attribute line and no `WL` reached the user as a visual observation. Non-zero is bad and names
/// exactly which rows are affected; the count alone could not, because `decoded < named` does not
/// say which slot lost its stats.
pub static PROFILE_SLOT_STATS_NAMED_WITHOUT_STATS_MASK: AtomicUsize = AtomicUsize::new(0);
/// Bitmask (bit N = save slot N) of live `CS::ProfileSummary` slots found marked occupied while
/// holding something that is not a character, at a moment when no save picker owned the rows.
///
/// This is the RAM signature of `er-effects-rs-fmy6`. The in-game picker renders its browse rows by
/// writing them into these game-owned records, and every exit is supposed to put the real ones
/// back; when one did not (the sticky-`committed` defect, 2026-08-29) the labels stayed, and the
/// user's next loading screens showed `[..] EldenRing` and `[ new ]` as character names beside
/// `RL 0`. That reached the user as something they saw, while `oracle_stats_text_slot_decoded` and
/// `oracle_profile_player_name_slot_decoded` were both already published and nothing compared them.
///
/// Sticky by design (`fetch_or`, never reset): the per-frame sweep heals an orphaned stomp within a
/// frame, so a counter that could be cleared would read 0 in the very run that proved the defect.
/// Non-zero is a defect, not a state, and it names exactly which rows were affected.
pub static PROFILE_SUMMARY_ORPHANED_RECORD_MASK: AtomicUsize = AtomicUsize::new(0);
/// Cumulative samples the orphaned-record scan judged. Read `PROFILE_SUMMARY_ORPHANED_RECORD_MASK`
/// with this: a zero mask means "checked and clean" only when this is non-zero, and "never checked"
/// otherwise -- the distinction a bare mask of 0 cannot make, and the one that turns a silent
/// oracle into false assurance.
pub static PROFILE_SUMMARY_ORPHANED_RECORD_SCANS: AtomicUsize = AtomicUsize::new(0);
/// 1 once the live record table has been seen holding at least one real character, i.e. the boot
/// `CS::ProfileSummary::Deserialize` has run and the bytes are worth judging.
///
/// A latch, not a per-sample test, and that is the whole point. The allocation exists long before
/// it is filled, so an unread table must not be judged -- but "does THIS sample hold a character"
/// is the wrong way to ask: a picker staging its browse rows marks the slots past its listing
/// unoccupied and zeroes all ten records, so during the very defect the mask exists to report, the
/// table holds no character at all. Latching once and gating on the latch keeps the oracle awake
/// exactly then.
pub static PROFILE_SUMMARY_ORPHANED_RECORD_TABLE_READ: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_PRESS_START_BIND_HITS: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_PRESS_START_BIND_HIDE_CALLS: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_PRESS_START_GFX_HIDE_CALLS: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_LOGO_SET_VISIBLE_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_LOGO_CTOR_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_LOGO_GFX_HIDE_CALLS: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_MENU_RESOURCE_ACQUIRE_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_MENU_RESOURCE_ACQUIRE_HITS: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_MENU_RESOURCE_ACQUIRE_LOGO_HITS: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_MENU_RESOURCE_ACQUIRE_LAST_PARAM3: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_SCALEFORM_FILE_OPEN_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_SCALEFORM_FILE_OPEN_HITS: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_SCALEFORM_FILE_OPEN_LOGO_HITS: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_SCALEFORM_FILE_OPEN_LAST_FLAGS: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_SCALEFORM_MEMORY_GFX_REPLACEMENTS: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_SCALEFORM_05_000_MEMORY_GFX_REPLACEMENTS: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_05_000_RUNTIME_STRIP_ARMED: AtomicUsize = AtomicUsize::new(0);
pub static SOUND_POST_EVENT_CORE_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static SOUND_POST_EVENT_HITS: AtomicUsize = AtomicUsize::new(0);
pub static SOUND_POST_EVENT_MUTED_HITS: AtomicUsize = AtomicUsize::new(0);
pub static SOUND_POST_EVENT_FORWARDED_HITS: AtomicUsize = AtomicUsize::new(0);
pub static SOUND_POST_EVENT_FIRST_ID: AtomicUsize = AtomicUsize::new(0);
pub static SOUND_POST_EVENT_LAST_ID: AtomicUsize = AtomicUsize::new(0);
pub static SOUND_POST_EVENT_FIRST_MUTED_ID: AtomicUsize = AtomicUsize::new(0);
pub static SOUND_POST_EVENT_LAST_MUTED_ID: AtomicUsize = AtomicUsize::new(0);
pub static SOUND_POST_EVENT_LAST_PLAYING_ID: AtomicUsize = AtomicUsize::new(0);
pub static SOUND_POST_EVENT_LAST_FLAGS: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_05_000_RUNTIME_STRIP_SERVES: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_05_000_RUNTIME_STRIP_FAILURES: AtomicUsize = AtomicUsize::new(0);
/// Title file-opens that were left vanilla because no cover was drawing over them.
///
/// The strip takes the title's own `PRESS ANY BUTTON` prompt and menu out of the movie, which is
/// right while the boot cover is painted over it and wrong the moment it is not. A non-zero count
/// is the evidence that a title after boot -- the one a player reaches through System > Quit Game
/// -- kept the controls it needs to load, continue or reach settings.
pub static TITLE_05_000_RUNTIME_STRIP_DECLINED: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_05_000_RUNTIME_STRIP_INPUT_LEN: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_05_000_RUNTIME_STRIP_OUTPUT_LEN: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_05_000_RUNTIME_STRIP_INPUT_CLASS: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_05_000_RUNTIME_STRIP_OUTPUT_VALIDATED: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_05_010_RUNTIME_EDIT_ARMED: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_05_010_RUNTIME_EDIT_SERVES: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_05_010_RUNTIME_EDIT_FAILURES: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_05_010_RUNTIME_EDIT_INPUT_LEN: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_05_010_RUNTIME_EDIT_OUTPUT_LEN: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_05_010_RUNTIME_EDIT_INPUT_CLASS: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_05_010_RUNTIME_EDIT_OUTPUT_VALIDATED: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_SCALEFORM_RESOURCE_CTOR_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_SCALEFORM_RESOURCE_CTOR_HITS: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_SCALEFORM_RESOURCE_CTOR_LOGO_HITS: AtomicUsize = AtomicUsize::new(0);
pub static STATS_PANEL_TEX_REGISTERED_MASK: AtomicUsize = AtomicUsize::new(0);
pub static STATS_PANEL_TEX_REGISTER_ATTEMPTS: AtomicUsize = AtomicUsize::new(0);
pub static STATS_PANEL_TEX_REGISTER_FAILURES: AtomicUsize = AtomicUsize::new(0);
pub static STATS_PANEL_BIND_REDIRECTS: AtomicUsize = AtomicUsize::new(0);
pub static STATS_PANEL_BIND_REDIRECT_MASK: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_GFX_VALUE_SET_VISIBLE_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_GFX_VISIBLE_TITLE_FADEIN_SEEN: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_TEXT_GFX_VALUE_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_PRESS_START_GFX_FORCE_FALSE_CALLS: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_PROFILE_FACE_BIND_HITS: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_PROFILE_FACE_LAST_PROXY: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_PROFILE_FACE_LAST_VALUE: AtomicUsize = AtomicUsize::new(0);
pub static ER_TPF_COVER_REGISTER_ATTEMPTED: AtomicUsize = AtomicUsize::new(0);
pub static ER_TPF_COVER_TARGET_REWRITE_FIRED: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_CAM_APPLY_CALLS: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_CAM_LATCHED_MASK: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_CAM_LAST_MATRIX_OK: AtomicUsize = AtomicUsize::new(0);
/// The applied orbit camera of the last `apply_profile_camera_override` -- the values actually written
/// into the renderer (engine baseline * the PROFILE_CAM_*_SCALE/DELTA transform), not the baseline.
///
/// Why these exist (2026-08-21): the portrait camera was believed to be identical for every character,
/// because the engine baseline is read from `MenuOffscrRendParam` row `DAT_143b39858[slot * 0x20]` and
/// that row id is 20 for all ten slots (dumped from `eldenring-deobf.bin`, RVA 0x3b39848, stride 0x20).
/// Believed, but never confirmed from a RUN: no oracle reported a single camera value, so an artifact
/// set could not distinguish "every character is framed the same" from "the framing differs and the
/// difference is what we are chasing". These seven make the applied camera comparable across runs.
///
/// TRANSPORT: raw `f32::to_bits()` widened into the `AtomicUsize` counter, decoded back with
/// `f32::from_bits(v as u32)` at the oracle writer. Same encoding as `PROFILE_LOOKAT_YAW_BITS` /
/// `PROFILE_LOOKAT_PITCH_BITS`; it is lossless and keeps the sign, which a scaled-integer counter
/// would not (pitch, yaw and the target components are all routinely negative). 0 bits == +0.0 ==
/// never applied, which is also what `oracle_profile_cam_apply_calls == 0` says.
pub static PROFILE_CAM_LAST_TARGET_X_BITS: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_CAM_LAST_TARGET_Y_BITS: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_CAM_LAST_TARGET_Z_BITS: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_CAM_LAST_DISTANCE_BITS: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_CAM_LAST_PITCH_BITS: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_CAM_LAST_YAW_BITS: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_CAM_LAST_FOV_BITS: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_GAITEM_RESET_RELEASED_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_GAITEM_RESET_INVOCATIONS: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_GAITEM_RESET_LAST_SLACK_BEFORE: AtomicUsize = AtomicUsize::new(0);
pub static AUTO_CONFIRM_FRAME: AtomicUsize = AtomicUsize::new(0);
pub static AUTO_CONFIRM_MODAL_SEEN: AtomicUsize = AtomicUsize::new(0);
pub static LOAD_CORRECTNESS_DUMPED: AtomicUsize = AtomicUsize::new(0);
pub static OBSERVE_T0_EMITTED: AtomicUsize = AtomicUsize::new(0);
pub static SW_BP_INSTALLED: AtomicUsize = AtomicUsize::new(0);

// ---- migrated group: autoload_state, profile_render, system_quit, own_load_pump, constants (237 counters) ----
pub static FULLREAD_DRAIN_WAITS: AtomicUsize = AtomicUsize::new(0);
pub static FULLREAD_REQ_DISARM_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static FULLREAD_REQ_DISARM_LAST_PREV_SLOT: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static LOADED_PEAK_SEEN_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static LOADED_PEAK_LEVEL: AtomicUsize = AtomicUsize::new(0);
pub static LOADED_PEAK_C30: AtomicI32 = AtomicI32::new(0);
pub static LOADED_PEAK_NAME_LEN: AtomicUsize = AtomicUsize::new(0);
pub static MSGBOX_STALL_JOB: AtomicUsize = AtomicUsize::new(0);
/// `MSGBOX_BUILDER_LOG` (== `oracle_msgbox_total_builds`) sampled at the instant a System->Quit
/// ->Load-Character switch arms, so a reload can be scored on its own `CS::MessageBoxDialog`
/// builds instead of the process-lifetime total. `usize::MAX` == no switch has armed yet.
/// `oracle_msgbox_builds_since_switch_arm` is the delta, and AGENTS.md's "product proof requires
/// zero MessageBoxDialog builds" is exactly `delta == 0` across the reload. A process-total of 0
/// proves nothing about a reload that has not happened yet, which is how run
/// br-20260831-160354-2513 shipped a "zero MessageBox" claim 0.67 s before a build.
pub static MSGBOX_BUILDS_AT_SWITCH_ARM: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static AUTO_ACCEPT_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static AUTO_ACCEPT_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static IN_WORLD_REACHED: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_EPOCH_WORLD_LIVE: AtomicUsize = AtomicUsize::new(usize::MAX);
/// Consecutive frames the reloaded world has been genuinely live (play_time advancing). Once high enough,
/// the child-done-query override releases the held MoveMapStep child so it tears down like vanilla (the
/// override only needs to prevent premature teardown during the load; post-stabilization it must let go, or
/// it strands the child alive forever = the ez10-set + ~4fps steady-state divergence). bd
/// correction-STEP4-finalize-substate-is-0.
pub static WORLD_LIVE_STABLE_FRAMES: AtomicUsize = AtomicUsize::new(0);
// ---- Phase-3 outgoing-world TEARDOWN (bd PHASE3-render-release-is-CommonFinalize-...-2026-07-23) ----
/// One-shot install guard for the observe-only `CS::InGameStep::_Common_Finalize` hook.
pub static COMMON_FINALIZE_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
/// Count of native `_Common_Finalize` invocations (the world render-release that frees GLOBAL_WorldChrMan,
/// CSDistViewManager, g_GxDrawContext, WorldRes area lists, FieldArea, ...). This is the teardown oracle:
/// on the broken in-place switch it stays flat across a reload (0 finalizes); the Phase-3 fix routes the
/// outgoing world through this release so it increments once per switch (like a native quit->Continue).
/// Exposed as `oracle_common_finalize_count` (distinct from `oracle_switch_teardown_count`, which merely
/// counts our menuData+0x5d arm writes).
pub static COMMON_FINALIZE_CALLS: AtomicUsize = AtomicUsize::new(0);
/// `COMMON_FINALIZE_CALLS` captured at switch-arm, so the reload gate can detect the outgoing finalize.
pub static OUTGOING_TEARDOWN_BASELINE: AtomicUsize = AtomicUsize::new(0);
/// Latched 1 once the outgoing world's `_Common_Finalize` was observed for the current switch (before the
/// reload's continue_confirm), i.e. the pre-quit world was released so the rebuild starts fresh.
pub static OUTGOING_TEARDOWN_DONE: AtomicUsize = AtomicUsize::new(0);
/// Frames own_load_switch_reload_fire has held continue_confirm waiting for the outgoing finalize.
pub static OUTGOING_TEARDOWN_WAIT_TICKS: AtomicUsize = AtomicUsize::new(0);
/// Latched 1 when the bounded wait for the outgoing finalize expired -> fail-soft to the old in-place
/// reload (the two holds re-engage to protect the reused world). Keeps the fix from ever softlocking.
pub static OUTGOING_TEARDOWN_FAILSOFT: AtomicUsize = AtomicUsize::new(0);
// ---- WORLDRESWAIT streaming-settle hold (bd reload-overlap-fix-design-worldreswait-defer-release-on-
//      streaming-settle-2026-07-24) -- the armed switch-reload movable-while-streaming dip fix. A hook on
//      CS::MoveMapStep::STEP_WorldResWait's residency predicate FUN_140624bd0 (deobf 0x624bd0; that step
//      is its sole code caller) defers STEP_WorldResWait's player warp + step advance (i.e. the coupled
//      movability/loading-close release) until CS::CSWorldGeomMan geometry streaming settles, scoped to
//      the System-Quit switch reload only. Bounded fail-soft; never writes WorldBlockRes phase/gate bytes. ----
/// One-shot install guard for the STEP_WorldResWait gate (FUN_140624bd0) defer-release hook.
pub static WORLDRESWAIT_GATE_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
/// Total gate-hook invocations (per-frame during any world load). Telemetry oracle_worldreswait_gate_calls.
pub static WORLDRESWAIT_GATE_HOOK_CALLS: AtomicUsize = AtomicUsize::new(0);
/// Per-switch arm latch (1 == this switch reload's WorldResWait release should be held). Set by
/// `arm_worldreswait_hold()` from `own_load_switch_reload_fire` (switch-only, marker-gated), cleared per
/// switch by `reset_worldreswait_hold_latches()` and on release. On boot/load1 it is never set, so the
/// gate hook is a pure passthrough there (the anti-softlock crux).
pub static WORLDRESWAIT_HOLD_ARMED: AtomicUsize = AtomicUsize::new(0);
/// Per-switch latch: WorldBlockRes residency was reached while armed (so the gate's one legit
/// `FUN_14066d610` residency-pop already ran). Once set, the hook stops calling the original (no repeat
/// pop / no repeat pending-vector erase) and holds on geometry-settle instead.
pub static WORLDRESWAIT_RESIDENCY_SEEN: AtomicUsize = AtomicUsize::new(0);
/// Frames since residency was reached (the hold window length); bounds the fail-soft cap.
pub static WORLDRESWAIT_HOLD_WAIT_TICKS: AtomicUsize = AtomicUsize::new(0);
/// Consecutive frames CS::CSWorldGeomMan reported settled (for the K-frame sustain before release).
pub static WORLDRESWAIT_SETTLE_STREAK: AtomicUsize = AtomicUsize::new(0);
/// Run-cumulative outcome telemetry: 1 == the hold engaged (residency seen while armed) at least once.
pub static WORLDRESWAIT_HOLD_ENGAGED: AtomicUsize = AtomicUsize::new(0);
/// Run-cumulative: total frames the gate hook returned not-ready to defer the release (hold length).
pub static WORLDRESWAIT_HELD_FRAMES: AtomicUsize = AtomicUsize::new(0);
/// Run-cumulative: 1 == a hold released because geometry settled (the good outcome).
pub static WORLDRESWAIT_RELEASED_ON_SETTLE: AtomicUsize = AtomicUsize::new(0);
/// Run-cumulative: 1 == a hold released on the bounded fail-soft cap (geometry never settled -> fall
/// back to today's in-place release; no softlock, no regression).
pub static WORLDRESWAIT_RELEASED_ON_FAILSOFT: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_COMPOSITE_EPOCH: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static BOOT_VIEW_COMPOSITE_FIRST_MS: AtomicUsize = AtomicUsize::new(0);
pub static POLICY_TOS_TITLE_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static POLICY_TOS_TITLE_SUPPRESSED_BUILDS: AtomicUsize = AtomicUsize::new(0);
pub static SERVER_STATUS_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static AUTO_ACCEPT_VT_LAST: AtomicUsize = AtomicUsize::new(0);
pub static AUTO_ACCEPT_VT_LOG: AtomicUsize = AtomicUsize::new(0);
pub static SCENE_OBJ_PROXY_CTOR_ORIG: AtomicUsize = AtomicUsize::new(0);
pub static LATCHED_MENU_WINDOW: AtomicUsize = AtomicUsize::new(0);
pub static MENU_WINDOW_LATCH_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_DUPLICATE_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_NOOP_ACTION_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_RETURN_DESKTOP_ACTION_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static PROPERTY_NEW_BUTTON_CONTROLLER_ACTIVATE_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_SAVE_GAME_TEXT_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_SAVE_GAME_CONFIRM_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_DUPLICATE_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_NATIVE_SAVE_GAME_ACTION_LAST_OBJECT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_NOOP_SELECTION_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_SAVE_GAME_TEXT_SUBSTITUTION_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_SAVE_GAME_ACTION_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_SAVE_GAME_CONFIRM_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_SAVE_GAME_CLOSE_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_SAVE_GAME_DEFER_TOP_WINDOW: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_SAVE_GAME_DEFER_TOP_FRAMES: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_NOOP_ACTION_LAST_OBJECT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_LOAD_PROFILE_CONTROLLER_LAST_OBJECT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_OPEN_SAVE_DIR_ACTION_LAST_OBJECT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_OPEN_SAVE_DIR_CONTROLLER_LAST_OBJECT: AtomicUsize = AtomicUsize::new(0);
/// Recorded cloned action object and `PropertyNewButtonController` for the "Load Build from URL"
/// row. Same shape as the two rows above: recorded so a run can prove the row was built, never used
/// as the row identity (which is the list cursor, and only the list cursor).
pub static SYSTEM_QUIT_LOAD_BUILD_URL_ACTION_LAST_OBJECT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_LOAD_BUILD_URL_CONTROLLER_LAST_OBJECT: AtomicUsize = AtomicUsize::new(0);
/// Row presses, and the subset of them actually handed to `er-build-import-runtime`.
/// `REQUEST_COUNT < ACTION_COUNT` means presses were refused -- no `build_url` configured, an
/// import already in flight, or a link with no `?b=<id>` -- and REFUSED_COUNT says how many.
pub static SYSTEM_QUIT_LOAD_BUILD_URL_ACTION_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_LOAD_BUILD_URL_REQUEST_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Presses the runtime refused outright -- no `build_url` configured, an import already in flight,
/// or a link with no `?b=<id>`. Synchronous: the press itself came back with this.
pub static SYSTEM_QUIT_LOAD_BUILD_URL_REFUSED_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Accepted requests that then failed asynchronously (fetch error, unparseable payload, a build
/// whose level and attributes disagree). Counted separately from the refusals above because these
/// are the ones a press reported as started, so the two must never be added together.
pub static SYSTEM_QUIT_LOAD_BUILD_URL_FAILED_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Imports that reached the character and were read back.
pub static SYSTEM_QUIT_LOAD_BUILD_URL_IMPORTED_COUNT: AtomicUsize = AtomicUsize::new(0);
// ---- the in-game link field -----------------------------------------------------------------
// The four outcomes of a row press are counted separately because they are four different stories:
// the field never opened, the player backed out, the player accepted something the gate refused, or
// the player accepted something that imported. Summing any of them would hide which.
/// Link fields opened (one per row press, not per re-open).
pub static SYSTEM_QUIT_LOAD_BUILD_URL_EDITOR_OPEN_COUNT: AtomicUsize = AtomicUsize::new(0);
/// The back action: the field closed and nothing was applied.
pub static SYSTEM_QUIT_LOAD_BUILD_URL_CANCELLED_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Accepts whose link validated and became an import request.
pub static SYSTEM_QUIT_LOAD_BUILD_URL_ACCEPTED_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Accepts the gate refused. Each one re-opened the field rather than applying anything, so this
/// rising while imported does not is the feature working, not failing.
pub static SYSTEM_QUIT_LOAD_BUILD_URL_REJECTED_COUNT: AtomicUsize = AtomicUsize::new(0);
/// `UrlRejection::code()` of the most recent refusal (`0` = none yet).
pub static SYSTEM_QUIT_LOAD_BUILD_URL_LAST_REJECTION: AtomicUsize = AtomicUsize::new(0);
// ---- the dim behind the link field ------------------------------------------------------------
// Two independent things can go wrong with it and they have opposite fixes, so they are counted
// apart. The derived movie may not carry the placement at all, which is an asset problem in
// `er_gfx::build_url_backdrop`; or it may carry it and the live movie may not show it, which would
// mean the native `CS::SoftwareKeyboard` controller rebuilds the root display list it was handed.
// No edit in this repo had ever added a root-level child to `02_990` before this one, so the second
// is genuinely unmeasured and the counter exists to measure it.
/// Derivations of the link field's movie whose bytes carried the dim, read back out of the payload
/// the MemoryFile swap is about to install.
pub static SYSTEM_QUIT_LOAD_BUILD_URL_BACKDROP_DERIVED: AtomicUsize = AtomicUsize::new(0);
/// Derivations whose bytes did not. A non-zero value here means the field went up undimmed and the
/// cause is offline.
pub static SYSTEM_QUIT_LOAD_BUILD_URL_BACKDROP_MISSING: AtomicUsize = AtomicUsize::new(0);
/// Frames of the open field on which the live movie's root resolved a child by the dim's instance
/// name through the game's own `assignComponentWithName`. This is the memory read that says the
/// display object survived into the running movie.
pub static SYSTEM_QUIT_LOAD_BUILD_URL_BACKDROP_RESOLVED: AtomicUsize = AtomicUsize::new(0);
/// Frames on which it did not resolve. Derived but never resolved is the controller-rebuild case.
pub static SYSTEM_QUIT_LOAD_BUILD_URL_BACKDROP_UNRESOLVED: AtomicUsize = AtomicUsize::new(0);

// ---- did the field land where we put it, and did anyone see it --------------------------------
// The two counters below are the closest thing this repo has to "the link field RENDERED". Neither
// is a pixel: what they read is that the movie's root proxy accepted a transform written through
// the game's own setter, on a window the engine was still running. That is a memory read, so it
// belongs here rather than in the module's own statics, where it was unreadable to any watcher and
// the only evidence a run left behind was a prose log line.
/// Frames of an open link field on which the placement wrote a transform through the root proxy.
pub static SYSTEM_QUIT_LOAD_BUILD_URL_WINDOW_PLACED: AtomicUsize = AtomicUsize::new(0);
/// Frames on which the placement wrote nothing. A field counted open with this at its attempt count
/// and `PLACED` at zero is one the player saw at the movie's authored top-left origin, not centred.
pub static SYSTEM_QUIT_LOAD_BUILD_URL_WINDOW_UNPLACED: AtomicUsize = AtomicUsize::new(0);

// ---- the Generate Build Link row: the inverse of everything above -----------------------------
// That row takes a link and rewrites the character; this one takes the character and writes a link.
// It touches no game state at all, so it has no "applied" counter -- what it has instead is a
// separate count for each of the three things that can independently succeed or fail once the URL
// exists: encoding it, putting it on the clipboard, and getting a browser to open it. A run where
// the URL was built but no browser appeared is a different failure from one where the read came
// back empty, and summing them would hide which.
/// Recorded cloned action object and `PropertyNewButtonController` for the row. Telemetry only:
/// the row identity is the list cursor, here as everywhere else on this tab.
pub static SYSTEM_QUIT_GENERATE_BUILD_LINK_ACTION_LAST_OBJECT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_GENERATE_BUILD_LINK_CONTROLLER_LAST_OBJECT: AtomicUsize =
    AtomicUsize::new(0);
/// Row presses, and the subset that actually claimed the exporter.
pub static SYSTEM_QUIT_GENERATE_BUILD_LINK_ACTION_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_GENERATE_BUILD_LINK_REQUEST_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Presses refused because an export was genuinely still running. A refusal that could not be
/// proven live is not counted here -- it is counted below as a stale latch and the press proceeds.
pub static SYSTEM_QUIT_GENERATE_BUILD_LINK_REFUSED_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Presses that found a busy flag with no worker behind it and cleared it. This rising is the
/// safety valve working: a dead latch must never outrank the player. See
/// `generate_build_link_row::export_latch_is_stale`.
pub static SYSTEM_QUIT_GENERATE_BUILD_LINK_STALE_LATCH_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Characters successfully read and encoded into a share URL.
pub static SYSTEM_QUIT_GENERATE_BUILD_LINK_ENCODED_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Length in characters of the most recent URL produced (`0` = none yet). The cheapest proof that
/// the encode produced something of the right order of size rather than an empty string.
pub static SYSTEM_QUIT_GENERATE_BUILD_LINK_LAST_URL_LEN: AtomicUsize = AtomicUsize::new(0);
/// URLs put on the Windows clipboard.
pub static SYSTEM_QUIT_GENERATE_BUILD_LINK_CLIPBOARD_COUNT: AtomicUsize = AtomicUsize::new(0);
/// URLs `ShellExecuteW` accepted (return value > 32), i.e. handed to winebrowser.
pub static SYSTEM_QUIT_GENERATE_BUILD_LINK_OPENED_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Exports that failed after being accepted -- no character in the world, an unreadable catalog, or
/// a shell-execute the OS refused.
pub static SYSTEM_QUIT_GENERATE_BUILD_LINK_FAILED_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_OPEN_SAVE_DIR_ACTION_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_OPEN_SAVE_DIR_SUCCESS_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_OPEN_SAVE_DIR_FAILURE_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_SAVE_GAME_ARMED_DIALOG: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_PROFILE_LOAD_JOB_SLOT: AtomicUsize = AtomicUsize::new(0);
// ---- System->Quit row identity table + resolution oracles ----------------------------------
// The rows of the patched Quit tab share only two dispatchable `PropertyNewButtonController`
// objects, and each row's "action object" is nothing but `controller + 0x70` (that controller's own
// inline std::function storage). So neither pointer is a row identity. These record the row table
// captured at build time and, per activation, which evidence actually resolved the row -- so a run
// shows the gate working instead of merely not crashing.
/// `PropertyNewButtonController` of the native first Quit row (relabelled Save Game).
pub static SYSTEM_QUIT_NATIVE_SAVE_GAME_CONTROLLER_LAST_OBJECT: AtomicUsize = AtomicUsize::new(0);
/// `PropertyNewButtonController` of the native second Quit row (Return to Desktop).
pub static SYSTEM_QUIT_NATIVE_RETURN_DESKTOP_CONTROLLER_LAST_OBJECT: AtomicUsize =
    AtomicUsize::new(0);
/// The `PropertyEditDialog` the row table below was captured from. An activation whose dialog does
/// not match this makes every captured pointer/index stale, so the row is treated as ambiguous.
pub static SYSTEM_QUIT_ROW_TABLE_DIALOG: AtomicUsize = AtomicUsize::new(0);
/// Property-list index of each row, stored as `index + 1` so 0 means "not captured".
pub static SYSTEM_QUIT_ROW_INDEX_SAVE_GAME_PLUS1: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_ROW_INDEX_RETURN_DESKTOP_PLUS1: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_ROW_INDEX_LOAD_PROFILE_PLUS1: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_ROW_INDEX_LOAD_SAVE_PROFILES_PLUS1: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_ROW_INDEX_LOAD_BUILD_URL_PLUS1: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_ROW_INDEX_GENERATE_BUILD_LINK_PLUS1: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_ROW_RESOLVE_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Resolutions that came from the dialog's own list cursor -- the only row identity, shared by mouse,
/// keyboard and pad. Equal to `RESOLVE_COUNT - AMBIGUOUS_COUNT` by construction; a divergence would
/// mean a second identity source was reintroduced.
pub static SYSTEM_QUIT_ROW_RESOLVED_BY_CURSOR_ROW_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_ROW_AMBIGUOUS_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Last resolution: discriminator code (`QuitRowDiscriminator`), resolved row (`QuitRow` + 1),
/// ambiguity reason code (`QuitRowAmbiguity`), live list cursor (`cursor + 1`), and the label kind
/// read live at that cursor row.
pub static SYSTEM_QUIT_ROW_LAST_DISCRIMINATOR: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_ROW_LAST_RESOLVED_ROW: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_ROW_LAST_AMBIGUITY: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_ROW_LAST_CURSOR_PLUS1: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_ROW_LAST_CURSOR_LABEL_KIND: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_ROW_LAST_INPUT_KIND: AtomicUsize = AtomicUsize::new(0);
/// The P0 oracle: an instant-quit that was refused because the activated row could not be
/// positively identified as the Return-to-Desktop row. Any nonzero value means the gate fired.
pub static SYSTEM_QUIT_QUIT_REFUSED_AMBIGUOUS_ROW_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Instant-quits authorized by positive row evidence.
pub static SYSTEM_QUIT_QUIT_AUTHORIZED_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Activations where the action-object alias claimed the Return-to-Desktop row while the resolved
/// row was one of the two cloned rows -- i.e. the exact false identity that terminated the process.
pub static SYSTEM_QUIT_ACTION_ALIAS_FALSE_QUIT_CLAIMS: AtomicUsize = AtomicUsize::new(0);
/// Activations refused because two independent row discriminators named different rows. Two sources
/// disagreeing is an ambiguity, not a tie to break by preference: the row runs nothing at all.
pub static SYSTEM_QUIT_ROW_REFUSED_DISAGREEMENT_COUNT: AtomicUsize = AtomicUsize::new(0);
/// The patched Quit tab's `CS::GridControl` geometry, read live right after the rows are appended.
/// `COLS`/`ROWS` are what `GridControl::MeasureGridFromMovie` derived from the served movie's
/// `Item_<row>_<col>` components; `NAVIGABLE_CELLS` is `cols * rows` (the exact bound of the mouse
/// hit-test loop) and `ITEM_COUNT` is the cursor bound. All four rows are reachable by mouse,
/// keyboard and pad only when `NAVIGABLE_CELLS >= 4`, `ITEM_COUNT == 4` and `ROWS >= 2`.
pub static SYSTEM_QUIT_GRID_COLS: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_GRID_ROWS: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_GRID_NAVIGABLE_CELLS: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_GRID_ITEM_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SCALEFORM_HANDLER_TRACE_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static SCALEFORM_HANDLER_CTORS: AtomicUsize = AtomicUsize::new(0);
pub static SCALEFORM_HANDLER_DTORS: AtomicUsize = AtomicUsize::new(0);
pub static SCALEFORM_HANDLER_DOUBLE_FREES: AtomicUsize = AtomicUsize::new(0);
pub static SCALEFORM_HANDLER_LAST_DOUBLE_FREE_OBJ: AtomicUsize = AtomicUsize::new(0);
pub static MENU_WINDOW_JOB_DTOR_TRACE_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static MENU_WINDOW_JOB_DTOR_DOOMED_GUARDS: AtomicUsize = AtomicUsize::new(0);

/// One-shot install guard for the `MenuWindowJob` FINALIZE hook (deobf 0x1407ada40).
pub static MENU_WINDOW_JOB_FINALIZE_INSTALLED: AtomicUsize = AtomicUsize::new(0);
/// Trampoline for the finalize hook. 0 / `usize::MAX` = not hooked.
pub static MENU_WINDOW_JOB_FINALIZE_ORIG: AtomicUsize = AtomicUsize::new(0);
/// Times the finalize hook nulled a doomed `owningMenuWindow` before the native code virtual-called
/// it. The `~MenuWindowJob` guard covers only the destructor call site (0x7ac720); the finalize has
/// five callers and the observed switch crash arrives via `MenuWindowJob::Run`, so this counter is
/// the one that moves on the crashing path. Exposed as `oracle_menu_window_finalize_guards`.
pub static MENU_WINDOW_JOB_FINALIZE_GUARDS: AtomicUsize = AtomicUsize::new(0);
/// Last window pointer the finalize hook neutralized (diagnostic).
pub static MENU_WINDOW_JOB_FINALIZE_LAST_WINDOW: AtomicUsize = AtomicUsize::new(0);

// The MSB-PARSE / LOADLIST-wait / DLC-root trace counters left this table on 2026-08-25, with the
// traces that owned them: `crates/er-diag-harness/` now holds them as private statics. They were
// never read outside those traces -- no `push_json_*` consumer, no `oracle_*` field -- and a second
// image gets its own copy of any static regardless, so hosting them centrally bought nothing.
//
// `DLC_ROOTS_REFILL_ORIG` below is the one that stayed: the DLC-root self-heal in
// `er-title-flow/src/dlc_roots_self_heal.rs` reads it, and it belongs beside that self-heal's own
// state rather than with the departed traces.
/// Trampoline for the DLC-root refill (`FUN_140e05fb0`), stored by whichever image detoured it.
///
/// Now always 0 in the product, and that is the intended reading. The `er-diag-harness` trace that
/// used to fill it in lives in another image, so the self-heal takes the `game_rva` fallback it has
/// always carried: in a product-only profile that resolves the un-detoured native (identical
/// behaviour), and in a product + harness profile it enters the harness's detour, which forwards.
pub static DLC_ROOTS_REFILL_ORIG: AtomicUsize = AtomicUsize::new(0);

/// Cached address of the `mapstudio_dlc2` entry in `DLFileDeviceManager::virtualRoots`.
pub static DLC_ROOT_ENTRY_ADDR: AtomicUsize = AtomicUsize::new(0);
/// 1 once the `mapstudio_dlc2` root has been observed populated. Arms the self-heal: we only ever
/// restore a root the game itself filled in correctly, never guess one during early boot.
pub static DLC_ROOT_SEEN_POPULATED: AtomicUsize = AtomicUsize::new(0);
/// FNV-1a hash of the `mapstudio_dlc2` root as the game populated it. The heal compares against
/// this rather than a literal, because a literal transcribed from the decompile was wrong (the
/// native stores a trailing slash the source literal lacks) and silently broke the alarm counter.
pub static DLC_ROOT_GOOD_PATH_HASH: AtomicUsize = AtomicUsize::new(0);
/// Self-heal invocations (populated -> empty edges acted on).
pub static DLC_ROOT_HEAL_ATTEMPTS: AtomicUsize = AtomicUsize::new(0);
/// Heals that produced the expected root string. This is the success metric -- not the attempt count.
pub static DLC_ROOT_HEAL_OK: AtomicUsize = AtomicUsize::new(0);
/// Heals that produced a non-empty but wrong root (e.g. the `L"system:/"` fallback the native takes
/// when DLC ownership is unresolved). Non-zero means the heal fired too early and DLC content is
/// resolving to the wrong place -- treat as a failure, not a partial success.
pub static DLC_ROOT_HEAL_WRONG: AtomicUsize = AtomicUsize::new(0);

/// Blocks whose stale file cap stayed (status=0x04, data=null) after the single native re-enqueue.
/// This is the DETERMINISTIC "the map archive backing this file is not mounted" signal -- the read
/// genuinely ran and returned nothing -- so a non-zero value means the load cannot complete and the
/// phase-2 handler will wait forever. Exposed as `oracle_blockres_stalecap_unrecoverable`.
pub static BLOCKRES_STALECAP_UNRECOVERABLE: AtomicUsize = AtomicUsize::new(0);
/// The file cap that tripped it (diagnostic).
pub static BLOCKRES_STALECAP_LAST_DEAD_CAP: AtomicUsize = AtomicUsize::new(0);
/// Ticks on which the map-mount guard-flip driver declined to act. It logged nothing on the
/// 2026-07-30 stall it exists to fix, and with five ANDed conditions there was no way to tell which
/// one refused. Exposed as `oracle_map_mount_guard_declines`.
pub static MOUNT_GUARD_DECLINE_LOGS: AtomicUsize = AtomicUsize::new(0);
/// Boot-phase (`!in_world`) declines, budgeted separately. These are expected and would otherwise
/// exhaust the shared budget long before the reload stall, which is exactly what happened on the
/// instrumentation's first run.
pub static MOUNT_GUARD_DECLINE_BOOT_LOGS: AtomicUsize = AtomicUsize::new(0);

pub static MENU_WINDOW_JOB_DTOR_LIST_REMOVALS: AtomicUsize = AtomicUsize::new(0);
pub static MENU_WINDOW_JOB_DTOR_LAST_GUARDED_WINDOW: AtomicUsize = AtomicUsize::new(0);
pub static MENU_WINDOW_JOB_DTOR_LAST_GUARDED_INDEX: AtomicUsize = AtomicUsize::new(0);
pub static MENU_WINDOW_JOB_DTOR_PRESERVED_STALE_DETACHES: AtomicUsize = AtomicUsize::new(0);
pub static MENU_OFFSCR_REND_PARAM_LOOKUP_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static QUIT_TO_DESKTOP_CLEAN_KILLS: AtomicUsize = AtomicUsize::new(0);
pub static OPTIONSETTING_PANE_SAMPLE_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static OPTIONSETTING_PANE_LAST_WINDOWLIST_RESOLVED: AtomicUsize = AtomicUsize::new(0);
pub static OPTIONSETTING_PANE_LAST_WINDOWLIST_VISIBLE: AtomicUsize = AtomicUsize::new(0);
pub static OPTIONSETTING_PANE_LAST_RESOLVED_MASK: AtomicUsize = AtomicUsize::new(0);
pub static OPTIONSETTING_PANE_LAST_VISIBLE_MASK: AtomicUsize = AtomicUsize::new(0);
pub static OPTIONSETTING_PANE_LAST_DATATYPE: AtomicUsize = AtomicUsize::new(0);
pub static OPTIONSETTING_PANE_GUARD_SKIPS: AtomicUsize = AtomicUsize::new(0);
pub static OPTIONSETTING_PANE_COMPOSITE_BOUND: AtomicUsize = AtomicUsize::new(0);
pub static OPTIONSETTING_PANE_BLANK_DETECTED_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static OPTIONSETTING_CURRENT_DIALOG: AtomicUsize = AtomicUsize::new(0);
pub static OPTIONSETTING_CURRENT_PANE_VISIBLE: AtomicUsize = AtomicUsize::new(0);
pub static OPTIONSETTING_CURRENT_PANE_DATATYPE: AtomicUsize = AtomicUsize::new(0);
pub static OPTIONSETTING_ACTIVELY_SHOWN: AtomicUsize = AtomicUsize::new(0);
pub static OPTIONSETTING_LAST_FLAG: AtomicUsize = AtomicUsize::new(0);
pub static OPTIONSETTING_CURRENT_PANE_EVER_VISIBLE: AtomicUsize = AtomicUsize::new(0);
pub static OPTIONSETTING_REAL_BLANK_DETECTED_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static OPTIONSETTING_CURRENT_TAB: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static OPTIONSETTING_CURRENT_TAB_AT_BLANK: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static SYSTEM_QUIT_OPTIONSETTING_DIRECT_REFRESH_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static OPTIONSETTING_PANE_FIX_APPLIED: AtomicUsize = AtomicUsize::new(0);
pub static OPTIONSETTING_ACTIVE_ROW_SAMPLE_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static OPTIONSETTING_ACTIVE_ROW_DIALOG: AtomicUsize = AtomicUsize::new(0);
pub static OPTIONSETTING_ACTIVE_ROW_TAB: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static OPTIONSETTING_ACTIVE_ROW_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static OPTIONSETTING_ACTIVE_ROW_CLONED_MASK: AtomicUsize = AtomicUsize::new(0);
pub static OPTIONSETTING_ACTIVE_ROW_NATIVE_SAVE_MASK: AtomicUsize = AtomicUsize::new(0);
pub static OPTIONSETTING_ACTIVE_ROW_ACTION_HASH: AtomicUsize = AtomicUsize::new(0);
pub static OPTIONSETTING_ACTIVE_ROW_LABEL_HASH: AtomicUsize = AtomicUsize::new(0);
pub static OPTIONSETTING_ACTIVE_ROW_QUIT_LABEL_MASK: AtomicUsize = AtomicUsize::new(0);
pub static OPTIONSETTING_GAME_OPTIONS_CLONED_ROW_HITS: AtomicUsize = AtomicUsize::new(0);
pub static OPTIONSETTING_GAME_OPTIONS_QUIT_LABEL_HITS: AtomicUsize = AtomicUsize::new(0);
pub static GX_RESERVE_CMD_QUEUE_SLOT_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static GX_CMD_QUEUE_MAX_FILL: AtomicUsize = AtomicUsize::new(0);
pub static GX_CMD_QUEUE_SWITCH_MAX_FILL: AtomicUsize = AtomicUsize::new(0);
pub static GX_CMD_QUEUE_CAP_SEEN: AtomicUsize = AtomicUsize::new(0);
pub static GX_CMD_QUEUE_SUBMITS: AtomicUsize = AtomicUsize::new(0);
pub static GX_CMD_QUEUE_HIST_DROPPED: AtomicUsize = AtomicUsize::new(0);
pub static GX_CMD_QUEUE_NEARFULL_HITS: AtomicUsize = AtomicUsize::new(0);
pub static GX_CMD_PUMP_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static GX_CMD_PUMP_CTX: AtomicUsize = AtomicUsize::new(0);
pub static GX_CMD_QUEUE_PEAK_LAST_LOGGED: AtomicUsize = AtomicUsize::new(0);
pub static GX_CMD_ARENA_MIN_REMAINING: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static GX_CMD_ARENA_SWITCH_MIN_REMAINING: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static PROFILE_SPARE_ORPHAN: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_SPARE_ORPHANS_DELETED: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_MENU_WINDOW_JOB_RUN_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_MENU_WINDOW_JOB_RUN_LOG_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_INGAME_TOP_WINDOW: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_OPTION_SETTING_WINDOW: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_PROFILE_SELECT_WINDOW: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_PROFILE_LOAD_FLOW_ACTIVE: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_HIDE_REAL_WINDOWS_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_RESTORE_REAL_WINDOWS_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_SKIP_RESTORE_AFTER_QUICKLOAD_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_REAL_WINDOWS_HIDDEN: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_WINDOW_LIST_PUSH_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_PROFILESELECT_NATIVE_CLOSE_FIRED: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_PROFILESELECT_NATIVE_CLOSE_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_INWORLD_LOAD_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_INWORLD_LOAD_SKIP_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_INWORLD_LOAD_ALLOW_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_INWORLD_LOAD_ABORT_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_REQUEST_LOAD_SLOT_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_REQUEST_LOAD_SLOT_BLOCK_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_REQUEST_LOAD_SLOT_ALLOW_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_PROFILE_LOAD_CONFIRMED_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_PROFILE_LOAD_JOB_RUN_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_ADDR: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_GAITEM_DESERIALIZE_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_GAITEM_LOOKUP_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_GAITEM_FINALIZE_INSTALLED: AtomicUsize = AtomicUsize::new(0);
/// ProfileLoadDialog activations, both kinds summed: save-file browse/pick steps plus character-slot
/// arms. Do not read this as a load count -- it is per browse step and per slot arm, so
/// `activations / 2` matches the load count only in a session that never navigated a directory. The
/// split below is what a load-count reader wants.
pub static SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Activations routed to the DLL's save-file browser (browse steps and file picks).
pub static SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_PICKER_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Activations that armed a character-slot load -- one per user pick of a slot.
pub static SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_SLOT_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_PROFILE_LOAD_CONFIRMED_BLOCK_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_PROFILE_LOAD_CONFIRMED_ALLOW_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_PROFILE_LOAD_JOB_RUN_BLOCK_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_BLOCK_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_ALLOW_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_GAITEM_DESERIALIZE_SKIP_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_GAITEM_DESERIALIZE_ALLOW_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_GAITEM_DESERIALIZE_RESET_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_GAITEM_LOOKUP_EMPTY_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_GAITEM_LOOKUP_ALLOW_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_GAITEM_FINALIZE_SKIP_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_GAITEM_FINALIZE_ALLOW_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_PROFILE_LOAD_JOB_RUN_LAST_JOB: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_PROFILE_LOAD_JOB_RUN_LAST_LIST: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_CONTINUE_CONFIRM_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_DONE: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_SWITCH_MENU_FREE_RELOAD_FIRED: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_MENU_FREE_STABLE_TICKS: AtomicUsize = AtomicUsize::new(0);
// Deleted 2026-09-05 with the control-file switch driver (user directive): SWITCH_TRIGGER_ARM_COUNT,
// SWITCH_TRIGGER_TEARDOWN_COUNT, SWITCH_TRIGGER_LAST_SLOT, SWITCH_TRIGGER_DEFERRED_COUNT,
// SWITCH_SLOT_CONTROL_MTIME, SWITCH_SLOT_CONTROL_PRIMED and DETERMINISTIC_SWITCH_DRIVER_ACTIVE, plus
// the `oracle_switch_arm_count` / `_teardown_count` / `_deferred_count` / `_last_slot` /
// `_slot_control_mtime` / `_slot_control_primed` fields they fed. They measured a driver that armed a
// character switch without the Quit menu, which is the one thing a second load has to go through.
pub static SYSTEM_QUIT_CONTINUE_CONFIRM_BLOCK_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Forwarded `continue_confirm` calls == world loads this session, boot included. The authoritative
/// total-load witness; see [`crate::load_count`] for why the epoch is not.
///
/// Exactly one increment per forwarded call. It used to increment twice on the `!native_slot_proven`
/// branch (once for that branch's `FORWARD #n` label, once at the unconditional tail), inflating the
/// only honest total by one per unproven reload; that branch now labels itself from
/// [`SYSTEM_QUIT_CONTINUE_CONFIRM_UNPROVEN_FORWARD_COUNT`] instead. Blocked confirms return early
/// and never reach here.
pub static SYSTEM_QUIT_CONTINUE_CONFIRM_ALLOW_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Forwards from outside the switch machine -- in practice the boot/title Continue. The gap between
/// `SYSTEM_QUIT_CONTINUE_CONFIRM_ALLOW_COUNT` and the load epoch, and the reason a 3-load session
/// reports `oracle_current_load_epoch = 2`.
pub static SYSTEM_QUIT_CONTINUE_CONFIRM_NON_SWITCH_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Forwards that arrived while the previous world was still up -- a state we never drive. Logged
/// loudly since forever but counted by nothing, so it was invisible to every load-count audit.
pub static SYSTEM_QUIT_CONTINUE_CONFIRM_WORLD_UP_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Switch-machine forwards whose native requested-slot proof did not fire. Carries the `FORWARD #n`
/// log label that used to be taken from the allow counter.
pub static SYSTEM_QUIT_CONTINUE_CONFIRM_UNPROVEN_FORWARD_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Load-count invariant failures, as the [`crate::load_count::LoadCountMismatch`] bit set. Nonzero
/// means the run's own load counters contradict each other and none of them should be quoted.
pub static LOAD_COUNT_MISMATCH_BITS: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_QUICKLOAD_PHASE: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_INWORLD_ARMED_STABLE_TICKS: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_INWORLD_ARMED_DISARM_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_ARM_PLAYER_WAS_ABSENT: AtomicUsize = AtomicUsize::new(0);
pub static ENDING_REQUEST_STALL_STREAK: AtomicUsize = AtomicUsize::new(0);
pub static ENDING_REQUEST_SET: AtomicUsize = AtomicUsize::new(0);
pub static ENDING_REQUEST_SET_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static ENDING_REQUEST_WHYNOT_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static INWORLD_FINALIZE_DRIVE_STREAK: AtomicUsize = AtomicUsize::new(0);
pub static INWORLD_FINALIZE_DRIVE_SET: AtomicUsize = AtomicUsize::new(0);
pub static INWORLD_FINALIZE_DRIVE_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static INWORLD_FINALIZE_DRIVE_WHYNOT_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static ORACLE_RELIABLE_INGAME_PTR: AtomicUsize = AtomicUsize::new(0);
pub static ORACLE_RELIABLE_MMS_PTR: AtomicUsize = AtomicUsize::new(0);
pub static CHILD_DONE_QUERY_ORIG: AtomicUsize = AtomicUsize::new(0);
pub static CHILD_DONE_QUERY_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static CHILD_DONE_HELD_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static CHILD_DONE_DIAG_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_QUICKLOAD_SELECTED_SLOT: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static INJECT_NAV_FRAME: AtomicUsize = AtomicUsize::new(0);
// INJECT_NAV_LOG_COUNT (per-tap log throttle) and INJECT_NAV_CUR_BUTTONS (the schedule's per-frame
// synthesized wButtons) were the inject-NAV drive's own counters. Writer and reader both sat behind
// `inject_nav_enabled()`, which could only return `false`; they were left with no writer and no
// reader in any crate and went with the gate (2026-08-26). INJECT_NAV_FRAME above keeps its name
// but is now purely the sq-repro fresh-packet counter, which is a real live reader.
pub static FRAME_TIME_WORST_EPOCH: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static MOVE_PROBE_MOVED_FRAMES: AtomicUsize = AtomicUsize::new(0);
pub static SUPPLIED_MOVEMENT_INPUT_FRAMES: AtomicUsize = AtomicUsize::new(0);
pub static DID_MOVE_FRAMES: AtomicUsize = AtomicUsize::new(0);
pub static MOVE_PROBE_EPOCH: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static BOOT_FIRST_FRAME_LOGGED: AtomicUsize = AtomicUsize::new(0);
pub static SAFE_INPUT_CONFIRM_FRAMES_REMAINING: AtomicUsize = AtomicUsize::new(0);
pub static GET_ASYNC_KEY_STATE_ORIG: AtomicUsize = AtomicUsize::new(0);
pub static GET_KEY_STATE_ORIG: AtomicUsize = AtomicUsize::new(0);
pub static DIRECT_INPUT8_CREATE_ORIG: AtomicUsize = AtomicUsize::new(0);
pub static DIRECT_INPUT_CREATE_DEVICE_ORIG: AtomicUsize = AtomicUsize::new(0);
pub static DIRECT_INPUT_GET_DEVICE_STATE_ORIG: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_HANDOFF_COMPLETE: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_ANIM_SPEED_ORIG: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_ANIM_SPEED_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_SETSTATE_TRACE_ORIG: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_SETSTATE_TRACE_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_SETSTATE_TRACE_LAST_OWNER: AtomicUsize = AtomicUsize::new(0);
pub static SYNTHETIC_OUTER_PTR: AtomicUsize = AtomicUsize::new(0);
pub static ASSERT_LOG_LINES_WRITTEN: AtomicUsize = AtomicUsize::new(0);
pub static RENDER_FRAME_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static AV_LOG_LINES_WRITTEN: AtomicUsize = AtomicUsize::new(0);
/// Nested VEH entries the crash logger's re-entrancy latch refused.
///
/// The VEH stack-overflow semaphore. Non-zero means describing one fault faulted again on the same
/// thread and the latch caught it -- the descent that killed ELDEN RING 1.17 with no crash record
/// on 2026-08-28, at 4704 bytes of stack a level against a 1 MiB stack. A run that reports a fault
/// and a non-zero refusal count is telling you the report you are reading is the outermost of a
/// pile, and that the first `access-violation` line is the real one.
pub static VEH_REENTRANT_REFUSALS: AtomicUsize = AtomicUsize::new(0);
/// Crash-log lines spent on the process-fatal exception codes (stack overflow, fastfail, heap
/// corruption, illegal instruction). Separate from the general budget below so a first-chance
/// C++/Rust throw storm cannot consume the line that names the actual kill.
pub static FATAL_EXCEPTION_LOG_LINES_WRITTEN: AtomicUsize = AtomicUsize::new(0);
/// Crash-log lines spent on the remaining error-severity exception codes.
pub static OTHER_EXCEPTION_LOG_LINES_WRITTEN: AtomicUsize = AtomicUsize::new(0);
pub static SELF_DLL_SIZE: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_FLOW_CONTEXT_RECORD_REGULATION_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_FLOW_CONTEXT_RECORD_REGULATION_FIXUPS: AtomicUsize = AtomicUsize::new(0);
pub static SEQ_ITER_CHILD_LOG_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SEQ_ITER_CHILD_LAST: AtomicUsize = AtomicUsize::new(0);
pub static SEQ_ITER_DEBUG_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static PLAYER_CURRENT_ANIMATION_ID: AtomicI32 = AtomicI32::new(0);
pub static C30_WATCH_HITS: AtomicUsize = AtomicUsize::new(0);
pub static C30_WATCH_FRAME_COUNTER: AtomicUsize = AtomicUsize::new(0);

// ---- migrated group: cached_depth_readback, boot_progress, resource_readback, save_picker_overlay, portrait_overlay, portrait_worker, stats_overlay (178 counters) ----
pub static PROFILE_LIVE_RT_RES: AtomicUsize = AtomicUsize::new(0);
pub static RB_FAST_QUEUE: AtomicUsize = AtomicUsize::new(0);
pub static RB_FAST_ALLOC: AtomicUsize = AtomicUsize::new(0);
pub static RB_FAST_LIST: AtomicUsize = AtomicUsize::new(0);
pub static RB_FAST_FENCE: AtomicUsize = AtomicUsize::new(0);
pub static RB_FAST_BUFFER: AtomicUsize = AtomicUsize::new(0);
pub static RB_FAST_BUFSIZE: AtomicU64 = AtomicU64::new(0);
pub static RB_FAST_FENCEVAL: AtomicU64 = AtomicU64::new(0);
pub static PROFILE_DET_RESOLVE_DIAG: AtomicUsize = AtomicUsize::new(0);
pub static RB_DEPTH_QUEUE: AtomicUsize = AtomicUsize::new(0);
pub static RB_DEPTH_ALLOC: AtomicUsize = AtomicUsize::new(0);
pub static RB_DEPTH_LIST: AtomicUsize = AtomicUsize::new(0);
pub static RB_DEPTH_FENCE: AtomicUsize = AtomicUsize::new(0);
pub static RB_DEPTH_BUFFER: AtomicUsize = AtomicUsize::new(0);
pub static RB_DEPTH_BUFSIZE: AtomicU64 = AtomicU64::new(0);
pub static RB_DEPTH_FENCEVAL: AtomicU64 = AtomicU64::new(0);
pub static DEPTH_KEY_DIAG_LOGGED: AtomicUsize = AtomicUsize::new(0);
pub static DEPTH_KEY_APPLIED: AtomicUsize = AtomicUsize::new(0);
pub static DEPTH_KEY_BG_PCT: AtomicUsize = AtomicUsize::new(0);
pub static DEPTH_KEY_FRESH: AtomicUsize = AtomicUsize::new(0);
pub static DEPTH_KEY_NOGAP_LOGGED: AtomicUsize = AtomicUsize::new(0);
pub static DEPTH_KEY_DEGENERATE: AtomicUsize = AtomicUsize::new(0);
pub static DEPTH_KEY_SECOND_PASS: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_PUBLISH_SKIPPED_BADIOU: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_PUBLISH_SKIPPED_BADIOU_WINDOW_MARK: AtomicUsize = AtomicUsize::new(0);
pub static DEPTH_KEY_HIST_DUMPED: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_PUBLISH_SHARE_MIN: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static PROFILE_LOWMASK_SHARE_MAX: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_PORTRAIT_INCARNATION: AtomicUsize = AtomicUsize::new(0);
pub static LAST_DEPTH_MASK_INCARNATION: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_MASK_STALE_REUSE: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_MASK_STALE_REUSE_LOGGED: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_MASK_HEAD_MISMATCH_STREAK: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_MASK_HEAD_MISMATCH_TOTAL: AtomicUsize = AtomicUsize::new(0);
pub static RB_COH_QUEUE: AtomicUsize = AtomicUsize::new(0);
pub static RB_COH_ALLOC: AtomicUsize = AtomicUsize::new(0);
pub static RB_COH_LIST: AtomicUsize = AtomicUsize::new(0);
pub static RB_COH_FENCE: AtomicUsize = AtomicUsize::new(0);
pub static RB_COH_FENCEVAL: AtomicU64 = AtomicU64::new(0);
pub static RB_COH_FRAME: AtomicUsize = AtomicUsize::new(0);
pub static RB_COH_SLOT_BUSY_DROPS: AtomicUsize = AtomicUsize::new(0);
pub static COHERENT_READ_OK: AtomicUsize = AtomicUsize::new(0);
pub static COHERENT_READ_FALLBACK: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_RT_PIN: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_RT_PIN_SWITCHES: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_DEPTH_PIN: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_COLOR_SRC_BUNDLE_LAST: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_COLOR_FROM_BUNDLE: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_COLOR_FROM_SCAN: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_DEPTH_FROM_CHAIN: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_DEPTH_FROM_BFS: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_PUBLISH_SKIPPED_UNPAIRED: AtomicUsize = AtomicUsize::new(0);
pub static DEPTH_CHAIN_DIAG: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_OVERLAY_ARMED: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_OVERLAY_PREV_ACTIONS: AtomicUsize = AtomicUsize::new(0);
pub static GET_ASYNC_KEY_STATE_PROC: AtomicUsize = AtomicUsize::new(0);
pub static XINPUT_GET_STATE_PROC: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_OVERLAY_OPEN_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_OVERLAY_DRAW_HITS: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_OVERLAY_INPUT_HITS: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_OVERLAY_PICK_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_OVERLAY_PICK_REJECT_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_OVERLAY_POLL_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_OVERLAY_HELD_POLLS: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_STAGE_CHARS: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_CHAR_CURSOR: AtomicUsize = AtomicUsize::new(0);
pub static MISSING_SAVE_PICKER_SELECTED_SLOT: AtomicUsize = AtomicUsize::new(usize::MAX);
/// Consecutive ticks the product autoload has read an empty-like profile for its Continue slot.
/// Reset by a single real read, so it measures an unbroken window rather than uptime; past
/// `er_title_flow::boot_hold::EMPTY_PROFILE_ESCALATE_TICKS` the autoload rejects its own save
/// selection and arms the missing-save picker.
pub static PRODUCT_CONTINUE_EMPTY_PROFILE_TICKS: AtomicUsize = AtomicUsize::new(0);
/// One-shot latch: the empty-profile window has already handed the choice back to the user, so the
/// loud hand-back line is never repeated (the arm itself is idempotent regardless).
pub static PRODUCT_CONTINUE_EMPTY_PROFILE_ESCALATED: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_KBD_HOOK_HITS: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_ONTO_DRAW_HITS: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_ALPHA_COVER_PCT: AtomicUsize = AtomicUsize::new(0);

pub static PORTRAIT_CROP_MINX: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static PORTRAIT_CROP_MINY: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static PORTRAIT_CROP_MAXX: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_CROP_MAXY: AtomicUsize = AtomicUsize::new(0);
/// Frames actually folded into the crop envelope, saturating at `PORTRAIT_CROP_SEED_N` (40).
///
/// It used to increment on every `portrait_onto` call, seeding or not, which made its name a lie
/// and its value useless: a live run read 324 against a seed window of 40, so the one question the
/// counter exists to answer -- "is the envelope frozen yet?" -- could not be answered from it at
/// all. Saturating makes `== PORTRAIT_CROP_SEED_N` mean frozen and `< N` mean still seeding, which
/// is what every reader already assumed it meant. Written by `er_loading_portrait_core::portrait_onto`;
/// read by `oracle_portrait_crop_seed_frames`; reset per portrait window alongside the four bounds.
pub static PORTRAIT_CROP_SEED_FRAMES: AtomicUsize = AtomicUsize::new(0);
/// Times a seeding frame actually moved one of the four crop bounds outward, i.e. the number of
/// times the frozen-to-be rect changed shape during the seed window.
///
/// Separate from the frame count because the two answer different questions and only this one is
/// about the defect the user reported -- the portrait making "micro adjustments for a few frames
/// before settling". Apparent head size is `dst_h / crop_h` (`crop_w` cancels out of the scale), so
/// every growth event is one visible size step, and the count is how many steps the settle took.
/// 1 means the envelope was right from the first frame and never moved; a large count means the
/// head shrank repeatedly on screen. Written by `er_loading_portrait_core::portrait_onto` next to the
/// `portrait-crop[..]` log lines that carry the per-event detail; read by
/// `oracle_portrait_crop_growth_events`; reset per portrait window with the bounds.
pub static PORTRAIT_CROP_GROWTH_EVENTS: AtomicUsize = AtomicUsize::new(0);
/// Frames the portrait compositor refused to draw because the source frame was not depth-keyed --
/// every pixel opaque, i.e. the mask cut nothing. Written by the mask gate in
/// `er_loading_portrait_core::portrait_onto`; read by `oracle_portrait_draw_refused_unmasked`.
///
/// Why the gate needs it (2026-08-21): a live run measured `oracle_portrait_alpha_cover_pct = 99`
/// against `oracle_depth_key_bg_pct = 76`. Those cannot both describe a keyed frame -- 99% coverage
/// means the crop envelope grew to (near) the whole render target, which is what a single fully
/// opaque frame folded into the 40-frame seed union does. An unmasked frame therefore does not just
/// look wrong for one frame: it permanently pollutes the frozen crop rect and so the apparent size
/// of the portrait for the rest of the loading screen. Refusing it is the fix; counting the refusals
/// is how a run proves the gate engaged (0 with a bad cover_pct = the gate is not catching it).
pub static PORTRAIT_DRAW_REFUSED_UNMASKED: AtomicUsize = AtomicUsize::new(0);
/// Same refusal one stage earlier: bakes/publishes rejected because the captured frame was unmasked,
/// so an opaque frame never reaches the published head at all. Split from the draw counter because
/// the two say different things -- publish refusals mean the capture side produced a bad frame, draw
/// refusals mean one got past publish. Written by the two colour-only readback writers (the
/// FrameBegin bake in `save_swap_profile_table.rs` and the default-off diagnostic publish in
/// `dlstring_lookat_math.rs`); read by `oracle_portrait_bake_publish_refused_unmasked`.
pub static PORTRAIT_BAKE_PUBLISH_REFUSED_UNMASKED: AtomicUsize = AtomicUsize::new(0);
pub static ALPHA_DIAG_LOGGED: AtomicUsize = AtomicUsize::new(0);
pub static MOTION_LOG_TICKS: AtomicUsize = AtomicUsize::new(0);
pub static OVERLAY_STATS_DRAW_HITS: AtomicUsize = AtomicUsize::new(0);

// ---- migrated group: product_core_own_stepper, path_hooks, input_block, input_trace, drive, loaders, bootstrap_drive, can_move_probe, native_overlay, lifecycle, product_continue, title_tick_cover (170 counters) ----
pub static PRODUCT_AUTOLOAD_ARMED: AtomicUsize = AtomicUsize::new(0);
pub static OWN_STEPPER_FILE_ARMED: AtomicUsize = AtomicUsize::new(0);
pub static COLD_CHAR_MOUNT_FILE_ARMED: AtomicUsize = AtomicUsize::new(0);
pub static OWN_LOAD_FILE_ARMED: AtomicUsize = AtomicUsize::new(0);
pub static OWN_LOAD_CONTINUE_FILE_ARMED: AtomicUsize = AtomicUsize::new(0);
pub static OWN_DISPATCH_FILE_ARMED: AtomicUsize = AtomicUsize::new(0);
pub static OWN_LOAD_INSTALL_JOB_FILE_ARMED: AtomicUsize = AtomicUsize::new(0);
pub static OWN_LOAD_INSTALL_JOB_FIRED: AtomicU64 = AtomicU64::new(0);
pub static OWN_LOAD_PHASE_PUB: AtomicUsize = AtomicUsize::new(0);
pub static COLD_CHAR_MOUNT_PHASE_PUB: AtomicUsize = AtomicUsize::new(0);
pub static OWN_LOAD_STREAM_FRAMES: AtomicU64 = AtomicU64::new(0);
pub static OWN_LOAD_OWNER_CACHED: AtomicUsize = AtomicUsize::new(0);
pub static OWN_LOAD_INGAMESTEP_CACHED: AtomicUsize = AtomicUsize::new(0);
pub static OWN_LOAD_STREAM_RECUR_FRAMES: AtomicU64 = AtomicU64::new(0);
pub static OWN_LOAD_PUMP_FILE_ARMED: AtomicUsize = AtomicUsize::new(0);
pub static OWN_LOAD_PUMP_JOB: AtomicUsize = AtomicUsize::new(0);
pub static OWN_LOAD_PUMP_FIRED: AtomicU64 = AtomicU64::new(0);
pub static PRODUCT_CORE_CALLSITE_TICKS: AtomicU64 = AtomicU64::new(0);
pub static PRODUCT_CORE_CALLSITE_BASE_OK_TICKS: AtomicU64 = AtomicU64::new(0);
pub static PRODUCT_CORE_CALLSITE_SLOT_OK_TICKS: AtomicU64 = AtomicU64::new(0);
pub static PRODUCT_CORE_CALLSITE_LAST_SLOT: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static PRODUCT_CORE_AUTOLOAD_TICKS: AtomicU64 = AtomicU64::new(0);
pub static PRODUCT_CORE_READY_BLOCKS: AtomicU64 = AtomicU64::new(0);
pub static PRODUCT_CORE_READY_SUCCESSES: AtomicU64 = AtomicU64::new(0);
pub static PRODUCT_CORE_OWNER_TICKS: AtomicU64 = AtomicU64::new(0);
pub static PRODUCT_CORE_LAST_TITLE_IN_LOOP: AtomicUsize = AtomicUsize::new(0);
pub static PRODUCT_CORE_LAST_TITLE_IN_TEXTFADEOUT: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_OWNER_SCAN_ATTEMPTS: AtomicU64 = AtomicU64::new(0);
pub static TITLE_OWNER_SCAN_VTABLE_HITS: AtomicU64 = AtomicU64::new(0);
pub static TITLE_OWNER_SCAN_TABLE_REJECTS: AtomicU64 = AtomicU64::new(0);
pub static TITLE_OWNER_SCAN_STATE_REJECTS: AtomicU64 = AtomicU64::new(0);
pub static TITLE_OWNER_SCAN_LAST_STATE_BITS: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static MENU_WINDOW_JOB_CTOR_HITS: AtomicU64 = AtomicU64::new(0);
pub static MENU_WINDOW_JOB_CTOR_SEMANTIC_HITS: AtomicU64 = AtomicU64::new(0);
pub static MENU_WINDOW_JOB_NATIVE_CTOR_B_HITS: AtomicU64 = AtomicU64::new(0);
pub static MENU_WINDOW_JOB_NATIVE_CTOR_B_CONTINUE_HITS: AtomicU64 = AtomicU64::new(0);
pub static MENU_WINDOW_JOB_IDLE_CTOR_HITS: AtomicU64 = AtomicU64::new(0);
pub static MENU_WINDOW_JOB_IDLE_CTOR_CONTINUE_HITS: AtomicU64 = AtomicU64::new(0);
pub static MENU_CONTINUE_IDLE_INSERT_HITS: AtomicU64 = AtomicU64::new(0);
pub static TASK_ENQUEUE_GENERIC_HITS: AtomicU64 = AtomicU64::new(0);
pub static TASK_ENQUEUE_GENERIC_IDLE_ITEM_MATCH_HITS: AtomicU64 = AtomicU64::new(0);
pub static MENU_ITEM_UPDATE_HITS: AtomicU64 = AtomicU64::new(0);
pub static MENU_ITEM_UPDATE_SEMANTIC_HITS: AtomicU64 = AtomicU64::new(0);
pub static MENU_CONTINUE_CANDIDATE_HITS: AtomicU64 = AtomicU64::new(0);
pub static MENU_CONTINUE_CANDIDATE_IDLE_ACCEPT_HITS: AtomicU64 = AtomicU64::new(0);
pub static MENU_CONTINUE_CANDIDATE_NATIVE_ACCEPT_HITS: AtomicU64 = AtomicU64::new(0);
pub static MENU_CONTINUE_CANDIDATE_OTHER_ACCEPT_HITS: AtomicU64 = AtomicU64::new(0);
pub static MENU_CONTINUE_CANDIDATE_ACCEPT_CHANGES: AtomicU64 = AtomicU64::new(0);
pub static TITLE_NATIVE_READY_PREDICATE_HITS: AtomicU64 = AtomicU64::new(0);
pub static TITLE_NATIVE_READY_PREDICATE_LAST_FLAGS: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_NATIVE_READY_PREDICATE_LAST_MASKED: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_NATIVE_READY_PREDICATE_LAST_RET: AtomicUsize = AtomicUsize::new(0);
pub static TFC_CONTINUE_FIRED: AtomicUsize = AtomicUsize::new(0);
pub static TFC_FORCED_CONTINUE_HANDOFF_MS: AtomicU64 = AtomicU64::new(0);
pub static OWN_LOAD_FORCED_CONTINUE_HANDOFF_MS: AtomicU64 = AtomicU64::new(0);
pub static TFC_DRAIN_DIALOG: AtomicUsize = AtomicUsize::new(0);
pub static TFC_AUTO_MENU_OPENED: AtomicUsize = AtomicUsize::new(0);
pub static TFC_LOAD_VEC_WAIT_TICKS: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_UPDATE_ORIG: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_UPDATE_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static TFC_DRAIN_JOB: AtomicUsize = AtomicUsize::new(0);
pub static TFC_DRAIN_TICKS: AtomicUsize = AtomicUsize::new(0);
pub static FORCE_OFFLINE_BYTES_CLEARED: AtomicUsize = AtomicUsize::new(0);
pub static PAB_ADVANCE_ORIG: AtomicUsize = AtomicUsize::new(0);
pub static PAB_ADVANCE_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static PAB_ADVANCE_FIRED: AtomicUsize = AtomicUsize::new(0);
pub static PAB_ADVANCE_SETTLE: AtomicUsize = AtomicUsize::new(0);
pub static OBSERVED_ACTIVE_STEAM_ID64: AtomicU64 = AtomicU64::new(0);
pub static SAVE_DIRECT_STAGE_DONE_STEAM_ID: AtomicU64 = AtomicU64::new(0);
pub static SAVE_DIRECT_STAGE_IN_PROGRESS_STEAM_ID: AtomicU64 = AtomicU64::new(0);
pub static SAVE_DIRECT_STAGE_DIAG_HITS: AtomicU64 = AtomicU64::new(0);
pub static SAVE_DIRECT_STAGE_NO_STEAMID_HITS: AtomicU64 = AtomicU64::new(0);
/// Containers this staging pass wrote from the configured source (every name, every case dir).
pub static SAVE_DIRECT_STAGE_CONTAINERS_WRITTEN: AtomicU64 = AtomicU64::new(0);
/// Leftover save artifacts from an earlier run that staging deleted so they cannot be served.
pub static SAVE_DIRECT_STAGE_STALE_REMOVED: AtomicU64 = AtomicU64::new(0);
/// The stale-serve semaphore. Nonzero means a leftover container survived the staging sweep and
/// the game may open it instead of the configured source -- the silent soft lock of 2026-08-11.
pub static SAVE_DIRECT_STAGE_STALE_REMOVE_FAILED: AtomicU64 = AtomicU64::new(0);
pub static SAVE_REDIRECT_SHGFP_LOGGED: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_REDIRECT_SHGFP_APPDATA_REQUESTS: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_REDIRECT_SHGFP_DIRECT_FILE_BLOCKS: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_REDIRECT_SHGFP_FIRST_LOAD_DONE_BLOCKS: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_REDIRECT_SHGFP_NO_ROOT_BLOCKS: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_NTCREATE_DIAG_LOGGED: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_DISKFREE_LOGGED: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_VOLINFO_LOGGED: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_STEAM_ID_ENV_NORMALIZE_DONE: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_STEAM_API_STEAM_ID_LOGGED: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_REDIRECT_HITS: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_CREATEFILEW_CALLS: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_CREATEFILEW_DIAG_LOGGED: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_CREATEFILEW_DIAG_HITS: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_CREATEFILEW_STAGE_STEAMID_DIR_HITS: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_CREATEFILEW_STAGE_SAVE_FILE_HITS: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_CREATEFILEW_CONFIGURED_FILE_HITS: AtomicUsize = AtomicUsize::new(0);
/// Deepest nesting ever reached in the WIN32 save-redirect file detours (CreateFileW / CopyFileW /
/// GetFileAttributes(Ex)W / FindFirstFileW), counted per thread by `SaveDetourDepth`. 1 = no detour
/// ever re-entered; 2 = a detour's own `fs::read`/`fs::write` re-entered once and was passed
/// through, the expected steady state. Any value above 2 means a pass-through decision was lost and
/// the unbounded-recursion stack overflow of 2026-07-30 is back.
///
/// The ntdll `NtCreateFile` detour deliberately does not count here: it is the layer beneath these,
/// firing again under every Win32 open, so including it would put a healthy open at 2 and a healthy
/// normalize-triggering open at 3 -- an alarm that fires on a working game is an alarm nobody reads.
pub static SAVE_REDIRECT_DETOUR_MAX_DEPTH: AtomicUsize = AtomicUsize::new(0);
/// Nested save-redirect detour entries that were degraded to a pure pass-through. Nonzero is
/// normal (the detours do their own file I/O); it is the depth above, not this count, that
/// distinguishes a healthy re-entry from a recursion.
pub static SAVE_REDIRECT_DETOUR_REENTRANT_PASSTHROUGHS: AtomicUsize = AtomicUsize::new(0);
pub static MISSING_SAVE_BLOCKED_IO_LOGGED: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_QUERY_STAGE_STEAMID_DIR_HITS: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_QUERY_STAGE_SAVE_FILE_HITS: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_QUERY_CONFIGURED_FILE_HITS: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_SL2_QUERY_LOGGED: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_WATCHDOG_ZERO_FRAMES: AtomicUsize = AtomicUsize::new(0);
pub static BLOCK_INPUT_ACTIVE: AtomicUsize = AtomicUsize::new(0);
pub static XINPUT_GET_STATE_ORIG: AtomicUsize = AtomicUsize::new(0);
/// Chain slot for ordinal-100 `XInputGetStateEx`, which is a different export at a different address
/// and therefore needs its own cell. It shares a handler with `XInputGetState` because the two have
/// the same signature and the same thing is done to both, but sharing the slot would be a bug: the
/// union stores the next-in-chain pointer per registration, so a second registration into one cell
/// would send `XInputGetState` callers down `XInputGetStateEx`'s chain. Before 2026-09-05 the Ex
/// detour stored no original at all and silently reused `XINPUT_GET_STATE_ORIG`'s.
pub static XINPUT_GET_STATE_EX_ORIG: AtomicUsize = AtomicUsize::new(0);
/// One INSTALLER at a time for the XInput detours (2026-08-31). `XINPUT_GET_STATE_ORIG == 0` was the
/// only guard, and it is set after `MhHook::new` returns -- so two threads that both read 0 both call
/// `MhHook::new` on the same export. `install_xinput_block` is reached from the game task
/// (`enforce_input_block_now` / `input_trace_tick`) and from the menu thread
/// (`system_quit_menu_window_run_post` -> `save_picker_menu_pump_drive_strip_mouse`), which is the last
/// live path that can print `HOOK REGISTRY DUPLICATE`. Claimed with compare-exchange and released again
/// when the install genuinely did not land: the xinput DLL loads late, so the retry is real and a
/// permanent claim would disarm the harness on every run where the first attempt is early.
pub static XINPUT_BLOCK_INSTALL_CLAIMED: AtomicUsize = AtomicUsize::new(0);
/// Times that claim was released for a retry (no xinput DLL yet, or no hook landed). Non-zero is
/// normal on an early first attempt; a value that keeps climbing means the DLL never appeared.
pub static XINPUT_BLOCK_INSTALL_RETRIES: AtomicUsize = AtomicUsize::new(0);
pub static XINPUT_KEEPALIVE_PACKET: AtomicUsize = AtomicUsize::new(0);
pub static XINPUT_GET_CAPABILITIES_ORIG: AtomicUsize = AtomicUsize::new(0);
pub static XINPUT_SLOT0_POLLS: AtomicUsize = AtomicUsize::new(0);
pub static XINPUT_SLOT0_CAPS_QUERIES: AtomicUsize = AtomicUsize::new(0);
pub static SQ_REPRO_ER_HWND: AtomicUsize = AtomicUsize::new(0);
pub static SQ_REPRO_HELD_VK: AtomicUsize = AtomicUsize::new(0);
pub static SQ_REPRO_BEST_HWND: AtomicUsize = AtomicUsize::new(0);
pub static SQ_REPRO_BEST_AREA: AtomicUsize = AtomicUsize::new(0);
pub static RAWINPUT_MOUSE_MOVE_EVENTS: AtomicUsize = AtomicUsize::new(0);
pub static RAWINPUT_MOUSE_BUTTON_EVENTS: AtomicUsize = AtomicUsize::new(0);
pub static RAWINPUT_KEY_EVENTS: AtomicUsize = AtomicUsize::new(0);
pub static RAWINPUT_HOOK_CALLS: AtomicUsize = AtomicUsize::new(0);
pub static RAWINPUT_BLOCKED_UNFOCUSED_EVENTS: AtomicUsize = AtomicUsize::new(0);
pub static PRESENT: AtomicUsize = AtomicUsize::new(0);
pub static DINPUT_BLOCK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static MISSING_SAVE_INPUT_RELEASE_LOGGED: AtomicUsize = AtomicUsize::new(0);
pub static INPUT_TRACE_ARMED: AtomicUsize = AtomicUsize::new(0);
pub static TRACE_REAL_POLLS: AtomicUsize = AtomicUsize::new(0);
pub static TRACE_PAD_WORD_A: AtomicU64 = AtomicU64::new(0);
pub static TRACE_PAD_WORD_B: AtomicU64 = AtomicU64::new(0);
pub static TRACE_HOOK_LAST_SYNTH: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static TRACE_RING_SEQ: AtomicUsize = AtomicUsize::new(0);
pub static TRACE_RING_READ: AtomicUsize = AtomicUsize::new(0);
pub static TRACE_DROPPED: AtomicUsize = AtomicUsize::new(0);
pub static TRACE_GAME_INPUT_ACCEPT: AtomicUsize = AtomicUsize::new(0);
pub static TRACE_UNFOCUSED_EDGES: AtomicUsize = AtomicUsize::new(0);
pub static TRACE_DRAIN_PREV: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static TRACE_FRAME: AtomicU64 = AtomicU64::new(0);
pub static TRACE_HDR_WRITTEN: AtomicUsize = AtomicUsize::new(0);
pub static TRACE_SEM_LAST_KEY: AtomicU64 = AtomicU64::new(0);
pub static TRACE_SEM_SEQ: AtomicUsize = AtomicUsize::new(0);
pub static TRACE_LAST_HB_MS: AtomicU64 = AtomicU64::new(0);
pub static PLAY_TIME_TRACE_EPOCH: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static OWN_LOAD_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static OWN_LOAD_BODY_PTR: AtomicUsize = AtomicUsize::new(0);
pub static OWN_LOAD_BODY_LEN: AtomicUsize = AtomicUsize::new(0);
pub static OWN_LOAD_FED_BYTES: AtomicUsize = AtomicUsize::new(0);
pub static OWN_LOAD_WBR_UPDATE_CALLS: AtomicU64 = AtomicU64::new(0);
pub static OWN_LOAD_WBR_MAX_PHASE: AtomicU64 = AtomicU64::new(0);
// OWN_LOAD_M28_DISPATCH_FIRED removed 2026-08-31. `own_load_m28_dispatch` is verify-ONLY: the
// AddDefaultFileLoadProcess call it counted was disabled after the block getter AV-faulted, and the
// function's own comment says "NO native call is made here". The counter therefore could not move,
// yet it was one of five components of `world_stream_progress_watermark` in
// scripts/er-readiness-watch.py -- a run-stopping stall decision -- documented there as
// "increments on a working stream". It contributed a constant 0 to every stall verdict. Re-add it
// with the increment if and when the dispatch is re-enabled. OWN_LOAD_M28_DISPATCH_DIAG_CALLS (the
// throttle counter, genuinely written) stays.
pub static WBR_PHASE2_DIAG_CALLS: AtomicUsize = AtomicUsize::new(0);
pub static WBR_UPDATE_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static REQUEST_MOVE_MAP_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static REQUEST_MOVE_MAP_ARM_COUNTDOWN: AtomicUsize = AtomicUsize::new(0);
pub static REQUEST_MOVE_MAP_HOOK_CALLS: AtomicU64 = AtomicU64::new(0);
pub static REQUEST_MOVE_MAP_FIXUPS: AtomicU64 = AtomicU64::new(0);
pub static REQUEST_MOVE_MAP_LAST_BEFORE: AtomicU64 = AtomicU64::new(0);
pub static REQUEST_MOVE_MAP_LAST_C30: AtomicU64 = AtomicU64::new(0);
pub static OWN_LOAD_M28_DISPATCH_DIAG_CALLS: AtomicUsize = AtomicUsize::new(0);
pub static SWITCH_RELOAD_FD4IO_PHASE: AtomicUsize = AtomicUsize::new(0);
pub static SWITCH_RELOAD_FD4IO_DRAIN_WAITS: AtomicUsize = AtomicUsize::new(0);
pub static SWITCH_RELOAD_FD4IO_COMMITTED: AtomicUsize = AtomicUsize::new(0);
/// The three states `SWITCH_RELOAD_FD4IO_PHASE` takes. They live here, beside the atomic they
/// describe, because reading that phase correctly requires them and the readers now span crates:
/// the writer/owner is the root crate's `own_load::loaders` (submit -> drain -> commit), while
/// `er-title-flow`'s b78 guard reads it to decide whether fd4io currently owns `GameMan+0xb78`.
/// er-title-flow must not depend on the root crate, so a private `const` root-side would have
/// forced either a duplicated literal or a host-seam call for a plain comparison against 0.
/// Idle(0): no reload in flight, nobody owns b78. Drain(1): the full read was SUBMITted and is
/// being pumped to residency. Commit(2): residency reached (or the bounded drain timed out) and
/// the feed + continue_confirm own the load.
pub const SWITCH_RELOAD_FD4IO_IDLE: usize = 0;
pub const SWITCH_RELOAD_FD4IO_DRAIN: usize = 1;
pub const SWITCH_RELOAD_FD4IO_COMMIT: usize = 2;
/// Frames the b78 guard stood down because the fd4io reload machine was non-idle, i.e. frames on
/// which the guard would have forced `GameMan+0xb78 = -1` and no longer does (bd er-effects-rs-9jbe).
/// This is the engagement oracle for that stand-down: a clean switch run proves only that nothing
/// regressed, whereas `> 0` proves the new condition actually fired against a live fd4io overlap --
/// the exact race (fd4io non-idle inside the guard's active window) that produced the black-screen
/// softlock. Published as `oracle_switch_b78_guard_standdowns`.
pub static SWITCH_RELOAD_B78_GUARD_STANDDOWNS: AtomicUsize = AtomicUsize::new(0);
pub static MOUNT_WAITS: AtomicUsize = AtomicUsize::new(0);
pub static WARM_KICK_FIRED: AtomicUsize = AtomicUsize::new(0);
pub static ORIG_PAD_POLL: AtomicUsize = AtomicUsize::new(0);
pub static PHASE_FRAME: AtomicUsize = AtomicUsize::new(0);
pub static ON_TOTAL: AtomicUsize = AtomicUsize::new(0);
pub static ON_MOVED: AtomicUsize = AtomicUsize::new(0);
pub static OFF_TAIL_TOTAL: AtomicUsize = AtomicUsize::new(0);
pub static OFF_TAIL_MOVED: AtomicUsize = AtomicUsize::new(0);
pub static NATIVE_OVERLAY_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static NATIVE_OVERLAY_FRAMES: AtomicUsize = AtomicUsize::new(0);
pub static NATIVE_OVERLAY_STAGE: AtomicUsize = AtomicUsize::new(0);
pub static LAST_LOADSCREEN_HITS: AtomicUsize = AtomicUsize::new(0);
pub static LOADSCREEN_GRACE: AtomicUsize = AtomicUsize::new(0);
pub static NATIVE_PROFILE_READ_PHASE: AtomicUsize = AtomicUsize::new(0);
pub static NATIVE_PROFILE_READ_LAST_POLL_STATUS: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static LOADLIST_INIT_CALLS: AtomicUsize = AtomicUsize::new(0);

// ---- migrated group: menu_trace_hooks, window_reconfig_observer, startup_modals_menu_cover, save_picker_menu, loading_cover_save_slot, system_quit_repro_guards, system_quit_hooks, profile_rows_system_quit_menu, title_scaleform_msgbox, stats_loading_text, lookat_bone_hooks, system_quit_dialog_handlers, effects, input_blocker, hooks, task_registration (107 counters) ----
pub static MMS_CHILD_CLEANUP_ORIG: AtomicUsize = AtomicUsize::new(0);
pub static MMS_STEP_INIT_ORIG: AtomicUsize = AtomicUsize::new(0);
pub static MMS_STEP_FINISH_ORIG: AtomicUsize = AtomicUsize::new(0);
pub static POPULATE_BLOCKS_LISTS_ORIG: AtomicUsize = AtomicUsize::new(0);
pub static POPULATE_BLOCKS_LISTS_CALLS: AtomicUsize = AtomicUsize::new(0);
pub static WORLDRES_ENTRY_CTOR_ORIG: AtomicUsize = AtomicUsize::new(0);
pub static WORLDRES_ENTRY_CTOR_1C_HITS: AtomicUsize = AtomicUsize::new(0);
pub static WORLDRES_BLOCKRES_GETTER_ORIG: AtomicUsize = AtomicUsize::new(0);
pub static WORLDRES_GETTER_LAST_1C: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static WORLDRES_CAPSTATE_DUMPED: AtomicUsize = AtomicUsize::new(0);
pub static BLOCKRES_PHASE2_ORIG: AtomicUsize = AtomicUsize::new(0);
pub static BLOCKRES_STALECAP_RETRIES: AtomicUsize = AtomicUsize::new(0);
pub static BLOCKRES_STALECAP_LAST_BRES: AtomicUsize = AtomicUsize::new(0);
pub static EBL_CENSUS_DONE: AtomicUsize = AtomicUsize::new(0);
pub static MOUNT_GUARD_DETECTOR_ORIG: AtomicUsize = AtomicUsize::new(0);
pub static MOUNT_GUARD_DET_LOGS_L1: AtomicUsize = AtomicUsize::new(0);
pub static MOUNT_GUARD_DET_LOGS_L2: AtomicUsize = AtomicUsize::new(0);
pub static MOUNT_GUARD_FLIP_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static MOUNT_GUARD_FLIP_LAST_TICK: AtomicUsize = AtomicUsize::new(0);
pub static MOUNT_GUARD_TICK: AtomicUsize = AtomicUsize::new(0);
pub static WINRECONFIG_CREATE_WINDOW_ORIG: AtomicUsize = AtomicUsize::new(0);
pub static WINRECONFIG_SET_WINDOW_POS_ORIG: AtomicUsize = AtomicUsize::new(0);
pub static WINRECONFIG_SET_WINDOW_LONG_ORIG: AtomicUsize = AtomicUsize::new(0);
pub static WINRECONFIG_MOVE_WINDOW_ORIG: AtomicUsize = AtomicUsize::new(0);
pub static WINRECONFIG_CHANGE_DISPLAY_ORIG: AtomicUsize = AtomicUsize::new(0);
pub static WINRECONFIG_CREATE_WINDOW_CALLS: AtomicUsize = AtomicUsize::new(0);
pub static WINRECONFIG_SET_WINDOW_POS_CALLS: AtomicUsize = AtomicUsize::new(0);
pub static WINRECONFIG_SET_WINDOW_LONG_CALLS: AtomicUsize = AtomicUsize::new(0);
pub static WINRECONFIG_MOVE_WINDOW_CALLS: AtomicUsize = AtomicUsize::new(0);
pub static WINRECONFIG_CHANGE_DISPLAY_CALLS: AtomicUsize = AtomicUsize::new(0);
pub static WINRECONFIG_LAST_SET_POS_SIZE: AtomicUsize = AtomicUsize::new(0);
pub static WINRECONFIG_LAST_SET_POS_FLAGS: AtomicUsize = AtomicUsize::new(0);
pub static WINRECONFIG_LAST_MOVE_SIZE: AtomicUsize = AtomicUsize::new(0);
pub static WINRECONFIG_LAST_CHANGE_DISPLAY_SIZE: AtomicUsize = AtomicUsize::new(0);
pub static WINRECONFIG_LAST_CHANGE_DISPLAY_FLAGS: AtomicUsize = AtomicUsize::new(0);
pub static WINRECONFIG_EARLY_APPLY_RESULT: AtomicUsize = AtomicUsize::new(0);
pub static WINRECONFIG_EARLY_APPLY_MS: AtomicUsize = AtomicUsize::new(0);
pub static WINRECONFIG_EARLY_APPLY_RECT: AtomicUsize = AtomicUsize::new(0);
pub static GR_SYSMSG_LOG_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static GR_SYSMSG_LOG_ORIG: AtomicUsize = AtomicUsize::new(0);
pub static GR_SYSMSG_LOG_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static CORRUPTED_SAVE_SEEN_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static NETWORK_CHECK_SHORTCIRCUIT_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static NETWORK_CHECK_SHORTCIRCUIT_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SHOW_PROGRESS_SHORTCIRCUIT_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static SHOW_PROGRESS_SHORTCIRCUIT_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SHOW_PROGRESS_TYPE_LOGGED: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_OPEN_MENU_SUPPRESS_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static TITLE_OPEN_MENU_SUPPRESSED_COUNT: AtomicUsize = AtomicUsize::new(0);
/// `TitleTopDialog::open_menu` calls the suppression detour let through.
///
/// The suppressed count alone cannot answer "did the native title ever open its menu again",
/// because a pass-through is invisible: the detour only logged the calls it dropped. That
/// ambiguity is what made the 2026-08-26 softlock unreadable from the log. Counting both sides
/// makes the question a subtraction.
pub static TITLE_OPEN_MENU_PASSTHROUGH_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Pass-throughs that happened after at least one suppression, i.e. after the missing-save hold
/// released.
///
/// **This is the decisive one.** If a late pick releases the hold and this stays 0, the native
/// title never re-issued `open_menu` and its rows can never be rebuilt with the save present --
/// the pick must then trigger the open rather than wait for a retry. Nonzero says the title does
/// retry on its own and the drop-and-retry model is sound.
pub static TITLE_OPEN_MENU_PASSTHROUGH_AFTER_SUPPRESS_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Boot default-save check vs. the container the runtime opens: 0 = not decided yet,
/// 1 = the accepted container is the one the runtime opens (or nothing was accepted and the
/// picker is armed, which is also correct), 2 = mismatch -- the check validated a file the
/// runtime will never read.
///
/// 2 is the 2026-08-26 failure: under Seamless the blank `ER0000.co2` was rejected, the check
/// fell back to `ER0000.sl2`, and reported "there is a save" while ersc.dll went on to open the
/// blank `.co2`. It cost two runs while being invisible in RAM. See
/// `er_save_redirect::boot_save_container_matches_runtime`.
pub static BOOT_SAVE_CONTAINER_MATCHES_RUNTIME: AtomicUsize = AtomicUsize::new(0);
pub static SCENE_OBJ_PROXY_CTOR_HITS: AtomicUsize = AtomicUsize::new(0);
/// 1 = an OS Save-As returned an existing file, so the Box3 overwrite confirm is owed.
///
/// A latch rather than a direct `SAVE_FLOW_STAGE` write: the menu thread must not become a second
/// writer of the stage (a filed defect the in-game arm already has). The save-flow tick consumes
/// this and performs the transition through `save_flow_enter_stage`, staying the sole owner.
pub static SAVE_DEST_CONFIRM_PENDING: AtomicUsize = AtomicUsize::new(0);
/// Browse rows with no character on which the hide of the per-slot info fields (`Level`
/// caption/value, `PlayTime`) was driven -- the native setter was called; pair with
/// `PROFILE_ROW_SLOT_INFO_NON_DISPLAY` to know it took effect. Doubles as the latch that arms the
/// symmetric re-show.
pub static PROFILE_ROW_SLOT_INFO_HIDDEN_ROWS: AtomicUsize = AtomicUsize::new(0);
/// Rows on which the re-show of the per-slot info fields was driven (row clips are reused).
pub static PROFILE_ROW_SLOT_INFO_SHOWN_ROWS: AtomicUsize = AtomicUsize::new(0);
/// Per-field visibility calls skipped fail-closed (child unresolved, or unexpected proxy vtable).
pub static PROFILE_ROW_SLOT_INFO_VIS_SKIPS: AtomicUsize = AtomicUsize::new(0);
/// Per-field visibility calls whose resolved GFx value was not a display object (setter no-ops).
pub static PROFILE_ROW_SLOT_INFO_NON_DISPLAY: AtomicUsize = AtomicUsize::new(0);
/// Summary populates left alone because the row proxy belongs to a movie this mod never edited --
/// the game's own System>Quit `GameEnd` panel is the one that matters. `CS::MenuSaveDataSummary`'s
/// populate is a shared template, so every surface that shows a character summary arrives at the
/// same hook; this counts the ones handed straight back to the game untouched.
pub static PROFILE_FOREIGN_SUMMARY_ROWS: AtomicUsize = AtomicUsize::new(0);
/// Summary populates recognised as our edited `05_010_ProfileSelect` row template (the probe field
/// resolved to a real GFx value). Pair with `PROFILE_FOREIGN_SUMMARY_ROWS`: the split is the whole
/// decoupling claim, and a zero here with a live ProfileSelect list means the probe is wrong.
pub static PROFILE_OWN_SUMMARY_ROWS: AtomicUsize = AtomicUsize::new(0);
/// Text pushes refused because the named child does not exist on that movie (the resolve came back
/// undefined). Before this existed those pushes were counted as successes -- SetText was called on a
/// self-linked empty proxy and reported 109k "successful" writes to a field the movie did not have.
pub static PROFILE_STATS_PUSH_MISSING_FIELD: AtomicUsize = AtomicUsize::new(0);
/// `MenuWindowJob::Run` passes observed for `05_010_ProfileSelect`. It ticks once per frame while
/// that window exists, so a rise between two samples means the view is on screen right now -- which
/// is the only question the live editor's safety gate needs answered.
pub static PROFILE_SELECT_WINDOW_RUN_TICKS: AtomicUsize = AtomicUsize::new(0);
/// Live-editor commands not applied from the asynchronous `FrameBegin` path because the ProfileSelect
/// view was rendering. They are left un-acked so the in-band row-populate path applies them instead.
/// Non-zero is the guard working, not an error.
pub static PROFILE_EDITOR_DEFERRED_APPLIES: AtomicUsize = AtomicUsize::new(0);
/// Times the per-slot stats/name caches were dropped because the save they described stopped being
/// the save on screen. They used to be a process-lifetime latch with no invalidation at all, so a
/// session's first save described every ProfileSelect row forever; non-zero means a swap was noticed.
pub static PROFILE_SLOT_CACHE_INVALIDATIONS: AtomicUsize = AtomicUsize::new(0);
/// Times those caches were refilled straight from bytes the picker already held (no second read).
pub static PROFILE_SLOT_CACHE_PREVIEW_RELOADS: AtomicUsize = AtomicUsize::new(0);
/// Last GFx value type seen by the row-field visibility path.
pub static PROFILE_ROW_SLOT_INFO_LAST_DATATYPE: AtomicUsize = AtomicUsize::new(usize::MAX);
/// Browse rows whose `PlayTime` was replaced with the file's last-saved timestamp.
pub static PROFILE_ROW_LAST_SAVED_ROWS: AtomicUsize = AtomicUsize::new(0);
/// Rows where the last-saved text could not be staged into the row model (field unreadable), so the
/// native playtime string stood.
pub static PROFILE_ROW_LAST_SAVED_STAGE_FAILURES: AtomicUsize = AtomicUsize::new(0);
pub static LAST_HITS: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_FACE_IDENTITY_CHECKS: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_FACE_IDENTITY_MISMATCHES: AtomicUsize = AtomicUsize::new(0);
/// Times the previewed save's `CS::ProfileSummary` records were put back after the game's
/// return-title save overwrote them, and the slot mask that write covered.
///
/// A switch to a foreign save that ends with zero here is a switch whose loading-screen portrait was
/// built from whatever the game left in the records -- which, when the picked slot is the resident
/// character's slot, is the previous character (measured run br-20260907-191016-4020). A non-zero
/// count with a mask that omits the picked slot is the same failure with a different cause.
pub static PROFILE_SUMMARY_REAPPLIED_AFTER_RETURN_TITLE: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_SUMMARY_REAPPLIED_SLOT_MASK: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_SAVE_SWAP_POLL_TICK: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_STATS_PREVIEW_ROW_CURSOR: AtomicUsize = AtomicUsize::new(0);
pub static TESTNET_FF_STUCK_FRAMES: AtomicUsize = AtomicUsize::new(0);
pub static TESTNET_FF_LAST_MMS: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static TESTNET_FF_FIRED_EPOCH: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static OPTIONSETTING_ROW_LAST_LOG_KEY: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static OPTIONSETTING_LAST_ACTIVE_TAB: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static SEAMLESS_TOS_SKIP_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static OPTIONS_02_040_QUIT6_RUNTIME_SERVES: AtomicUsize = AtomicUsize::new(0);
pub static OPTIONS_02_040_QUIT6_RUNTIME_FAILURES: AtomicUsize = AtomicUsize::new(0);
pub static STATS_TEXT_SCREEN_VERSION: AtomicUsize = AtomicUsize::new(0);
pub static STATS_TEXT_BUILT: AtomicUsize = AtomicUsize::new(0);
/// Loading-screen stats panel reads that were declined because the slot's live
/// `CS::ProfileSummary` record is not a character (telemetry oracle
/// `oracle_stats_record_not_a_character`; cumulative, never reset).
///
/// The point of this counter is that a blank panel is ambiguous. When the in-game save picker's
/// browse-row labels were left in the live records, the panel rendered `[..] EldenRing` / `[ new ]`
/// beside `RL 0` -- and once it correctly refuses to draw that, the screen looks exactly like a
/// build with the stats feature switched off. A non-zero here says "we saw a record and refused
/// it"; a zero alongside `oracle_stats_text_built > 0` says the feature ran and every record it
/// read was a character. Nothing else in the telemetry can tell those two apart.
pub static STATS_RECORD_NOT_A_CHARACTER: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_OFFSCREEN_SETTLE_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static MODEL_WAS_LIVE: AtomicUsize = AtomicUsize::new(0);
pub static RETURN_DESKTOP_CONTROLLER_DIAG: AtomicUsize = AtomicUsize::new(0);
