//! Destination for the ~900 telemetry atomic counters/latches being inverted
//! out of the product's `experiments/*` + `constants/*` trees.
//!
//! OWNERSHIP INVERSION (in progress): today these atomics are DEFINED in the
//! product and telemetry merely mirrors them through `crate::*` glob imports.
//! The target state is that they are DEFINED here (`pub` statics) and the
//! product write-sites reference `er_telemetry_core::counters::X`, so telemetry never
//! reaches up into product for state.
//!
//! This module currently holds only the counters that the standalone read-side
//! tick needs; the bulk migration (own_load / move_probe / rawinput / profile /
//! depth families) lands file-group by file-group per the plan's Step 3.

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, AtomicU64, AtomicUsize, Ordering};

mod cover_predicates;

pub use cover_predicates::{cover_owns_current_loading_screen, title_visual_suppression_active};

/// Number of standalone read-side ticks that have executed (proves the game-thread
/// callback is live in the telemetry-only DLL). Owned here from the start.
pub static STANDALONE_TICKS: AtomicU64 = AtomicU64::new(0);

// ---- migrated group: present_overlay (23 counters) ----
pub static PRESENT_ORIG: AtomicUsize = AtomicUsize::new(0);
pub static PRESENT1_ORIG: AtomicUsize = AtomicUsize::new(0);
pub static PRESENT_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static PRESENT_HOOK_HITS: AtomicUsize = AtomicUsize::new(0);
/// Microseconds spent INSIDE the last original IDXGISwapChain::Present/Present1 call (measured in the
/// present detour). Discriminates a present-BLOCK (compositor/vsync throttle => ~40ms) from a real
/// CPU/GPU per-frame WORK stall (present fast ~1-2ms but the frame is still 50ms). bd
/// FOCUS-AB-falsifies-unfocused-throttle...next-present-duration-2026-07-21.
pub static PRESENT_CALL_LAST_US: AtomicUsize = AtomicUsize::new(0);
/// The `SyncInterval` argument the GAME passes to its own Present(this, SyncInterval, Flags) call,
/// latched in the present detour. DECISIVE for the reload 20fps: SyncInterval=3 => the game DELIBERATELY
/// requests present-every-3rd-vblank (a 20fps loading/low-priority throttle); =1 while frames are still
/// 3 vblanks apart => the game requests 60 but the GPU cannot keep up (render-bound). 0 = no-vsync.
/// bd GPU-timestamp-semaphore-split-reload-20fps-residual-2026-07-22.
pub static PRESENT_SYNC_INTERVAL_LAST: AtomicUsize = AtomicUsize::new(usize::MAX);
/// From IDXGISwapChain::GetFrameStatistics on the GAME swapchain: display-refreshes elapsed per present,
/// x100 (ratio ΔSyncRefreshCount/ΔPresentCount). ~300 (=3.00) on a 20fps flip-model reload means the
/// swapchain is vsync-locked to every 3rd vblank; ~100 (=1.00) means one present per vblank. 0 = no
/// stats yet / DISJOINT. Companion to PRESENT_SYNC_INTERVAL_LAST (requested) -- this is the OBSERVED
/// cadence. bd GPU-timestamp-semaphore-split-reload-20fps-residual-2026-07-22.
pub static PRESENT_REFRESH_PER_PRESENT_X100: AtomicUsize = AtomicUsize::new(0);
/// Wall-clock microseconds between the last two GetFrameStatistics SyncQPCTime samples (present-to-present
/// spacing straight from DXGI, independent of our Instant timing). ~49920 on the pinned reload frame.
pub static PRESENT_QPC_DELTA_US: AtomicUsize = AtomicUsize::new(0);
/// Per-frame GPU-busy time in MICROSECONDS: the median-of-recent span between two D3D12 TIMESTAMP
/// queries the DLL injects onto the GAME's ID3D12CommandQueue -- START on the first ExecuteCommandLists
/// after a present, END at the top of the Present detour (before the original Present). Excludes the
/// vsync/flip present-wait (that happens INSIDE the original Present, after the END stamp), so a large
/// value == render-bound (GPU genuinely busy ~50ms) while a small value with a 50ms frame == a
/// present/vblank throttle. This is the goal-doc §3.3 `gpu_frame_us` oracle, splitting the reload-20fps
/// residual into GPU-render vs present-wait. bd er-effects-rs-03ma /
/// switch-reload-framerate-parity-acceptance.md.
pub static GPU_FRAME_US_LAST: AtomicUsize = AtomicUsize::new(0);
/// Count of successful GPU-timestamp readbacks (each = one resolved START/END pair). Emitted as
/// `oracle_gpu_frame_samples` so a `gpu_frame_us == 0` is attributable: 0 samples == the oracle never
/// produced (queue not latched / D3D12 setup failed / not under Wine), NOT "GPU is instant".
pub static GPU_FRAME_ORACLE_SAMPLES: AtomicUsize = AtomicUsize::new(0);
/// GPU-timestamp oracle lifecycle state (emitted `oracle_gpu_frame_state`): 0=not started,
/// 1=game device + query heap/list/readback created, 2=game ExecuteCommandLists hooked + queue latched,
/// 3=producing (at least one full START..END pair resolved). Distinguishes WHERE setup stopped when
/// `gpu_frame_us` stays 0.
pub static GPU_FRAME_ORACLE_STATE: AtomicUsize = AtomicUsize::new(0);
/// Internal previous-sample state for the GetFrameStatistics deltas (not emitted): last PresentCount,
/// last SyncRefreshCount, last SyncQPCTime (QPC ticks, low bits).
pub static PRESENT_STATS_PREV_PRESENT_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static PRESENT_STATS_PREV_SYNC_REFRESH: AtomicUsize = AtomicUsize::new(0);
pub static PRESENT_STATS_PREV_QPC: AtomicU64 = AtomicU64::new(0);
/// Microseconds spent in the DLL's boot-view composite (composite_on_game_swapchain) in the present
/// detour, BEFORE the original Present. If this is ~tens of ms in-world on reloads it is the per-frame
/// WORK stall (present_call_us is fast but the composite is invisible to it, yet counts in the
/// present-to-present frame time). bd PRESENT-FAST-work-stall...dll-bootview-composite-2026-07-22.
pub static COMPOSITE_LAST_US: AtomicUsize = AtomicUsize::new(0);
/// Microseconds spent in the DLL's MAIN recurring game-task body (FrameBegin) last frame. Splits a
/// DLL per-frame CODE cost (large on reloads => our bug) from a game-side loop cost (fast => game/env).
/// bd CORRECTION-scan-fix-didnt-recover...suspect-moveprobe-2026-07-22.
pub static GAME_TASK_LAST_US: AtomicUsize = AtomicUsize::new(0);
/// Free-running count of MAIN recurring game-task bodies entered, readable FROM ANY THREAD.
///
/// `EffectsState::game_task_ticks` already counts this, but it lives behind the state mutex and is
/// only observable through a telemetry write the game task itself performs -- so it can answer "how
/// many ticks happened" only for as long as the task is alive to report it, which is precisely when
/// the question is uninteresting. A thread that needs to know whether the game task is STILL RUNNING
/// (the boot picker, which blocks for as long as a user browses) cannot use it: taking the mutex is
/// the one thing that can block forever if the task froze while holding it.
///
/// Measured need, run pr109-boot-oscancel-20260730-110704: the task reached tick 60 at +16.9s and
/// then stopped for the remaining 17s of the run. Nothing in the telemetry said so -- the file simply
/// stopped changing, which is indistinguishable from a file nobody looked at.
pub static GAME_TASK_TICKS_TOTAL: AtomicUsize = AtomicUsize::new(0);
/// Microseconds in the DLL build-driver FrameBegin task (maybe_register_stats_panel_textures +
/// force_profile_render_tick) last frame -- the last untimed DLL per-frame task. bd
/// SWEEP-DIAG-CHEAP-last-dll-suspect-is-build-driver-2026-07-22.
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
// ..."), but both were still PRINTED every `lookat-phase-sweep` line as `defer_some=`/`defer_nonblack=`.
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
/// The slot THIS loading-screen window committed its portrait to, +1 (0 == not yet committed).
/// Latched at the window's first slot resolution and held until the window closes, so the face on
/// screen cannot change character mid-load. See `er_loading_portrait_core::portrait_window_target_slot`.
pub static PORTRAIT_WINDOW_TARGET_SLOT: AtomicUsize = AtomicUsize::new(0);
/// Times the freshly-resolved target DISAGREED with what this window already committed to, i.e.
/// retargets that were suppressed. Each one is a mid-load face change the user did not see.
/// Nonzero proves the latch is load-bearing; it was 1 in the 2026-08-02 21:05 repro (slot 0 -> 9).
pub static PORTRAIT_WINDOW_RETARGETS_SUPPRESSED: AtomicUsize = AtomicUsize::new(0);
/// Times a window latch adopted from a GUESS was promoted to the user's explicit pick.
///
/// The window latch exists so a committed face cannot change mid-load, but it was committing to
/// whatever the boot autoload guessed before the user had picked anything -- and then refusing the
/// pick as a "mid-window retarget". Measured 2026-08-26: latched slot 0 at +1061ms with
/// `picker=None b78=None ac0=-1`, user picked slot 1 eighteen minutes later, retarget suppressed,
/// and the loading screen showed slot 0's character. Exactly one promotion per window is possible
/// (a latch that came FROM the pick never yields), so this counts windows the pick rescued.
pub static PORTRAIT_WINDOW_TARGET_PICK_PROMOTIONS: AtomicUsize = AtomicUsize::new(0);
/// Whether this window's latched portrait target came from the user's explicit pick (1) or from a
/// guess (0). Reset with `PORTRAIT_WINDOW_TARGET_SLOT` on window close.
pub static PORTRAIT_WINDOW_TARGET_FROM_PICK: AtomicUsize = AtomicUsize::new(0);
/// WHICH source this window's latch rests on, as `PortraitSlotSource::rank()`: 0 = not committed,
/// 1 = `GameMan.save_slot` (ac0), 2 = the `GameMan+0xb78` load-REQUEST register, 3 = the user's
/// explicit pick. Reset with `PORTRAIT_WINDOW_TARGET_SLOT` on window close.
///
/// `PORTRAIT_WINDOW_TARGET_FROM_PICK` is the top rank collapsed to a bit, and that collapse is
/// what hid bd `er-effects-rs-fmy6`: a latch taken off a STALE `ac0` and a latch taken off the
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
/// copy, the readback and the publish attempt. This is the tick that actually FEEDS the publish
/// path, so its per-window delta is the honest "did the portrait pipeline run this window" number.
/// Distinct from [`PROFILE_DRIVE_FRAMES_WINDOW`], which counts the separate POSE drive (model
/// update task + per-frame push) and is gated behind `off_resources_ready`.
pub static PROFILE_RENDER_DRIVE_HITS_WINDOW_MARK: AtomicUsize = AtomicUsize::new(0);
/// Window mark for [`PORTRAIT_PUMP_BLOCK_OFF_RESOURCE`]. Its per-window delta is the ATTRIBUTION for
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
/// PUBLISHED-vs-LOADED semaphore (bd er-effects-rs-qoqc defect 6 / er-effects-rs-91zb). The
/// pre-existing identity semaphore compared our TARGET slot against the currently-resident
/// character, which is silent about the failure that actually reached the screen: on 2026-08-02
/// slot 9's face was published and displayed for 29.7s while slot 5 loaded, and every oracle said
/// ok. These compare what was PUBLISHED against the slot whose load actually COMPLETED, asserted
/// at every loading-window close (`PORTRAIT-LOADWIN VERDICT`). Both must stay 0.
pub static PORTRAIT_PUBLISHED_SLOT_MISMATCHES: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_PUBLISHED_NAME_HASH_MISMATCHES: AtomicUsize = AtomicUsize::new(0);
/// Number of loading windows whose published-vs-loaded identity was actually CHECKED. A run with
/// 0 mismatches and 0 checks proved nothing -- read this before believing the two counters above.
pub static PORTRAIT_PUBLISHED_IDENTITY_CHECKS: AtomicUsize = AtomicUsize::new(0);
/// The slot whose fresh deserialize COMPLETED, as slot+1 (0 = none this process yet). Written at
/// each `SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_DONE = 1` site, all of which know their slot.
/// This is the "which character actually loaded" ground truth the publish check compares against;
/// `GameMan.save_slot` is not (both the game and our own code write it for other reasons).
pub static SYSTEM_QUIT_FRESH_DESER_DONE_SLOT: AtomicUsize = AtomicUsize::new(0);
/// Name-hash of the slot the portrait pipeline currently TARGETS, stamped on the game thread at the
/// per-slot build kick (the consume worker may not read game memory, so it copies this atomic into
/// `LS_PORTRAIT_PUBLISHED_NAME_HASH` at publish). 0 = unknown/never kicked this window.
pub static PORTRAIT_TARGET_NAME_HASH: AtomicUsize = AtomicUsize::new(0);
/// Boot-view-epoch ms of the last switch confirm (RETARGET); consumed (swap 0) by the first publish
/// after it to compute `PORTRAIT_CONFIRM_TO_PUBLISH_MS_LAST`. 0 = no confirm pending.
pub static PORTRAIT_CONFIRM_MS: AtomicUsize = AtomicUsize::new(0);
/// ms from the last switch confirm (RETARGET) to the NEXT portrait publish (version bump); keeps the
/// last measured value (oracle_portrait_confirm_to_publish_ms). 0 = never measured.
pub static PORTRAIT_CONFIRM_TO_PUBLISH_MS_LAST: AtomicUsize = AtomicUsize::new(0);
/// Same-identity bridge holds across an own-menu-switch rearm (bd er-effects-rs-dpf6 Phase 3): the
/// incoming slot+name-hash matched the published head, so the window reset KEPT the bridge.
pub static PORTRAIT_BRIDGE_SAME_IDENTITY_HOLDS: AtomicUsize = AtomicUsize::new(0);
/// The OUTSTANDING provisional bridge hold, as slot+1 (0 = none). A hold is taken at the switch
/// rearm on a name-hash comparison whose two operands both come from the SAME ProfileSummary
/// record, so it cannot detect that the record itself is wrong (2026-08-22, see
/// `same_identity_bridge_hold`). It is therefore recorded as PROVISIONAL and stays that way until
/// something independent resolves it: this window's own publish clears it (proof), a
/// face-fingerprint mismatch revokes it (refutation), and reaching the NEXT rearm still set means
/// neither ever happened.
pub static PORTRAIT_BRIDGE_HOLD_PROVISIONAL: AtomicUsize = AtomicUsize::new(0);
/// Provisional holds REVOKED by the record-vs-preview face fingerprint -- the one portrait identity
/// signal that compares the record against a source outside itself. A revocation drops the held head
/// and the frozen crop envelope, so the window shows NO portrait rather than the previous
/// character's. `> 0` is a defect signal about the RECORD, not a healthy safety check firing: an
/// intact record cannot produce one (bd k979 -- do not gate on this being non-zero).
pub static PORTRAIT_BRIDGE_HOLD_REVOCATIONS: AtomicUsize = AtomicUsize::new(0);
/// Provisional holds that reached the NEXT switch rearm having neither published nor been revoked:
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
/// THE LATCH THIS EXISTS TO BREAK (measured 2026-09-04, run br-20260904-181251-0586). The only
/// path back to `PHASE_IDLE` is the post-finish stable-proof block, and it is gated on
/// `phase >= AUTOLOAD_HANDOFF (4)`. A switch that reaches `TITLE_OWNER_SEEN (3)` and is then torn
/// down never advances to 4, so it can never reach that reset: `active_switch` stays true for the
/// rest of the process and the load-job Run guard never lifts. Observed effect -- `Load Character
/// from File` becomes a silent no-op FOREVER: the row resolves, the picker opens, the ProfileSelect
/// activation is ALLOWED, and then the log says `forwarding native (load-job Run remains guarded)`
/// while two WHY-NOT lines spin for the rest of the session naming `active_switch=true(phase=3)`.
/// Meanwhile SWITCH-ORACLE reported a perfectly healthy world: `player=true ig_d8=1 pstep=7/7`.
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
// AND-term of `oracle_title_loaded_character_portrait_rendered`, whose second and third terms are
// already pinned `false`/`0`, so that oracle was STRUCTURALLY incapable of being true and has been
// removed with it rather than left emitting a permanent `false`.
pub static PAB_RUN_POST_CALLS: AtomicUsize = AtomicUsize::new(0);
// TITLE_OVERLAY_COVER_* removed 2026-07-31: six counters with zero writers, read once each to emit
// oracles for the unbuilt custom title render surface (er-effects-rs-trp). A permanently-0 counter
// cannot be distinguished from a feature that ran and did nothing, so they reported an ABSENT
// feature as a FAILING one. Re-add with writers at the real render site when trp lands.
// LOADING-SCREEN OBSERVER INSTALL STATE (2026-08-30). These are not booleans any more: 0 = not
// attempted yet, 1 = installed, 2 = permanently refused, 3 = created and queued, waiting on
// MH_ApplyQueued. The third value is the one that had to exist. Five observers used to share one
// install flag, so a counter reading 0 could equally mean "installed and the game never called it"
// or "never installed at all", and the run where four detours were created but never applied read
// exactly like a quiet loading screen. See `install_now_loading_helper_observer_hooks`.
//
// `NOW_LOADING_HELPER_HOOKS_INSTALLED` is the AGGREGATE, and it is also the caller's poll gate:
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
/// Fade-outs played on the LOADING SCREEN'S OWN clip (`LOADING_SCREEN_LAST_THIS +
/// LOADING_SCREEN_FADEOUT_CLIP_OFFSET`). This is the signal that decides whether the custom cover
/// may start its release fade, so it has to mean the game's loading screen and nothing else.
///
/// It did not, until 2026-09-05. The `Scaleform label goto` detour stamped here for ANY movie
/// whose timeline hit a label containing "fadeout", and 98 of the 106 vanilla menu `.gfx` files
/// carry one -- including `02_000_ingametop.gfx`, the pause menu. In run br-20260905-221201-969c
/// ALL 129 stamps were foreign: not one `this` matched the loading screen's clip, so the cover was
/// being held open by ordinary menu transitions and stayed up ~15s after the world was playable.
pub static LOADING_SCREEN_GFX_FADEOUT_HITS: AtomicUsize = AtomicUsize::new(0);
/// Fade-out labels the same detour saw on SOME OTHER movie and refused to count above.
///
/// Two jobs, both of which the narrowed counter alone cannot do. It is the detour's LIVENESS
/// proof: `LOADING_SCREEN_GFX_FADEOUT_HITS == 0` on its own cannot tell "the loading screen never
/// faded" from "the hook is dead", and a nonzero value here settles it. And it is the size of the
/// noise that used to be counted as the loading screen's fade -- the number that was 129 in the
/// reproducing run.
pub static LOADING_SCREEN_GFX_FADEOUT_FOREIGN_HITS: AtomicUsize = AtomicUsize::new(0);
/// Armed at every own-menu switch: this cover window must span TWO native loading screens, not one.
///
/// THE FLOW THE GAME DOES NOT NATIVELY DO (user report 2026-09-05, measured on
/// br-20260905-234626-ce9a). A System->Quit->Load Character switch shows the native loading plate
/// TWICE -- once while the OUTGOING world is torn down, once while the incoming character loads --
/// and the two are distinguishable in the game's own bar:
///
/// ```text
///   REARM   +164497ms   epoch 1
///   screen 1 (0xb2710080)  frame 1/500 for its whole life, FINISH at frame 1/500   <- the unload
///   screen 2 (0xab408c80)  frame 61 -> 421 -> FINISH at frame 500/500              <- the load
/// ```
///
/// The cover is supposed to own the screen from the first plate coming up to the second fading out.
/// It did not: it released 682 ms after the arm, 350 ms BEFORE screen 1 even opened, and the user
/// watched 12.9 s of bare native loading screen. Same shape on the third switch (689 ms, 8.2 s bare).
///
/// So the release is gated on the character load's own screen having finished. That was first
/// written as `LOADING_SCREEN_CLOSE_SENT_HITS >= 2` -- the ordinal -- which the user's actual
/// ProfileSelect path (ONE plate, already the load's) could never satisfy; it now reads
/// `LOADING_SCREEN_COMPLETED_CLOSE_HITS >= 1`, the same instant on the two-plate shape above and a
/// reachable one on the single-plate shape. 0 = boot (one screen only, no gate).
pub static BOOT_VIEW_RELEASE_REQUIRE_SECOND_SCREEN: AtomicUsize = AtomicUsize::new(0);
/// Frames the release was held because the second native loading screen had not finished yet. The
/// direct measure of the defect above: it was 0 on every switch of br-20260905-234626-ce9a, because
/// nothing was holding.
pub static BOOT_VIEW_RELEASE_HELD_FOR_SECOND_SCREEN: AtomicUsize = AtomicUsize::new(0);
/// Mirror of `LOADING_SCREEN_COMPLETED_CLOSE_HITS` as the gate last read it: COMPLETED native
/// loading screens (gauge at 500/500 when they finished) in this cover window. 0 = nothing has
/// loaded a world yet, only teardown plates; 1 = the character load's plate has faded, which is the
/// moment the cover is allowed to let go.
pub static BOOT_VIEW_NATIVE_SCREENS_SEEN: AtomicUsize = AtomicUsize::new(0);
/// Boot-view-epoch ms of the LAST clean portrait publish, i.e. the last frame on which the head the
/// user is looking at actually changed. 0 = none published in this window.
///
/// WHY A TIMESTAMP AND NOT ANOTHER COUNT. The window-reset line already reports how MANY frames were
/// published; what it cannot say is WHEN the last one landed, and that is the whole question behind
/// "does the portrait animate for as long as it is on screen". On br-20260906-000112-021a the
/// portrait-motion oracle samples every ~4.5 s and its last line for the switch window is +114405ms
/// while the cover did not stop until +118876ms -- so the final 4.5 s, including the entire release
/// fade, had no evidence either way. Paired with `BOOT_VIEW_STOP_MS` this turns that blind spot into
/// a subtraction.
pub static PORTRAIT_LAST_PUBLISH_MS: AtomicUsize = AtomicUsize::new(0);
/// Boot-view-epoch ms of the last frame the portrait DRAW TICK ran. Distinguishes "the pipeline
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
/// Native loading screens that finished with the gauge AT ITS TERMINAL FRAME, this cover window.
///
/// The discriminator `LOADING_SCREEN_CLOSE_SENT_HITS` is missing. That one counts plates; this one
/// counts plates THAT FILLED. The distinction is already written down in
/// `BOOT_VIEW_RELEASE_REQUIRE_SECOND_SCREEN`'s own table -- the unload plate finishes at
/// `frame 1/500`, the character load's finishes at `frame 500/500` -- but the gate read the count
/// rather than the frame, so it could only express "the second one" and not "the one that loaded a
/// world".
///
/// WHAT THAT COST, measured on this run (er-quickload-autoload-debug.log, 2026-09-06). The user's
/// ProfileSelect switch path shows exactly ONE plate, not two: `loadscreen_builds` went 1 -> 2 and
/// 2 -> 3 across the two switches, and each window's single finish reported `frame=500/500`. So
/// `screens < 2` held forever, `world_handoff=false` in every DECISION line of both windows, and
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
/// Bitmask (bit N = save slot N) of slots the per-slot cache NAMED but could not decode STATS for.
///
/// This is the semaphore for a Load Character row that renders its header and nothing else. The
/// two caches already disagreed in the log (`9/10 slots decoded, 10/10 names decoded`, run
/// `br-20260901-161521-9f7d`) and no oracle carried the disagreement, so a row with a name, no
/// attribute line and no `WL` reached the user as a visual observation. Non-zero is BAD and names
/// exactly which rows are affected; the count alone could not, because `decoded < named` does not
/// say which slot lost its stats.
pub static PROFILE_SLOT_STATS_NAMED_WITHOUT_STATS_MASK: AtomicUsize = AtomicUsize::new(0);
/// Bitmask (bit N = save slot N) of live `CS::ProfileSummary` slots found marked OCCUPIED while
/// holding something that is not a character, at a moment when no save picker owned the rows.
///
/// This is the RAM signature of `er-effects-rs-fmy6`. The in-game picker renders its browse rows by
/// writing them into these game-owned records, and every exit is supposed to put the real ones
/// back; when one did not (the sticky-`committed` defect, 2026-08-29) the labels stayed, and the
/// user's next loading screens showed `[..] EldenRing` and `[ new ]` as character names beside
/// `RL 0`. That reached the user as something they SAW, while `oracle_stats_text_slot_decoded` and
/// `oracle_profile_player_name_slot_decoded` were both already published and nothing compared them.
///
/// STICKY BY DESIGN (`fetch_or`, never reset): the per-frame sweep heals an orphaned stomp within a
/// frame, so a counter that could be cleared would read 0 in the very run that proved the defect.
/// Non-zero is a DEFECT, not a state, and it names exactly which rows were affected.
pub static PROFILE_SUMMARY_ORPHANED_RECORD_MASK: AtomicUsize = AtomicUsize::new(0);
/// Cumulative samples the orphaned-record scan judged. Read `PROFILE_SUMMARY_ORPHANED_RECORD_MASK`
/// WITH this: a zero mask means "checked and clean" only when this is non-zero, and "never checked"
/// otherwise -- the distinction a bare mask of 0 cannot make, and the one that turns a silent
/// oracle into false assurance.
pub static PROFILE_SUMMARY_ORPHANED_RECORD_SCANS: AtomicUsize = AtomicUsize::new(0);
/// 1 once the live record table has been seen holding at least one real character, i.e. the boot
/// `CS::ProfileSummary::Deserialize` has run and the bytes are worth judging.
///
/// A LATCH, NOT A PER-SAMPLE TEST, and that is the whole point. The allocation exists long before
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
/// The APPLIED orbit camera of the last `apply_profile_camera_override` -- the values actually written
/// into the renderer (engine baseline * the PROFILE_CAM_*_SCALE/DELTA transform), not the baseline.
///
/// WHY these exist (2026-08-21): the portrait camera was believed to be identical for every character,
/// because the engine baseline is read from `MenuOffscrRendParam` row `DAT_143b39858[slot * 0x20]` and
/// that row id is 20 for ALL TEN slots (dumped from `eldenring-deobf.bin`, RVA 0x3b39848, stride 0x20).
/// Believed, but never CONFIRMED FROM A RUN: no oracle reported a single camera VALUE, so an artifact
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
/// ->Load-Character switch ARMS, so a reload can be scored on its OWN `CS::MessageBoxDialog`
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
/// Consecutive frames the reloaded world has been genuinely LIVE (play_time advancing). Once high enough,
/// the child-done-query override RELEASES the held MoveMapStep child so it tears down like vanilla (the
/// override only needs to prevent PREMATURE teardown DURING the load; post-stabilization it must let go, or
/// it strands the child alive forever = the ez10-set + ~4fps steady-state divergence). bd
/// CORRECTION-STEP4-finalize-substate-is-0.
pub static WORLD_LIVE_STABLE_FRAMES: AtomicUsize = AtomicUsize::new(0);
// ---- PHASE-3 OUTGOING-WORLD TEARDOWN (bd PHASE3-render-release-is-CommonFinalize-...-2026-07-23) ----
/// One-shot install guard for the observe-only `CS::InGameStep::_Common_Finalize` hook.
pub static COMMON_FINALIZE_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
/// Count of native `_Common_Finalize` invocations (the world render-release that frees GLOBAL_WorldChrMan,
/// CSDistViewManager, g_GxDrawContext, WorldRes area lists, FieldArea, ...). This is THE teardown oracle:
/// on the broken in-place switch it stays flat across a reload (0 finalizes); the Phase-3 fix routes the
/// OUTGOING world through this release so it increments once per switch (like a native quit->Continue).
/// Exposed as `oracle_common_finalize_count` (distinct from `oracle_switch_teardown_count`, which merely
/// counts our menuData+0x5d ARM writes).
pub static COMMON_FINALIZE_CALLS: AtomicUsize = AtomicUsize::new(0);
/// `COMMON_FINALIZE_CALLS` captured at switch-arm, so the reload gate can detect the OUTGOING finalize.
pub static OUTGOING_TEARDOWN_BASELINE: AtomicUsize = AtomicUsize::new(0);
/// Latched 1 once the OUTGOING world's `_Common_Finalize` was observed for the current switch (before the
/// reload's continue_confirm), i.e. the pre-quit world was released so the rebuild starts fresh.
pub static OUTGOING_TEARDOWN_DONE: AtomicUsize = AtomicUsize::new(0);
/// Frames own_load_switch_reload_fire has held continue_confirm waiting for the OUTGOING finalize.
pub static OUTGOING_TEARDOWN_WAIT_TICKS: AtomicUsize = AtomicUsize::new(0);
/// Latched 1 when the bounded wait for the OUTGOING finalize expired -> fail-soft to the OLD in-place
/// reload (the two holds re-engage to protect the reused world). Keeps the fix from ever softlocking.
pub static OUTGOING_TEARDOWN_FAILSOFT: AtomicUsize = AtomicUsize::new(0);
// ---- WORLDRESWAIT streaming-settle HOLD (bd reload-overlap-fix-design-worldreswait-defer-release-on-
//      streaming-settle-2026-07-24) -- the armed switch-reload movable-while-streaming dip fix. A hook on
//      CS::MoveMapStep::STEP_WorldResWait's residency predicate FUN_140624bd0 (deobf 0x624bd0; that step
//      is its SOLE code caller) defers STEP_WorldResWait's player warp + step advance (i.e. the coupled
//      movability/loading-close release) until CS::CSWorldGeomMan geometry streaming settles, scoped to
//      the System-Quit switch reload ONLY. Bounded fail-soft; never writes WorldBlockRes phase/gate bytes. ----
/// One-shot install guard for the STEP_WorldResWait gate (FUN_140624bd0) defer-release hook.
pub static WORLDRESWAIT_GATE_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
/// Total gate-hook invocations (per-frame during any world load). Telemetry oracle_worldreswait_gate_calls.
pub static WORLDRESWAIT_GATE_HOOK_CALLS: AtomicUsize = AtomicUsize::new(0);
/// Per-switch ARM latch (1 == this switch reload's WorldResWait release should be held). Set by
/// `arm_worldreswait_hold()` from `own_load_switch_reload_fire` (switch-only, marker-gated), cleared per
/// switch by `reset_worldreswait_hold_latches()` and on release. On boot/load1 it is never set, so the
/// gate hook is a pure passthrough there (the anti-softlock crux).
pub static WORLDRESWAIT_HOLD_ARMED: AtomicUsize = AtomicUsize::new(0);
/// Per-switch latch: WorldBlockRes residency was reached while armed (so the gate's ONE legit
/// `FUN_14066d610` residency-pop already ran). Once set, the hook stops calling the original (no repeat
/// pop / no repeat pending-vector erase) and holds on geometry-settle instead.
pub static WORLDRESWAIT_RESIDENCY_SEEN: AtomicUsize = AtomicUsize::new(0);
/// Frames since residency was reached (the hold window length); bounds the fail-soft cap.
pub static WORLDRESWAIT_HOLD_WAIT_TICKS: AtomicUsize = AtomicUsize::new(0);
/// Consecutive frames CS::CSWorldGeomMan reported settled (for the K-frame sustain before release).
pub static WORLDRESWAIT_SETTLE_STREAK: AtomicUsize = AtomicUsize::new(0);
/// Run-cumulative outcome telemetry: 1 == the hold engaged (residency seen while armed) at least once.
pub static WORLDRESWAIT_HOLD_ENGAGED: AtomicUsize = AtomicUsize::new(0);
/// Run-cumulative: total frames the gate hook returned not-ready to DEFER the release (hold length).
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
/// row. Same shape as the two rows above: recorded so a run can prove the row was BUILT, never used
/// as the row identity (which is the list cursor, and only the list cursor).
pub static SYSTEM_QUIT_LOAD_BUILD_URL_ACTION_LAST_OBJECT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_LOAD_BUILD_URL_CONTROLLER_LAST_OBJECT: AtomicUsize = AtomicUsize::new(0);
/// Row presses, and the subset of them actually handed to `er-build-import-runtime`.
/// `REQUEST_COUNT < ACTION_COUNT` means presses were refused -- no `build_url` configured, an
/// import already in flight, or a link with no `?b=<id>` -- and REFUSED_COUNT says how many.
pub static SYSTEM_QUIT_LOAD_BUILD_URL_ACTION_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_LOAD_BUILD_URL_REQUEST_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Presses the runtime refused OUTRIGHT -- no `build_url` configured, an import already in flight,
/// or a link with no `?b=<id>`. Synchronous: the press itself came back with this.
pub static SYSTEM_QUIT_LOAD_BUILD_URL_REFUSED_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Accepted requests that then FAILED asynchronously (fetch error, unparseable payload, a build
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
/// Accepts the gate REFUSED. Each one re-opened the field rather than applying anything, so this
/// rising while IMPORTED does not is the feature working, not failing.
pub static SYSTEM_QUIT_LOAD_BUILD_URL_REJECTED_COUNT: AtomicUsize = AtomicUsize::new(0);
/// `UrlRejection::code()` of the most recent refusal (`0` = none yet).
pub static SYSTEM_QUIT_LOAD_BUILD_URL_LAST_REJECTION: AtomicUsize = AtomicUsize::new(0);
// ---- the Generate Build Link row: the INVERSE of everything above -----------------------------
// That row takes a link and rewrites the character; this one takes the character and writes a link.
// It touches no game state at all, so it has no "applied" counter -- what it has instead is a
// separate count for each of the three things that can independently succeed or fail once the URL
// exists: encoding it, putting it on the clipboard, and getting a browser to open it. A run where
// the URL was built but no browser appeared is a DIFFERENT failure from one where the read came
// back empty, and summing them would hide which.
/// Recorded cloned action object and `PropertyNewButtonController` for the row. Telemetry only:
/// the row identity is the list cursor, here as everywhere else on this tab.
pub static SYSTEM_QUIT_GENERATE_BUILD_LINK_ACTION_LAST_OBJECT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_GENERATE_BUILD_LINK_CONTROLLER_LAST_OBJECT: AtomicUsize =
    AtomicUsize::new(0);
/// Row presses, and the subset that actually claimed the exporter.
pub static SYSTEM_QUIT_GENERATE_BUILD_LINK_ACTION_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SYSTEM_QUIT_GENERATE_BUILD_LINK_REQUEST_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Presses refused because an export was genuinely still running. A refusal that could NOT be
/// proven live is not counted here -- it is counted below as a stale latch and the press proceeds.
pub static SYSTEM_QUIT_GENERATE_BUILD_LINK_REFUSED_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Presses that found a busy flag with no worker behind it and cleared it. This rising is the
/// safety valve working: a dead latch must never outrank the player. See
/// `generate_build_link_row::export_latch_is_stale`.
pub static SYSTEM_QUIT_GENERATE_BUILD_LINK_STALE_LATCH_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Characters successfully read and encoded into a share URL.
pub static SYSTEM_QUIT_GENERATE_BUILD_LINK_ENCODED_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Length in characters of the most recent URL produced (`0` = none yet). The cheapest proof that
/// the encode produced something of the right ORDER of size rather than an empty string.
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
// ---- System->Quit ROW IDENTITY table + resolution oracles ----------------------------------
// The rows of the patched Quit tab share only TWO dispatchable `PropertyNewButtonController`
// objects, and each row's "action object" is nothing but `controller + 0x70` (that controller's own
// inline std::function storage). So neither pointer is a row identity. These record the row TABLE
// captured at build time and, per activation, which evidence actually resolved the row -- so a run
// shows the gate working instead of merely not crashing.
/// `PropertyNewButtonController` of the native FIRST Quit row (relabelled Save Game).
pub static SYSTEM_QUIT_NATIVE_SAVE_GAME_CONTROLLER_LAST_OBJECT: AtomicUsize = AtomicUsize::new(0);
/// `PropertyNewButtonController` of the native SECOND Quit row (Return to Desktop).
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
/// Resolutions that came from the dialog's own list cursor -- the ONLY row identity, shared by mouse,
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
/// The P0 oracle: an instant-quit that was REFUSED because the activated row could not be
/// positively identified as the Return-to-Desktop row. Any nonzero value means the gate fired.
pub static SYSTEM_QUIT_QUIT_REFUSED_AMBIGUOUS_ROW_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Instant-quits AUTHORIZED by positive row evidence.
pub static SYSTEM_QUIT_QUIT_AUTHORIZED_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Activations where the action-object alias claimed the Return-to-Desktop row while the resolved
/// row was one of the two cloned rows -- i.e. the exact false identity that terminated the process.
pub static SYSTEM_QUIT_ACTION_ALIAS_FALSE_QUIT_CLAIMS: AtomicUsize = AtomicUsize::new(0);
/// Activations REFUSED because two independent row discriminators named DIFFERENT rows. Two sources
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
/// Times the finalize hook nulled a DOOMED `owningMenuWindow` before the native code virtual-called
/// it. The `~MenuWindowJob` guard covers ONLY the destructor call site (0x7ac720); the finalize has
/// five callers and the observed switch crash arrives via `MenuWindowJob::Run`, so this counter is
/// the one that moves on the crashing path. Exposed as `oracle_menu_window_finalize_guards`.
pub static MENU_WINDOW_JOB_FINALIZE_GUARDS: AtomicUsize = AtomicUsize::new(0);
/// Last window pointer the finalize hook neutralized (diagnostic).
pub static MENU_WINDOW_JOB_FINALIZE_LAST_WINDOW: AtomicUsize = AtomicUsize::new(0);

// THE MSB-PARSE / LOADLIST-WAIT / DLC-ROOT TRACE COUNTERS LEFT THIS TABLE on 2026-08-25, with the
// traces that owned them: `crates/er-diag-harness/` now holds them as private statics. They were
// never read outside those traces -- no `push_json_*` consumer, no `oracle_*` field -- and a second
// image gets its own copy of any static regardless, so hosting them centrally bought nothing.
//
// `DLC_ROOTS_REFILL_ORIG` below is the one that stayed: the DLC-root self-heal in
// `er-title-flow/src/dlc_roots_self_heal.rs` reads it, and it belongs beside that self-heal's own
// state rather than with the departed traces.
/// Trampoline for the DLC-root REFILL (`FUN_140e05fb0`), stored by whichever image detoured it.
///
/// NOW ALWAYS 0 IN THE PRODUCT, and that is the intended reading. The `er-diag-harness` trace that
/// used to fill it in lives in another image, so the self-heal takes the `game_rva` fallback it has
/// always carried: in a product-only profile that resolves the un-detoured native (identical
/// behaviour), and in a product + harness profile it enters the harness's detour, which forwards.
pub static DLC_ROOTS_REFILL_ORIG: AtomicUsize = AtomicUsize::new(0);

/// Cached address of the `mapstudio_dlc2` entry in `DLFileDeviceManager::virtualRoots`.
pub static DLC_ROOT_ENTRY_ADDR: AtomicUsize = AtomicUsize::new(0);
/// 1 once the `mapstudio_dlc2` root has been observed POPULATED. Arms the self-heal: we only ever
/// restore a root the game itself filled in correctly, never guess one during early boot.
pub static DLC_ROOT_SEEN_POPULATED: AtomicUsize = AtomicUsize::new(0);
/// FNV-1a hash of the `mapstudio_dlc2` root as the GAME populated it. The heal compares against
/// this rather than a literal, because a literal transcribed from the decompile was wrong (the
/// native stores a trailing slash the source literal lacks) and silently broke the alarm counter.
pub static DLC_ROOT_GOOD_PATH_HASH: AtomicUsize = AtomicUsize::new(0);
/// Self-heal invocations (populated -> empty edges acted on).
pub static DLC_ROOT_HEAL_ATTEMPTS: AtomicUsize = AtomicUsize::new(0);
/// Heals that produced the EXPECTED root string. This is the success metric -- not the attempt count.
pub static DLC_ROOT_HEAL_OK: AtomicUsize = AtomicUsize::new(0);
/// Heals that produced a non-empty but WRONG root (e.g. the `L"system:/"` fallback the native takes
/// when DLC ownership is unresolved). Non-zero means the heal fired too early and DLC content is
/// resolving to the wrong place -- treat as a failure, not a partial success.
pub static DLC_ROOT_HEAL_WRONG: AtomicUsize = AtomicUsize::new(0);

/// Blocks whose stale file cap stayed (status=0x04, data=null) AFTER the single native re-enqueue.
/// This is the DETERMINISTIC "the map archive backing this file is not mounted" signal -- the read
/// genuinely ran and returned nothing -- so a non-zero value means the load cannot complete and the
/// phase-2 handler will wait forever. Exposed as `oracle_blockres_stalecap_unrecoverable`.
pub static BLOCKRES_STALECAP_UNRECOVERABLE: AtomicUsize = AtomicUsize::new(0);
/// The file cap that tripped it (diagnostic).
pub static BLOCKRES_STALECAP_LAST_DEAD_CAP: AtomicUsize = AtomicUsize::new(0);
/// Ticks on which the map-mount guard-flip driver declined to act. It logged NOTHING on the
/// 2026-07-30 stall it exists to fix, and with five ANDed conditions there was no way to tell which
/// one refused. Exposed as `oracle_map_mount_guard_declines`.
pub static MOUNT_GUARD_DECLINE_LOGS: AtomicUsize = AtomicUsize::new(0);
/// Boot-phase (`!in_world`) declines, budgeted separately. These are EXPECTED and would otherwise
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
/// ProfileLoadDialog activations, BOTH kinds summed: save-file browse/pick steps plus character-slot
/// arms. Do NOT read this as a load count -- it is per browse step and per slot arm, so
/// `activations / 2` matches the load count only in a session that never navigated a directory. The
/// split below is what a load-count reader wants.
pub static SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Activations routed to the DLL's save-file browser (browse steps AND file picks).
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
// DELETED 2026-09-05 with the control-file switch driver (user directive): SWITCH_TRIGGER_ARM_COUNT,
// SWITCH_TRIGGER_TEARDOWN_COUNT, SWITCH_TRIGGER_LAST_SLOT, SWITCH_TRIGGER_DEFERRED_COUNT,
// SWITCH_SLOT_CONTROL_MTIME, SWITCH_SLOT_CONTROL_PRIMED and DETERMINISTIC_SWITCH_DRIVER_ACTIVE, plus
// the `oracle_switch_arm_count` / `_teardown_count` / `_deferred_count` / `_last_slot` /
// `_slot_control_mtime` / `_slot_control_primed` fields they fed. They measured a driver that armed a
// character switch WITHOUT the Quit menu, which is the one thing a second load has to go through.
pub static SYSTEM_QUIT_CONTINUE_CONFIRM_BLOCK_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Forwarded `continue_confirm` calls == world loads this session, BOOT INCLUDED. The authoritative
/// total-load witness; see [`crate::load_count`] for why the epoch is not.
///
/// Exactly one increment per forwarded call. It used to increment twice on the `!native_slot_proven`
/// branch (once for that branch's `FORWARD #n` label, once at the unconditional tail), inflating the
/// only honest total by one per unproven reload; that branch now labels itself from
/// [`SYSTEM_QUIT_CONTINUE_CONFIRM_UNPROVEN_FORWARD_COUNT`] instead. Blocked confirms return early
/// and never reach here.
pub static SYSTEM_QUIT_CONTINUE_CONFIRM_ALLOW_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Forwards from OUTSIDE the switch machine -- in practice the boot/title Continue. The gap between
/// `SYSTEM_QUIT_CONTINUE_CONFIRM_ALLOW_COUNT` and the load epoch, and the reason a 3-load session
/// reports `oracle_current_load_epoch = 2`.
pub static SYSTEM_QUIT_CONTINUE_CONFIRM_NON_SWITCH_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Forwards that arrived while the PREVIOUS world was still up -- a state we never drive. Logged
/// loudly since forever but counted by nothing, so it was invisible to every load-count audit.
pub static SYSTEM_QUIT_CONTINUE_CONFIRM_WORLD_UP_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Switch-machine forwards whose native requested-slot proof did NOT fire. Carries the `FORWARD #n`
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
// synthesized wButtons) were the INJECT-NAV drive's own counters. Writer and reader both sat behind
// `inject_nav_enabled()`, which could only return `false`; they were left with no writer AND no
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
/// THE VEH stack-overflow semaphore. Non-zero means describing one fault faulted again on the same
/// thread and the latch caught it -- the descent that killed ELDEN RING 1.17 with no crash record
/// on 2026-08-28, at 4704 bytes of stack a level against a 1 MiB stack. A run that reports a fault
/// AND a non-zero refusal count is telling you the report you are reading is the outermost of a
/// pile, and that the first `access-violation` line is the real one.
pub static VEH_REENTRANT_REFUSALS: AtomicUsize = AtomicUsize::new(0);
/// Crash-log lines spent on the process-FATAL exception codes (stack overflow, fastfail, heap
/// corruption, illegal instruction). Separate from the general budget below so a first-chance
/// C++/Rust throw storm cannot consume the line that names the actual kill.
pub static FATAL_EXCEPTION_LOG_LINES_WRITTEN: AtomicUsize = AtomicUsize::new(0);
/// Crash-log lines spent on the remaining ERROR-severity exception codes.
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
pub static BOOT_VIEW_DRAW_STATE: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_STOPPED: AtomicUsize = AtomicUsize::new(0);
/// WHY the boot-view cover last stopped (bd er-effects-rs-dpf6 Phase 1; 0 = armed/none,
/// 1 = release-fade after render-release, 2 = FPS bail, 3 = release-fade after can-move world proof).
/// Values are the `BOOT_VIEW_STOP_REASON_*` consts in boot_progress.rs. Reset to 0 on every rearm.
pub static BOOT_VIEW_STOP_REASON: AtomicUsize = AtomicUsize::new(0);
/// Boot-view-epoch ms when the current cover window was (re)armed (0 = initial boot window).
pub static BOOT_VIEW_WINDOW_ARM_MS: AtomicUsize = AtomicUsize::new(0);
/// Rearm -> stop duration (ms) of the LAST completed cover window (oracle_boot_view_cover_window_ms).
pub static BOOT_VIEW_COVER_WINDOW_MS_LAST: AtomicUsize = AtomicUsize::new(0);
/// `LOADING_BG_PORTRAIT_RGBA_VERSION` snapshotted at the FPS-bail stop; a LATER version bump while the
/// native loading screen is still active is the Phase-2 resume trigger (bd er-effects-rs-dpf6).
pub static BOOT_VIEW_FPS_BAIL_PUBLISH_VERSION: AtomicUsize = AtomicUsize::new(0);
/// `BOOT_VIEW_OWN_MENU_LOAD_ACTIVE` slot key at the FPS-bail stop (the bail clears the live one; the
/// resume restores it so the own-menu stop semantics survive the resume).
pub static BOOT_VIEW_FPS_BAIL_SLOT_KEY: AtomicUsize = AtomicUsize::new(0);
/// Once-per-epoch resume latch: 1 after a publish-triggered FPS-bail resume in the current cover
/// window; also suppresses the permille re-bail for the rest of the window (the 20s composite cap
/// stays armed as the FPS backstop). Reset on every rearm.
pub static BOOT_VIEW_FPS_BAIL_RESUMED: AtomicUsize = AtomicUsize::new(0);
/// Cumulative count of publish-triggered FPS-bail resumes (oracle_boot_view_fps_bail_resumes).
pub static BOOT_VIEW_FPS_BAIL_RESUMES: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_OWN_MENU_LOAD_ACTIVE: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_LOADSCREEN_TABLE_BASELINE: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_DRAW_HITS: AtomicUsize = AtomicUsize::new(0);
/// `BOOT_VIEW_DRAW_HITS` as it stood when the cover window was last ARMED.
///
/// `BOOT_VIEW_DRAW_HITS` is cumulative for the life of the process and `boot_view_reset_cover_window`
/// deliberately does not clear it, so it cannot answer "has THIS cover epoch drawn anything". That
/// question is what `title_visual_suppression_active` needs: a rearmed cover that never composites
/// must not be allowed to force-hide the title. Subtracting this baseline turns the cumulative
/// counter into the per-epoch answer without disturbing any existing reader of the total.
pub static BOOT_VIEW_DRAW_HITS_ARM_BASELINE: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_LAST_PERMILLE: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_DECISION_LOG_MS: AtomicU64 = AtomicU64::new(0);
pub static BOOT_VIEW_MONO_EPOCH: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static BOOT_VIEW_MONO_ORD: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_MONO_LABEL_PTR: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_MONO_LABEL_LEN: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_REACHED_MASK: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_MILESTONE_IDX: AtomicUsize = AtomicUsize::new(0);
/// LOAD EPOCH IDENTITY (bd er-effects-rs-ok8d). One epoch = one arm-to-teardown lifetime of the bar.
/// `SEQ` increments on every epoch reset and is the key every per-epoch high-water latch is stamped
/// with, so a new epoch invalidates them all at once. It deliberately does NOT reuse the fresh-deser
/// counter, which only bumps at the reload's DESERIALIZE -- far too late to bound the epoch, and the
/// reason the visible label walked backwards mid-load.
pub static BOOT_VIEW_EPOCH_SEQ: AtomicUsize = AtomicUsize::new(0);
/// Which phase sequence this epoch publishes: 0 = process boot, 1 = character reload.
pub static BOOT_VIEW_EPOCH_KIND: AtomicUsize = AtomicUsize::new(0);
/// Per-epoch baselines for counters that are STICKY for the whole process. A reload epoch must
/// assert its phases from what happened SINCE the rearm, never from `!= 0` on a counter that a
/// previous load already moved (bd er-effects-rs-ok8d: load 2's mask opened at 0x9f because
/// `boot_milestone_reached` re-latched five boot phases from sticky counters the instant it ran).
pub static BOOT_VIEW_CONTINUE_ALLOW_BASELINE: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_TFC_CONTINUE_BASELINE: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_PORTRAIT_SPARED_BASELINE: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_FRESH_DESER_BASELINE: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_LAST_LABEL_HASH: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_ALLOCATOR: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_LIST: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_FENCE: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_QUEUE: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_UPLOAD: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_UPLOAD_SIZE: AtomicU64 = AtomicU64::new(0);
pub static BOOT_VIEW_RTV_HEAP: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_DRAW_BUSY: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_SELF_PRESENTS: AtomicUsize = AtomicUsize::new(0);
// Full-backbuffer black clears before copying the boot bar. `_PRESENT_` is the important product
// oracle: after the self-present pump yields to the game's render loop, every boot-view Present frame
// must still cover the rest of the game instead of leaving whatever Elden Ring rendered under/around
// the loading bar.
pub static BOOT_VIEW_SELF_FULL_CLEAR_HITS: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_PRESENT_FULL_CLEAR_HITS: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_PRESENT_COVER_FAILURES: AtomicUsize = AtomicUsize::new(0);
// Nonzero means the cover stopped before a world/playable handoff. Native loading becoming visible is
// not enough; the product owns the full backbuffer until the game can safely show.
pub static BOOT_VIEW_PRE_WORLD_STOP_FAILURES: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_SWAPCHAIN_FOUND_MS: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_PUMP_STOP_REASON: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_PUMP_STOP_MS: AtomicU64 = AtomicU64::new(0);
pub static BOOT_VIEW_STRIP_W: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_STRIP_H: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_DRAWN_PERMILLE: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static BOOT_VIEW_DRAWN_IDX: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static BOOT_VIEW_DRAWN_BG_ACTIVE: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static BOOT_VIEW_HANDOFF_SEEN_MS: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_FADE_START_MS: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_FADE_COMPLETE_MS: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_FADE_HITS: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_FADE_LAST_ALPHA: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_FADE_FAILURES: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_NATIVE_GFX_FADE_HOLD_HITS: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_NATIVE_GFX_FADE_HOLD_COMPLETE_MS: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_STOP_NATIVE_HITS: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_HANDOFF_NATIVE_HITS_BASELINE: AtomicUsize = AtomicUsize::new(0);

// ONE-WAY RELEASE FADE (user report 2026-08-22, second round: "I still see my portrait come back
// very briefly if I press escape too quickly after getting in game").
//
// THE DEFECT THESE MEASURE. `native_gfx_hold_pending` in `composite_boot_progress_inner` is
// recomputed from scratch every frame out of two RECENCY predicates -- "the loading screen's
// Scaleform fade-out was stamped in the last 600 ms" and "CS::LoadingScreen::Update ticked in the
// last 900 ms". It was written as a START GATE for the release fade ("is it safe to begin fading
// yet?"), but nothing stopped it being re-asked once the fade was already running, and when it
// re-asserted the code fell through to the OPAQUE cover path -- which, unlike the fade frame,
// rasterizes with `draw_portrait: true`. So a single fresh stamp mid-fade put the portrait back on
// screen at full alpha and then let the fade finish, which is exactly the "comes back very briefly
// and tears down" the user described.
//
// The stamp that did it was not the loading screen's. `scaleform_label_goto_hook` stamped
// `LOADING_SCREEN_GFX_FADEOUT_LAST_MS` on ANY timeline label merely CONTAINING "fadeout", on ANY
// movie, so opening the in-world menu was enough to refresh it. Narrowed 2026-09-05 to the loading
// screen's own fade clip; a foreign label now lands in `LOADING_SCREEN_GFX_FADEOUT_FOREIGN_HITS`.
/// `LOADING_SCREEN_UPDATE_HITS` snapshotted the frame the release fade started.
///
/// This is what tells a REAL hold from an over-matched one. Only the `CS::LoadingScreen::Update`
/// detour writes that counter, so a hold arriving mid-fade is backed by the game's own loading
/// screen if and only if the count has moved past this snapshot. A Scaleform label from some other
/// movie cannot move it. Per cover window.
pub static BOOT_VIEW_FADE_START_LS_UPDATE_HITS: AtomicUsize = AtomicUsize::new(0);
/// Frames the OPAQUE cover path drew while this process's release fade was already running and had
/// not yet completed. THE defect counter: it is the number that was 10 in the reproducing run
/// br-20260822-184123-fa3d (draws 528 -> 538 across the fade window) while every existing detector
/// read 0, because they were all gated on `BOOT_VIEW_STOPPED`, which the fade had not set yet.
///
/// Expected 0 for the life of the process now that the fade is one-way. Session-cumulative and
/// never cleared at a rearm, for the reason spelled out at `BOOT_VIEW_DRAW_AFTER_STOP_TOTAL`: a
/// detector a rearm can silently empty is not a detector.
pub static BOOT_VIEW_NONFADE_DRAW_DURING_FADE: AtomicUsize = AtomicUsize::new(0);
/// Boot-view-epoch ms of the first such frame (0 = never). Subtract `oracle_boot_view_fade_start_ms`
/// and the answer is how far into the fade the cover went opaque again.
pub static BOOT_VIEW_NONFADE_DRAW_DURING_FADE_FIRST_MS: AtomicUsize = AtomicUsize::new(0);
/// Frames on which the start-gate predicate (`fadeout_pending || update_quiet_pending`) was true
/// while the release fade was already running -- i.e. every frame the old code would have taken
/// back to full opacity. Counted whether or not the hold was then honored, so the raw pressure on
/// the fade stays visible even when the new rule refuses all of it. Session-cumulative.
pub static BOOT_VIEW_FADE_HOLD_REASSERTS: AtomicUsize = AtomicUsize::new(0);
/// Boot-view-epoch ms of the first re-assert (0 = none).
pub static BOOT_VIEW_FADE_HOLD_REASSERTS_FIRST_MS: AtomicUsize = AtomicUsize::new(0);
/// Re-asserts REFUSED: no `CS::LoadingScreen::Update` tick past
/// `BOOT_VIEW_FADE_START_LS_UPDATE_HITS` backed them, so they were an over-matched Scaleform label
/// and the fade carried on. This is the fix engaging; a nonzero value with
/// `BOOT_VIEW_NONFADE_DRAW_DURING_FADE == 0` is the defect being caught and refused.
pub static BOOT_VIEW_FADE_HOLD_REFUSED: AtomicUsize = AtomicUsize::new(0);
/// Re-asserts HONORED: the game's own loading screen really did tick again mid-fade, so the fade
/// PAUSED at its current alpha rather than completing over live loading art (er-effects-rs-wmw
/// defect #1, the vanilla flash-through, is what that pause protects).
pub static BOOT_VIEW_FADE_HOLD_HONORED: AtomicUsize = AtomicUsize::new(0);
/// Total ms this cover window's release fade spent PAUSED by honored holds. The fade clock
/// subtracts it, so the visible fade is always the full `BOOT_VIEW_RELEASE_FADE_MS` of ramp however
/// often it was interrupted. Uncapped on purpose: the cap is applied where it is USED, so this
/// stays the honest measure of how long the game held us.
pub static BOOT_VIEW_FADE_HELD_MS: AtomicUsize = AtomicUsize::new(0);
/// Pause accumulator state, not an answer: the boot-view ms of the previous PAUSED frame, or 0 when
/// the last frame was not paused. Differencing it is what turns per-frame pauses into `_HELD_MS`.
pub static BOOT_VIEW_FADE_HOLD_TICK_MS: AtomicUsize = AtomicUsize::new(0);
/// Consecutive re-asserting frames right now; 0 on any fade frame with no re-assert. Only the
/// frame that takes it from 0 to 1 logs, so one Escape press produces one line instead of the ~36
/// a 600 ms recency window would otherwise emit from inside Present.
pub static BOOT_VIEW_FADE_HOLD_REASSERT_RUN: AtomicUsize = AtomicUsize::new(0);
// Loud gap oracle: nonzero means the boot cover stopped from the bail clock before
// the native loading screen produced enough update ticks to be visibly lit.
pub static BOOT_VIEW_DARK_GAP_FAILURES: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_DARK_GAP_LAST_HELD_MS: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_DARK_GAP_LAST_NATIVE_HITS: AtomicUsize = AtomicUsize::new(0);
// Handoff stamps made from telemetry/update context because the draw path may already be yielded or
// skipped on the exact frame the native loading screen first appears.
pub static BOOT_VIEW_TELEMETRY_HANDOFF_STAMPS: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_IDX_CHANGED_MS: AtomicU64 = AtomicU64::new(0);
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

// NATIVE LOADING-SCREEN EXPOSURE (er-effects-rs-wmw defect #1: "the custom loading screen
// disappeared for about one frame and the vanilla loading screen flashed through"). One Present
// frame is an EXPOSURE frame when the game's own CS::LoadingScreen is live but our cover did not
// draw over the backbuffer -- exactly the frame the user sees vanilla. Counted in the Present
// detour, attributed to the gate that blocked the composite (`NATIVE_LS_GATE_*`).
/// Present frames with the native loading screen live and our cover NOT drawn.
///
/// NOT the defect count on its own -- read [`NATIVE_LS_EXPOSURE_OWNED_FRAMES`] for that. This stays
/// the total of every such frame so no information is lost, but a loading screen the product does
/// not cover (fast travel, death, area transition) lands in it too; see
/// [`cover_owns_current_loading_screen`] for the split and the run that forced it.
pub static NATIVE_LS_EXPOSURE_FRAMES: AtomicUsize = AtomicUsize::new(0);
/// Present frames with the native loading screen live and our cover drawn (the healthy case).
pub static NATIVE_LS_COVERED_FRAMES: AtomicUsize = AtomicUsize::new(0);
/// Consecutive exposure frames right now; resets on any covered frame.
pub static NATIVE_LS_EXPOSURE_CUR_RUN: AtomicUsize = AtomicUsize::new(0);
/// Longest consecutive exposure run this session. 1 == the user's "about one frame" flash.
pub static NATIVE_LS_EXPOSURE_MAX_RUN: AtomicUsize = AtomicUsize::new(0);
pub static NATIVE_LS_EXPOSURE_FIRST_MS: AtomicUsize = AtomicUsize::new(0);
pub static NATIVE_LS_EXPOSURE_LAST_MS: AtomicUsize = AtomicUsize::new(0);
/// `NATIVE_LS_GATE_*` code for the most recent exposure frame.
pub static NATIVE_LS_EXPOSURE_LAST_GATE: AtomicUsize = AtomicUsize::new(0);
/// `BOOT_VIEW_STOP_REASON` sampled at the most recent exposure frame.
pub static NATIVE_LS_EXPOSURE_LAST_STOP_REASON: AtomicUsize = AtomicUsize::new(0);
/// Per-gate exposure tallies, indexed by the `NATIVE_LS_GATE_*` codes.
pub static NATIVE_LS_EXPOSURE_BY_GATE: [AtomicUsize; NATIVE_LS_GATE_COUNT] =
    [const { AtomicUsize::new(0) }; NATIVE_LS_GATE_COUNT];

// COVER-AFTER-RELEASE SEMAPHORES (user report 2026-08-22: "pressing Escape quickly after the
// loading screen fades out -- while the location banner is still on screen -- makes the loading
// screen and portrait BRIEFLY REAPPEAR and tear down").
//
// The run that reproduced it (br-20260822-184123-fa3d) left ZERO trace in the DLL log, and that
// invisibility is what this group exists to end. Every per-frame oracle we had switches itself off
// in exactly the window where the defect happens: `native_ls_exposure_record` early-returns unless
// the game's `CS::LoadingScreen` ticked within 250 ms, and in that run the native screen stopped
// ticking ~2.2 s BEFORE our cover stopped. So the one moment worth watching was the one moment
// nothing was watching.
//
// Two independent questions, deliberately kept apart:
//   1. Did OUR compositor draw after it latched stopped?  -> `BOOT_VIEW_DRAW_AFTER_STOP*`.
//      Expected 0 forever. It is a NULL DETECTOR: the whole diagnosis rests on the claim that our
//      cover did not draw the thing the user saw, and this is the counter that can refute it.
//   2. Was the GAME's own cover plate up, and was its loading screen still working?
//      -> `COVER_PLATE_*_AFTER_RELEASE` / `NATIVE_LS_ACTIVITY_AFTER_RELEASE_*`.
/// Frames on which the boot-view compositor incremented a draw/fade counter while
/// `BOOT_VIEW_STOPPED` was ALREADY set. Per cover window (cleared at every rearm).
///
/// Gate on this being NONZERO -- it is the check firing when it should not (bd k979). A nonzero
/// value means the 2026-08-22 diagnosis is wrong and our own compositor is drawing after release.
pub static BOOT_VIEW_DRAW_AFTER_STOP: AtomicUsize = AtomicUsize::new(0);
/// Session-cumulative twin of `BOOT_VIEW_DRAW_AFTER_STOP`, never cleared. The per-window counter
/// is the one to read when asking about the CURRENT cover window, but a rearm zeroes it, and a
/// detector that a rearm can silently empty is not a detector. This is the copy that remembers.
pub static BOOT_VIEW_DRAW_AFTER_STOP_TOTAL: AtomicUsize = AtomicUsize::new(0);
/// Boot-view-epoch ms of the first post-stop draw in the current cover window (0 = none).
pub static BOOT_VIEW_DRAW_AFTER_STOP_FIRST_MS: AtomicUsize = AtomicUsize::new(0);
/// Boot-view-epoch ms at which the current cover window latched `BOOT_VIEW_STOPPED` (0 = armed).
///
/// Deliberately NOT `BOOT_VIEW_FADE_COMPLETE_MS`, which the FPS-bail exit never sets. Written at
/// both stop sites, cleared at rearm and by the FPS-bail resume, so it always describes the live
/// latch rather than the last release fade.
pub static BOOT_VIEW_STOP_MS: AtomicUsize = AtomicUsize::new(0);
/// `LOADING_SCREEN_UPDATE_HITS` snapshotted at the stop, so post-release native ticks are a delta.
pub static BOOT_VIEW_STOP_LS_UPDATE_BASELINE: AtomicUsize = AtomicUsize::new(0);
/// `SYSTEM_QUIT_CONTINUE_CONFIRM_ALLOW_COUNT` snapshotted at the stop, so a world load STARTED
/// after the cover let go is a delta rather than a guess. Read by
/// [`cover_owns_current_loading_screen`], which is the whole reason it exists.
pub static BOOT_VIEW_STOP_LOAD_WITNESS: AtomicUsize = AtomicUsize::new(0);
/// `LOADING_SCREEN_GFX_FADEOUT_HITS` snapshotted at the stop, for the same reason.
pub static BOOT_VIEW_STOP_LS_FADEOUT_BASELINE: AtomicUsize = AtomicUsize::new(0);
/// Present frames the post-release watch actually sampled. 0 means the watch never opened, which
/// is NOT the same answer as "sampled and saw nothing" -- without it every zero below is ambiguous.
pub static COVER_PLATE_AFTER_RELEASE_SAMPLES: AtomicUsize = AtomicUsize::new(0);
/// Sampled frames where the game's own `CSFakeLoadingScreenImp` cover plate read VISIBLE after our
/// cover had already released. This is the decisive one: it says whether the surface the user
/// reported was the GAME's plate, on a frame that actually reached Present.
pub static COVER_PLATE_VISIBLE_AFTER_RELEASE: AtomicUsize = AtomicUsize::new(0);
pub static COVER_PLATE_VISIBLE_AFTER_RELEASE_FIRST_MS: AtomicUsize = AtomicUsize::new(0);
pub static COVER_PLATE_VISIBLE_AFTER_RELEASE_LAST_MS: AtomicUsize = AtomicUsize::new(0);
/// Consecutive visible-plate frames right now; resets on any sampled frame with the plate down.
pub static COVER_PLATE_VISIBLE_AFTER_RELEASE_CUR_RUN: AtomicUsize = AtomicUsize::new(0);
/// Longest consecutive visible-plate run since release. A brief reappearance is a short run; a
/// plate that simply never went down is one run as long as the watch.
pub static COVER_PLATE_VISIBLE_AFTER_RELEASE_MAX_RUN: AtomicUsize = AtomicUsize::new(0);
/// Largest `LOADING_SCREEN_UPDATE_HITS` delta observed past the stop baseline: the game's own
/// loading screen still ticking after our cover let go.
pub static NATIVE_LS_ACTIVITY_AFTER_RELEASE_UPDATES: AtomicUsize = AtomicUsize::new(0);
/// Same for `LOADING_SCREEN_GFX_FADEOUT_HITS` (Scaleform fade-out stamps past the stop).
pub static NATIVE_LS_ACTIVITY_AFTER_RELEASE_FADEOUTS: AtomicUsize = AtomicUsize::new(0);
/// Boot-view-epoch ms the first post-release native activity was observed (0 = none).
pub static NATIVE_LS_ACTIVITY_AFTER_RELEASE_FIRST_MS: AtomicUsize = AtomicUsize::new(0);

// IN-GAME MENU OPEN STAMP (2026-08-22). The post-release cover watch above can say a cover plate
// came back at ms X, and the user's report says the trigger is pressing Escape quickly after a
// load. Nothing in telemetry stamped the Escape press, so X could only be tied to the press BY
// HAND -- on a defect whose entire signature is the interval between the two. The three oracles
// below (plus one internal edge-detector state) make that interval a measured number.
//
// THE SIGNAL. Not a new hook and not a state poll: the game's own `02_000_IngameTop`
// `MenuWindowJob` running. The product `MenuWindowJob::Run` detour (the PAB one, the deterministic
// winner at 0x7ad1c0) already dispatches on that wide resource name to maintain
// `SYSTEM_QUIT_INGAME_TOP_WINDOW`, and `02_000_IngameTop` is the game's own name for the in-world
// pause/System menu -- the window Escape opens. The job runs once per frame WHILE that menu is up
// and not at all otherwise, so a tick after a gap IS the open. Ground truth for both halves of
// that claim, from the shipped DLL's own log of a real session (2026-08-22 11:41:30 run, game-dir
// `er-quickload-autoload-debug.log`): zero `02_000_IngameTop` `MenuWindowJob::Run` lines through the
// first 39.9 s of boot, character load and gameplay, then a first line at `[+39905ms]` carrying
// `prev=0x0`, followed by lines every ~20-60 ms -- one per presented frame -- while the menu was
// open.
//
// WHAT THE EDGE COSTS IN PRECISION. "After a gap" needs a threshold
// (`IN_GAME_MENU_TICK_GAP_MS`), so two menu sessions closer together than that read as one, and
// if the job ever pauses while a SUBMENU owns the screen, coming Back reads as a second open. Both
// errors are in `_EDGES`; the latch below takes only the FIRST edge past a cover stop, which is
// the press being asked about.
/// Boot-view-epoch ms of the most recent `02_000_IngameTop` `MenuWindowJob::Run` tick (0 = never).
/// This is the edge detector's own state, not an answer: it is compared against the next tick to
/// decide whether that tick continues a menu session or starts one.
pub static IN_GAME_MENU_RUN_LAST_MS: AtomicUsize = AtomicUsize::new(0);
/// Times the in-game menu was observed OPENING (a tick more than `IN_GAME_MENU_TICK_GAP_MS` after
/// the previous one, or the first ever). Session-cumulative.
pub static IN_GAME_MENU_OPEN_EDGES: AtomicUsize = AtomicUsize::new(0);
/// Boot-view-epoch ms of the most recent open edge (0 = the menu never opened this session).
pub static IN_GAME_MENU_OPEN_LAST_MS: AtomicUsize = AtomicUsize::new(0);
/// Boot-view-epoch ms of the FIRST open edge that landed while `BOOT_VIEW_STOPPED` was set -- i.e.
/// the first time the user opened the menu after a cover released. That is the press the
/// 2026-08-22 report describes, so `COVER_PLATE_VISIBLE_AFTER_RELEASE_FIRST_MS` minus this is the
/// press-to-reappearance interval, on one clock, with no hand derivation. 0 = no such open.
///
/// Process-lifetime, deliberately: the counter it is meant to be subtracted from has exactly the
/// same lifetime (also a compare-exchange-from-0 latch that no rearm clears), so on a run with
/// several loads both describe the FIRST occurrence and the subtraction stays meaningful. Give this
/// one a per-window reset and the pair would silently start describing different windows.
pub static IN_GAME_MENU_OPEN_FIRST_MS_AFTER_COVER_STOP: AtomicUsize = AtomicUsize::new(0);
/// How many of the session's EARLIEST open edges are stamped individually.
///
/// Eight, because the reported defect is driven by the FIRST press after a load and the loads that
/// matter come a handful of menu sessions into a run; carrying more would cost atomics in a Present
/// path for edges nobody reads.
pub const IN_GAME_MENU_OPEN_MS_FIRST_N_LEN: usize = 8;
/// Boot-view-epoch ms of the FIRST open edge of the session, whenever it happened (0 = never).
///
/// WHY IT EXISTS. The 2026-08-22 round shipped only `_LAST_MS` and
/// `_FIRST_MS_AFTER_COVER_STOP`, and both missed the press being investigated: in run
/// br-20260822-184123-fa3d the user's fast Escape landed DURING the release fade, before
/// `BOOT_VIEW_STOPPED` latched, so the after-stop latch recorded the LATER control press (36537 ms)
/// and edge #1's timestamp was simply not recoverable from telemetry at all. A press that precedes
/// the stop is precisely the press the report is about, so it must be in the record.
pub static IN_GAME_MENU_OPEN_FIRST_MS: AtomicUsize = AtomicUsize::new(0);
/// The first `IN_GAME_MENU_OPEN_MS_FIRST_N_LEN` open edges, in order, each at its own index.
///
/// Edges past the end are counted by `IN_GAME_MENU_OPEN_EDGES` and stamped by `_LAST_MS`; they are
/// not individually recoverable, which is the deliberate cost of a fixed-size array in a per-frame
/// path. Zero at an index below `_EDGES` means that edge fired before the boot-view clock was
/// anchored, not that it did not happen.
pub static IN_GAME_MENU_OPEN_MS_FIRST_N: [AtomicUsize; IN_GAME_MENU_OPEN_MS_FIRST_N_LEN] =
    [const { AtomicUsize::new(0) }; IN_GAME_MENU_OPEN_MS_FIRST_N_LEN];

// THE TWO CLOCKS (2026-08-22). The DLL debug log's `[+Nms]` prefix and every telemetry `*_ms`
// field are measured from DIFFERENT epochs, both lazily anchored `Instant`s: the log's
// `PROCESS_LOG_EPOCH` starts at the first log line (near DLL_PROCESS_ATTACH) and the telemetry
// clock's `BOOT_VIEW_EPOCH` starts at the first `boot_view_epoch_ms()` call (once the boot view
// runs, seconds later). The gap between them is a constant for the whole process, but it had to be
// measured by pairing a log line against the oracle it wrote -- by hand, once per session, from
// scratch every time. Measured this way on the 2026-08-22 run: log `+40554` == `fade_complete_ms
// 37714`, log `+35735` == `release_ready_ms 32895`, log `+39281` == `fade_start_ms 36440` -- 2840,
// 2840, 2841. The DLL knows both numbers; it can simply say so.
/// `log_ms - telemetry_ms`: ADD this to any telemetry `*_ms` to get the `[+Nms]` log prefix it
/// corresponds to, SUBTRACT it from a log prefix to get the telemetry clock. Measured once, from
/// both clocks read back to back. 0 = not measured yet (the boot-view clock had not started).
pub static LOG_EPOCH_OFFSET_MS: AtomicUsize = AtomicUsize::new(0);
/// One-shot latch for the clock-map log line, so it is stated once per process and not per frame.
pub static LOG_EPOCH_OFFSET_LOGGED: AtomicUsize = AtomicUsize::new(0);

// COVER RELEASE LATCHES (er-effects-rs-drb7). The cover's release needs the player to be
// render-ready AND the native loading screen to be finishing. Both happen in a normal session but
// NOT at the same instant (measured: render-ready at +27491ms, native close much later), and the
// predicate required them simultaneously, so it never fired in product. Latch each per cover
// window; both latched = release. Cleared by `boot_view_reset_cover_window`.
/// Set once the local player has been observed render-enabled during this cover window.
pub static BOOT_VIEW_RELEASE_RENDER_READY_SEEN: AtomicUsize = AtomicUsize::new(0);
/// Set once the native loading screen has been observed closing/complete during this cover window.
pub static BOOT_VIEW_RELEASE_NATIVE_DONE_SEEN: AtomicUsize = AtomicUsize::new(0);
/// Epoch ms at which both latches were first satisfied (the real handoff instant); 0 = not yet.
pub static BOOT_VIEW_RELEASE_READY_MS: AtomicUsize = AtomicUsize::new(0);
/// Cover windows released by the real end condition rather than by a bail. The product-health
/// counter: on a healthy session this should equal the number of CHARACTER loads (boot + N
/// switches) -- NOT the number of native loading screens, of which a switch shows two.
pub static BOOT_VIEW_SEMANTIC_RELEASES: AtomicUsize = AtomicUsize::new(0);

// PORTRAIT REJECT ATTRIBUTION (er-effects-rs-k979). `LS_PORTRAIT_REJECTED_PUBLISHES` is a bare
// count with no reason and no ordering, so a proof could only ask "were there any rejects", and
// answering yes failed the run. But refusing a blank frame is the neutral gate WORKING: measured in
// run slot-portrait-proof-20260731-130803, the neutral leak was first seen at capture version 1 --
// the very first capture -- 2 frames were refused out of 1542, and all 1540 publishes were clean.
// That is warm-up, not a defect. What WOULD be a defect is a refusal AFTER the window has published
// cleanly: the pipeline started emitting blanks mid-window. These let the two be told apart.
/// Capture version stamped at the most recent rejected publish. Compared against a window's publish
/// baseline to place the reject before or after that window's first clean publish.
pub static LS_PORTRAIT_REJECT_LAST_VERSION: AtomicUsize = AtomicUsize::new(0);
/// Neutral percentage of the most recent rejected frame. `LS_PORTRAIT_LAST_NEUTRAL_PCT` is
/// overwritten by every capture, so the value that actually caused the refusal was being lost.
pub static LS_PORTRAIT_REJECT_LAST_NEUTRAL_PCT: AtomicUsize = AtomicUsize::new(0);
/// `LOADING_BG_PORTRAIT_RGBA_VERSION` snapshotted when the current portrait window opened. The
/// version counter is cumulative for the whole PROCESS, so "has anything published yet" is only
/// answerable against this baseline -- comparing against 0 would misfile every warm-up reject from
/// the second window onward as a post-publish fault.
pub static LS_PORTRAIT_REJECT_PUBLISH_BASELINE: AtomicUsize = AtomicUsize::new(0);
/// Rejects that occurred before THIS window published a clean frame (pipeline warm-up).
pub static LS_PORTRAIT_REJECTS_BEFORE_WINDOW_PUBLISH: AtomicUsize = AtomicUsize::new(0);
/// Rejects that occurred after THIS window published cleanly -- the signal worth failing a proof
/// on: the pipeline began emitting blanks mid-window.
pub static LS_PORTRAIT_REJECTS_AFTER_WINDOW_PUBLISH: AtomicUsize = AtomicUsize::new(0);

// CHARACTER-LOAD RELEASE GATE (er-effects-rs-q6vk). A profile switch presents TWO native loading
// screens: the return-to-title teardown, then the character load after continue_confirm. Both
// satisfy "player render-ready + native screen finishing", so the cover released on the FIRST and
// left the character load bare. These hold the release until THIS switch's character load has
// actually begun, identified by the fresh-deser count advancing past its value at arm time.
/// `SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT` snapshotted when the cover armed for a switch.
pub static BOOT_VIEW_RELEASE_CONFIRM_BASELINE: AtomicUsize = AtomicUsize::new(0);
/// 1 while the current cover window must wait for a character load (set at an own-menu switch arm,
/// cleared for boot, whose single load has no teardown screen in front of it).
pub static BOOT_VIEW_RELEASE_REQUIRE_CONFIRM: AtomicUsize = AtomicUsize::new(0);
/// Times the gate held a release that would otherwise have fired on the teardown screen. Proves the
/// gate engaged; 0 on a chain with switches means it is not doing anything.
pub static BOOT_VIEW_RELEASE_HELD_FOR_CONFIRM: AtomicUsize = AtomicUsize::new(0);
/// Releases that still landed before their switch's character load began. MUST stay 0.
pub static BOOT_VIEW_RELEASE_BEFORE_CONFIRM: AtomicUsize = AtomicUsize::new(0);

// ABSOLUTE COVER BACKSTOP (user report 2026-08-30). A session spent 7+ minutes with the loading
// cover full-clearing the backbuffer over live gameplay, with no way out short of killing the
// process: the proximate cause was a game-image detour that failed to install, and BOTH of the
// cover's exits are downstream of that same detour (see `boot_view_absolute_backstop`). These
// three count/describe the last-resort release that now exists for that case.
//
// EVERY ONE OF THESE IS A DEFECT REPORT, NOT A SUCCESS. `BOOT_VIEW_BACKSTOP_RELEASES` is expected
// to be 0 for the life of a healthy process; a run that reports any is a run to investigate, and
// `_TRIGGER` says which of the two arms fired so the investigation starts in the right place.
// Deliberately NOT cleared by `boot_view_reset_cover_window`: a detector a rearm can silently
// empty is not a detector (same argument as `BOOT_VIEW_DRAW_AFTER_STOP_TOTAL`).
/// Cover windows released by the absolute backstop rather than by any healthy exit. MUST stay 0.
pub static BOOT_VIEW_BACKSTOP_RELEASES: AtomicUsize = AtomicUsize::new(0);
/// Boot-view epoch ms at which the backstop FIRST tripped in this process; 0 = never.
pub static BOOT_VIEW_BACKSTOP_FIRST_MS: AtomicUsize = AtomicUsize::new(0);
/// Which arm tripped most recently: 0 = never, 1 = world demonstrably live under an opaque cover,
/// 2 = wall-clock cover lifetime exceeded (see `BOOT_VIEW_BACKSTOP_TRIGGER_*`).
pub static BOOT_VIEW_BACKSTOP_TRIGGER: AtomicUsize = AtomicUsize::new(0);

/// The cover drew this frame -- not an exposure.
pub const NATIVE_LS_GATE_DREW: usize = 0;
/// `portrait_overlay_enabled()` was false, so the composite was never attempted.
pub const NATIVE_LS_GATE_OVERLAY_DISABLED: usize = 1;
/// The in-world epoch fast-path skip fired (world clock live for the current load epoch).
pub const NATIVE_LS_GATE_EPOCH_WORLD_LIVE: usize = 2;
/// The native-Windows pre-loading-screen composite suppression fired.
pub const NATIVE_LS_GATE_NATIVE_SUPPRESSED: usize = 3;
/// The composite ran but drew nothing (internally gated: `BOOT_VIEW_STOPPED` / draw-state).
pub const NATIVE_LS_GATE_COVER_STOPPED: usize = 4;
/// The composite drew nothing because THE PRODUCT DOES NOT COVER THIS LOADING SCREEN -- see
/// [`cover_owns_current_loading_screen`]. Expected, not a defect: the cover is a boot/character-load
/// surface, and a fast travel, a death respawn or an area transition is the game's own screen.
pub const NATIVE_LS_GATE_UNOWNED_LOAD: usize = 5;
pub const NATIVE_LS_GATE_COUNT: usize = 6;

/// Present frames the cover SHOULD have covered and did not: [`NATIVE_LS_EXPOSURE_FRAMES`] minus
/// the [`NATIVE_LS_GATE_UNOWNED_LOAD`] frames. THIS is the number an acceptance gate reads.
pub static NATIVE_LS_EXPOSURE_OWNED_FRAMES: AtomicUsize = AtomicUsize::new(0);
/// Boot-view epoch ms of the first owned exposure frame (0 = none).
pub static NATIVE_LS_EXPOSURE_OWNED_FIRST_MS: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_CROP_MINX: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static PORTRAIT_CROP_MINY: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static PORTRAIT_CROP_MAXX: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_CROP_MAXY: AtomicUsize = AtomicUsize::new(0);
/// Frames actually FOLDED INTO the crop envelope, saturating at `PORTRAIT_CROP_SEED_N` (40).
///
/// It used to increment on every `portrait_onto` call, seeding or not, which made its name a lie
/// and its value useless: a live run read 324 against a seed window of 40, so the one question the
/// counter exists to answer -- "is the envelope frozen yet?" -- could not be answered from it at
/// all. Saturating makes `== PORTRAIT_CROP_SEED_N` mean FROZEN and `< N` mean still seeding, which
/// is what every reader already assumed it meant. Written by `er_loading_portrait_core::portrait_onto`;
/// read by `oracle_portrait_crop_seed_frames`; reset per portrait window alongside the four bounds.
pub static PORTRAIT_CROP_SEED_FRAMES: AtomicUsize = AtomicUsize::new(0);
/// Times a seeding frame actually MOVED one of the four crop bounds outward, i.e. the number of
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
/// Frames the portrait compositor REFUSED to draw because the source frame was not depth-keyed --
/// every pixel opaque, i.e. the mask cut nothing. Written by the mask gate in
/// `er_loading_portrait_core::portrait_onto`; read by `oracle_portrait_draw_refused_unmasked`.
///
/// WHY the gate needs it (2026-08-21): a live run measured `oracle_portrait_alpha_cover_pct = 99`
/// against `oracle_depth_key_bg_pct = 76`. Those cannot both describe a keyed frame -- 99% coverage
/// means the crop envelope grew to (near) the whole render target, which is what a single fully
/// opaque frame folded into the 40-frame seed union does. An unmasked frame therefore does not just
/// look wrong for one frame: it permanently pollutes the FROZEN crop rect and so the apparent size
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
/// Leftover save artifacts from an EARLIER run that staging deleted so they cannot be served.
pub static SAVE_DIRECT_STAGE_STALE_REMOVED: AtomicU64 = AtomicU64::new(0);
/// THE stale-serve semaphore. Nonzero means a leftover container survived the staging sweep and
/// the game may open it INSTEAD of the configured source -- the silent soft lock of 2026-08-11.
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
/// through, the expected steady state. ANY value above 2 means a pass-through decision was lost and
/// the unbounded-recursion stack overflow of 2026-07-30 is back.
///
/// The ntdll `NtCreateFile` detour deliberately does NOT count here: it is the layer BENEATH these,
/// firing again under every Win32 open, so including it would put a healthy open at 2 and a healthy
/// normalize-triggering open at 3 -- an alarm that fires on a working game is an alarm nobody reads.
pub static SAVE_REDIRECT_DETOUR_MAX_DEPTH: AtomicUsize = AtomicUsize::new(0);
/// Nested save-redirect detour entries that were degraded to a pure pass-through. Nonzero is
/// normal (the detours do their own file I/O); it is the DEPTH above, not this count, that
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
/// Chain slot for ordinal-100 `XInputGetStateEx`, which is a DIFFERENT export at a DIFFERENT address
/// and therefore needs its own cell. It shares a handler with `XInputGetState` because the two have
/// the same signature and the same thing is done to both, but sharing the SLOT would be a bug: the
/// union stores the next-in-chain pointer per registration, so a second registration into one cell
/// would send `XInputGetState` callers down `XInputGetStateEx`'s chain. Before 2026-09-05 the Ex
/// detour stored no original at all and silently reused `XINPUT_GET_STATE_ORIG`'s.
pub static XINPUT_GET_STATE_EX_ORIG: AtomicUsize = AtomicUsize::new(0);
/// ONE INSTALLER AT A TIME for the XInput detours (2026-08-31). `XINPUT_GET_STATE_ORIG == 0` was the
/// only guard, and it is set AFTER `MhHook::new` returns -- so two threads that both read 0 both call
/// `MhHook::new` on the same export. `install_xinput_block` is reached from the GAME task
/// (`enforce_input_block_now` / `input_trace_tick`) and from the MENU thread
/// (`system_quit_menu_window_run_post` -> `save_picker_menu_pump_drive_strip_mouse`), which is the last
/// live path that can print `HOOK REGISTRY DUPLICATE`. Claimed with compare-exchange and RELEASED again
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
// OWN_LOAD_M28_DISPATCH_FIRED removed 2026-08-31. `own_load_m28_dispatch` is VERIFY-ONLY: the
// AddDefaultFileLoadProcess call it counted was disabled after the block getter AV-faulted, and the
// function's own comment says "NO native call is made here". The counter therefore could not move,
// yet it was one of five components of `world_stream_progress_watermark` in
// scripts/er-readiness-watch.py -- a run-STOPPING stall decision -- documented there as
// "increments on a working stream". It contributed a constant 0 to every stall verdict. Re-add it
// WITH the increment if and when the dispatch is re-enabled. OWN_LOAD_M28_DISPATCH_DIAG_CALLS (the
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
/// The three states `SWITCH_RELOAD_FD4IO_PHASE` takes. They live HERE, beside the atomic they
/// describe, because reading that phase correctly requires them and the readers now span crates:
/// the writer/owner is the root crate's `own_load::loaders` (SUBMIT -> DRAIN -> COMMIT), while
/// `er-title-flow`'s b78 guard reads it to decide whether fd4io currently owns `GameMan+0xb78`.
/// er-title-flow must NOT depend on the root crate, so a private `const` root-side would have
/// forced either a duplicated literal or a host-seam call for a plain comparison against 0.
/// IDLE(0): no reload in flight, nobody owns b78. DRAIN(1): the full read was SUBMITted and is
/// being pumped to residency. COMMIT(2): residency reached (or the bounded drain timed out) and
/// the feed + continue_confirm own the load.
pub const SWITCH_RELOAD_FD4IO_IDLE: usize = 0;
pub const SWITCH_RELOAD_FD4IO_DRAIN: usize = 1;
pub const SWITCH_RELOAD_FD4IO_COMMIT: usize = 2;
/// Frames the b78 guard STOOD DOWN because the fd4io reload machine was non-IDLE, i.e. frames on
/// which the guard would have forced `GameMan+0xb78 = -1` and no longer does (bd er-effects-rs-9jbe).
/// This is the ENGAGEMENT oracle for that stand-down: a clean switch run proves only that nothing
/// regressed, whereas `> 0` proves the new condition actually fired against a live fd4io overlap --
/// the exact race (fd4io non-IDLE inside the guard's active window) that produced the black-screen
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
/// `TitleTopDialog::open_menu` calls the suppression detour let THROUGH.
///
/// The suppressed count alone cannot answer "did the native title ever open its menu again",
/// because a pass-through is invisible: the detour only logged the calls it DROPPED. That
/// ambiguity is what made the 2026-08-26 softlock unreadable from the log. Counting both sides
/// makes the question a subtraction.
pub static TITLE_OPEN_MENU_PASSTHROUGH_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Pass-throughs that happened AFTER at least one suppression, i.e. after the missing-save hold
/// released.
///
/// **This is the decisive one.** If a late pick releases the hold and this stays 0, the native
/// title never re-issued `open_menu` and its rows can never be rebuilt with the save present --
/// the pick must then TRIGGER the open rather than wait for a retry. Nonzero says the title does
/// retry on its own and the drop-and-retry model is sound.
pub static TITLE_OPEN_MENU_PASSTHROUGH_AFTER_SUPPRESS_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Boot default-save check vs. the container the runtime opens: 0 = not decided yet,
/// 1 = the accepted container IS the one the runtime opens (or nothing was accepted and the
/// picker is armed, which is also correct), 2 = MISMATCH -- the check validated a file the
/// runtime will never read.
///
/// 2 is the 2026-08-26 failure: under Seamless the blank `ER0000.co2` was rejected, the check
/// fell back to `ER0000.sl2`, and reported "there is a save" while ersc.dll went on to open the
/// blank `.co2`. It cost two runs while being invisible in RAM. See
/// `er_save_redirect::boot_save_container_matches_runtime`.
pub static BOOT_SAVE_CONTAINER_MATCHES_RUNTIME: AtomicUsize = AtomicUsize::new(0);
pub static SCENE_OBJ_PROXY_CTOR_HITS: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_MODE_ACTIVE: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_REOPEN_PENDING: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_OPEN_SLOTS_PENDING: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_ACTION_OBJ: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_OPEN_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_REPOPULATE_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_PICK_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_PICK_REJECT_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_RESUBMIT_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_CANCEL_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_STAGED_ROW_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Times the game's own `CS::ProfileSummary` records were restored over the picker's staged
/// browse-row labels (telemetry oracle `oracle_save_picker_row_records_restored`; cumulative).
///
/// PAIR IT WITH `oracle_save_picker_open_count`. The defect this counter exists to make visible was
/// diagnosed by ABSENCE -- a 600 MB debug log with pickers opening and not one restore line -- and
/// absence is exactly what a counter turns into a number. Fewer restores than picker opens means
/// staged labels were left in a game-owned structure, which is what put `[..] EldenRing` and
/// `[ new ]` on the user's loading screens as character names.
pub static SAVE_PICKER_ROW_RECORDS_RESTORED: AtomicUsize = AtomicUsize::new(0);
/// Of those restores, the ones that could NOT write the snapshot back because the live
/// `CS::ProfileSummary` allocation is no longer the one it was taken from
/// (`oracle_save_picker_row_records_restore_unwritable`). The latch is still cleared -- writing a
/// dead allocation's image into whatever now occupies the address would be worse than losing it --
/// so a non-zero here means some records stayed stale and the restore knew it.
pub static SAVE_PICKER_ROW_RECORDS_RESTORE_UNWRITABLE: AtomicUsize = AtomicUsize::new(0);
/// Staged-row restores that were POSTPONED because `GameDataMan+0x78` read as 0 -- the live
/// summary is unreadable this frame, which is normal through the clean-title window
/// (`oracle_save_picker_row_records_restore_deferred`). The snapshot stays armed and the per-frame
/// sweep retries, so a non-zero here is expected and only interesting beside a non-zero
/// `_restore_unwritable`.
pub static SAVE_PICKER_ROW_RECORDS_RESTORE_DEFERRED: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_REBUILD_PENDING_DIALOG: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_LIST_BUILDER_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_LIST_BUILDER_RESTAGE_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Which file-picker surface this session runs: 0 = the in-game `05_010` browser (default),
/// 1 = the OS common file dialog (`er-quickload.toml os_native_save_picker = true`).
///
/// A LATCH set once from `init_runtime_config`, not a lazy read, so it is exported even in a
/// session where no picker ever opens. Every other `SAVE_PICKER_OS_*` counter is only meaningful
/// once this reads 1, and a report can state the mode without the reporter knowing the config.
pub static SAVE_PICKER_SURFACE: AtomicUsize = AtomicUsize::new(0);
/// 1 while an OS common file dialog is up and BLOCKING the thread that owns the menu pump.
///
/// One word of state doing triple duty: the re-entrancy claim (taken by compare-exchange, so only
/// the first caller proceeds and a message comdlg32 dispatches back into our own row-action detour
/// cannot open a second dialog), the freeze predicate for `SAVE_FLOW_STAGE_TICKS`, and the stage-3
/// "a browser is live" term. Released by a guard whose `Drop` clears it, so an unwind cannot leave
/// it stuck.
pub static SAVE_PICKER_OS_DIALOG_OPEN: AtomicUsize = AtomicUsize::new(0);
/// Game-task ticks whose `SAVE_FLOW_STAGE_TICKS` accrual was SUPPRESSED because a dialog was open.
///
/// Load-bearing, and the only thing that answers a question nothing static can: `> 0` proves the
/// game task kept ticking while the menu pump was blocked -- so every save-flow deadline WOULD have
/// expired under a browsing user, and the freeze is what saved the flow. `== 0` with a dialog
/// demonstrably open instead says the whole frame stalled with the pump.
pub static SAVE_PICKER_OS_TICKS_FROZEN: AtomicUsize = AtomicUsize::new(0);
/// OS common file dialogs opened this session.
pub static SAVE_PICKER_OS_OPEN_COUNT: AtomicUsize = AtomicUsize::new(0);
/// OS dialogs that closed returning a path we accepted.
pub static SAVE_PICKER_OS_CLOSED_WITH_PATH: AtomicUsize = AtomicUsize::new(0);
/// OS dialogs the user cancelled (`FALSE` with `CommDlgExtendedError() == 0`).
pub static SAVE_PICKER_OS_CANCEL_COUNT: AtomicUsize = AtomicUsize::new(0);
/// OS dialogs comdlg32 FAILED (`FALSE` with a non-zero extended error), and the last such error.
/// Distinguished from a cancel because only a failure is a bug of ours, and neither reopens.
pub static SAVE_PICKER_OS_ERROR_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_OS_LAST_ERROR: AtomicUsize = AtomicUsize::new(0);
/// Picks the shared save-validity predicate rejected, and the last `PickRejection as usize`.
pub static SAVE_PICKER_OS_REJECT_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_OS_LAST_REJECT_REASON: AtomicUsize = AtomicUsize::new(0);
/// Dialog reopens after an invalid pick, and 1 if the bound was ever hit.
///
/// The bound is not about user patience: a comdlg32 that fails INSTANTLY (Wine's is a
/// reimplementation) would spin the reopen loop at full speed on the thread that owns the menu
/// pump, an unbreakable hang. Exhaustion takes the cancel path.
pub static SAVE_PICKER_OS_REOPEN_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_OS_REOPEN_EXHAUSTED: AtomicUsize = AtomicUsize::new(0);
/// The `hwndOwner` handed to comdlg32 (0 = none found).
///
/// WHICH window this is changed on 2026-07-31 and the old expectation is now WRONG. It used to be
/// required to be the GAME window; it is now the DIM COVER whenever a cover is up, because an owned
/// window is always above its owner and that is the only way to make "the picker is in front of the
/// blur" structural instead of a race. Read it together with `SAVE_PICKER_OS_OWNER_IS_COVER`:
/// `is_cover = 1` means this equals `SAVE_PICKER_DIM_HWND`, and `is_cover = 0` means it equals the
/// game window (the boot arm, which raises no cover, and the fallback when the cover did not come
/// up in time).
pub static SAVE_PICKER_OS_OWNER_HWND: AtomicUsize = AtomicUsize::new(0);
/// 1 when the last dialog was owned by the DIM COVER, 0 when it fell back to the ER window.
///
/// This is the field that says whether the z-order guarantee was actually in force for a given
/// open. A System>Quit open with `SAVE_PICKER_DIM_ARM_COUNT` advancing but `is_cover = 0` means the
/// cover was armed and the dialog STILL took the game window as its owner -- i.e. the cover did not
/// finish coming up inside `SAVE_PICKER_DIM_ARM_WAIT_MS` and the ordering is back to a race.
pub static SAVE_PICKER_OS_OWNER_IS_COVER: AtomicUsize = AtomicUsize::new(0);
/// Save-like `CreateFileW` opens observed while a dialog was open. Attribution for the shell
/// browsing traffic that otherwise pollutes the save CreateFileW diagnostics.
pub static SAVE_PICKER_OS_SAVELIKE_OPENS: AtomicUsize = AtomicUsize::new(0);

// ---- OS picker at the MISSING-SAVE BOOT (startup_hooks/save_picker_boot.rs) ----
//
// The `SAVE_PICKER_OS_*` family above counts DIALOGS and is shared by all three intents. This
// family counts the BOOT intent's OUTCOMES, which the shared family cannot express: at a
// missing-save boot a cancel is not "the user backed out of a menu", it QUITS THE GAME, and that
// terminal step has to be provable from telemetry rather than from watching the screen.

/// Where the boot missing-save pick stands. `0` idle (nothing opened, or not a missing-save boot),
/// `1` a surface owns the pick, `2` a file was accepted and the character sub-picker owns it,
/// `3` the user cancelled the OS dialog and the game is quitting, `4` comdlg32 was unusable and the
/// in-game browser took the pick over.
pub static SAVE_PICKER_OS_BOOT_STATE: AtomicUsize = AtomicUsize::new(0);
/// Boot OS dialogs this session. At a missing-save boot exactly one open is ever started, so `> 1`
/// means the one-shot latch leaked and the reopen loop this design exists to prevent came back.
pub static SAVE_PICKER_OS_BOOT_OPEN_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Boot OS picks that cleared the shared validity predicate and reached the character sub-picker.
pub static SAVE_PICKER_OS_BOOT_PICK_COUNT: AtomicUsize = AtomicUsize::new(0);
/// THE ACCEPTANCE ORACLE for the boot cancel path: the user pressed Cancel on the boot OS dialog
/// and the game is quitting.
///
/// Only trustworthy when `SAVE_PICKER_BOOT_TELEMETRY_FLUSHED` reads 1. When it reads 0 this field
/// is whatever it was before the cancel, and `er-quickload-bootstrap.jsonl`'s
/// `boot_picker_cancel_exit` record is the outcome instead.
pub static SAVE_PICKER_OS_BOOT_CANCEL_EXIT_COUNT: AtomicUsize = AtomicUsize::new(0);
/// 1 once the picker thread is calling `ExitProcess(0)`.
///
/// (An earlier `SAVE_PICKER_OS_BOOT_EXIT_PENDING` companion was removed with the game-task exit
/// hand-off it belonged to: the hand-off never executed at a missing-save boot, because the game
/// task had stopped ticking long before the user answered the dialog. The picker thread now does
/// the whole thing, so there is no interval during which an exit is owed but not performed.)
pub static SAVE_PICKER_OS_BOOT_EXIT_PERFORMED: AtomicUsize = AtomicUsize::new(0);
/// Times the boot OS surface gave up and handed the pick to the in-game browser (comdlg32 failed,
/// the reopen bound was exhausted, or the core `CreateFileW` detour never went live). Non-zero says
/// the user still got a picker, which is why this is a fallback and not a failure.
pub static SAVE_PICKER_OS_BOOT_FALLBACK_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Ticks the boot OS open was deferred waiting for the core `CreateFileW` detour to go live. The
/// wait is bounded; on exhaustion the in-game browser takes over rather than the boot stranding.
pub static SAVE_PICKER_OS_BOOT_DEFER_TICKS: AtomicUsize = AtomicUsize::new(0);
/// Did the picker thread manage to refresh the telemetry file before quitting?
///
/// `1` the file you are reading describes the cancel. `0` the flush could not run (the state mutex
/// was held by a thread that is not giving it back), so **every other field in this file predates
/// the cancel** and only `er-quickload-bootstrap.jsonl` plus the debug log describe the outcome.
///
/// THIS FIELD EXISTS BECAUSE ITS ABSENCE COST A DIAGNOSIS. In run pr109-boot-oscancel-20260730-110704
/// the cancel worked perfectly and the telemetry showed `boot_state = OPEN`, `cancel_exit_count = 0`
/// -- identical to what a dialog that never returned would have written, because the file had gone
/// stale 12s earlier. A reader had no way to tell a working feature from a broken one.
pub static SAVE_PICKER_BOOT_TELEMETRY_FLUSHED: AtomicUsize = AtomicUsize::new(0);
/// `GAME_TASK_TICKS_TOTAL` sampled by the PICKER THREAD when the boot dialog opened, and again when
/// the user answered it. Both are written by a thread that is demonstrably alive, so their
/// DIFFERENCE is the direct answer to "was the game task running while the dialog was up" -- the
/// question the first live run left open and no existing field could settle.
///
/// Equal values mean the game task did not tick once across the dialog's entire life.
pub static SAVE_PICKER_BOOT_GAME_TICKS_AT_OPEN: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_BOOT_GAME_TICKS_AT_ANSWER: AtomicUsize = AtomicUsize::new(0);

// ---- OS-picker DIM OVERLAY (save_picker_dim_overlay.rs) ----
//
// These are DELIBERATELY a new family rather than a reuse of `SAVE_PICKER_OVERLAY_*`. That older
// family belongs to the DLL-DRAWN STARTUP picker (`gpu_readback/save_picker_overlay.rs`, the
// no-save-boot browser) and is live; borrowing its counters would make two unrelated surfaces
// indistinguishable in one telemetry field.
//
/// 1 while the dim overlay is armed (a blocking OS dialog is up and we are covering the game).
/// Cleared by the arming guard's `Drop`, so an unwind through the dialog cannot strand it.
pub static SAVE_PICKER_DIM_ARMED: AtomicUsize = AtomicUsize::new(0);
/// Arms and disarms. They must END equal; `arm - disarm == 1` with the process alive is a stranded
/// fullscreen dim, which is worse than not having the feature at all.
pub static SAVE_PICKER_DIM_ARM_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_DIM_DISARM_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Frames pushed to the compositor via `UpdateLayeredWindow` while armed.
///
/// THE CORE ORACLE. The game/menu thread is BLOCKED inside comdlg32 for the dialog's whole life, so
/// the game logs nothing and presents nothing during that window. This counter advancing across the
/// same interval is the objective proof that the animation ticked on a thread we own -- something no
/// game-render-path overlay could produce.
pub static SAVE_PICKER_DIM_FRAMES: AtomicUsize = AtomicUsize::new(0);
/// `SAVE_PICKER_DIM_FRAMES` sampled at the START of the current arm, so the disarm can subtract and
/// report THIS ARM'S frames.
///
/// The counter above is process-cumulative and the disarm line used to print it raw, which read as
/// a per-arm figure and was not one: a four-open run logged 108/241/362/423 where the arms had
/// actually pushed 108/133/121/61. Every one of those lines overstated its own arm, and the last
/// overstated it by 7x. Snapshotting at arm and subtracting at disarm is what makes the line say
/// what it claims to say. Written by the ARMING thread before the generation bump, so the overlay
/// thread cannot have pushed a frame of the new arm yet.
pub static SAVE_PICKER_DIM_FRAMES_AT_ARM: AtomicUsize = AtomicUsize::new(0);
/// Wall-clock milliseconds of the LAST completed armed interval (arm -> disarm). Pairs with the
/// dialog's own `after=Nms` log line: the two must agree, or the dim did not bracket the call.
pub static SAVE_PICKER_DIM_ALIVE_MS: AtomicUsize = AtomicUsize::new(0);
/// Why the last disarm happened: 1 = the dialog returned (normal), 2 = arming failed and rolled
/// back, 3 = the overlay thread bailed out and hid the window itself.
pub static SAVE_PICKER_DIM_TEARDOWN_REASON: AtomicUsize = AtomicUsize::new(0);
/// Furthest overlay-thread init stage reached: 1 = thread, 2 = class, 3 = window, 4 = DIB,
/// 5 = render loop entered. Anything below 5 says where bring-up died.
pub static SAVE_PICKER_DIM_STAGE: AtomicUsize = AtomicUsize::new(0);
/// Our overlay window, and the ER window we sized/stacked against.
pub static SAVE_PICKER_DIM_HWND: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_DIM_GAME_HWND: AtomicUsize = AtomicUsize::new(0);
/// `UpdateLayeredWindow` calls that returned an error while armed.
pub static SAVE_PICKER_DIM_UPDATE_FAILS: AtomicUsize = AtomicUsize::new(0);
/// Z-ORDER ORACLE, sampled while armed: the top-down z-order ordinal of our overlay, of the ER
/// window, and of the foreign foreground window (the OS dialog). `usize::MAX` = not found.
///
/// This is what settles the ordering requirement WITHOUT a screenshot. The contract is
/// `foreign < self < game`: the dialog above us, us above the game. `self > game` means the dim is
/// behind the game and invisible; `self < foreign` means the dim is covering the dialog the user
/// has to interact with.
pub static SAVE_PICKER_DIM_Z_SELF: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static SAVE_PICKER_DIM_Z_GAME: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static SAVE_PICKER_DIM_Z_FOREIGN: AtomicUsize = AtomicUsize::new(usize::MAX);
/// The foreground window seen while armed that is neither ours nor the game's -- i.e. comdlg32's.
/// 0 means no foreign foreground window was ever observed while the dim was up.
pub static SAVE_PICKER_DIM_FOREIGN_FG_HWND: AtomicUsize = AtomicUsize::new(0);
/// Frames whose sampled z-order VIOLATED the cover's contract while armed, counted SEPARATELY for
/// the two ways it can break.
///
/// COUNTERS, not last-sample snapshots, because `SAVE_PICKER_DIM_Z_*` only carry the most recent
/// frame -- and the most recent frame is the one taken as the dialog is already tearing down, which
/// is exactly when the ordering is least representative. `0` across a run where frames were pushed
/// is the real proof the stacking held for the whole dialog, not just at the end.
///
/// THEY ARE TWO FIELDS BECAUSE THEY MEAN OPPOSITE THINGS AND CARRY OPPOSITE SEVERITIES (split
/// 2026-08-01, er-effects-rs-mc1d). The fused predecessor `SAVE_PICKER_DIM_Z_VIOLATIONS` scored
/// `behind_game || covering_dialog` into one atomic, and the live run that was supposed to prove
/// the ownership fix came back with 130 of them across 424 dim frames -- a number from which
/// neither failure could be confirmed nor excluded. The oracle could not answer the single question
/// it was built to answer, so the run could neither pass nor fail the fix. The severities:
///
/// - `_Z_COVERING_DIALOG` (`self_z < foreign_z`, our cover NEARER THE FRONT than the dialog) is
///   precisely the defect the ownership chain exists to eliminate. Non-zero means the fix is
///   INCOMPLETE and for those frames the user was looking at a dim laid over the controls they have
///   to click. Treat any non-zero value as a failure of the z-order fix.
/// - `_Z_BEHIND_GAME` (`self_z >= game_z`) is a lower-severity COSMETIC failure: the cover is
///   invisible for those frames, but the dialog is still fully usable. Non-zero deserves its own
///   issue, not a block on the ownership work.
///
/// Unknown ordinals (`usize::MAX`) are excluded from BOTH, so neither counts a window that had
/// merely dropped out of the z-chain while being created or destroyed.
pub static SAVE_PICKER_DIM_Z_BEHIND_GAME: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_DIM_Z_COVERING_DIALOG: AtomicUsize = AtomicUsize::new(0);
/// The FIRST offending sample of each kind: the `(self, game, foreign)` ordinals that broke the
/// contract, plus the milliseconds between that arm's cover coming up and the break. `usize::MAX`
/// (emitted as `-1`) means that kind never fired.
///
/// A TOTAL ALONE CANNOT SAY *WHERE IN THE ARM* THE BREAK SAT, and the phase is most of the
/// diagnosis: ordinals that break at `+0ms` and then settle are a bring-up transient the compositor
/// resolves, while the same ordinals still breaking hundreds of milliseconds in are a stacking that
/// never took. The run that motivated this recorded 130 breaks over 4 arms with no way to tell
/// those two apart.
///
/// FIRST-WINS, NOT LAST-WINS, and enforced rather than assumed: the `_FIRST_SELF` field is the
/// whole record's claim ticket, taken by a `compare_exchange` off the `usize::MAX` sentinel, and
/// only the sample that wins that CAS writes the other three. A violating sample always has a known
/// `self_z` (both disjuncts require it), so the sentinel can never collide with a real value. The
/// first break is the one that shows the transition into failure; a last-wins record would decay
/// into the same tear-down-moment snapshot the counters above exist to avoid.
pub static SAVE_PICKER_DIM_Z_BEHIND_GAME_FIRST_SELF: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static SAVE_PICKER_DIM_Z_BEHIND_GAME_FIRST_GAME: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static SAVE_PICKER_DIM_Z_BEHIND_GAME_FIRST_FOREIGN: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static SAVE_PICKER_DIM_Z_BEHIND_GAME_FIRST_MS: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static SAVE_PICKER_DIM_Z_COVERING_DIALOG_FIRST_SELF: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static SAVE_PICKER_DIM_Z_COVERING_DIALOG_FIRST_GAME: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static SAVE_PICKER_DIM_Z_COVERING_DIALOG_FIRST_FOREIGN: AtomicUsize =
    AtomicUsize::new(usize::MAX);
pub static SAVE_PICKER_DIM_Z_COVERING_DIALOG_FIRST_MS: AtomicUsize = AtomicUsize::new(usize::MAX);
/// Frames whose push had to fall back to a FULL-surface upload because the dirty-rectangle path was
/// refused. The cover is a mostly-static image with a small animating mark, so pushing only the
/// mark's rectangle is what keeps the pulse smooth; a run where this equals the frame count is a run
/// whose animation is paying a full-screen upload per frame (measured: ~9fps on a 3846x2172 window).
pub static SAVE_PICKER_DIM_FULL_PUSHES: AtomicUsize = AtomicUsize::new(0);
/// Result of the ONE bring-up push of `UpdateLayeredWindow`, done at attach on a hidden,
/// fully-transparent 1x1 layer: 0 = not attempted, 1 = accepted, 2 = rejected.
///
/// `UpdateLayeredWindow` is the single API in this feature that Wine could plausibly not implement
/// the way we need. Proving it at ATTACH -- when nothing is waiting -- rather than at the instant a
/// user's dialog opens means a broken environment is visible in telemetry from a run that never even
/// opened a picker, instead of surfacing as a missing cover at the worst moment.
pub static SAVE_PICKER_DIM_SELFTEST: AtomicUsize = AtomicUsize::new(0);
// ---- COVER OWNERSHIP + ARM HANDSHAKE (user report 2026-07-31) ----
//
// Two defects were reported against the same window: the OS picker came up BEHIND the cover, and
// the cover could be dragged off the game as if it were an unrelated application. Both were the
// same root cause -- the cover was an UNOWNED top-level popup whose only claim to a z-order was one
// `HWND_TOP` raise, issued by the overlay thread up to a frame period AFTER `arm` returned and
// therefore quite possibly after comdlg32 had already created its window. The fix makes both
// relations structural (game owns cover, cover owns dialog), and these fields are how a run proves
// the relations actually took rather than being assumed.
//
/// Did the cover get installed as an owned window of the ER window? 0 = never attempted (no game
/// window known), 1 = `SetWindowLongPtrW(GWLP_HWNDPARENT)` stored AND the owner read back equal,
/// 2 = attempted and the read-back did NOT match, i.e. this environment ignored the store.
///
/// A READ-BACK rather than the call's return value on purpose: `SetWindowLongPtrW` returns the
/// PREVIOUS value, and 0 means both "there was no owner" and "the call failed", so its return
/// cannot distinguish success from failure on the very first store.
pub static SAVE_PICKER_DIM_OWNER_SET: AtomicUsize = AtomicUsize::new(0);
/// The owner HWND read back out of the cover's `GWLP_HWNDPARENT`. Equal to
/// `SAVE_PICKER_DIM_GAME_HWND` is the proof the attachment took; 0 with `_owner_set = 2` says the
/// store was silently dropped.
pub static SAVE_PICKER_DIM_OWNER_READBACK: AtomicUsize = AtomicUsize::new(0);
/// Milliseconds the ARMING thread waited for the overlay thread to report the cover up at the
/// game's geometry, on the last arm.
///
/// `arm` used to return immediately and the caller went straight into `GetOpenFileNameW`, so the
/// cover's raise and comdlg32's window creation were unordered. The arm now blocks on an atomic
/// handshake, which is what makes the ordering real; this field is its cost. Tens of milliseconds
/// is the expected value (one overlay frame plus the full-screen DIB fill).
pub static SAVE_PICKER_DIM_ARM_WAIT_MS: AtomicUsize = AtomicUsize::new(0);
/// Arms that hit the handshake DEADLINE instead of the cover reporting ready. Non-zero means the
/// overlay thread is wedged or too slow, the dialog fell back to owning itself to the game window,
/// and the stacking for those opens is a race again -- not a silent degradation.
pub static SAVE_PICKER_DIM_ARM_WAIT_TIMEOUTS: AtomicUsize = AtomicUsize::new(0);
/// Frames on which the cover was found to have DRIFTED off the ER window's rect and was snapped
/// back. Ownership is what a compositor is supposed to honour, but a Wayland compositor with a
/// move-modifier can still drag any toplevel; this counts the times something moved the cover and
/// we pulled it back, so "the blur is attached to the game" is measured rather than hoped for.
pub static SAVE_PICKER_DIM_REANCHOR_COUNT: AtomicUsize = AtomicUsize::new(0);
/// 1 = an OS Save-As returned an EXISTING file, so the Box3 overwrite confirm is owed.
///
/// A latch rather than a direct `SAVE_FLOW_STAGE` write: the menu thread must not become a second
/// writer of the stage (a filed defect the in-game arm already has). The save-flow tick consumes
/// this and performs the transition through `save_flow_enter_stage`, staying the sole owner.
pub static SAVE_DEST_CONFIRM_PENDING: AtomicUsize = AtomicUsize::new(0);
/// Browse rows with no character on which the hide of the per-slot info fields (`Level`
/// caption/value, `PlayTime`) was DRIVEN -- the native setter was called; pair with
/// `PROFILE_ROW_SLOT_INFO_NON_DISPLAY` to know it took effect. Doubles as the latch that arms the
/// symmetric re-show.
pub static PROFILE_ROW_SLOT_INFO_HIDDEN_ROWS: AtomicUsize = AtomicUsize::new(0);
/// Rows on which the re-show of the per-slot info fields was driven (row clips are reused).
pub static PROFILE_ROW_SLOT_INFO_SHOWN_ROWS: AtomicUsize = AtomicUsize::new(0);
/// Per-field visibility calls skipped fail-closed (child unresolved, or unexpected proxy vtable).
pub static PROFILE_ROW_SLOT_INFO_VIS_SKIPS: AtomicUsize = AtomicUsize::new(0);
/// Per-field visibility calls whose resolved GFx value was not a display object (setter no-ops).
pub static PROFILE_ROW_SLOT_INFO_NON_DISPLAY: AtomicUsize = AtomicUsize::new(0);
/// Summary populates left ALONE because the row proxy belongs to a movie this mod never edited --
/// the game's own System>Quit `GameEnd` panel is the one that matters. `CS::MenuSaveDataSummary`'s
/// populate is a SHARED template, so every surface that shows a character summary arrives at the
/// same hook; this counts the ones handed straight back to the game untouched.
pub static PROFILE_FOREIGN_SUMMARY_ROWS: AtomicUsize = AtomicUsize::new(0);
/// Summary populates recognised as OUR edited `05_010_ProfileSelect` row template (the probe field
/// resolved to a real GFx value). Pair with `PROFILE_FOREIGN_SUMMARY_ROWS`: the split is the whole
/// decoupling claim, and a zero here with a live ProfileSelect list means the probe is wrong.
pub static PROFILE_OWN_SUMMARY_ROWS: AtomicUsize = AtomicUsize::new(0);
/// Text pushes REFUSED because the named child does not exist on that movie (the resolve came back
/// undefined). Before this existed those pushes were counted as successes -- SetText was called on a
/// self-linked empty proxy and reported 109k "successful" writes to a field the movie did not have.
pub static PROFILE_STATS_PUSH_MISSING_FIELD: AtomicUsize = AtomicUsize::new(0);
/// `MenuWindowJob::Run` passes observed for `05_010_ProfileSelect`. It ticks once per FRAME while
/// that window exists, so a rise between two samples means the view is on screen RIGHT NOW -- which
/// is the only question the live editor's safety gate needs answered.
pub static PROFILE_SELECT_WINDOW_RUN_TICKS: AtomicUsize = AtomicUsize::new(0);
/// Live-editor commands NOT applied from the asynchronous `FrameBegin` path because the ProfileSelect
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
// LOADING-SCREEN PORTRAIT ARMOR ORACLE (bd er-effects-rs-91l5 Layer 1). Written every game tick by
// `portrait_equip_oracle_sample` off the profile renderer's LIVE stage-0 `ChrAsm` (+0x130). These
// replace `PORTRAIT_EQUIP_SLOT_RESOLVED_MASK` / `_UNRESOLVED_TOTAL` / `_PROTECTOR_REFEEDS`, which
// sampled the wrong stage, once, through a bare `.store()`, on a field the renderer overrides -- and
// reported a clean pass on a run the user saw render entirely nude.
//
// The load window these per-window accumulators belong to: `PROFILE_LOADSCREEN_TABLE_BUILDS` at the
// time of sampling. A change rolls every per-window value below back to its unset state.
pub static PORTRAIT_EQUIP_ORACLE_WINDOW: AtomicUsize = AtomicUsize::new(0);
/// Profile slot the sampler read this window, biased by 1 (0 = nothing sampled yet).
pub static PORTRAIT_EQUIP_ORACLE_SLOT: AtomicUsize = AtomicUsize::new(0);

/// The `CS::ModelIns` the current portrait-equip window opened against, latched on its first
/// sample. DIAGNOSTIC ONLY -- nothing classifies on it. It exists to answer bd er-effects-rs-7m5y:
/// a run measured 40 bad HEAD/CHEST frames out of 235 while every capture frame was clean, and the
/// suspicion is that they are all sampled BEFORE the model is rebuilt for the incoming character.
/// Comparing each failing frame's `model_ins` against this settles that from one run instead of
/// from an assumption -- and if a bad frame reports a DIFFERENT model, the mismatch survives the
/// rebuild and is a real defect rather than a sampling artifact.
pub static PORTRAIT_EQUIP_WINDOW_OPEN_MODEL_INS: AtomicUsize = AtomicUsize::new(0);
/// Frames THIS window on which a portrait model existed and its live `ChrAsm` was configured. ZERO is
/// a FAILURE verdict, not a pass: it means the oracle never got to look, which is the `naked_kicks=0`
/// false negative in a different costume.
pub static PORTRAIT_EQUIP_SAMPLED_FRAMES: AtomicUsize = AtomicUsize::new(0);
/// Frames THIS window whose effective protector rows would not render the character's own armor.
/// Any value > 0 is a FAILURE that a later good frame cannot erase (`fetch_add`, never `.store`).
pub static PORTRAIT_EQUIP_BAD_FRAMES: AtomicUsize = AtomicUsize::new(0);
/// OR of every failing frame's reason mask this window: bit 0 forced whole-outfit override active
/// (`unk0`/`unkd4`/`unkd8` non-negative), bit 1 head != record, bit 2 chest != record, bit 3 hands !=
/// bare-body default, bit 4 legs != bare-body default.
pub static PORTRAIT_EQUIP_BAD_MASK: AtomicUsize = AtomicUsize::new(0);
/// Session total of bad frames across every window. Never reset, so one snapshot at any time proves
/// whether the session EVER rendered a wrong portrait outfit.
pub static PORTRAIT_EQUIP_BAD_FRAMES_TOTAL: AtomicUsize = AtomicUsize::new(0);
/// Session count of load windows that produced at least one sample. Compare against
/// `oracle_portrait_loadscreen_table_builds`: a shortfall names windows the oracle never observed.
pub static PORTRAIT_EQUIP_WINDOWS_SAMPLED: AtomicUsize = AtomicUsize::new(0);
/// Session count of load windows that produced at least one BAD frame.
pub static PORTRAIT_EQUIP_WINDOWS_BAD: AtomicUsize = AtomicUsize::new(0);
/// FIRST sample of this window, `compare_exchange`-from-zero so the value belongs to the first frame
/// rather than whichever tick ran last. Packed: bit 32 = present, low 32 = the `i32`. Raw 0 means
/// never sampled, which is NOT the same as a param id of 0.
pub static PORTRAIT_EQUIP_FIRST_EFFECTIVE_ID: [AtomicUsize; 4] = [const { AtomicUsize::new(0) }; 4];
/// The target save record's own head/chest/hands/legs param ids, first sample of this window; the
/// comparison basis for `PORTRAIT_EQUIP_BAD_MASK` bits 1 and 2. Same packing.
pub static PORTRAIT_EQUIP_RECORD_PARAM_ID: [AtomicUsize; 4] = [const { AtomicUsize::new(0) }; 4];
/// `ChrAsm::unk0` / `unkd4` / `unkd8` verbatim, first sample of this window. All three read -1 on a
/// correctly built `ChrAsm`; a non-negative value in any of them IS the nude bug. Same packing.
pub static PORTRAIT_EQUIP_FIRST_UNK0: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_EQUIP_FIRST_UNKD4: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_EQUIP_FIRST_UNKD8: AtomicUsize = AtomicUsize::new(0);
/// The four effective rows as of the first game tick on which `PROFILE_BAKE_RGBA_CAPTURED` was
/// observed set -- the frame whose pixels the user is shown. Same packing.
pub static PORTRAIT_EQUIP_CAPTURE_EFFECTIVE_ID: [AtomicUsize; 4] =
    [const { AtomicUsize::new(0) }; 4];
/// Tri-state verdict for that capture-frame sample: 0 never sampled, 1 clean, 2 bad. Deliberately not
/// a boolean -- "the oracle never ran" must not read as a pass.
pub static PORTRAIT_EQUIP_CAPTURE_VERDICT: AtomicUsize = AtomicUsize::new(0);

// --- THE REPAIR ITSELF (2026-09-06) ------------------------------------------------------------
// The native feed `FUN_140bbe1a0` erases the record's armaments and replaces its gauntlets and
// greaves with the bare-body rows before the portrait is built; the kick now writes the record's own
// `equipment_param_ids` back over the fed array. These count what that write actually changed, so a
// portrait that still renders bare can be told apart from a portrait whose character IS bare.
/// Build kicks on which the write-back ran at all (the inbox and record both read cleanly).
pub static PORTRAIT_EQUIP_RESTORE_KICKS: AtomicUsize = AtomicUsize::new(0);
/// Kicks where the write-back found NOTHING to change -- the character is genuinely bare-handed and
/// bare-armed. Not a failure, and the reason `..._KICKS` alone cannot be read as proof of a repair.
pub static PORTRAIT_EQUIP_RESTORE_NOOP_KICKS: AtomicUsize = AtomicUsize::new(0);
/// Armament indices restored, summed across kicks.
pub static PORTRAIT_EQUIP_RESTORE_WEAPON_SLOTS: AtomicUsize = AtomicUsize::new(0);
/// Ammunition indices restored, summed across kicks.
pub static PORTRAIT_EQUIP_RESTORE_AMMO_SLOTS: AtomicUsize = AtomicUsize::new(0);
/// Protector indices restored, summed across kicks (hands and legs in practice).
pub static PORTRAIT_EQUIP_RESTORE_PROTECTOR_SLOTS: AtomicUsize = AtomicUsize::new(0);
/// Kicks where a write was attempted but the inbox read or write-back failed, so the portrait is
/// being built from the feed's mutilated array. A non-zero value invalidates the run's picture.
pub static PORTRAIT_EQUIP_RESTORE_FAILURES: AtomicUsize = AtomicUsize::new(0);
/// First kick's record ids, packed by `portrait_equip_pack` so a real `-1` (slot empty) is
/// distinguishable from "never sampled". Order: right weapon, left weapon, hands, legs.
pub static PORTRAIT_EQUIP_RESTORE_RECORD_ID: [AtomicUsize; 4] = [const { AtomicUsize::new(0) }; 4];
/// What the LIVE ChrAsm at `renderer+0x130` -- the one `FUN_1409e6fb0` re-reads every frame -- holds
/// for the armaments and the handedness, first sample of the window. Writing the inbox proves only
/// that we wrote the inbox; these are the values the model build actually resolves from, so they are
/// what separates "the repair reached the renderer" from "the repair reached a buffer".
/// Packed by `portrait_equip_pack`: right weapon, left weapon.
pub static PORTRAIT_EQUIP_LIVE_WEAPON_ID: [AtomicUsize; 2] = [const { AtomicUsize::new(0) }; 2];
/// `ChrAsm::equipment.armStyle` (ChrAsm+0x08), the handedness input
/// `getSelectedWeaponSlotIndex` reads. Packed the same way, so 0 is a real value and not "unsampled".
pub static PORTRAIT_EQUIP_LIVE_ARM_STYLE: AtomicUsize = AtomicUsize::new(0);
/// `armStyle` as the SAVE RECORD carries it, latched at the build kick.
///
/// Measured 2026-09-07 on Onyx Lord slot 1: the serialized `ChrAsmEquipment` block is
/// `[3, 0, 0, 1, 1, 1, 1]` -- armStyle 3 = `RightBothHands`, i.e. two-handing, confirmed by the
/// character loading into the world two-handed -- while `PORTRAIT_EQUIP_LIVE_ARM_STYLE` read 1 off
/// `renderer+0x130`. The grip is therefore present in the record and LOST somewhere before the live
/// stage, so anything that wants the saved grip must read the record, not the renderer. The walk
/// that produced those bytes is self-checked: the same block's param ids come out
/// right=4080001 left=110000 hands=1040200 legs=5210300, matching the live oracle exactly.
pub static PORTRAIT_EQUIP_RECORD_ARM_STYLE: AtomicUsize = AtomicUsize::new(0);
/// Times the record's arm style was written into `CSChrAsmModelIns+0x328`, and what a read-back
/// immediately afterwards saw. A write count with a read-back that does not match is the engine
/// overwriting us per frame; a match with no visible change means the field is consumed only when
/// the parts are attached, i.e. it needs to be set before the model build rather than after.
/// `armStyle` as the FEED left it in the renderer inbox (`renderer+0x548+0x08`), read immediately
/// before the repair overwrites it, packed by `portrait_equip_pack`. This is the value that
/// separates the two candidate explanations for a portrait that will not two-hand: if the feed's
/// `ChrAsm::Copy` carried the record's grip through, this equals `PORTRAIT_EQUIP_RECORD_ARM_STYLE`
/// and the grip is lost LATER (inbox -> live); if the eight `EquipItemBySpecialIndex` clears
/// recompute it, this reads 0/1 while the record reads 3, and the loss is the feed's.
pub static PORTRAIT_EQUIP_INBOX_ARM_STYLE_FED: AtomicUsize = AtomicUsize::new(0);
/// Kicks where the record's `armStyle` was written back over the inbox's, and the read-back that
/// followed. Same shape as the model-instance pair below and for the same reason: a write that does
/// not stick is a different bug from a write that sticks and changes nothing.
pub static PORTRAIT_EQUIP_INBOX_ARM_STYLE_WRITES: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_EQUIP_INBOX_ARM_STYLE_READBACK: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_MODEL_ARM_STYLE_WRITES: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_MODEL_ARM_STYLE_READBACK: AtomicUsize = AtomicUsize::new(0);
/// Times the previewed save's `CS::ProfileSummary` records were put back after the game's
/// return-title save overwrote them, and the slot mask that write covered.
///
/// A switch to a foreign save that ends with ZERO here is a switch whose loading-screen portrait was
/// built from whatever the game left in the records -- which, when the picked slot IS the resident
/// character's slot, is the PREVIOUS character (measured run br-20260907-191016-4020). A non-zero
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
/// Loading-screen stats panel reads that were DECLINED because the slot's live
/// `CS::ProfileSummary` record is not a character (telemetry oracle
/// `oracle_stats_record_not_a_character`; cumulative, never reset).
///
/// THE POINT OF THIS COUNTER IS THAT A BLANK PANEL IS AMBIGUOUS. When the in-game save picker's
/// browse-row labels were left in the live records, the panel rendered `[..] EldenRing` / `[ new ]`
/// beside `RL 0` -- and once it correctly refuses to draw that, the screen looks EXACTLY like a
/// build with the stats feature switched off. A non-zero here says "we saw a record and refused
/// it"; a zero alongside `oracle_stats_text_built > 0` says the feature ran and every record it
/// read was a character. Nothing else in the telemetry can tell those two apart.
pub static STATS_RECORD_NOT_A_CHARACTER: AtomicUsize = AtomicUsize::new(0);
pub static PROFILE_OFFSCREEN_SETTLE_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static MODEL_WAS_LIVE: AtomicUsize = AtomicUsize::new(0);
pub static RETURN_DESKTOP_CONTROLLER_DIAG: AtomicUsize = AtomicUsize::new(0);
pub static EFFECT_HOTKEY_PENDING_UP: AtomicUsize = AtomicUsize::new(0);
pub static EFFECT_HOTKEY_PENDING_DOWN: AtomicUsize = AtomicUsize::new(0);
pub static EFFECT_HOTKEY_PENDING_LEFT: AtomicUsize = AtomicUsize::new(0);
pub static EFFECT_HOTKEY_PENDING_RIGHT: AtomicUsize = AtomicUsize::new(0);
pub static EFFECT_HOTKEY_PENDING_TOGGLE: AtomicUsize = AtomicUsize::new(0);
pub static EFFECT_HOTKEY_PENDING_OVERLAY_TOGGLE: AtomicUsize = AtomicUsize::new(0);
pub static EFFECT_HOTKEY_HOOK_STARTED: AtomicBool = AtomicBool::new(false);
pub static EFFECT_HOTKEY_HOOK_ACTIVE: AtomicBool = AtomicBool::new(false);
pub static EFFECT_SELECTOR_OVERLAY_VISIBLE_FOR_HOOK: AtomicBool = AtomicBool::new(false);
pub static EFFECT_SELECTOR_DINPUT_HOOK_INSTALL_ATTEMPTED: AtomicBool = AtomicBool::new(false);
pub static EFFECT_HOTKEY_HOOK_HITS: AtomicUsize = AtomicUsize::new(0);
pub static EFFECT_HOTKEY_APPLIED_ACTIONS: AtomicUsize = AtomicUsize::new(0);
pub static EFFECT_INPUT_SUPPRESSED_KEYS: AtomicUsize = AtomicUsize::new(0);
pub static EFFECT_INPUT_SUPPRESSED_ARROW_KEYS: AtomicUsize = AtomicUsize::new(0);
pub static INJECTED_KEY: AtomicU8 = AtomicU8::new(0);

/// The cursor position the game is told it has, packed `(x << 32) | y`, or `u64::MAX` for "no
/// injection". One atomic rather than two so the pair cannot be read half-updated.
pub static INJECTED_CURSOR_POS: AtomicU64 = AtomicU64::new(u64::MAX);
/// How many `GetCursorPos` calls were answered with the injected position.
pub static USER32_INJECTED_CURSOR_STAMPS: AtomicUsize = AtomicUsize::new(0);
pub static SUPPRESS_ARROW_KEYS: AtomicBool = AtomicBool::new(false);
pub static DINPUT_SUPPRESSED_ARROW_KEYS: AtomicUsize = AtomicUsize::new(0);
pub static DINPUT_KB_HOOK_FIRES: AtomicUsize = AtomicUsize::new(0);
/// How many times the DInput keyboard `GetDeviceState` detour STAMPED the harness's injected DIK
/// into the buffer the game is about to read. This is the FOCUS-INDEPENDENT injection stage: the
/// stamp happens AFTER DInput has filled (or zeroed) the buffer, so it lands whether or not ER owns
/// the keyboard focus. Non-zero here with `DINPUT_KB_HOOK_FIRES` non-zero means the game read a
/// buffer we authored; a zero here while the harness is injecting means the stamp never ran.
pub static DINPUT_INJECTED_KEY_STAMPS: AtomicUsize = AtomicUsize::new(0);
/// Win32 virtual-key code the harness is holding down at the USER32 layer (0 = nothing held). ER
/// 1.17 imports `GetKeyState`/`GetKeyboardState`/`ToAscii` from USER32 and NO RawInput API at all,
/// so this is a keyboard stage the game genuinely reads. Stamped into the results of the two USER32
/// getters below, which makes it FOCUS-INDEPENDENT: those getters return the calling thread's key
/// table, which Windows only populates for the focused thread -- we author the answer afterwards.
pub static INJECTED_VK: AtomicU8 = AtomicU8::new(0);
/// How many times the game called USER32 `GetKeyboardState` through our detour. Zero means the game
/// does not read the keyboard that way and the stamp below is meaningless.
pub static USER32_GET_KEYBOARD_STATE_FIRES: AtomicUsize = AtomicUsize::new(0);
/// How many times the game called USER32 `GetKeyState` through our detour.
pub static USER32_GET_KEY_STATE_FIRES: AtomicUsize = AtomicUsize::new(0);
/// How many times a USER32 detour actually reported `INJECTED_VK` as held to the game.
pub static USER32_INJECTED_VK_STAMPS: AtomicUsize = AtomicUsize::new(0);
/// How many times the game called USER32 `GetCursorPos` through our detour. The OptionSetting
/// tab-switch (Game / Quit Game) has no keyboard bind -- it is MOUSE-ONLY -- so driving a real menu
/// path to the cloned load rows needs a focus-independent MOUSE stage the same way movement needed a
/// keyboard one. `eldenring.exe` 1.17 imports `GetCursorPos`, `SetCursorPos`, `ClientToScreen`,
/// `ScreenToClient` and `ClipCursor` from USER32; this counter says whether the pointer position the
/// menu uses comes through that import (and is therefore stampable) or from DirectInput's mouse
/// device instead. Measurement only -- nothing is injected on the mouse path yet.
pub static USER32_GET_CURSOR_POS_FIRES: AtomicUsize = AtomicUsize::new(0);
/// THE PAD GATE, sampled on the can-move probe's own inject-on frames. These decide whether ANY
/// injected input is read, and they are the difference between "the key never arrived" and "the key
/// arrived at a device the game had already decided to skip". `FD4PadManager+0x2f8` is the
/// inactive-window REQUEST `CS::CSPadStep::STEP_Update` raises on an unfocused frame;
/// `FD4PadManager::Update` latches it forward into `+0x2f9`; and every `CSInGamePad` query
/// (`FUN_142664380`/`142664280`/`1426640f0`, all from `PollInput@0x142665060`) opens with
/// `if (field625_0x2f9 == false)`. `GAME_DEBUG_BYTE` is the `.data` byte
/// `Game.Debug.IsEnableControlOnDisactiveWindow` reads (1.16.2 0x144588af1), which `er-focus-input`
/// holds at 1 -- sampling it says whether that hold is actually in effect at the instant we inject,
/// rather than assuming it from the DLL being loaded. 0xff = could not read.
pub static PAD_GATE_MGR_2F8: AtomicUsize = AtomicUsize::new(0xff);
pub static PAD_GATE_MGR_2F9: AtomicUsize = AtomicUsize::new(0xff);
pub static PAD_GATE_DEBUG_BYTE: AtomicUsize = AtomicUsize::new(0xff);
/// Inject-on frames on which `FD4PadManager+0x2f9` was LATCHED SHUT -- i.e. frames where the game
/// short-circuited every pad read no matter what we had stamped into the device.
pub static PAD_GATE_SHUT_ON_INJECT_FRAMES: AtomicUsize = AtomicUsize::new(0);
/// Total horizontal displacement, in THOUSANDTHS of a world unit, accumulated across the can-move
/// probe's INJECT-ON frames, and across its INJECT-OFF tail. These exist because the frame-COUNT
/// verdict cannot tell "the key never reached the game" from "the key reached the game and the
/// character is standing against a wall": both report a low moved-frame ratio. Run
/// br-20260905-033648-7f4c is the case in point -- 30 of 30 inject-on frames stamped DIK_W into the
/// buffer the game read, and only 5 of 29 frames cleared the per-frame threshold.
pub static ON_DISP_MILLI: AtomicUsize = AtomicUsize::new(0);
pub static OFF_TAIL_DISP_MILLI: AtomicUsize = AtomicUsize::new(0);
pub static DINPUT_MOUSE_HOOK_FIRES: AtomicUsize = AtomicUsize::new(0);
pub static DINPUT_KB_GET_STATE_ORIG: AtomicUsize = AtomicUsize::new(0);
pub static DINPUT_MOUSE_GET_STATE_ORIG: AtomicUsize = AtomicUsize::new(0);
pub static DINPUT_KB_ALSO_MOUSE: AtomicBool = AtomicBool::new(false);
pub static SIMULATED_INPUT_PRESSES_TOTAL: AtomicUsize = AtomicUsize::new(0);
pub static AUTOLOAD_HANDOFF_PARENT_STATE_FIX_COUNT: AtomicUsize = AtomicUsize::new(0);

// ---- save-flow (System->Quit "Save Game" close-then-fire commit; save-game-flow WP1) ----
/// Save-flow stage machine value, exported as `oracle_save_flow_stage`. Stage map:
/// 0 IDLE, 1 BOX1_WAIT (WP2), 2 BOX2_WAIT (WP2), 3 DEST_BROWSE (WP3), 4 BOX3_WAIT (WP3),
/// 5 CLOSING_ABORT (WP2), 6 CLOSING_COMMIT, 7 FIRE_GATE_WAIT, 8 COMMIT_WAIT.
pub static SAVE_FLOW_STAGE: AtomicUsize = AtomicUsize::new(0);
/// Game-task ticks spent in the CURRENT save-flow stage (reset on every transition; drives
/// the stage-7 fire-gate timeout and the stage-8 commit watchdog).
pub static SAVE_FLOW_STAGE_TICKS: AtomicUsize = AtomicUsize::new(0);
/// The System/Quit tab PropertyEditDialog captured at the Save Game row press (diagnostic
/// correlation pointer; WP2/WP3 reuse it as the confirm-box submit context).
pub static SAVE_FLOW_DIALOG: AtomicUsize = AtomicUsize::new(0);
/// Times the stage-7 fire gate found the CSMenuMan[+0x80] +0x290/+0x298 failure latch set.
/// Latched means SaveRequest_Profile's gate fails PERMANENTLY for the session, so the flow
/// aborts instead of firing (exported as `oracle_save_flow_gate_latch_blocked`).
pub static SAVE_FLOW_GATE_LATCH_BLOCKED_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Completed Save Game commits: the bypassed save reported terminal status 0 (success) AND the
/// file it was supposed to produce verified on disk. The file check is part of the condition on
/// purpose -- the SL status is the game's opinion of its own job and says nothing about bytes.
pub static SAVE_FLOW_COMMIT_COMPLETE_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Save Game row presses. One bypass arm and one commit are expected PER PRESS, so this is what
/// tells a double-arm bug from a user who simply pressed the row twice.
pub static SAVE_FLOW_ROW_PRESS_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Commits where the game reported terminal status 0 but the on-disk check FAILED: the save was
/// announced as successful and no usable file exists. Non-zero is a hard product failure.
pub static SAVE_FLOW_COMMIT_VERIFY_FAIL_COUNT: AtomicUsize = AtomicUsize::new(0);
/// `er_save_suppress::bypass_allowed_total()` sampled at the instant the forced save request was
/// fired. Stage 8 compares against it to tell "the save enqueue ARRIVED and is being written"
/// (total advanced -> a real write is in flight, protect it) from "the enqueue never arrived"
/// (total unchanged -> nothing is in flight and the flow is already dead). Without that
/// distinction stage 8 held the Save Game row hostage for the full watchdog even when the fire
/// had silently failed.
pub static SAVE_FLOW_BYPASS_ALLOWED_AT_FIRE: AtomicUsize = AtomicUsize::new(0);
/// Save flows whose forced request never produced a save enqueue: the fire reached the native
/// request flags but no SL save arrived at the suppressor inside the grace window. The user's
/// save did NOT happen -- a hard failure oracle, distinct from a user-declined abort.
pub static SAVE_FLOW_ENQUEUE_MISSING_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Commits that ended on the stage-8 watchdog instead of on an observed outcome. The
/// watchdog is a BACKSTOP: it expires a stranded token and frees the UI, but it never learns
/// what happened, so every one of these is a DEGRADED commit even when the file turns out to
/// be fine. Non-zero means the write-completion signal did not reach the flow and the reason
/// has to be found -- silence here used to be indistinguishable from success.
pub static SAVE_FLOW_COMMIT_WATCHDOG_COUNT: AtomicUsize = AtomicUsize::new(0);
/// `er_save_suppress::save_job_starts()` sampled at the fire. Stage 8 compares against it to
/// timestamp the tick the SL worker actually began writing.
pub static SAVE_FLOW_SAVE_JOB_STARTS_AT_FIRE: AtomicUsize = AtomicUsize::new(0);
/// Commit tick on which the SL worker was first seen to have STARTED writing (0 = not yet /
/// never). Read beside the tick count in the completion line: the difference between them is
/// how long the native write itself took, and the rest is how long the flow took to notice.
pub static SAVE_FLOW_COMMIT_JOB_START_TICK: AtomicUsize = AtomicUsize::new(0);
/// `er_save_suppress::dispatch_calls()` sampled at the fire. A stage-8 failure compares
/// against it to say whether the native save dispatcher ran AT ALL after the request flags
/// were set -- the difference between "nothing consumed the request" and "the dispatcher
/// consumed it and refused", which the enqueue-side counters alone cannot distinguish.
pub static SAVE_FLOW_DISPATCH_CALLS_AT_FIRE: AtomicUsize = AtomicUsize::new(0);
/// `er_save_suppress::dispatch_declines()` sampled at the fire.
pub static SAVE_FLOW_DISPATCH_DECLINES_AT_FIRE: AtomicUsize = AtomicUsize::new(0);
/// `er_save_suppress::serialize_failures()` sampled at the fire. A post-fire increase means
/// the character serializer `FUN_14067dc00` is what refused, which is upstream of both the
/// submit builder and the suppressor.
pub static SAVE_FLOW_SERIALIZE_FAILURES_AT_FIRE: AtomicUsize = AtomicUsize::new(0);
/// `er_save_suppress::serialize_calls()` sampled at the fire. This is the ALLOCATION oracle:
/// both character lanes allocate their MainHeap buffers (`0x280000`, plus `0x60000` on the
/// combined lane) and null-check them BEFORE calling `FUN_14067dc00`, so a post-fire
/// increase proves the allocations succeeded and the lane got as far as the serializer. No
/// increase, with declines climbing, means the lane bailed earlier -- an allocation
/// returned null, or one of the pre-allocation gates (`CanShowSaveMenu()`, `saveState != 0`,
/// slot index >= 10) turned it away.
pub static SAVE_FLOW_SERIALIZE_CALLS_AT_FIRE: AtomicUsize = AtomicUsize::new(0);
/// `er_save_suppress::submits_swallowed()` sampled at the fire. A post-fire increase with no
/// bypass allow means a submit WAS built and this DLL swallowed it by mistake -- the one
/// failure mode where the fault is ours rather than the game's.
pub static SAVE_FLOW_SUBMITS_SWALLOWED_AT_FIRE: AtomicUsize = AtomicUsize::new(0);
/// `GameMan+0xb72` / `+0xb73` sampled immediately BEFORE the forced request pair is fired.
///
/// This is what makes a retraction scoped rather than a broad clear. A flag that was
/// ALREADY set before our fire belongs to the game and is left alone; only a flag that went
/// 0 -> 1 across our own call is ours to take back. Stored as the raw byte, or
/// [`SAVE_FLOW_FLAG_UNREAD`] when GameMan was not readable at the fire -- which disqualifies
/// the retraction for that flag, because "we could not see it" is not "it was clear".
pub static SAVE_FLOW_B72_BEFORE_FIRE: AtomicUsize = AtomicUsize::new(SAVE_FLOW_FLAG_UNREAD);
/// `GameMan+0xb73` sampled immediately before the forced request pair. See
/// [`SAVE_FLOW_B72_BEFORE_FIRE`].
pub static SAVE_FLOW_B73_BEFORE_FIRE: AtomicUsize = AtomicUsize::new(SAVE_FLOW_FLAG_UNREAD);
/// Sentinel for a request flag that could not be read at the fire.
pub const SAVE_FLOW_FLAG_UNREAD: usize = usize::MAX;
/// Save-request flags this DLL retracted after its own fire provably went nowhere.
///
/// Each retraction ends a per-frame spin: a refused save lane touches nothing, so the
/// dispatcher re-enters it every frame and re-serializes the whole character (0x280000
/// bytes) forever. Measured at ~33 serializations/second on a stuck run. Non-zero here is
/// the flow cleaning up after itself, not an error.
pub static SAVE_FLOW_REQUEST_RETRACTIONS: AtomicUsize = AtomicUsize::new(0);
/// Retractions the flow declined to perform because it could not prove the latched flags
/// were its own (or because a return-title sequence was in flight and needs the request to
/// survive). Non-zero means a spin was left running on purpose; read it beside
/// `oracle_save_dispatch_declines` to see the cost.
pub static SAVE_FLOW_RETRACT_DECLINED: AtomicUsize = AtomicUsize::new(0);

// ---- save-flow confirm box (save-game-flow WP2, reduced to ONE box 2026-07-31) ----
/// Number of confirm boxes the save flow can build. It is ONE: "Overwrite this file?", asked
/// only when the chosen destination already exists. The two up-front confirms this flow used to
/// open ("Are you sure you want to save?" and "Overwrite your loaded save?") were removed -- they
/// asked the user to predict a destination before seeing the list, which is the mistake the
/// reviewer reported. Indexes the per-box counters below; box ids are 1-based so 0 stays the
/// "no box" sentinel.
pub const SAVE_FLOW_BOX_COUNT: usize = 1;
/// Box id (1..=SAVE_FLOW_BOX_COUNT) the NEXT `CS::MessageBoxDialog` build belongs to, set
/// immediately before the confirm-box MenuJob is submitted and cleared by the builder hook
/// that captures the dialog. Non-zero makes the builder hook forward the build and capture
/// it into `SAVE_FLOW_BOX_DIALOG` instead of applying the product msgbox suppression.
pub static SAVE_FLOW_BOX_EXPECTED: AtomicUsize = AtomicUsize::new(0);
/// The captured confirm-box `CS::MessageBoxDialog` (0 = none live). Deliberately a DEDICATED
/// slot: `MSGBOX_LAST_DIALOG` / `CONNECTION_ERROR_DIALOG` feed the startup auto-accept, which
/// must never touch a user-facing save confirm.
pub static SAVE_FLOW_BOX_DIALOG: AtomicUsize = AtomicUsize::new(0);
/// Menu-pump pending: build+submit this confirm box id from `system_quit_menu_window_run_post`
/// (the proven menu-job submit context). 0 = nothing pending.
pub static SAVE_FLOW_SUBMIT_BOX_PENDING: AtomicUsize = AtomicUsize::new(0);
/// Confirm boxes whose dialog was captured, per box id - 1.
pub static SAVE_FLOW_BOX_OPEN_COUNTS: [AtomicUsize; SAVE_FLOW_BOX_COUNT] =
    [const { AtomicUsize::new(0) }; SAVE_FLOW_BOX_COUNT];
/// Affirmative decisions per box id - 1.
pub static SAVE_FLOW_BOX_YES_COUNTS: [AtomicUsize; SAVE_FLOW_BOX_COUNT] =
    [const { AtomicUsize::new(0) }; SAVE_FLOW_BOX_COUNT];
/// Negative/cancel decisions per box id - 1.
pub static SAVE_FLOW_BOX_NO_COUNTS: [AtomicUsize; SAVE_FLOW_BOX_COUNT] =
    [const { AtomicUsize::new(0) }; SAVE_FLOW_BOX_COUNT];
/// Save flows that ended back in the world with NOTHING written (user said No/cancel, or a
/// recipe failure aborted the chain).
pub static SAVE_FLOW_ABORT_COUNT: AtomicUsize = AtomicUsize::new(0);
/// UNDECIDABLE confirm boxes, per box id - 1: the dialog stopped being a MessageBoxDialog
/// (freed/reused) or reported that it had emitted a result we could not map to a button. These
/// are FAILURES, deliberately kept OUT of the No counters: a box we could not read is not the
/// user pressing No, and conflating the two makes an agent-invented answer indistinguishable
/// from a real one. An undecidable box always ends the flow WITHOUT writing.
pub static SAVE_FLOW_BOX_UNDECIDABLE_COUNTS: [AtomicUsize; SAVE_FLOW_BOX_COUNT] =
    [const { AtomicUsize::new(0) }; SAVE_FLOW_BOX_COUNT];
/// Times a captured confirm-box dialog failed its structural identity check (its vtable no
/// longer carries `CS::MessageBoxDialog::Update` in slot 2) -- the object was freed or reused
/// while we were polling it. Subset of `SAVE_FLOW_BOX_UNDECIDABLE_COUNTS`.
pub static SAVE_FLOW_BOX_IDENTITY_LOST_COUNT: AtomicUsize = AtomicUsize::new(0);
/// `CS::MenuJob::EmitResult` observations attributed to the live confirm box: the emitted
/// `MenuJobResult` state (2 = Success/Yes, 3 = Failed/No or cancel) plus the dialog it came
/// from, latched by the emit hook for the save-flow poll. `..._DIALOG` 0 = nothing captured.
pub static SAVE_FLOW_BOX_EMIT_DIALOG: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_FLOW_BOX_EMIT_STATE: AtomicUsize = AtomicUsize::new(0);
/// Emitted-result observations for the live confirm box (diagnostic count).
pub static SAVE_FLOW_BOX_EMIT_COUNT: AtomicUsize = AtomicUsize::new(0);
/// The confirm box's `MenuJobResult` state AS BUILT, sampled the moment the builder hook
/// captures the dialog (stored as a `u32` bit pattern). The poll only believes that field
/// once it has CHANGED away from this baseline, and refuses to use it at all when the
/// baseline is already terminal -- the discipline that the 2026-07-28 defect was missing,
/// where a value present at construction was mistaken for the user's answer.
pub static SAVE_FLOW_BOX_RESULT_BASELINE: AtomicUsize = AtomicUsize::new(0);
/// Install flag for the `CS::MenuJob::EmitResult` observer hook (0 = not installed).
pub static MENU_JOB_EMIT_RESULT_INSTALLED: AtomicUsize = AtomicUsize::new(0);
/// Submitted confirm boxes whose `CS::MessageBoxDialog` build was never captured within
/// `SAVE_FLOW_BOX_BUILD_TIMEOUT_TICKS` -- the recipe produced no visible box (failure path).
pub static SAVE_FLOW_BOX_BUILD_TIMEOUT_COUNT: AtomicUsize = AtomicUsize::new(0);
/// 1 once a MessageBoxBuilder recipe RVA failed its prologue byte check: the overwrite confirm
/// cannot be built on this build. Save Game still opens the destination list (that needs no
/// message box), and a destination that would OVERWRITE an existing file is REFUSED rather than
/// written unconfirmed -- a free name still commits.
pub static SAVE_FLOW_RECIPE_UNAVAILABLE: AtomicUsize = AtomicUsize::new(0);
/// Destination picks refused because the overwrite confirm could not be built on this build.
/// Non-zero means a user chose an existing file and nothing was written; the free-name path is
/// unaffected.
pub static SAVE_DEST_OVERWRITE_UNCONFIRMABLE_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Dialog the confirm box is built against and submitted to (its MenuJob queue at +0x10 and
/// MenuWindow list at +0x50). 0 = the System/Quit dialog captured at the row press. The in-game
/// destination browser sets the live `05_010` ProfileLoadDialog instead, exactly like the game's
/// own load-confirm (`FUN_1409a4670` submits its confirm to `profile_load_dialog+0x10`), so the
/// confirm never contends with the System dialog queue that still owns the open picker window job.
pub static SAVE_FLOW_BOX_HOST_DIALOG: AtomicUsize = AtomicUsize::new(0);

// ---- save destination browser (save-game-flow WP3) ----
/// 1 while the live `05_010` picker is the save-DESTINATION chooser (the Save Game row opens it
/// directly) rather than the load-source browser. Cleared by `save_picker_reset` like the rest of the picker latches.
pub static SAVE_PICKER_DEST_MODE: AtomicUsize = AtomicUsize::new(0);
/// System/Quit PropertyEditDialog the live picker window was submitted from. The load-source
/// picker resolves it from its row action object; the destination picker is opened by the save
/// flow, which has the dialog but no action object. The menu-pump resubmit reopens through it.
pub static SAVE_PICKER_SYSTEM_DIALOG: AtomicUsize = AtomicUsize::new(0);
/// Menu-pump pending: open the destination browser from `system_quit_menu_window_run_post` (the
/// proven menu-job submit context). Set by the Save Game row press, and again whenever the OS
/// surface has to re-show its Save-As after a declined overwrite.
pub static SAVE_DEST_OPEN_PICKER_PENDING: AtomicUsize = AtomicUsize::new(0);
/// Times the menu pump tried to open the destination browser and LEFT THE REQUEST ARMED because no
/// picker ran (a MenuJob the dialog's queue deferred). The direct oracle for the reopen loop of bd
/// `er-effects-rs-rsxi`: a picker that RAN -- including one the user cancelled -- discharges the
/// request, so with the OS surface active this must read 0. Any positive value there means a
/// terminal outcome was retried, which is the loop that trapped the user.
pub static SAVE_DEST_PICKER_OPEN_RETRY_COUNT: AtomicUsize = AtomicUsize::new(0);
/// 1 once a destination has been chosen and confirmed: the save-flow tick closes the menus with
/// the commit staged as soon as the picker window has finished tearing down.
pub static SAVE_DEST_COMMIT_PENDING: AtomicUsize = AtomicUsize::new(0);
/// 1 while the scoped write-open redirect window is armed (`oracle_save_dest_redirect_armed`):
/// a `CreateFileW` write-open of the live save leaf is diverted to the chosen destination.
pub static SAVE_DEST_REDIRECT_ARMED: AtomicUsize = AtomicUsize::new(0);
/// Write-opens diverted to the destination during the armed window. One PER DIRTY BLOCK, not one
/// per commit: the native save takes the in-place path (`FUN_1424142e0`) whenever every supplied
/// block still fits its existing entry, and that opens the container once per block. Only the full
/// rebuild (`FUN_142413860`) is a single whole-buffer write. Measured 2026-07-28: 2 hits for one
/// Save Game commit. Zero hits is the failure -- any positive count is normal.
pub static SAVE_DEST_REDIRECT_HITS: AtomicUsize = AtomicUsize::new(0);
/// Destinations pre-seeded with a byte copy of the live container before the fire. The in-place
/// block writer seeks to offsets read from the live index, so an unseeded destination receives a
/// sparse fragment instead of a save.
pub static SAVE_DEST_SEEDED_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Commits aborted because the destination seed could not be written. The request is NOT fired.
pub static SAVE_DEST_SEED_FAIL_COUNT: AtomicUsize = AtomicUsize::new(0);
/// 1 once THIS commit's verified destination parsed as a structurally COMPLETE BND4 container: its
/// own entry index accounts for every byte up to EOF. This is the check that separates a loadable
/// container from a file that merely has the right length. Per-commit: cleared by
/// [`save_dest_reset_commit_verdicts`] at every arm.
pub static SAVE_DEST_TARGET_STRUCTURE_OK: AtomicUsize = AtomicUsize::new(0);
/// Commits whose destination IS the loaded save -- a browsed pick (or `[ new ]` in the loaded
/// save's own folder) that resolves back to it. Non-zero means this flow deliberately rewrote the user's live save file -- the ONLY
/// sanctioned way that happens, and the counter that keeps such a rewrite from reading as an
/// anonymous mutation or a suppression leak.
pub static SAVE_DEST_LIVE_OVERWRITE_COUNT: AtomicUsize = AtomicUsize::new(0);
/// 1 if the loaded save's `.bak` twin moved during THIS DESTINATION commit. Not a failure: the
/// native backup step (`FUN_142410830`) is not redirected and can only copy the untouched live
/// container over its own backup. Named so the movement is never unattributed. Per-commit: cleared
/// by [`save_dest_reset_commit_verdicts`] at every arm.
pub static SAVE_DEST_LIVE_BAK_MUTATED: AtomicUsize = AtomicUsize::new(0);
/// Destination browsers opened (one per Save Game row press, plus a re-open after a declined
/// overwrite on the OS surface).
pub static SAVE_DEST_PICKER_OPEN_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Destination picks that landed on an EXISTING file (a pre-existing row, or `[ new ]` whose
/// filename already exists in the browsed folder) -- these go through the Box3 overwrite confirm.
pub static SAVE_DEST_TARGET_EXISTING_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Destination picks that created a NEW file via `[ new ]` (no Box3; nothing is overwritten).
pub static SAVE_DEST_TARGET_NEW_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Destination commits staged (a target was chosen and confirmed; the menus are closing).
pub static SAVE_DEST_COMMIT_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Destination browsers abandoned (backed out / closed without choosing) -- nothing is written.
pub static SAVE_DEST_CANCEL_COUNT: AtomicUsize = AtomicUsize::new(0);
/// 1 once THIS destination commit verified: the target exists, starts with `BND4`, matches the live
/// container size, and changed on disk during the armed window. Per-commit: cleared by
/// [`save_dest_reset_commit_verdicts`] at every arm. Cumulative failure history lives in
/// [`SAVE_DEST_COMMIT_FAIL`].
pub static SAVE_DEST_TARGET_WRITTEN_OK: AtomicUsize = AtomicUsize::new(0);
/// Destination commits whose target verification FAILED (missing/short/unchanged target, or zero
/// redirect hits): the user's save did not land where they asked. Cumulative for the process.
pub static SAVE_DEST_COMMIT_FAIL: AtomicUsize = AtomicUsize::new(0);
/// 1 if the LIVE save file changed during THIS destination commit -- the redirect leaked and the
/// loaded save was overwritten anyway. Hard failure: the pre-fire snapshot is restored over it.
/// Per-commit: cleared by [`save_dest_reset_commit_verdicts`] at every arm, with the process-wide
/// count kept in [`SAVE_DEST_LIVE_FILE_MUTATED_TOTAL`].
pub static SAVE_DEST_LIVE_FILE_MUTATED: AtomicUsize = AtomicUsize::new(0);
/// Every leak this process ever saw: incremented, never cleared, wherever
/// [`SAVE_DEST_LIVE_FILE_MUTATED`] is raised. The per-commit flag has to be cleared at arm time or
/// one leak condemns every later commit, and a leak that the snapshot restore then repaired
/// increments no other cumulative counter -- so without this the worst event the flow can produce
/// would be erasable by the next arm.
pub static SAVE_DEST_LIVE_FILE_MUTATED_TOTAL: AtomicUsize = AtomicUsize::new(0);
// ---- DESTINATION-COMMIT SAFETY ORACLES (2026-07-29) ----
// Every counter below names a refusal, a deferral, or a fact the commit could not establish.
// They exist because the previous shape of this flow could destroy the loaded save while its
// log read "restored pre-fire snapshot ok=true": a decision it got wrong had no name, so no
// run could report it. Each of these is that missing name.
/// Commits refused because the destination could not be PROVEN either identical to, or distinct
/// from, the loaded save (a handle-identity probe that neither succeeded nor said "absent").
/// Firing on an unproven answer is what turns a save into a self-redirect that restores the
/// pre-save snapshot over the save that just succeeded, so the commit refuses instead.
pub static SAVE_DEST_IDENTITY_UNKNOWN_ABORT: AtomicUsize = AtomicUsize::new(0);
/// Browsed destinations PROVEN to be the loaded save by handle identity while their path strings
/// differed (the Wine `C:\users\steamuser\...` vs `Z:\...\pfx\drive_c\users\steamuser\...`
/// spelling of one file). Non-zero means a self-redirect was blocked and the commit took the
/// sanctioned overwrite-the-loaded-save path instead.
pub static SAVE_DEST_SELF_REDIRECT_BLOCKED: AtomicUsize = AtomicUsize::new(0);
/// Destination commits refused because the SL save-job body observer is not installed. Without
/// it the writer's completion is unobservable, and the redirect window would have to be torn
/// down on a tick count -- which can close it between two of the in-place writer's per-block
/// opens and patch the rest into the loaded save.
pub static SAVE_DEST_NO_WRITER_OBSERVER_ABORT: AtomicUsize = AtomicUsize::new(0);
/// Write-opens of a save-container leaf that were NOT diverted because their directory is not
/// the loaded save's. Every one of these would previously have been rewritten into the user's
/// chosen destination purely because its file name matched.
pub static SAVE_DEST_FOREIGN_OPEN_PASSED: AtomicUsize = AtomicUsize::new(0);
/// Teardown attempts deferred because the native writer was still inside a save-job body. The
/// redirect window must span every one of the in-place writer's per-block opens.
pub static SAVE_DEST_DISARM_DEFERRED: AtomicUsize = AtomicUsize::new(0);
/// Redirect windows torn down WITHOUT positive evidence that the writer ever ran (the enqueue
/// was forwarded, no job body ever started, and the extended teardown bound elapsed). A failure
/// oracle: the commit is over and nothing can say whether the writer will still appear.
pub static SAVE_DEST_DISARM_UNPROVEN: AtomicUsize = AtomicUsize::new(0);
/// 1 if the loaded save's stat could not be READ at THIS commit's verification. Distinct from
/// [`SAVE_DEST_LIVE_FILE_MUTATED`]: unreadable is not changed, and treating it as changed is
/// what triggered a blind whole-container overwrite of the live save on a transient stat error.
/// Per-commit: cleared by [`save_dest_reset_commit_verdicts`] at every arm; every occurrence also
/// increments the cumulative [`SAVE_DEST_RESTORE_SUPPRESSED`].
pub static SAVE_DEST_LIVE_STAT_UNREADABLE: AtomicUsize = AtomicUsize::new(0);
/// Restores of the pre-fire snapshot that were DECLINED: the loaded save's stamp moved but its
/// bytes are unchanged, its stat is unreadable, or the destination turned out to be the same
/// file. Writing the snapshot in any of those cases destroys rather than protects.
pub static SAVE_DEST_RESTORE_SUPPRESSED: AtomicUsize = AtomicUsize::new(0);
/// Restores that were attempted and FAILED. The restore is temp-file + rename, so a failure
/// leaves the loaded save byte-for-byte as the writer left it rather than truncated.
pub static SAVE_DEST_RESTORE_FAILED: AtomicUsize = AtomicUsize::new(0);
/// Every 0/1 oracle that describes ONE destination commit, in a single list so the arm-time reset
/// and the per-commit export can never disagree about which oracles those are.
///
/// Each is STORED only on the branch that observes it and left untouched otherwise, so nothing
/// clears them by itself: after one verified commit `oracle_save_dest_target_written_ok` and
/// `..._structure_ok` stayed 1 for the life of the process, and every LATER failed commit
/// published a save that did not land as a save that did -- the one direction a proof oracle must
/// never fail in. The mutation/unreadable oracles latch the same way in reverse and condemn a good
/// commit for an earlier one's leak.
pub fn save_dest_commit_verdict_oracles() -> [&'static AtomicUsize; 6] {
    [
        &SAVE_DEST_REDIRECT_HITS,
        &SAVE_DEST_TARGET_WRITTEN_OK,
        &SAVE_DEST_TARGET_STRUCTURE_OK,
        &SAVE_DEST_LIVE_FILE_MUTATED,
        &SAVE_DEST_LIVE_BAK_MUTATED,
        &SAVE_DEST_LIVE_STAT_UNREADABLE,
    ]
}

/// Clear the previous commit's verdict, so what is exported is always THIS commit's result.
///
/// Called from every arm site (`save_dest_arm_redirect`, `save_dest_arm_live_overwrite`) -- the
/// point at which a commit becomes the one being scored. Cumulative history is deliberately NOT
/// reset with it: [`SAVE_DEST_COMMIT_FAIL`], [`SAVE_DEST_RESTORE_SUPPRESSED`],
/// [`SAVE_DEST_RESTORE_FAILED`] and [`SAVE_DEST_LIVE_FILE_MUTATED_TOTAL`] span the whole process,
/// so a run still reports every failure it ever had alongside the current verdict.
pub fn save_dest_reset_commit_verdicts() {
    for oracle in save_dest_commit_verdict_oracles() {
        oracle.store(0, Ordering::SeqCst);
    }
}
/// 1 when the CURRENT commit was fired on the degraded fail-open path (suppression never armed,
/// so no bypass token exists). These are real native saves; they are completed on the writer's
/// own job-completion signal, never on the token-consumption test, which can never move here.
/// Rewritten at every fire, so it always describes the commit stage 8 is waiting on.
pub static SAVE_FLOW_DEGRADED_FIRE: AtomicUsize = AtomicUsize::new(0);
/// Degraded-path commits that reached a verdict on the writer's job-completion signal.
pub static SAVE_FLOW_DEGRADED_COMPLETE_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Degraded-path commits that ended with the write UNOBSERVED (no job-completion observer, or
/// none arrived inside the watchdog). Reported as degraded, never as "the save did not happen".
pub static SAVE_FLOW_DEGRADED_UNOBSERVED_COUNT: AtomicUsize = AtomicUsize::new(0);
/// [`er_save_suppress::save_job_completions`] sampled immediately before the forced request was
/// fired. The teardown gate and the degraded completion test are both relative to this.
pub static SAVE_FLOW_SAVE_JOB_COMPLETIONS_AT_FIRE: AtomicUsize = AtomicUsize::new(0);

// ---- picked/loose save source: title-time ProfileSummary re-read + deser accounting -------------
//
// A save the user picks after boot has no `CS::ProfileSummary` record: the boot save-data job that
// would have read one already ran and passed through. Without a record the native Continue row has
// nothing to load, which is why every picked save used to be routed into a TITLE-TIME deserialize
// -- `0x14067b290`, a function whose only caller in the whole image is `CS::MoveMapStep::DoSaveStuff`
// (in-world). These counters make both halves of that visible.

/// Total autoload ticks that entered the direct-source summary refresh (throttle denominator).
pub static PICKED_SUMMARY_REFRESH_TICKS: AtomicUsize = AtomicUsize::new(0);
/// Real re-read attempts made (a file read + record rewrite), not ticks. Capped.
pub static PICKED_SUMMARY_REFRESH_ATTEMPTS: AtomicUsize = AtomicUsize::new(0);
/// How the picked container's summary came to be readable, if it did:
/// 0 = not (yet) readable, 1 = the game's own boot save-data read had already populated it,
/// 2 = this DLL re-read it from the staged container at the title.
pub static PICKED_SUMMARY_REFRESH_STATE: AtomicUsize = AtomicUsize::new(0);
/// Bitmask of slots whose records the re-read rewrote (bit N = slot N).
pub static PICKED_SUMMARY_REFRESH_SLOT_MASK: AtomicUsize = AtomicUsize::new(0);

// ---- record drift watch: did something overwrite the body-derived records? -----------------------
//
// The container carries two descriptions of a slot -- the `USER_DATA010` summary table the game
// deserializes, and the BODY that actually loads -- and they can disagree (measured: 6 of 10 slots
// on `100-Lilbro/ER0000.co2`, run br-20260903-204517-82d2). This DLL writes the body-derived
// version; the game's own boot read can then overwrite it with the stale one, and the loading
// screen renders whoever the record names. These make that overwrite visible and counted.

/// Ticks on which the drift watch found the target slot's record naming a DIFFERENT character than
/// the container's body gives it. **A correct run reports 0.**
pub static PICKED_SUMMARY_RECORD_DRIFTS: AtomicUsize = AtomicUsize::new(0);
/// Body-derived record rewrites performed BECAUSE of drift (capped by `REASSERT_MAX_REWRITES`).
pub static PICKED_SUMMARY_REASSERTS: AtomicUsize = AtomicUsize::new(0);
/// FNV-1a 64 of the target slot's name in the container BODY, plus its level -- the identity the
/// drift watch defends. `0` until a container has been read.
pub static PICKED_SUMMARY_BODY_NAME_HASH: AtomicU64 = AtomicU64::new(0);
/// Rune Level of the target slot in the container BODY. `0` until a container has been read.
pub static PICKED_SUMMARY_BODY_LEVEL: AtomicUsize = AtomicUsize::new(0);
/// `PICKED_SUMMARY_REFRESH_TICKS` at the refresh that armed the watch, plus 1 (`0` = unarmed).
pub static PICKED_SUMMARY_WATCH_ARMED_TICK: AtomicUsize = AtomicUsize::new(0);

/// Calls into the TITLE-TIME save deserialize `0x14067b290` from the full-read chain.
///
/// **A correct run reports 0.** Non-zero means a save was deserialized at the boot title rather
/// than in-world from `CS::MoveMapStep::DoSaveStuff`, which is the crash this counter exists to
/// stop being silent about (`gaitemInsTable[-1]` AV at `0x67141a`). It counts the call being made,
/// not the call surviving, so a run that dies inside the deserialize still leaves the 1 behind.
pub static TITLE_TIME_DESER_CALLS: AtomicUsize = AtomicUsize::new(0);
/// Slot (+1, so 0 means "never") passed to the most recent title-time deserialize.
pub static TITLE_TIME_DESER_LAST_SLOT: AtomicUsize = AtomicUsize::new(0);

/// Times the switch retired its OWN `menuData+0x5d` return-title request before creating the
/// incoming world. Non-zero on a switch means the request was served and cleared; a switch that
/// completes with this at 0 left the request set, which is the black-screen precondition (the
/// incoming child inherits it, walks 18->20, and `STEP_GameStepWait` tears the world down).
pub static SWITCH_RETURN_TITLE_REQUEST_RETIRED_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Times a GENUINELY LOADED world reverted to the title/new-game map default -- the black screen,
/// counted as a transition (real map id -> `FULLREAD_C30_M10_DEFAULT`) rather than as a level, so
/// the long stretch of every boot that legitimately sits at the default cannot trip it.
///
/// This is the run-stopping oracle for the second-load teardown. It is deliberately blind to HOW
/// the switch was driven, so a run driven through the real ProfileSelect rows and a run driven by
/// the diagnostic control file are scored by the same measurement.
pub static WORLD_LOST_TO_TITLE_COUNT: AtomicUsize = AtomicUsize::new(0);
