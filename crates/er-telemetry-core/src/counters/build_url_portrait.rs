//! The character panel's portrait after a build import, and whether the repair took.
//!
//! A build import rewrites `PlayerGameData` and writes no save, so the panel's own
//! `CSMenuProfModelRend` keeps dressing the model from a `CS::ProfileSummary` record that
//! still describes the previous loadout. This family measures the refresh that fixes
//! that: the record sync, the model rebuild it kicks, and the equipment and part-node
//! fingerprints that say whether the renderer came back wearing the imported gear rather
//! than merely being asked to.
//!
//! Split out of `counters.rs` as a pure code move: nothing is renamed and no initial value
//! changes. Every name here is re-exported from `er_telemetry_core::counters` with a glob, so
//! each consumer still spells it `er_telemetry_core::counters::<name>`.

use std::sync::atomic::{AtomicU64, AtomicUsize};

// ---- the character panel's portrait, after an import ------------------------------------------
// A build import mutates `PlayerGameData` and writes no save. The panel's portrait is the game's
// own `CSMenuProfModelRend`, dressed from a `CS::ProfileSummary` record, and the record only
// re-derives from the live character inside the two native save lanes -- so the portrait keeps
// showing the previous loadout until the dialog is rebuilt. These count the repair and, more
// importantly, measure whether it took: `_equip_verdict` compares the record's own equipment
// fingerprint against the renderer stage the model build actually reads.
/// Imports that reached the portrait refresh at all (one per applied import).
pub static BUILD_URL_PORTRAIT_REFRESH_ATTEMPTS: AtomicUsize = AtomicUsize::new(0);
/// `LiveSync::code()` of the most recent record sync: 0 never attempted, 1 synced, 2 no summary,
/// 3 slot out of range, 4 the native has no verified mapping for this build. Tri-plus-state on
/// purpose -- a counter that was never written must not read as a successful sync.
pub static BUILD_URL_PORTRAIT_RECORD_SYNC_STATE: AtomicUsize = AtomicUsize::new(0);
/// Syncs whose equipment fingerprint actually moved. Lower than the attempt count is not a
/// failure: re-importing the build already worn changes nothing, and neither does a sync that
/// follows the game's own save by a frame.
pub static BUILD_URL_PORTRAIT_RECORD_SYNCS: AtomicUsize = AtomicUsize::new(0);
/// The slot whose record was synced, plus one (`0` = none yet). Plus one because slot 0 is both a
/// real slot and the natural "nothing recorded" value.
pub static BUILD_URL_PORTRAIT_RECORD_SLOT_PLUS1: AtomicUsize = AtomicUsize::new(0);
/// The Rune Level the record carried after the sync. Compared against the import's own reported
/// level, this says the record now describes the imported character rather than the previous one.
pub static BUILD_URL_PORTRAIT_RECORD_LEVEL: AtomicUsize = AtomicUsize::new(0);
/// FNV-1a over the record's `ChrAsm::equipment_param_ids` after the sync (`0` = never read).
pub static BUILD_URL_PORTRAIT_RECORD_FINGERPRINT: AtomicU64 = AtomicU64::new(0);
/// The same fingerprint taken from the renderer's live stage-0 `ChrAsm` -- the block the per-frame
/// model-resource request reads, not the inbox the feed writes (`0` = never read).
pub static BUILD_URL_PORTRAIT_RENDERER_FINGERPRINT: AtomicU64 = AtomicU64::new(0);
/// `PortraitEquipmentVerdict::code()`: 0 unmeasured, 1 the renderer is dressing the model in the
/// record's gear, 2 it is still on the previous loadout. This is the field that says whether the
/// portrait re-rendered with the imported equipment; the kick count only says one was requested.
pub static BUILD_URL_PORTRAIT_EQUIP_VERDICT: AtomicUsize = AtomicUsize::new(0);
/// Model rebuilds this path actually kicked.
pub static BUILD_URL_PORTRAIT_KICKS: AtomicUsize = AtomicUsize::new(0);
/// Rebuilds it asked for and did not get -- no renderer for the slot, a renderer whose vtable is
/// not the profile renderer's, or a build already in flight. Counted apart from the kicks because
/// "nothing happened" and "it happened and did not help" are different defects.
pub static BUILD_URL_PORTRAIT_KICK_REFUSALS: AtomicUsize = AtomicUsize::new(0);
/// Ticks left in the bounded window that re-reads the renderer stage after a kick. The rebuild is
/// asynchronous (measured ~94ms, i.e. a handful of frames), so the verdict cannot be taken on the
/// frame the kick fires; the window is what keeps that from becoming an open-ended per-frame read.
pub static BUILD_URL_PORTRAIT_VERIFY_TICKS: AtomicUsize = AtomicUsize::new(0);
// ---- and whether the model was actually rebuilt, which the input comparison cannot see ---------
//
// Run `br-20260911-002901-a7e0` is why these exist. `_equip_verdict` read 1 -- the renderer's live
// stage-0 `ChrAsm` carried the imported ids -- and the player still saw the previous armour. A
// state write is not a draw: the portrait is a captured render, so the model has to be destroyed,
// rebuilt from the new rows and rasterized before a pixel moves. These observe the model object
// instead of its input, so the fields below can report the failure the input comparison scored as
// a pass.
/// The `CSChrAsmModelIns` (`renderer+0x778`) before the rebuild was asked for.
pub static BUILD_URL_PORTRAIT_MODEL_INS_BEFORE: AtomicUsize = AtomicUsize::new(0);
/// The same pointer at the last sample of the verify window.
pub static BUILD_URL_PORTRAIT_MODEL_INS_AFTER: AtomicUsize = AtomicUsize::new(0);
/// Set once the model instance has been read as null while the window was open -- what a teardown
/// looks like. This is the term that survives an allocator handing the replacement the address the
/// old model just freed, on which a pointer comparison alone would report no rebuild.
pub static BUILD_URL_PORTRAIT_MODEL_ABSENT_SEEN: AtomicUsize = AtomicUsize::new(0);
/// FNV-1a over the model's part-node array (`model_ins+0x28..+0x100`, the 27 slots the model submit
/// `FUN_1409e9ac0` walks and draws) before the rebuild was asked for.
pub static BUILD_URL_PORTRAIT_PARTS_BEFORE: AtomicU64 = AtomicU64::new(0);
/// The same fingerprint at the last sample. Different from `_BEFORE` means the model came back
/// wearing something else, which is the closest thing in RAM to "the picture changed".
pub static BUILD_URL_PORTRAIT_PARTS_AFTER: AtomicU64 = AtomicU64::new(0);
/// `PortraitRebuildVerdict`: 0 not measured, 1 rebuilt with different parts, 2 rebuilt with the
/// same parts, 3 never rebuilt (the input took and the image did not).
pub static BUILD_URL_PORTRAIT_REBUILD_VERDICT: AtomicUsize = AtomicUsize::new(0);
/// The headline field a run is judged on. `PortraitRenderVerdict`: 0 unproven, 1 proven (the input
/// matched and the model was torn down and rebuilt differently), 2 the input took but the image is
/// stale, 3 rebuilt unchanged, 4 the input was wrong. Only 1 is a pass, and reaching it requires
/// the model-object evidence that `_equip_verdict` alone cannot supply.
pub static BUILD_URL_PORTRAIT_RENDER_VERDICT: AtomicUsize = AtomicUsize::new(0);
/// Bitmask of `FD4StepTemplateBase` step indices seen at `renderer+0x40` while the window was open.
/// A data-change rebuild walks 6 -> 7 -> 8 -> 1 -> 2 -> 3 -> 4 -> 5 -> 6, so a mask holding only bit
/// 6 is a renderer that never moved, and bits 2 and 4 are the setup steps that promote the inbox
/// into the staged stage and the staged stage into live. Published as a diagnostic beside the
/// verdict rather than folded into it: the sampling cadence can miss a step the machine really did
/// pass through, and a headline that failed on that would be reporting its own sample rate.
pub static BUILD_URL_PORTRAIT_STEPS_SEEN: AtomicUsize = AtomicUsize::new(0);
/// The draw chain at the last sample: bit 0 the per-frame part-draw task is registered, bit 1 the
/// offscreen scene is registered with the render system, bit 2 the model's parts are attached to a
/// scene. All three are required for the headline verdict to pass, because a correct model that
/// nothing submits is still the previous picture.
pub static BUILD_URL_PORTRAIT_DRAW_BITS: AtomicUsize = AtomicUsize::new(0);
/// The renderer the build-import refresh asked to rebuild, so the draw task's detour can recognise
/// its own calls without walking the ten-slot table on every frame.
pub static BUILD_URL_PORTRAIT_TARGET_RENDERER: AtomicUsize = AtomicUsize::new(0);
/// Executions of the per-frame draw task `FUN_140bba7d0` for that renderer -- the function that
/// propagates the model's bones into every submodel and enqueues them into the offscreen pass, i.e.
/// the one that actually rasterizes. Counted unconditionally in the detour, unlike
/// `PROFILE_PERFRAME_HOOK_HITS`, which only counts frames where a look-at pose was applied.
///
/// Registration is not execution, and that distinction is the whole reason this exists: run
/// `br-20260911-005533-858a` had the task registered, the offscreen scene registered and the parts
/// in a scene -- all three draw bits -- on a screen that never changed. The renderer's own
/// `CSEzUpdateTask`s are driven by ResMan, which this repo has already measured under-scheduling
/// them (~4-19 times across a whole loading screen), so a registered task can simply not run.
pub static BUILD_URL_PORTRAIT_DRAW_TASK_CALLS: AtomicUsize = AtomicUsize::new(0);
/// That count as it stood when the rebuild was asked for. The verdict needs the delta, not the
/// total, because the task may have been running for the life of the dialog.
pub static BUILD_URL_PORTRAIT_DRAW_CALLS_AT_KICK: AtomicUsize = AtomicUsize::new(0);
/// `ChrAsmModelRes` entry 0's resolved param id (`res+0x30`) and the id that was requested
/// (`res+0x34`). Unequal means the new gear was asked for and its parts file has not finished
/// loading, which is a slow resource load rather than a broken rebuild -- and it is the one
/// explanation a 240-tick window is too short to distinguish from a failure on its own.
pub static BUILD_URL_PORTRAIT_MODELRES_RESOLVED: AtomicUsize = AtomicUsize::new(0);
pub static BUILD_URL_PORTRAIT_MODELRES_REQUESTED: AtomicUsize = AtomicUsize::new(0);
/// How many of the model-resource entries still have a request outstanding at the last sample.
pub static BUILD_URL_PORTRAIT_MODELRES_PENDING: AtomicUsize = AtomicUsize::new(0);
