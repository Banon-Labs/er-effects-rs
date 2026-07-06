// === Loading-screen portrait bug SEMAPHORES (2026-07-04) ==========================================
// Two user-reported bugs on the loading transition, resolved to RAM/pixel oracles derived from the
// captured `LOADING_BG_PORTRAIT_RGBA` (the content that feeds the loading-screen portrait display):
//   Bug A -- historically, the portrait rendered TOO SMALL unintentionally (correct content, ~256px
//            square). Root suspect: the `find_d3d12_resource_ex` "largest TEXTURE2D" scan picked a
//            small RT instead of the intended target RT (deterministic-pointer fix pending). Current
//            low-res experiments may intentionally trip the same size semaphore.
//   Bug B -- our NEUTRAL stats-panel texture (RGB 30,28,26) leaked onto the loading screen. Root
//            suspect: the same scan grabbed one of our 256x256 neutral CreateTpfResCap textures.
// These oracles let a monitor detect each condition live without reading the screenshot: they carry the
// captured dims, the neutral-color fraction, and once-seen latches stamped with the capture version.
/// Threshold (px): a captured portrait whose larger side is <= this is "too small" for monitoring.
/// The high-quality loading portrait target should be 1024x1024 after native supersampling, so anything
/// at or below the old 256/512 class remains a regression tripwire.
pub(crate) const LS_PORTRAIT_SMALL_MAX_SIDE: u32 = 512;
/// Latched capture width/height of the most recent loading-screen portrait capture (px, 0 if none).
pub(crate) static LS_PORTRAIT_LAST_W: AtomicUsize = AtomicUsize::new(0);
pub(crate) static LS_PORTRAIT_LAST_H: AtomicUsize = AtomicUsize::new(0);
/// Percent (0..100) of sampled texels within tolerance of the neutral bg color in the most recent
/// capture. High == our neutral texture is the portrait source (Bug B).
pub(crate) static LS_PORTRAIT_LAST_NEUTRAL_PCT: AtomicUsize = AtomicUsize::new(0);
/// Once-seen latch: a capture with correct (non-neutral) content but too-small dims (Bug A). Stores the
/// `LOADING_BG_PORTRAIT_RGBA_VERSION` at first detection (0 == never seen).
pub(crate) static LS_PORTRAIT_TOO_SMALL_SEEN_VERSION: AtomicUsize = AtomicUsize::new(0);
/// Once-seen latch: a capture that is our neutral texture (Bug B). Stores the capture version at first
/// detection (0 == never seen).
pub(crate) static LS_PORTRAIT_NEUTRAL_LEAK_SEEN_VERSION: AtomicUsize = AtomicUsize::new(0);
/// Count of portrait captures REJECTED by the readiness gate (neutral or too-small) -- i.e. transient
/// wrong-source frames that were kept OFF the loading screen. >0 with both seen-versions set means the
/// gate is actively suppressing the two bugs.
pub(crate) static LS_PORTRAIT_REJECTED_PUBLISHES: AtomicUsize = AtomicUsize::new(0);
/// The LOADING_BG_PORTRAIT_RGBA_VERSION last uploaded into the displayed now-loading texture by
/// maybe_reforge_loading_portrait. usize::MAX = never. Re-upload only when the version advances (new live
/// frame) so the displayed loading-screen head TRACKS the look-at, while never per-frame-hammering a
/// dim-mismatched/freed texture (the old crash). One log latch for the first successful upload.
pub(crate) static LOADING_BG_REFORGE_VERSION: AtomicUsize = AtomicUsize::new(usize::MAX);
pub(crate) static LOADING_BG_REFORGE_LOGGED: AtomicUsize = AtomicUsize::new(0);
/// 1 if the read-back portrait looks like a SOLID-COLOR-CHECKER PLACEHOLDER (magenta/white|yellow cover or
/// an unrendered RT) rather than a real shaded 3D head -- see `portrait_looks_like_checker`. The
/// `..._gx_nonblack` flag alone is a FALSE POSITIVE (a bright checker passes max(R,G,B)>24), so REAL-face
/// proof = `nonblack && !is_checker`. Exposed as `oracle_loading_bg_portrait_is_checker`.
pub(crate) static LOADING_BG_PORTRAIT_IS_CHECKER: AtomicUsize = AtomicUsize::new(0);
/// Read-back portrait dimensions packed as `(width << 16) | height`. 0 until captured. Exposed as
/// `oracle_loading_bg_portrait_gx_dims`.
pub(crate) static LOADING_BG_PORTRAIT_DIMS: AtomicUsize = AtomicUsize::new(0);
/// The DXGI_FORMAT value of the read-back offscreen render target. 0 until captured. Exposed as
/// `oracle_loading_bg_portrait_gx_format`.
pub(crate) static LOADING_BG_PORTRAIT_FORMAT: AtomicUsize = AtomicUsize::new(0);
/// CSMenuProfModelRend "marked-for-delete" byte (renderer+0x756) and the CSChrAsmModelIns* pointer
/// (renderer+0x778) that is non-null only once the character model has finished async-loading -- the
/// real "portrait is rendering" gate (the +0x754/+0x755 bytes are only a setup-submitted latch).
pub(crate) const PROFILE_RENDERER_MARKED_DELETE_OFFSET: usize = 0x756;
pub(crate) const PROFILE_RENDERER_MODEL_INS_OFFSET: usize = 0x778;
/// `CSMenuAsmModelRend`'s row-major model transform (`renderer+0x900..0x93f`), copied into the
/// rendered `CSModelIns` every rabbit-task tick by `FUN_140bba820`. The identity default is loaded from
/// `FLOAT_ARRAY_1430b07a0`; when this changes, its Z axis is the model's backing orientation and the
/// portrait camera should orbit to the model's face, not a hard-coded screen yaw.
pub(crate) const PROFILE_RENDERER_MODEL_MATRIX_OFFSET: usize = 0x900;
/// Per-slot model-facing yaw latched from the first live model pose/matrix and added to the profile camera
/// orbit. This keeps the loading portrait facing the viewer even when the model/root pose is off-axis.
pub(crate) static PROFILE_CAM_FACE_YAW: std::sync::Mutex<[Option<f32>; 10]> =
    std::sync::Mutex::new([None; 10]);
pub(crate) static PROFILE_CAM_FACE_YAW_LATCHED_MASK: AtomicUsize = AtomicUsize::new(0);
/// `CSGxTexture` GPU-resource child pointer (gx+0x10): non-null once at least one offscreen draw has
/// uploaded the texture. Refcount is the uniform DLReferenceCountObject i32 at obj+0x8.
pub(crate) const GX_TEXTURE_GPU_RESOURCE_OFFSET: usize = 0x10;
pub(crate) const GX_TEXTURE_REFCOUNT_OFFSET: usize = 0x8;
/// The GPU child of a profile-portrait `CSGxTexture` (gx+0x10) may be a `CSOffscreenGxTexture` C++
/// WRAPPER rather than a raw `ID3D12Resource`. Its C++ vtable lives at `game_base + this RVA`; when
/// `*(gpu_child)` equals that absolute address the gpu_child is a wrapper and the real
/// `ID3D12Resource` lives at one of the offsets below. The underlying COM resource MUST be resolved
/// before any D3D12 call -- invoking a COM vtable method on a non-COM pointer crashes. See
/// `experiments::gpu_readback::readback_offscreen_rgba8`.
pub(crate) const PROFILE_GX_GPU_WRAPPER_VTABLE_RVA: usize = 0x2b80278;
/// Wrapper -> real `ID3D12Resource` primary slot (`gpu_child + 0x18`); used when non-null.
pub(crate) const PROFILE_GX_GPU_WRAPPER_RESOURCE_PRIMARY_OFFSET: usize = 0x18;
/// Wrapper -> real `ID3D12Resource` fallback slot (`gpu_child + 0x10`); used when +0x18 is null.
pub(crate) const PROFILE_GX_GPU_WRAPPER_RESOURCE_FALLBACK_OFFSET: usize = 0x10;
/// DETERMINISTIC content-RT resolution chain (RE'd from a live /proc dump 2026-06-29, bd
/// live-portrait-d3d12-resource-buried-in-gx-wrapper-nest). The vkd3d ID3D12Resource is 4 fixed hops from
/// the CSGxTexture -- following these avoids the memory-scan+QI that races the teardown free:
///   srv_gx +0x10  -> CSOffscreenGxTexture  (vt game_base+PROFILE_GX_GPU_WRAPPER_VTABLE_RVA 0x2b80278)
///   +0x18         -> holder A              (vt game_base+0x2f05a60)
///   +0x40         -> holder B              (vt game_base+0x30a3ef0)
///   +0x20         -> ID3D12Resource        (vt in d3d12core.dll)
/// Each intermediate's vtable is validated so a layout change fails closed (no readback) instead of
/// dereferencing garbage. The final object's vtable must land in a d3d12 module.
pub(crate) const GX_RES_CHAIN_HOLDER_A_OFFSET: usize = 0x18;
pub(crate) const GX_RES_CHAIN_HOLDER_A_VTABLE_RVA: usize = 0x2f05a60;
pub(crate) const GX_RES_CHAIN_HOLDER_B_OFFSET: usize = 0x40;
pub(crate) const GX_RES_CHAIN_HOLDER_B_VTABLE_RVA: usize = 0x30a3ef0;
pub(crate) const GX_RES_CHAIN_RESOURCE_OFFSET: usize = 0x20;
/// TpfResCap container (the 0xb8 object CreateTpfResCap returns): texture count and the array of
/// `TexResCap*`. We rewrite `array[0]`'s `+0x78` CSGxTexture to the kept portrait.
pub(crate) const TPF_RESCAP_CONTAINER_COUNT_OFFSET: usize = 0x78;
pub(crate) const TPF_RESCAP_CONTAINER_ARRAY_OFFSET: usize = 0x80;
/// No-delay portrait render: the ProfileSelect portrait is a live per-frame 3D model render that the
/// fast autoload never finishes before the Continue teardown. To get it WITHOUT delaying boot we
/// SPARE slot-0's renderer from the teardown and keep driving its offscreen render into the (free,
/// multi-second) now-loading screen until the character model latches, then capture it.
/// Teardown-all `FUN_1409b2f00` (deobf 0x1409b2db0): unconditional 10-slot loop of
/// `FUN_140e77540(GLOBAL_CSDelayDeleteMan, table[i]); table[i]=0`. The enqueue is null-guarded, so we
/// null `table[slot]` before the original to spare that slot (its enqueue becomes a no-op).
pub(crate) const PROFILE_RENDERER_TEARDOWN_RVA: usize = 0x9b2db0;
/// Offscreen-draw driver `FUN_140bb8d90` (deobf 0x140bb8ca0): `fn(renderer)` -> submits the offscreen
/// render via `FUN_140bb73a0(*(renderer+0xa8))`, reading the global GxDrawContext itself (no arg).
/// The menu-owned per-frame caller stops at Continue, so we call this ourselves each frame.
pub(crate) const PROFILE_OFFSCREEN_DRIVE_RVA: usize = 0xbb8ca0;
/// Count of render-thread offscreen drives issued from the Present hook (gate
/// `portrait_render_drive_enabled`). Exposed as `oracle_portrait_render_drive_hits` -- proves the drive
/// ran on the render thread during loading; pair with `oracle_loading_bg_portrait_is_checker` flipping to
/// 0 (real face) as the success signal that the drive actually rasterized the head into the RT.
pub(crate) static PROFILE_RENDER_DRIVE_HITS: AtomicUsize = AtomicUsize::new(0);
/// Profile-portrait draw step `FUN_1409aa3e0` (dump VA) -> deobf RVA 0x1409aa290 (content-unique, shift
/// -0x150, ground-truthed via dump-deobf-shift). No-arg: loops the 10-slot renderer title table at
/// base+0x3d6d8d0 and, for each non-null slot, calls the offscreen-draw thunk (PROFILE_OFFSCREEN_DRIVE_RVA)
/// to rasterize that portrait into its offscreen RT. The engine only invokes it on profile data-change
/// (e.g. the reset/save menu action `FUN_14082bb20`), NOT per frame -- so the thumbnails are static. We
/// call it ourselves every frame from a DRAW-PHASE recurring task (CSTaskGroupIndex::GameSceneDraw),
/// where a GX frame is actively recording so the GX subcontext pool pop succeeds (it returns 0 -> a black
/// no-op at FrameBegin, before the frame records -- the real reason a game-task-thread drive went black).
pub(crate) const PROFILE_DRAW_STEP_RVA: usize = 0x9aa290;
/// Profile-renderer table BUILDER `FUN_1409af4f0` (dump) -> deobf RVA 0x1409af3a0 (content-unique, shift
/// -0x150). No-arg: tears down the existing 10 (FUN_1409b2f00, no-op on an already-null table) then
/// HeapAllocs (0xa30, align 0x10, GLOBAL_GfxHeapAllocator) + ctor's a fresh CSMenuProfModelRend into each
/// of the 10 title-table slots (base+0x3d6d8d0), each self-registering its build/draw tasks with ResMan.
/// We call it ONCE post-Continue (now-loading, table torn down) to repopulate the table so the existing
/// mark+refresh feed + per-frame look-at + draw + oracle re-engage on the loading screen. RE-confirmed the
/// ctor is self-contained off process-lifetime singletons (no TitleTopDialog dependency).
pub(crate) const PROFILE_TABLE_BUILDER_RVA: usize = 0x9af3a0;
/// One-shot latch: set when we've rebuilt the profile table for the current load window; cleared when
/// now-loading drops, so each load rebuilds at most once (no per-frame churn / teardown thrash).
pub(crate) static PROFILE_LOADSCREEN_REBUILT: AtomicUsize = AtomicUsize::new(0);
/// Count of post-Continue profile-table (re)builds for the loading-screen portrait (telemetry/sweep).
pub(crate) static PROFILE_LOADSCREEN_TABLE_BUILDS: AtomicUsize = AtomicUsize::new(0);
/// 1 while the currently populated profile-renderer table is the loading-screen-owned table we built,
/// not the ProfileSelect/menu table. Product portrait consumers must key on this ownership bit so the
/// early/menu static renderer is ignored instead of becoming a visible or source dependency.
pub(crate) static PROFILE_LOADSCREEN_TABLE_OWNED: AtomicUsize = AtomicUsize::new(0);
// PER-SLOT PROFILE BUILD KICK (replaces the engine's GLOBAL refresh in OUR post-Continue feed). The
// global refresh FUN_1409aa7d0 iterates all 10 slots and kicks every real+marked one -- on a multi-
// character save that built ALL the save's characters mid-load, and the readback scan flipped onto a
// foreign slot's RT (the cross-slot portrait swap, run strip-default-drive-20260702-194018). Writing the
// +0x754/+0x755 latches on unconfigured renderers to suppress those kicks CRASHED (GX command-queue
// overflow -> null slot write at deobf 0x141aeaf05, run portrait-swap-fix-noteardown-20260702-212024), so
// instead we never call the global refresh post-Continue and replicate its per-slot body for ONLY the
// loaded slot. All RVAs below are the refresh body's callees (dump FUN_1409aa7d0), dump->deobf mapped
// content-unique via scripts/dump-deobf-shift.py on 2026-07-02.
/// `FUN_140261c30(summary, slot) -> record*` (dump 0x140261c30): the slot's ProfileSummary record.
pub(crate) const PROFILE_SUMMARY_RECORD_RVA: usize = 0x261b80;
/// `CS::FaceData::GetFaceDataBuffer(record+0x38, true) -> FaceDataBuffer*` (dump 0x140252210).
pub(crate) const PROFILE_FACEDATA_BUFFER_RVA: usize = 0x252160;
/// `FUN_140bbe290(renderer, record+0x1a8)` (dump 0x140bbe290): model-source/ChrAsm equip config
/// (weapons cleared, default protectors, the record's equip source installed).
pub(crate) const PROFILE_RENDERER_SET_MODEL_SOURCE_RVA: usize = 0xbbe1a0;
/// `FUN_140bb9950(renderer, facedata_buffer)` (dump): `FaceDataBuffer::Copy(renderer+0x630, buf)`.
pub(crate) const PROFILE_RENDERER_SET_FACEDATA_RVA: usize = 0xbb9860;
/// `FUN_140bb9960(renderer, 1)` (dump 0x140bb9960): byte setter the refresh always passes 1.
pub(crate) const PROFILE_RENDERER_SET_FLAG_ONE_RVA: usize = 0xbb9870;
/// `FUN_140bb9970(renderer, record->0x290)` (dump 0x140bb9970).
pub(crate) const PROFILE_RENDERER_SET_BYTE290_RVA: usize = 0xbb9880;
/// `FUN_140bb9980(renderer, record->0x294)` (dump 0x140bb9980).
pub(crate) const PROFILE_RENDERER_SET_BYTE294_RVA: usize = 0xbb9890;
/// `FUN_140bb8cf0(renderer, slot*2)` (dump): `*(renderer+0x9a8) = slot*2` (the stream/pair index).
pub(crate) const PROFILE_RENDERER_SET_STREAM_INDEX_RVA: usize = 0xbb8c00;
/// `FUN_140bb9900(renderer)` (dump): `renderer+0x754 = 1` (the "load requested" idempotency latch).
pub(crate) const PROFILE_RENDERER_SET_REQ_754_RVA: usize = 0xbb9810;
/// `FUN_140bb9920(renderer)` (dump): `renderer+0x755 = 1` (set together with +0x754 at kick time).
pub(crate) const PROFILE_RENDERER_SET_REQ_755_RVA: usize = 0xbb9830;
/// RAM oracle: target-slot build kicks fired via the per-slot replica (`oracle_portrait_target_kicks`).
/// Expected >=1 per loading window; 0 means the loaded character's model was never requested.
pub(crate) static PROFILE_TARGET_KICKS: AtomicUsize = AtomicUsize::new(0);
/// RAM oracle tripwire: max count of NON-target renderers observed holding a live model (+0x778 != 0)
/// during our feed window (`oracle_portrait_foreign_models`). >0 = another character was built on the
/// loading screen -- the swap-bug precondition returned.
pub(crate) static PROFILE_FOREIGN_MODELS_MAX: AtomicUsize = AtomicUsize::new(0);
/// RAM oracle (`oracle_portrait_multi_model_publish_skips`): draw-tick publishes suppressed because more
/// than one profile model was live (the game's Load Profile 10-thumbnail window). >0 confirms the
/// only-target-live gate engaged and kept the cascade of other characters off the loading screen.
pub(crate) static PROFILE_MULTI_MODEL_PUBLISH_SKIPS: AtomicUsize = AtomicUsize::new(0);
/// Consecutive ticks the profile-renderer title table has been observed EMPTY. The menu's own
/// teardown+rebuild is synchronous (FUN_1409af3a0 tears down then re-ctors in one call), so the table is
/// never seen empty across our async ticks during menu cycling -- only a real Continue (teardown with NO
/// rebuild) leaves it sustained-empty. A short streak therefore fires the post-Continue own-renderer build
/// EARLY (right after teardown, ~17s) instead of waiting for the now-loading flag (~21s on a fast load,
/// too late for ResMan to build the model). Reset to 0 whenever a populated table is observed.
pub(crate) static PROFILE_TABLE_EMPTY_STREAK: AtomicUsize = AtomicUsize::new(0);
/// Empty-table streak (ticks) that triggers the early post-Continue own-renderer build. Small: the menu
/// never shows a multi-tick empty table, so even a few frames unambiguously means "Continue happened."
pub(crate) const PROFILE_TABLE_EMPTY_STREAK_BUILD_THRESHOLD: usize = 3;
/// Set once we've observed a POPULATED profile table (the menu built it -> the engine/ResMan are up and we
/// are past the title screen). The early empty-streak build MUST require this: at boot the table is empty
/// too, and calling the builder before the engine is ready crashes inside FUN_1409af3a0 (observed
/// 2026-06-29: access-violation in the builder at title, game_man unresolved). A later empty table after
/// this latch is set therefore means a genuine Continue teardown, when the builder is safe to call.
pub(crate) static PROFILE_TABLE_WAS_POPULATED: AtomicUsize = AtomicUsize::new(0);
/// The spared slot-0 CSMenuProfModelRend renderer (0 until the Continue teardown spares it). Its
/// global ResMan model-update task keeps loading/animating the model while the object lives.
pub(crate) static LOADING_BG_PORTRAIT_SPARED_RENDERER: AtomicUsize = AtomicUsize::new(0);
/// Pre-recorded spare CANDIDATE: the target slot's renderer pointer, captured by force_profile_render at
/// the MENU on a frame where its model is actually built (+0x778 valid). Because the menu cycles model_ins
/// (~4-11% of frames), capturing the candidate during the long menu dwell is robust; the teardown-spare
/// hook then protects THIS exact renderer (nulls its table entry) regardless of whether model_ins happens
/// to be valid at the single teardown instant. 0 = none recorded yet.
pub(crate) static PROFILE_SPARE_CANDIDATE: AtomicUsize = AtomicUsize::new(0);
/// The model_ins (renderer+0x778) captured at the instant the spare candidate is recorded -- when the
/// model is still built. By spare-time the renderer's own +0x778 field is already zeroed, so this holds
/// the only reference to the model object; used to probe whether the model OBJECT survives Continue (vs
/// just the renderer's pointer being cleared) and, if it does, to re-attach it post-Continue.
pub(crate) static PROFILE_SPARE_CANDIDATE_MODEL: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PROFILE_RENDERER_TEARDOWN_HOOK_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static PROFILE_RENDERER_TEARDOWN_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
/// Diagnostic + REPAIR hook on the native profile-portrait builder (`PROFILE_RENDERER_REFRESH_RVA`
/// = `FUN_1409aa7d0`). The builder walks all 10 `DAT_143d6d8d0` entries and derefs
/// `table[slot]+0x754` with NO null check for every slot whose profile record exists -- the
/// er-effects-rs-j3r AV. Its table setup (`PROFILE_TABLE_BUILDER_RVA`) is called from exactly ONE
/// native site, the TitleTopDialog constructor (Ghidra xref), so our cloned in-world ProfileSelect
/// reopens run the builder against whatever the last teardown left; by the 3rd in-session open the
/// table is fully empty. The detour logs degraded tables, REBUILDS a fully-empty one via the native
/// setup, and fail-soft SKIPS the builder when a slot would still null-deref. `LAST` is the
/// per-episode latch (distinct valid/null mask + caller) so the degraded log does not fire per frame.
pub(crate) static PROFILE_SELECT_TABLE_DIAG_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static PROFILE_SELECT_TABLE_DIAG_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PROFILE_SELECT_TABLE_DIAG_LAST: AtomicUsize = AtomicUsize::new(0);
/// All ten profile-renderer table slots (bit per slot 0..9): the fully-empty `null_mask` value that
/// triggers the in-world table rebuild.
pub(crate) const PROFILE_TABLE_ALL_SLOTS_MASK: u32 = (1 << TITLE_PROFILE_SLOT_COUNT) - 1;
/// Count of in-world profile-renderer table REPAIRS: the builder detour found the 10-slot table
/// fully empty (er-effects-rs-j3r: nothing repopulates it on our in-world ProfileSelect reopens) and
/// re-ran the native table setup to satisfy the native invariant. Exposed as
/// `oracle_profileselect_table_repairs`.
pub(crate) static PROFILE_SELECT_TABLE_REPAIR_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Count of fail-soft SKIPS of the native profile-portrait builder: after the (possible) repair a
/// slot still had a null/invalid renderer entry, so chaining the original would AV at
/// `[entry+0x754]`; the detour dropped that one call instead (the per-frame builder retries).
/// Exposed as `oracle_profileselect_table_guard_skips`.
pub(crate) static PROFILE_SELECT_TABLE_GUARD_SKIP_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Distinct-state latch for the guard-skip log line (same keying idea as the diag latch).
pub(crate) static PROFILE_SELECT_TABLE_GUARD_SKIP_LAST: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PROFILE_RENDERER_SPARE_HITS: AtomicUsize = AtomicUsize::new(0);
/// Minimal-delay portrait hold: the autoload's load-commit (`maybe_fire_tfc_continue`) waits at the
/// open main menu -- where the ProfileSelect render context is valid -- until the character portrait
/// has rendered + been captured (`LOADING_BG_PORTRAIT_GX_KEPT` set), or this many recurring-task
/// ticks elapse, then proceeds. ~60 ticks/s, so 240 ≈ a ~4s cap on the added delay.
pub(crate) static PORTRAIT_HOLD_WAIT_TICKS: AtomicUsize = AtomicUsize::new(0);
pub(crate) const PORTRAIT_HOLD_MAX_TICKS: usize = 240;
/// Profile-render refresh `FUN_1409aa7d0` (deobf 0x1409aa680): no-arg; gets GameDataMan ProfileSummary
/// and, per enabled slot with a profile + `+0x754/+0x755 == 0`, equips ChrAsm + copies FaceData +
/// kicks the async character-model build. The Continue autoload never runs it for our slot (req754=0),
/// so we call it ourselves once the renderer table is populated to REQUEST the portrait model render.
pub(crate) const PROFILE_RENDERER_REFRESH_RVA: usize = 0x9aa680;
pub(crate) static PROFILE_REFRESH_KICKED: AtomicUsize = AtomicUsize::new(0);
/// Loading-screen portrait FAIL-FAST SEMAPHORE (er-effects-rs-j3r; user directive 2026-07-02: "it
/// should crash, with our harness so we know early if we introduce a regression"). Our portrait
/// renderer must build the SAME slot the game actually loaded (`GameMan.save_slot`/ac0 -- the load
/// itself is correct; only our custom renderer picks the wrong slot). Packed state on a violation:
/// `(loaded_slot<<16) | (render_target_slot<<8) | cond`, `cond` bit0 = wrong-slot (render target !=
/// loaded), bit1 = null loaded-slot renderer while the table is live (the 3rd-open null-deref class).
/// 0 = healthy / never tripped. Exposed as `oracle_portrait_render_semaphore`.
pub(crate) static PORTRAIT_RENDER_SEMAPHORE_STATE: AtomicUsize = AtomicUsize::new(0);
/// One-shot log latch so the semaphore's crash-log/debug line prints exactly once before the fault.
pub(crate) static PORTRAIT_RENDER_SEMAPHORE_LOGGED: AtomicUsize = AtomicUsize::new(0);
/// Null-page address the semaphore deliberately writes to force a clean, VEH-captured fail-fast crash
/// on diagnostic runs (guaranteed unmapped -> AV -> `crash_vectored_handler` logs -> CONTINUE_SEARCH
/// -> terminate). Distinctive `fault_addr=0xdead` marks the crash log as OUR semaphore, not a native AV.
pub(crate) const PORTRAIT_RENDER_SEMAPHORE_FAULT_ADDR: usize = 0xDEAD;
/// Bitmask (bit per slot 0..9) of which profile-renderer slots have had their forced render dumped
/// to `portrait-capture-slot{N}.bin` -- so the all-slot diagnostic dumps each slot exactly once.
pub(crate) static PROFILE_SLOT_DUMP_MASK: AtomicUsize = AtomicUsize::new(0);
/// Per-call tick counter for `force_profile_render_tick`, used to re-fire the model build on a timer
/// (the timing test: a LATER rebuild, after LOAD GAME has loaded each slot's FaceData, should render
/// the real character instead of the default).
pub(crate) static PROFILE_FORCE_TICK_COUNTER: AtomicUsize = AtomicUsize::new(0);
/// Post-Continue feed window: ticks remaining during which the mark+refresh feed runs frequently (not just
/// every 240 ticks) to DRIVE the freshly-built renderers' async ResMan model build to completion and keep
/// it latched. Set when we build our own table post-Continue; decremented each force_profile_render_tick.
/// The menu kept its models live by feeding across a long dwell; the brief now-loading window needs the
/// feed driven continuously or the build is kicked once and decays (observed 2026-06-29: built[m] 10->0).
pub(crate) static PROFILE_LOADSCREEN_FEED_TICKS: AtomicUsize = AtomicUsize::new(0);
/// One-shot log latch for the IMMEDIATE build-kick (edge-triggered when the autoload target slot's
/// fingerprint first goes real and its renderer's +0x754 build-request latch is still 0). The kick
/// itself is idempotent via +0x754, but the debug line should print once.
pub(crate) static PROFILE_REAL_SLOT_KICK_LOGGED: AtomicUsize = AtomicUsize::new(0);
/// How many ticks to keep the post-Continue feed window open after an own-renderer build (~bounded so it
/// can't churn forever). Generous enough to outlast a real load; the refresh is idempotent so extra feeds
/// no-op once the model is built.
pub(crate) const PROFILE_LOADSCREEN_FEED_WINDOW_TICKS: usize = 1800;
/// HIGHER-RES. Per-slot offscreen base-size table read by `CSMenuProfModelRend` ctor (0x140bbe010):
/// `width = *(u32*)(base+0x3b39848 + slot*0x20)`, `height = *(u32*)(...+0x4)` -> packed u64
/// `(height<<32)|width`. Static init `FUN_1400a7bb0` writes every slot `0x8000000080` (base 128x128;
/// the menu's x2 supersample makes the observed 256x256 RT). Patch each entry that still holds the
/// init value to a larger base BEFORE the renderers are constructed (TitleTopDialog ctor) so the
/// offscreen render targets are bigger; the D3D12 readback reads desc.Width/Height dynamically.
pub(crate) const PROFILE_OFFSCREEN_SIZE_TABLE_RVA: usize = 0x3b39848;
pub(crate) const PROFILE_OFFSCREEN_SIZE_TABLE_STRIDE: usize = 0x20;
/// The value `FUN_1400a7bb0` writes (base 128x128 = `(128<<32)|128`); self-validate before patching.
pub(crate) const PROFILE_OFFSCREEN_SIZE_INIT: usize = 0x8000000080;
/// Diagnostic target base 2056x2056 = `(2056<<32)|2056`, with the native per-slot supersample flag OFF.
/// This forces the engine's env-dependent path (`FUN_140bbeee0`: `base*2` iff global flag &&
/// `size_struct[+0x8]`) to stay x1, producing a 2056x2056 portrait RT before the full-backbuffer GPU
/// composite scales it down/crops it.
pub(crate) const PROFILE_OFFSCREEN_SIZE_TARGET: usize = 0x0000_0808_0000_0808;
/// Byte offset within a size-table row of the per-slot supersample-enable flag (read as
/// `size_struct[+0x8]` by `FUN_140bbeee0`); zero it to force x1.
pub(crate) const PROFILE_OFFSCREEN_SIZE_SUPERSAMPLE_FLAG_OFFSET: usize = 0x8;
/// Bitmask of save slots whose profile offscreen base-size table row has been patched to the target size.
pub(crate) static PROFILE_SIZE_PATCHED: AtomicUsize = AtomicUsize::new(0);
/// LIGHTING. Renderer field holding the IBL env-map-region object (`param_1[0xec]`, allocated by
/// FUN_140b399e0, filled by the IBL build FUN_140b39a30). The IBL build stores the registered
/// env-region id into `*envObj` ONLY when the `GILM####_rem` env map is resident; if it was skipped
/// (GILM not resident at construction) `*envObj` stays 0 -> head is unlit/dark. So
/// `*(renderer+0x760)` then deref again = the residency oracle (non-zero = IBL built).
pub(crate) const PROFILE_RENDERER_ENV_REGION_OFFSET: usize = 0x760;

// === Candidate A (in-movie GFx head) runtime semaphores (er-effects-rs-jsm) =======================
// All read-only oracles for the `maybe_update_gfx_loading_portrait` path except the demote credit,
// which the Present-overlay decrements to yield the head draw to the movie when the in-movie head is
// live (so the native tips/bar render above it). Emitted into er-effects-telemetry.json.
/// Cumulative successful per-frame copies of the live head INTO the displayed GFx movie texture (the
/// `oracle_gfx_portrait_uploads` proof: >0 == the head is inside the movie, under the native tips).
pub(crate) static GFX_PORTRAIT_UPLOADS: AtomicUsize = AtomicUsize::new(0);
/// Cumulative resolves of the displayed `CSTextureImage` by name (name-change re-resolves; cached in
/// between). `oracle_gfx_portrait_resolves`.
pub(crate) static GFX_PORTRAIT_RESOLVES: AtomicUsize = AtomicUsize::new(0);
/// Cumulative resolve failures (repo singleton null, resolver returned 0, or bad HAL pointer) --
/// each leaves the Present-overlay fallback in charge. `oracle_gfx_portrait_resolve_fails`.
pub(crate) static GFX_PORTRAIT_RESOLVE_FAILS: AtomicUsize = AtomicUsize::new(0);
/// Last resolved `CSTextureImage*` (AddRef'd, held) and its GFx-sampled HAL texture pointer; 0 = none.
/// The image ref is dropped (Release) when the displayed name changes or the window resets.
pub(crate) static GFX_PORTRAIT_CACHED_IMG: AtomicUsize = AtomicUsize::new(0);
pub(crate) static GFX_PORTRAIT_CACHED_HAL: AtomicUsize = AtomicUsize::new(0);
/// A previously-cached image ref stranded by an OFF-game-thread window reset; the game-thread updater
/// Releases it on its next tick (Scaleform refcount frees must run on the game thread).
pub(crate) static GFX_PORTRAIT_ORPHAN_IMG: AtomicUsize = AtomicUsize::new(0);
/// Displayed-texture dims last written (`(w<<16)|h`), for the size/format oracle. `oracle_gfx_portrait_hal_dims`.
pub(crate) static GFX_PORTRAIT_HAL_DIMS: AtomicUsize = AtomicUsize::new(0);
/// Last error code from the updater (0=ok/none, 1=repo-null, 2=no-name, 3=resolve-0, 4=bad-hal,
/// 5=upload-failed). `oracle_gfx_portrait_last_error`.
pub(crate) static GFX_PORTRAIT_LAST_ERROR: AtomicUsize = AtomicUsize::new(0);
pub(crate) const GFX_PORTRAIT_ERR_NONE: usize = 0;
pub(crate) const GFX_PORTRAIT_ERR_REPO_NULL: usize = 1;
pub(crate) const GFX_PORTRAIT_ERR_NO_NAME: usize = 2;
pub(crate) const GFX_PORTRAIT_ERR_RESOLVE_ZERO: usize = 3;
pub(crate) const GFX_PORTRAIT_ERR_BAD_HAL: usize = 4;
pub(crate) const GFX_PORTRAIT_ERR_UPLOAD_FAILED: usize = 5;
/// Present-overlay demotion credit: the game-thread updater refills this to `GFX_PORTRAIT_DEMOTE_REFILL`
/// on every successful in-movie upload; the Present-overlay decrements it each frame and, while > 0,
/// SKIPS drawing the head (the movie owns the display, tips render on top). If the updater stalls for
/// more than the refill window the credit drains to 0 and the overlay resumes -- fail-open, so the
/// working overlay is never regressed even if the in-movie path degrades. `oracle_gfx_portrait_demote_credit`.
pub(crate) static GFX_PORTRAIT_DEMOTE_CREDIT: AtomicUsize = AtomicUsize::new(0);
/// Presents of overlay-head draw SKIPPED because the in-movie head was live (proof the handoff engaged).
pub(crate) static GFX_PORTRAIT_OVERLAY_YIELDS: AtomicUsize = AtomicUsize::new(0);
/// Refill count (in Present frames) granted per successful in-movie upload. The head updates ~15/s and
/// Present runs faster, so this must comfortably span the gap between two uploads.
pub(crate) const GFX_PORTRAIT_DEMOTE_REFILL: usize = 30;
/// One-shot log latch for the first confirmed in-movie head upload.
pub(crate) static GFX_PORTRAIT_FIRST_LOGGED: AtomicUsize = AtomicUsize::new(0);
