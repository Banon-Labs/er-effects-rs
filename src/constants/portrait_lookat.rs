// ---------------------------------------------------------------------------------------------------
// LOOK-AT LEVER (portrait head/eyes follow the mouse cursor). VERIFIED RE 2026-06-29 -- bd
// `portrait-lookat-RE-VERIFIED-2026-06-29`. ER's c0000 rig has NO eye bone: the eyes are FaceGen mesh
// rigidly skinned to the single "Head" bone, so gaze is delivered by rotating Spine2->Neck->Head; the
// eyes follow because they ride the head. We rotate those bones' LOCAL quaternions toward the cursor.
//
// REACH (per tick, from renderer R = CSMenuProfModelRend*): require *(R+0x778) != 0 (model built);
// X = *(R + ANIM_LOCATION) ; importer = *(X + IMPORTER) ; poseHolder = importer + POSEHOLDER (embedded,
// not a deref). Verified at FUN_140bba7d0 + GetPosHolder (lea rax,[rcx+0x48]).
pub(crate) const PROFILE_LOOKAT_ANIM_LOCATION_OFFSET: usize = 0x948;
pub(crate) const PROFILE_LOOKAT_IMPORTER_OFFSET: usize = 0x20;
pub(crate) const PROFILE_LOOKAT_POSEHOLDER_OFFSET: usize = 0x48;
/// `CSFD4LocationHkaPoseImporter::PoseHolder` (0x50) field offsets.
pub(crate) const POSEHOLDER_SKELETON_OFFSET: usize = 0x0; // hkaSkeleton*
pub(crate) const POSEHOLDER_LOCAL_BONE_DATA_OFFSET: usize = 0x8; // hkArray<BoneData>.data
pub(crate) const POSEHOLDER_MODEL_BONE_DATA_OFFSET: usize = 0x18; // hkArray<BoneData>.data
pub(crate) const POSEHOLDER_DIRTY_FLAGS_OFFSET: usize = 0x28; // uint*[boneCount] bitflags (stride 4)
pub(crate) const POSEHOLDER_IS_UPDATED_OFFSET: usize = 0x38; // bool
/// `BoneData` (0x30): xyz @+0x0, q (quaternion x,y,z,w) @+0x10, scale @+0x20.
pub(crate) const BONE_DATA_STRIDE: usize = 0x30;
pub(crate) const BONE_DATA_Q_OFFSET: usize = 0x10;
/// `hkaSkeleton` (0x90, get_structure-verified) + `hkaBone` (0x10) field offsets.
pub(crate) const HKA_SKELETON_PARENT_INDICES_DATA_OFFSET: usize = 0x20; // hkArray<i16>.data
pub(crate) const HKA_SKELETON_BONES_DATA_OFFSET: usize = 0x30; // hkArray<hkaBone>.data
pub(crate) const HKA_SKELETON_BONES_SIZE_OFFSET: usize = 0x38; // i32 bone count
pub(crate) const HKA_BONE_STRIDE: usize = 0x10;
pub(crate) const HKA_BONE_NAME_OFFSET: usize = 0x0; // hkStringPtr (char* ASCII; mask bit0 owner flag)
/// `dirtyFlags[idx] |= this` marks a bone's model-space transform stale so `updateBoneModelSpace`
/// rebuilds it (and its descendants) from the local pose before the offscreen render.
pub(crate) const POSE_DIRTY_MODEL_SPACE_BIT: u32 = 0x2;
/// Bone names we drive (standard ER c0000 names, confirmed via the ragdoll bone map FUN_141d700c0).
pub(crate) const LOOKAT_BONE_HEAD: &str = "Head";
pub(crate) const LOOKAT_BONE_NECK: &str = "Neck";
pub(crate) const LOOKAT_BONE_SPINE2: &str = "Spine2";
/// Max bones we will scan/dump (a c0000 skeleton is well under this; bounds the runtime enumeration).
pub(crate) const LOOKAT_MAX_BONES: usize = 512;
/// Cursor -> look angle gains (radians at the window edge). Head carries the bulk (eyes are welded to
/// it); neck/spine2 add a natural distributed turn. Yaw = horizontal, pitch = vertical. SIGN + which
/// local bone axis each maps to need ONE runtime visual calibration (the portrait camera mirrors L/R).
/// GAIN CALIBRATION IS BLOCKED until the model faces the camera (2026-06-30): once the posed model
/// re-rasterizes per frame, the rendered head shows the BACK of the head at BOTH cursor extremes AND at
/// center (look-at~0) -- so the model root/skeleton renders facing AWAY, independent of these gains
/// (cutting them 6x in calib-6 changed nothing). Until the facing is fixed (camera orbit to the model's
/// front; cf the concurrent PROFILE_CAM_FACE_YAW effort) the face is not visible, so the look-at strength
/// cannot be visually tuned. Keeping the original gains (they gave a clear ~23-37/px head-turn signal).
pub(crate) const LOOKAT_HEAD_YAW_GAIN: f32 = 0.34;
pub(crate) const LOOKAT_HEAD_PITCH_GAIN: f32 = 0.22;
pub(crate) const LOOKAT_NECK_YAW_GAIN: f32 = 0.15;
pub(crate) const LOOKAT_NECK_PITCH_GAIN: f32 = 0.10;
pub(crate) const LOOKAT_SPINE2_YAW_GAIN: f32 = 0.08;
pub(crate) const LOOKAT_SPINE2_PITCH_GAIN: f32 = 0.05;
/// Sign flips for runtime calibration without a rebuild loop (set from the first visual check).
pub(crate) const LOOKAT_YAW_SIGN: f32 = 1.0;
pub(crate) const LOOKAT_PITCH_SIGN: f32 = 1.0;
/// Per-renderer-slot cached look-at state: the resolved Head/Neck/Spine2 bone indices and the latched
/// base (idle) local quaternions, captured ONCE so the per-tick rotation composes from an immutable
/// base (drift-free). `-1` index = bone not found in this slot's skeleton.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LookatSlot {
    pub head: i32,
    pub neck: i32,
    pub spine2: i32,
    /// Idle (clean) LOCAL quaternions for Head/Neck/Spine2, latched ONCE from the freshly-rebuilt
    /// pose so the per-frame look-at composes `base ⊗ delta` (drift-free) instead of `current ⊗ delta`
    /// (which compounds at 60 Hz when the draw-phase task drives every frame). Re-latched on rebuild
    /// (the slot is reset to `None` each model rebuild, so this re-captures the clean pose).
    pub head_base: [f32; 4],
    pub neck_base: [f32; 4],
    pub spine2_base: [f32; 4],
    pub base_latched: bool,
}
pub(crate) static PROFILE_LOOKAT_SLOTS: std::sync::Mutex<[Option<LookatSlot>; 10]> =
    std::sync::Mutex::new([None; 10]);
/// Look-at telemetry (RAM semaphores): apply count, resolved bone indices (packed), live bone count,
/// last normalized cursor (packed i16 x/y * 1000), and a one-shot bone-name dump latch (bit per slot).
pub(crate) static PROFILE_LOOKAT_APPLY_CALLS: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PROFILE_LOOKAT_HEAD_IDX: AtomicUsize = AtomicUsize::new(usize::MAX);
pub(crate) static PROFILE_LOOKAT_NECK_IDX: AtomicUsize = AtomicUsize::new(usize::MAX);
pub(crate) static PROFILE_LOOKAT_SPINE2_IDX: AtomicUsize = AtomicUsize::new(usize::MAX);
pub(crate) static PROFILE_LOOKAT_BONE_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PROFILE_LOOKAT_LAST_CURSOR: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PROFILE_LOOKAT_BONES_DUMPED_MASK: AtomicUsize = AtomicUsize::new(0);
/// MOUSE-TRACK PROOF latch (selftest only): bit 0/1/2 set once the live head has been dumped at a
/// look-left / center / look-right yaw bucket (`portrait-capture-slot{200,201,202}.bin`). The three
/// dumps are visually distinct head poses, converting the ambiguous per-frame `rt changed%` into a
/// decisive before/after that the head pose tracks the drive signal (= the normalized cursor in
/// product). Mask == 0b111 once all three captured (`oracle_profile_lookat_track_buckets`).
pub(crate) static PROFILE_LOOKAT_TRACK_BUCKETS: AtomicUsize = AtomicUsize::new(0);
/// DIAGNOSTIC: per-frame readback outcomes -- how many readbacks returned content, and how many of those
/// were classified as a checker/placeholder (so did NOT publish). `oracle_profile_readback_some` /
/// `oracle_profile_readback_checker`; `_some - _checker` == the publish count.
pub(crate) static PROFILE_READBACK_SOME: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PROFILE_READBACK_CHECKER: AtomicUsize = AtomicUsize::new(0);
/// DEFERRED-READBACK DIAGNOSTIC (H2 vs H3): a readback of the content RT taken at the START of the draw
/// tick, BEFORE this frame's `drive(r)` queues a new rasterize -- so it captures the texture state left by
/// the PREVIOUS frame's GX work. If `_deferred_nonblack` is high while the post-drive immediate
/// `_some - _checker` is ~4, the blackness is a cross-queue TIMING artifact (the rasterize lands but our
/// same-tick readback races ahead of the game's GX queue) -> fix by syncing/reading at a settled point.
/// If `_deferred_nonblack` is ALSO ~4, the rasterize genuinely is not landing in this texture (H3).
pub(crate) static PROFILE_READBACK_DEFERRED_SOME: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PROFILE_READBACK_DEFERRED_NONBLACK: AtomicUsize = AtomicUsize::new(0);
/// One-shot latch: dump a single checker frame (slot 103) to see what the non-published frames hold.
pub(crate) static PROFILE_CHECKER_DUMPED: AtomicBool = AtomicBool::new(false);
/// `updateBoneModelSpace` (dump 0x141653370) -> deobf 0x141653350 (shift -0x20, content-unique). The
/// render calls this (via `GetBoneModelSpace`) each frame to rebuild `modelSpaceBoneData` from the
/// (anim-imported) `localSpaceBoneData` for every dirty bone. We HOOK it: before the original runs, we
/// compose the cursor rotation onto the Head/Neck/Spine2 LOCAL quaternions and mark all bones dirty, so
/// the original's recompute cascades our rotation into the final pose the render skins from. This is the
/// only injection point that survives the per-frame anim re-import (a game-task write is clobbered).
pub(crate) const UPDATE_BONE_MODEL_SPACE_RVA: usize = 0x1653350;
/// Per-frame per-model PUSH task `FUN_140bba7d0` (dump) -> deobf RVA 0x140bba6e0 (content-unique, shift
/// -0xf0). `fn(renderer, frame)`: if model_ins(+0x778) && X(+0x948) it reads importer=*(X+0x20) and calls
/// the submodel propagation `FUN_1409e9ac0(model_ins, frame, importer)` which copies the importer's
/// MODEL-space bones into every submodel's own poseHolder.modelSpaceBoneData (what the GPU skins from).
/// We HOOK it: write our Head/Neck/Spine2 rotation into the importer PoseHolder + updateBoneModelSpace
/// BEFORE the original, so the original propagates OUR pose to the submodels at 60 Hz with the correct
/// `frame` arg (which we cannot synthesize -- it feeds the render-entity commit + cloth). See bd
/// portrait-lookat-submodel-propagation-RE-2026-06-29.
pub(crate) const PROFILE_PER_FRAME_PUSH_RVA: usize = 0xbba6e0;
pub(crate) static PROFILE_PERFRAME_HOOK_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static PROFILE_PERFRAME_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PROFILE_PERFRAME_HOOK_HITS: AtomicUsize = AtomicUsize::new(0);
/// Profile-renderer UPDATE task `FUN_140bba820` (dump) -> deobf RVA 0x140bba730 (content-unique, shift
/// -0xf0). `fn(renderer, FD4TaskData*)`: runs the FD4 state-machine stepper (vtable+0xc0) then refreshes the
/// model transform/animation (anim step with `*(td+8)` delta-time, model matrix, FaceData) for the tick.
/// Paired with the DRAW task (== `PROFILE_PER_FRAME_PUSH_RVA`). RE-confirmed: both are `CSEzUpdateTask`s
/// the renderer self-registers; post-Continue their ResMan driver under-schedules them (~4-19x/loading
/// screen) so the model rasterizes sparsely. We drive both ourselves per render-thread frame to make the
/// portrait re-rasterize the live look-at pose EVERY frame. See keepalive-POOL-REFUTED-readback-crossqueue.
pub(crate) const PROFILE_MODEL_UPDATE_TASK_RVA: usize = 0xbba730;
/// Menu-model ANIM BIND `FUN_140bba300` (dump) -> deobf RVA 0xbba210 (content-unique, shift -0xf0,
/// verified via dump-deobf-shift 2026-07-03). `fn(renderer, &anim_id_i32, force, mode)`: stops the
/// current anim entry (via the handle at +0x96c), resolves + plays `anim_id` on the model's anim
/// holder X(+0x948), and caches id/handle at +0x968/+0x96c; id -1 unbinds (handle := the null
/// sentinel). Early-returns on an unchanged id unless `force != 0`. The update task steps the bound
/// anim by frame-dt each call ONLY while `+0x96c != sentinel` -- see bd
/// `portrait-anim-bind-RE-corrects-6hz-gate-2026-07-03` (corrects the earlier "~6Hz gparam gate"
/// reading). The native profile pipeline binds id 0 (FUN_140bbe290 <- refresh FUN_1409aa7d0), which
/// is the STATIC menu pose -- that, not cadence, is why the loading portrait never moved.
pub(crate) const PROFILE_ANIM_BIND_RVA: usize = 0xbba210;
/// Null anim-handle sentinel global `DAT_143b39470` (data RVA; data addresses do not shift between
/// the dump and the live binary -- same convention as `GX_DRAW_CONTEXT_RVA`). The CSMenuAsmModelRend
/// ctor inits renderer+0x96c to this global's value.
pub(crate) const PROFILE_ANIM_NULL_HANDLE_RVA: usize = 0x3b39470;
/// The bound-anim handle cache on the renderer (low16 = anim-entry index, high16 = generation).
pub(crate) const PROFILE_ANIM_HANDLE_OFFSET: usize = 0x96c;
/// Idle anim ids to bind on the loading portrait, in order. The menu model's anim holder is built
/// from the FULL c0000 ANIBND (`FUN_140bbb4a0`: `AnibndRepositoryImp::GetResCap(L"c0000")` ->
/// `FUN_1401ac2f0` -> renderer+0x948), so base c0000 anim ids resolve -- not just menu poses.
/// 3000000 = the in-world standing idle (grounded: our own in-world telemetry reports
/// `current_animation_id = 3000000`; visibly more movement than the menu idles, per user request
/// 2026-07-03). 0x18696=100022 / 0x1863c=99900 are the CSMenuPlayerModelRend ctor's equip-menu
/// idles. The first id whose bind leaves a real handle (!= sentinel and != 0xffffffff
/// resolve-failure) wins; a failed candidate leaves no active entry, so falling through is
/// side-effect-free beyond having stopped the static pose anim.
pub(crate) const PORTRAIT_IDLE_ANIM_IDS: [i32; 3] = [3000000, 100022, 99900];
/// The (renderer, anim-holder X) pair the idle anim was last bound on. The loading window's model
/// is rebuilt several times (content-RT pin moves) and a rebuild either ctor's a NEW renderer
/// (+0x968 = -1 -> static pose) or recreates X under the same renderer -- a one-shot bind latch
/// left the DISPLAYED model static after churn (run anim-bind-noteardown-20260703-074216). Rebind
/// whenever either pointer changes. (The engine helps once +0x968 survives: the model build fn
/// re-binds `*(+0x968)` force=1 itself -- but a fresh renderer starts at -1, so we must track.)
pub(crate) static PORTRAIT_ANIM_BOUND_RENDERER: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PORTRAIT_ANIM_BOUND_LOC: AtomicUsize = AtomicUsize::new(0);
/// REBUILD-DRIVER tripwire (2026-07-03): the portrait model is torn down + rebuilt ~1/s even with
/// the kick live-model guard (runs #2/#3: 85-94 rebinds, native anim-0 rebound each time), which is
/// why nothing ever visibly animated. Per drive frame we sample the renderer's request/teardown
/// latches (+0x754/+0x755/+0x756) and re-run `STEP_Wait_Play`'s own FaceData compare
/// (`GetFaceDataBuffer(renderer_FaceData@+0x788, true)` vs the staged buffer at +0x218, 0x120
/// bytes): a mismatch makes the step invalidate the model (`FUN_1409ecb40`) EVERY tick -- and we
/// drive that step at 60Hz. `NEQ_TICKS`/`DRIVE_TICKS` ~= 1.0 convicts the FaceData loop; latch
/// bytes != 0 at rebind time convict a latch raiser instead.
pub(crate) static PORTRAIT_FACEDATA_NEQ_TICKS: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PORTRAIT_DRIVE_TICKS: AtomicUsize = AtomicUsize::new(0);
/// SLOT-KEYED KICK LATCH (rebuild-storm root fix, run #4 tripwire `latches=0/1/0`; slot-keyed per
/// user eyewitness 2026-07-03 "wrong character rendered the whole load, no swap"). The engine's
/// global refresh (dump FUN_1409aa7d0) raises the +0x754/+0x755 request latches once per PROFILE
/// DATA CHANGE, gated on both reading 0. Our cadence re-kick hit the mid-pipeline phase where the
/// model is dead AND the latches are already consumed, re-raising them -- so `STEP_Wait_Play` saw
/// +0x755 != 0 on every pass and re-entered the rebuild state (7) forever: ~1 rebuild/second, anim
/// reset to pose 0 each time, lighting reset = the user-visible shadow flicker, portrait never
/// animated. A blanket one-shot is wrong the other way: ac0 (`portrait_loaded_slot`) can still be
/// the previous session's slot at first-kick time, and the storm's re-kick was what accidentally
/// swapped in the correct character once ac0 flipped. Latch = kicked slot + 1 (0 = none): each slot
/// value kicks exactly once per window, so the ac0 flip produces one deterministic corrective
/// rebuild. Reset by `loading_portrait_window_reset`.
pub(crate) static PORTRAIT_KICK_SLOT_KEY: AtomicUsize = AtomicUsize::new(0);
/// The renderer the kick was issued on. Run #10 measured the async build at 94ms (kick +16.19s ->
/// model-LIVE +16.28s), refuting the streaming-contention theory -- the model dies at ~+17s because
/// the CONTINUE TEARDOWN frees the menu-era renderer we kicked, and our post-teardown table rebuild
/// creates NEW renderer objects the slot-only latch refused to re-kick. Re-kick when the table's
/// renderer identity changes; the 755-landmine fix makes a re-kick on the fresh modelless renderer
/// safe (754-only).
pub(crate) static PORTRAIT_KICK_RENDERER: AtomicUsize = AtomicUsize::new(0);
/// Last confirmed loaded slot + 1 seen by the display tick (0 = none yet). User directive
/// 2026-07-03: never show the previous character's head between selecting a character and the
/// load-commit -- ac0 flips at deserialize, and pixels published before the flip belong to the OLD
/// slot's model. On a flip the displayed snapshot is wiped instantly (hidden) and the corrective
/// kick republished the right head when ready. Residual gap: the stale-slot pixels can still show
/// for the ac0-flip latency (~1s) on a cross-character manual reload; the proper future source for
/// "what was clicked" is the load dialog's selected row, not ac0.
pub(crate) static PORTRAIT_LAST_CONFIRMED_SLOT: AtomicUsize = AtomicUsize::new(0);
/// ac0 FLAPS transiently mid-load (run #16: 5->3->5->3 within 1.3s -- load-time code touches other
/// slots), so a raw slot change must persist for `PORTRAIT_SLOT_FLIP_ACCEPT_TICKS` consecutive
/// present ticks (~1s) before the pipeline believes it. Candidate = the differing value + 1;
/// streak = consecutive ticks it has held.
pub(crate) static PORTRAIT_SLOT_FLIP_CANDIDATE: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PORTRAIT_SLOT_FLIP_STREAK: AtomicUsize = AtomicUsize::new(0);
pub(crate) const PORTRAIT_SLOT_FLIP_ACCEPT_TICKS: usize = 60;
/// DEPTH-KEY branch attribution (the black-background-on-reload bug): every key call ends in
/// exactly one of applied-fresh / applied-cached / NO-MASK (fail-open, the black frames); the
/// readback-side counters split the no-mask cause (async in-flight skip vs depth resource find
/// failure vs dims mismatch vs no-gap). The old one-shot logs burned in window 1 and hid all
/// window-2+ behavior.
pub(crate) static DEPTH_KEY_CACHED: AtomicUsize = AtomicUsize::new(0);
pub(crate) static DEPTH_KEY_NOMASK: AtomicUsize = AtomicUsize::new(0);
pub(crate) static DEPTH_RB_INFLIGHT_SKIPS: AtomicUsize = AtomicUsize::new(0);
pub(crate) static DEPTH_RB_FIND_FAILS: AtomicUsize = AtomicUsize::new(0);
pub(crate) static DEPTH_RB_DIMS_MISMATCHES: AtomicUsize = AtomicUsize::new(0);
pub(crate) static DEPTH_RB_NOGAP: AtomicUsize = AtomicUsize::new(0);
/// PUMP-BLOCK REASON counters (run #7: modeldraws froze at 250 ~10s before the load-completed
/// window reset, cause unattributed). One per gate in the pump's path: renderer table entry
/// invalid, vtable mismatch (renderer freed/replaced), offscreen pointer invalid, multi-model
/// (menu churn). Exported as oracles + printed in the sweep line so a stall names its gate.
pub(crate) static PORTRAIT_PUMP_BLOCK_R: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PORTRAIT_PUMP_BLOCK_VTABLE: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PORTRAIT_PUMP_BLOCK_OFF: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PORTRAIT_PUMP_BLOCK_MULTI: AtomicUsize = AtomicUsize::new(0);
/// The renderer's staged FaceData compare buffer (`param_1 + 0x43` longlongs) and embedded FaceData
/// object (`param_1 + 0xf1`), from the `STEP_Wait_Play` decompile; compare length 0x120.
pub(crate) const PROFILE_RENDERER_FACEDATA_CMP_OFFSET: usize = 0x218;
pub(crate) const PROFILE_RENDERER_FACEDATA_OBJ_OFFSET: usize = 0x788;
pub(crate) const PROFILE_FACEDATA_CMP_LEN: usize = 0x120;
/// Idle-anim bind state: 0 = not attempted this load window, 1 = bound (real handle), 2 = every
/// candidate failed to resolve. Reset by `loading_portrait_window_reset` so the next load rebinds.
pub(crate) static PORTRAIT_ANIM_BIND_STATE: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PORTRAIT_ANIM_BIND_ATTEMPTS: AtomicUsize = AtomicUsize::new(0);
/// The idle anim id that actually bound (0 until success). `oracle_portrait_anim_bound_id`.
pub(crate) static PORTRAIT_ANIM_BOUND_ID: AtomicUsize = AtomicUsize::new(0);
/// renderer+0x96c read just before the first bind attempt; a value != sentinel proves the native
/// anim-0 (static pose) bind resolved on this model, i.e. anim resources ARE loaded for it.
pub(crate) static PORTRAIT_ANIM_HANDLE_BEFORE: AtomicUsize = AtomicUsize::new(0);
/// renderer+0x96c after the last bind attempt. `oracle_portrait_anim_handle`.
pub(crate) static PORTRAIT_ANIM_HANDLE: AtomicUsize = AtomicUsize::new(0);
/// Runtime value of the null-handle sentinel global (validates the sentinel RE at runtime; also
/// settles whether it ever changes mid-run -- the old "~6Hz gparam word" theory predicts changes,
/// the corrected sentinel reading predicts a constant).
pub(crate) static PORTRAIT_ANIM_SENTINEL: AtomicUsize = AtomicUsize::new(0);
/// PIXEL-MOTION oracle (the AGENTS.md rendered-output proof gate for "the portrait animates"),
/// LIGHTING-IMMUNE by construction: the scene's lighting visibly changes every frame (user report
/// 2026-07-03), so raw luma diffs cannot be the motion oracle. Instead this diffs the depth-keyed
/// ALPHA SILHOUETTE (mean abs alpha delta x1000 between successive published frames on a 32x32
/// downsample): alpha comes from the depth buffer via `apply_depth_alpha_key` (applied to the pixels
/// BEFORE publish), and depth does not respond to lighting -- only actual body/silhouette motion
/// moves it. Updated ONLY when both the current and previous frame carry a real cutout (some
/// transparent cells), so fail-open unkeyed frames cannot fake a spike. `_last` per publish, `_max`
/// per run: an idle anim must push `_max` clearly above ~0 while lighting flicker alone cannot.
pub(crate) static PORTRAIT_MOTION_METRIC_LAST: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PORTRAIT_MOTION_METRIC_MAX: AtomicUsize = AtomicUsize::new(0);
/// Companion LUMA-delta gauge on the same downsample grid (same units): measures the per-frame
/// LIGHTING FLICKER (plus any luma-visible motion). Comparing luma vs alpha metrics separates "the
/// lighting changed" from "the body moved"; this also finally quantifies the reported flicker.
pub(crate) static PORTRAIT_LUMA_FLICKER_LAST: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PORTRAIT_LUMA_FLICKER_MAX: AtomicUsize = AtomicUsize::new(0);
/// Previous published frame's 32x32 downsample planes for the motion/flicker metrics:
/// (alpha, luma, keyed) where `keyed` = the frame had transparent cells (depth cutout live).
/// Render thread only.
pub(crate) static PORTRAIT_MOTION_PREV_PLANES: std::sync::Mutex<Option<(Vec<u8>, Vec<u8>, bool)>> =
    std::sync::Mutex::new(None);
/// The live `FD4TaskData*` (`param_2`/`frame` arg) the ENGINE passes to the profile DRAW task on its own
/// (sparse) calls -- captured in `per_frame_push_hook`. The GX enqueue routes the model into the correct
/// OFFSCREEN render pass via this context, so driving the draw with OUR draw-phase task_data renders to the
/// wrong pass (nothing lands in the portrait RT). We reuse the most-recently-captured engine context for our
/// per-frame drive instead. RE note: prior agents found this `frame` arg cannot be synthesized.
pub(crate) static PROFILE_DRAW_TASK_CTX: AtomicUsize = AtomicUsize::new(0);
/// Guard so `per_frame_push_hook` does NOT re-capture the context while WE are driving it (only the engine's
/// own natural calls should seed `PROFILE_DRAW_TASK_CTX`). Doubles as the render-thread BUSY flag of the
/// teardown fence protocol (see `PROFILE_RENDERER_TEARDOWN_FENCE`): it is set BEFORE the fence check on the
/// pump side, so the game-thread teardown can wait it out instead of freeing a renderer mid-drive.
pub(crate) static PROFILE_IN_OUR_DRIVE: AtomicBool = AtomicBool::new(false);
/// Cross-thread teardown fence (freeze-after-capture relaxation, er-effects-rs-l1x 2026-07-03). The
/// game-thread profile-renderer teardown (`profile_renderer_teardown_spare_hook`) raises this BEFORE any
/// delete-enqueue runs (orphan reclaim + native table teardown) and lowers it after the native original
/// returns; the render-thread pump sets `PROFILE_IN_OUR_DRIVE` first and then skips its drive while the
/// fence is up. Both sides are SeqCst, so either the pump sees the fence and skips, or the teardown sees
/// the pump busy and waits -- closing the drive-vs-teardown TOCTOU UAF (three crash flavors: Scaleform
/// dtor, GX-queue null, garbage-vtable RIP) structurally instead of by freezing the drive after the first
/// captured frame.
pub(crate) static PROFILE_RENDERER_TEARDOWN_FENCE: AtomicUsize = AtomicUsize::new(0);
/// Render-thread pump invocations skipped because the teardown fence was up (expect a handful per switch).
pub(crate) static PROFILE_DRIVE_FENCE_SKIPS: AtomicUsize = AtomicUsize::new(0);
/// Teardowns that found the pump mid-drive and waited for it to exit (any value is fine; proves the fence
/// engaged rather than racing).
pub(crate) static PROFILE_TEARDOWN_FENCE_WAITS: AtomicUsize = AtomicUsize::new(0);
/// Teardown fence waits that hit the bounded 10ms cap and proceeded anyway. MUST stay 0 -- nonzero means
/// one frame of the old TOCTOU exposure leaked through (still strictly better than every frame).
pub(crate) static PROFILE_TEARDOWN_FENCE_TIMEOUTS: AtomicUsize = AtomicUsize::new(0);
/// Per-window publish-attribution marks: previous window-reset snapshot of each cumulative publish/skip
/// counter, so `loading_portrait_window_reset` can log per-window deltas (a frozen-on-prior-character
/// window shows clean=0 plus its dominant skip class). Written only from the reset.
pub(crate) static PROFILE_PUBLISH_CLEAN_WINDOW_MARK: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PROFILE_PUBLISH_SKIPPED_TORN_WINDOW_MARK: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PROFILE_PUBLISH_SKIPPED_UNKEYED_WINDOW_MARK: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PROFILE_MULTI_MODEL_PUBLISH_SKIPS_WINDOW_MARK: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PROFILE_RT_PIN_SWITCHES_WINDOW_MARK: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PROFILE_DRIVE_FENCE_SKIPS_WINDOW_MARK: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PROFILE_COLOR_FROM_BUNDLE_WINDOW_MARK: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PROFILE_COLOR_FROM_SCAN_WINDOW_MARK: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PROFILE_DEPTH_FROM_CHAIN_WINDOW_MARK: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PROFILE_DEPTH_FROM_BFS_WINDOW_MARK: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PROFILE_PUBLISH_SKIPPED_UNPAIRED_WINDOW_MARK: AtomicUsize = AtomicUsize::new(0);
/// Minimum percent of transparent pixels a frame's mask must cut for the frame to count as keyed
/// (er-effects-rs-hi2): a real portrait mask removes a large background share; a partial mask
/// (few cut pixels on an opaque IBL box) previously passed "any transparent pixel" and displayed
/// as an unmasked head. 5% is far below any real mask's share and far above the partial band.
pub(crate) const PORTRAIT_MIN_TRANSPARENT_PCT: usize = 5;
/// Frames whose mask cut SOMETHING but under the floor (the partial-mask band) -- held, counted.
pub(crate) static PROFILE_PUBLISH_SKIPPED_LOWMASK: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PROFILE_PUBLISH_SKIPPED_LOWMASK_WINDOW_MARK: AtomicUsize = AtomicUsize::new(0);
/// Display-frame index of the window's FIRST clean publish (usize::MAX = none yet this window):
/// how long the make-before-break bridge held the prior head. Snapshot + reset per window.
pub(crate) static PROFILE_WINDOW_FIRST_KEYED_DISPLAY: AtomicUsize = AtomicUsize::new(usize::MAX);
pub(crate) static PROFILE_WINDOW_FIRST_KEYED_DISPLAY_LAST: AtomicUsize = AtomicUsize::new(0);
/// One-shot latch for the per-run ProfileSummary slot->character-name dump (hi2 attribution).
pub(crate) static PROFILE_SLOT_NAMES_DUMPED: AtomicUsize = AtomicUsize::new(0);
/// Window mark for checker-classified readback frames (run 6 window 9: 266 frames vanished from the
/// publish[] accounting because checker frames were counted in no window class).
pub(crate) static PROFILE_READBACK_CHECKER_WINDOW_MARK: AtomicUsize = AtomicUsize::new(0);
/// Per-window EMA of ACCEPTED tear scores (adaptive baseline for textured characters whose honest
/// frames score high on the vertical-luma metric). 0 = window fresh; reset at each window reset.
pub(crate) static PROFILE_TEAR_EMA: AtomicUsize = AtomicUsize::new(0);
// NATIVE SCENE-ALPHA KEYING (strategy pivot 2026-07-03). Deobf RVAs read directly from the deobf
// disassembly of the engine's own offscreen clear (dump FUN_140bb73a0 -> deobf 0x140bb72b0,
// shift -0xf0, scripts/dump-deobf-shift.py content-unique): pop a GX frame context from
// g_GxDrawContext (pointer global at GX_DRAW_CONTEXT_RVA), clear the scene bundle's RTV
// (bundle+0x30) through the frame's subcontext (+0x25c8), release the frame context. We replicate
// that body with clear color {0,0,0,0} (the engine uses the SHARED opaque-black FloatVector4 at
// dump 0x14329e9b0 -- 136 xrefs, not patchable), so the RT's alpha channel becomes the native
// subject mask once the pump redraws only the model each frame.
/// Deobf RVA of the GX frame-context pop (deobf 0x1419e5830; dump FUN_1419e5850).
pub(crate) const GX_FRAME_CTX_POP_RVA: usize = 0x19e5830;
/// Deobf RVA of the ClearRTV wrapper (deobf 0x1419e0e10; dump FUN_1419e0e30).
/// Args: rcx = *(frame_ctx + GX_FRAME_SUBCTX_OFFSET), rdx = *(bundle+0x30) RTV view, r8 = &f32x4.
pub(crate) const GX_CLEAR_RTV_WRAPPER_RVA: usize = 0x19e0e10;
/// Deobf RVA of the GX frame-context release (deobf 0x1419eaa20; dump FUN_1419eaa40).
pub(crate) const GX_FRAME_CTX_RELEASE_RVA: usize = 0x19eaa20;
/// Offset of the clear-target subcontext pointer inside a popped GX frame context.
pub(crate) const GX_FRAME_SUBCTX_OFFSET: usize = 0x25c8;
/// Per-frame alpha-0 clears issued by the pump (`oracle_portrait_alpha0_clears`).
pub(crate) static PROFILE_ALPHA0_CLEARS: AtomicUsize = AtomicUsize::new(0);
/// One-shot latch for the model node-array enumerator (backdrop-part identification).
pub(crate) static PROFILE_MODEL_PARTS_DUMPED: AtomicUsize = AtomicUsize::new(0);
/// Diagnostic: the captured engine ctx pointer + its `+8` delta-time bits, logged once, to learn whether the
/// context is a stable persistent structure (safe to reuse across frames) or a transient per-call one.
pub(crate) static PROFILE_DRAW_TASK_CTX_LOGGED: AtomicUsize = AtomicUsize::new(0);
/// Count of per-frame model DRAW-task drives we issue (the enqueue/rasterize). Pairs with
/// `oracle_loading_bg_portrait_rgba_version`: once this is ~per-frame, the version should climb past the
/// old stuck ~4 if the per-frame rasterize lands.
pub(crate) static PROFILE_PERFRAME_MODEL_DRAWS: AtomicUsize = AtomicUsize::new(0);
/// Count of direct draws of the POST-Continue SPARED renderer (via the offscreen thunk), and an oracle of
/// whether it still has a live model post-Continue -- the persistent-model path the cycling menu can't give.
pub(crate) static PROFILE_PERFRAME_SPARED_DRAWS: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PROFILE_SPARED_MODEL_OK: AtomicUsize = AtomicUsize::new(0);
/// Q4 keepalive oracle: g_GxDrawContext global (gamebase + this RVA). The offscreen draw rasterizes only
/// when FUN_1419e5850(ctx) returns non-zero, i.e. the GX render-pass queue is non-empty: *(ctx+0xf8) !=
/// *(ctx+0x100). We READ those two qwords non-destructively each draw frame (NO pop) to detect whether a
/// GX pass is queued -- the decisive runtime question for whether a post-Continue / now-loading offscreen
/// render can produce pixels at all. Counters: total samples vs frames the queue was non-empty.
pub(crate) const GX_DRAW_CONTEXT_RVA: usize = 0x47ef360;
pub(crate) const GX_DRAW_CONTEXT_QUEUE_HEAD_OFFSET: usize = 0xf8;
pub(crate) const GX_DRAW_CONTEXT_QUEUE_TAIL_OFFSET: usize = 0x100;
pub(crate) static PROFILE_GX_QUEUE_SAMPLES: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PROFILE_GX_QUEUE_NONEMPTY: AtomicUsize = AtomicUsize::new(0);
/// GX SUBCONTEXT POOL stat offsets (RE-confirmed via Ghidra Initilize@0x1419e7d10 + pop FUN_1419e5850):
/// the `+0xf0` member is a `Vector<subctx*>` whose floor pointer is at `+0xf8` and movable stack-top at
/// `+0x100` (the pop checks `*(ctx+0xf8) == *(ctx+0x100)` for EMPTY, else pops `*(top-8)`, `top -= 8`). So
/// the number of FREE (poppable) subcontexts this frame == `(*(ctx+0x100) - *(ctx+0xf8)) / 8`. The
/// `+0x110` field is a 32-bit lazy-init USED-MASK indexed by `subctx+0x2580 & 0x1f`; once the pool has been
/// fully exercised its popcount == N (the allocated subcontext count = `min(config + clamp(threads,2,16),
/// 32)`). DECISIVE OBSERVABLE: if free-depth stays > 0 across the loading screen, the pool is NOT the cause
/// of the ~4x head refresh (pop never fails) -> the black readback is a cross-queue rasterize/sync issue.
pub(crate) const GX_DRAW_CONTEXT_POOL_FLOOR_OFFSET: usize = 0xf8;
pub(crate) const GX_DRAW_CONTEXT_POOL_TOP_OFFSET: usize = 0x100;
pub(crate) const GX_DRAW_CONTEXT_POOL_USED_MASK_OFFSET: usize = 0x110;
/// Min free-depth seen across the loading screen (init to usize::MAX; a value > 0 at run end proves the
/// pool always had a poppable subcontext -> refutes the "pop fails 96%" pool-contention theory).
pub(crate) static PROFILE_GX_POOL_FREE_MIN: AtomicUsize = AtomicUsize::new(usize::MAX);
/// Most-recent free-depth sample (diagnostic).
pub(crate) static PROFILE_GX_POOL_FREE_LAST: AtomicUsize = AtomicUsize::new(0);
/// Raw `+0x110` used-mask (popcount == N, the allocated subcontext count). Tells us the headroom under 32.
pub(crate) static PROFILE_GX_POOL_USED_MASK: AtomicUsize = AtomicUsize::new(0);
/// Registry of the live profile PoseHolder pointers the game-task tick has resolved as "ours" (0 =
/// empty). The hook only applies look-at to a holder in this set; the c0000 head/neck/spine2 indices
/// are the shared `PROFILE_LOOKAT_*_IDX` globals above, and the angle is the shared yaw/pitch below.
pub(crate) static PROFILE_LOOKAT_HOLDERS: [AtomicUsize; 10] = [const { AtomicUsize::new(0) }; 10];
/// Latest cursor look angles (f32 bits), written by the tick, read by the hook each render frame.
pub(crate) static PROFILE_LOOKAT_YAW_BITS: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PROFILE_LOOKAT_PITCH_BITS: AtomicUsize = AtomicUsize::new(0);
/// `updateBoneModelSpace` hook trampoline / install latch / per-frame hit count (RAM semaphore that the
/// hook is firing for our holders -- the proof the injection point is on the menu render path).
pub(crate) static PROFILE_LOOKAT_HOOK_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static PROFILE_LOOKAT_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PROFILE_LOOKAT_HOOK_HITS: AtomicUsize = AtomicUsize::new(0);
/// Count of per-tick offscreen re-render drives (`FUN_140bb8d90`). Without this the forced portrait only
/// re-renders at the ~4s model-rebuild cadence, so the head appears to track the cursor with seconds of
/// lag; driving the offscreen render each tick (menu phase only -- valid GxDrawContext) makes it smooth.
pub(crate) static PROFILE_LOOKAT_RENDER_DRIVES: AtomicUsize = AtomicUsize::new(0);
/// Set true once the DRAW-PHASE realtime task (`profile_lookat_realtime_draw_tick`) is live. While set,
/// the `updateBoneModelSpace` detour SKIPS its own look-at write (passthrough only): the draw-phase task
/// owns the write+recompute+draw each frame via the trampoline, so the detour must not also post-multiply
/// (that would double-apply / drift). The detour stays installed only to provide the clean recompute
/// trampoline and to passthrough the engine's own natural recompute calls.
pub(crate) static PROFILE_LOOKAT_REALTIME: AtomicBool = AtomicBool::new(false);
/// DRAW-PHASE SWEEP (diagnostic): the realtime draw task is registered in EACH of these candidate CS
/// task-group phases; every registration bumps its own `PROFILE_LOOKAT_PHASE_TICKS[i]` each frame, but
/// only the phase whose index == `PROFILE_LOOKAT_SELECTED_PHASE` actually drives the draw. This lets one
/// run measure which phases tick per-frame at the menu (vs GameSceneDraw, world-gated ~11%) AND switch
/// the active draw phase live (write the index to `er-effects-lookat-phase.txt`) without recompiling,
/// until one renders the portrait smoothly every frame. Order MUST match `LOOKAT_DRAW_PHASE_NAMES` and
/// the registration array in `spawn_game_task`. Default = AdhocDraw (index 5): adjacent to GameSceneDraw
/// (same draw region -> live GX pool) but not world-scene-gated, so the best first bet for per-frame.
pub(crate) const LOOKAT_DRAW_PHASE_COUNT: usize = 8;
pub(crate) const LOOKAT_DRAW_PHASE_NAMES: [&str; LOOKAT_DRAW_PHASE_COUNT] = [
    "Draw_Pre",
    "GraphicsStep",
    "DrawStep",
    "DrawBegin",
    "GameSceneDraw",
    "AdhocDraw",
    "DrawEnd",
    "Draw_Post",
];
pub(crate) const LOOKAT_DRAW_PHASE_DEFAULT: usize = 5;
pub(crate) static PROFILE_LOOKAT_PHASE_TICKS: [AtomicUsize; LOOKAT_DRAW_PHASE_COUNT] =
    [const { AtomicUsize::new(0) }; LOOKAT_DRAW_PHASE_COUNT];
pub(crate) static PROFILE_LOOKAT_SELECTED_PHASE: AtomicUsize =
    AtomicUsize::new(LOOKAT_DRAW_PHASE_DEFAULT);
/// FrameBegin-paced throttle counter for `profile_lookat_phase_diag_tick` (selector re-read + sweep log).
pub(crate) static PROFILE_LOOKAT_PHASE_DIAG_COUNTER: AtomicUsize = AtomicUsize::new(0);
/// Per-stage validity counters for the look-at resolution chain on a fixed probe slot (slot 0), bumped
/// every FrameBegin frame so the sweep log shows EXACTLY where the ~89% drop is (vs guessing). Stages:
/// [0]=renderer table-valid, [1]=model_ins(+0x778), [2]=anim-holder X(+0x948), [3]=importer(*(X+0x20)),
/// [4]=skeleton, [5]=local-bone-data, [6]=bone-count-sane, [7]=frames-probed (denominator).
pub(crate) const PROFILE_LOOKAT_STAGE_COUNT: usize = 8;
pub(crate) const PROFILE_LOOKAT_STAGE_NAMES: [&str; PROFILE_LOOKAT_STAGE_COUNT] = [
    "rend",
    "model_ins",
    "anim948",
    "importer",
    "skel",
    "local",
    "bones",
    "frames",
];
pub(crate) static PROFILE_LOOKAT_STAGE_OK: [AtomicUsize; PROFILE_LOOKAT_STAGE_COUNT] =
    [const { AtomicUsize::new(0) }; PROFILE_LOOKAT_STAGE_COUNT];
/// Draw-task frame counter (drives the selftest sinusoid + throttles the RT-readback oracle).
pub(crate) static PROFILE_LOOKAT_DRAW_FRAME: AtomicUsize = AtomicUsize::new(0);
/// IN-PROCESS PIXEL ORACLE (replaces the human-eyeball check). Each sample reads back the probe slot's
/// offscreen RT AFTER the draw step and records: rt_samples (readbacks taken), rt_nonblack (head rendered,
/// not black -> no flicker), rt_changed (hash != previous -> RT content moved with the driven angle ->
/// tracking), rt_lasthash. PASS under the sinusoid selftest = nonblack≈samples AND changed≈samples.
pub(crate) static PROFILE_LOOKAT_RT_SAMPLES: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PROFILE_LOOKAT_RT_NONBLACK: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PROFILE_LOOKAT_RT_CHANGED: AtomicUsize = AtomicUsize::new(0);
/// Last RT center-region max RGB and max ALPHA (0..255) from the readback. The nonblack oracle only
/// checks RGB, so a portrait that renders RGB content but with ALPHA=0 reads "nonblack" yet GFx
/// alpha-composites it to fully transparent (black shows through). rgb_max>0 with alpha_max==0 is the
/// signature of "renders black despite content" via a zero/premultiplied alpha channel.
pub(crate) static PROFILE_LOOKAT_RT_RGB_MAX: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PROFILE_LOOKAT_RT_ALPHA_MAX: AtomicUsize = AtomicUsize::new(0);
/// One-shot guards for dumping the content RT and the bound SRV to disk for visual inspection (0 = not
/// yet dumped). Lets the agent SEE whether the readback "content" texture is actually the portrait vs a
/// scratch/world RT, and what the SRV holds, before choosing the fix.
pub(crate) static PROFILE_RT_CONTENT_DUMPED: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PROFILE_SRV_DUMPED: AtomicUsize = AtomicUsize::new(0);
/// Count of forced D3D12 RT->SRV CopyResource calls (so the sampleable SRV the forge binds gets the
/// rendered head every frame instead of the engine's rarely-fired resolve). >0 = the copy path runs.
pub(crate) static PROFILE_RT_SRV_COPIES: AtomicUsize = AtomicUsize::new(0);
/// One-shot guard for the RT/SRV resource-identity diagnostic log.
pub(crate) static PROFILE_RT_SRV_COPY_DIAGGED: AtomicUsize = AtomicUsize::new(0);
/// One-shot guard for dumping the excluding-SRV content texture (slot 102) for visual inspection.
pub(crate) static PROFILE_CONTENT_EXCL_DUMPED: AtomicUsize = AtomicUsize::new(0);
/// DRIVE-FREEZE latch: set once a good capture publishes THIS load window, gating off the per-frame
/// renderer drive (the freeze-after-capture UAF fix). Cleared at the window reset AND at a confirm
/// retarget, so the drive re-engages to render the newly-selected character. Distinct from
/// `PROFILE_HAVE_KEYED_FRAME` below: this is per-window (freeze), that one is persistent (display).
pub(crate) static PROFILE_BAKE_RGBA_CAPTURED: AtomicUsize = AtomicUsize::new(0);
/// DISPLAY-AVAILABILITY signal: set the FIRST time a real depth-KEYED (masked) portrait is published
/// and NOT cleared at the window reset/retarget, so the last good masked head keeps displaying while
/// the drive re-renders the next character. This is the make-before-break bridge: the composite shows
/// this persisted frame until the new model produces its own keyed frame, which replaces it. Split
/// from the drive-freeze latch so "re-engage the drive" and "keep showing the old head" are
/// independent -- otherwise clearing the freeze to render the new model also blanked the display.
pub(crate) static PROFILE_HAVE_KEYED_FRAME: AtomicUsize = AtomicUsize::new(0);
/// Diagnostics for the keyed-publish gate + confirm retarget (never render an unmasked model; swap to
/// the newly-selected character at the button press). Exposed as oracles.
pub(crate) static PROFILE_PUBLISH_SKIPPED_UNKEYED: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PROFILE_PORTRAIT_RETARGETS: AtomicUsize = AtomicUsize::new(0);
/// TORN-READBACK detector (pixel semaphore for the scanline-corruption the user saw 2026-07-03). The
/// offscreen RT readback has no cross-queue sync against the game's render of that RT, so when the
/// per-frame drive is active our copy reads rows mid-write -> horizontal scanline tearing. The score
/// is the average absolute VERTICAL luma step across the masked (head) region: a clean face render is
/// smooth vertically (low), a torn readback has random per-row jumps (high). Publishing gates on
/// score <= threshold so a torn frame is never displayed -- the make-before-break bridge keeps the
/// last CLEAN masked head instead. Score range 0..255; the threshold is deliberately sensitive
/// (better to hold the prior clean head than flash garbage). `_last`/`_max` are oracles; the skip
/// counter proves the gate fires; a bimodal last-distribution in the log says clean frames DO land
/// (gate suffices) vs unimodal-high (all torn -> the readback needs real GPU sync).
// Clean face frames score 1-7 (runs 10m/10n); torn frames 16 (mild, run 10n) to 80 (severe, run
// 10m). 34 let the mild-16 tear through and -- because the freeze-after-capture latches the first
// published frame -- it froze on that garbage for the whole window. Tightened to 10: just above the
// clean band, below even mild tearing. Rejection is SAFE (the bridge holds the prior clean head and
// the drive keeps animating until a clean frame lands), so err tight.
pub(crate) const PROFILE_TEAR_SCORE_THRESHOLD: usize = 10;
pub(crate) static PROFILE_TEAR_SCORE_LAST: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PROFILE_TEAR_SCORE_MAX: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PROFILE_TEAR_SCORE_CLEAN_MIN: AtomicUsize = AtomicUsize::new(usize::MAX);
pub(crate) static PROFILE_PUBLISH_SKIPPED_TORN: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PROFILE_PUBLISH_CLEAN: AtomicUsize = AtomicUsize::new(0);
// (Removed the testing tear fail-fast: run autostep10m confirmed the detector separates cleanly
// -- clean frames score 1-7, the torn frame scored 80 -- and torn frames are rare, so the skip gate
// above is the product fix. Regressions surface via oracle_portrait_publish_skipped_torn.)

/// ANIMATION-STALL semaphore (user 2026-07-03: the portrait "stops animating" and stays frozen the
/// whole post-continue loading screen on some loads). freeze-after-capture stops the per-frame drive
/// once the first keyed frame is captured, so the head goes static early. These count, PER loading
/// window: drive frames actually rendered (animated) vs present frames the head was displayed. A low
/// drive/display ratio == froze early (the user's complaint); ~1.0 == animated throughout. Snapshotted
/// to `_LAST` at the window reset for the oracle, then zeroed for the next window.
pub(crate) static PROFILE_DRIVE_FRAMES_WINDOW: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PROFILE_DISPLAY_FRAMES_WINDOW: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PROFILE_DRIVE_FRAMES_WINDOW_LAST: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PROFILE_DISPLAY_FRAMES_WINDOW_LAST: AtomicUsize = AtomicUsize::new(0);
/// The FIRST (displayed) now-loading rti the forge bound, plus its bare texture name + encoding. The
/// sprite commits to the first bind, which happens BEFORE the real portrait is captured -- so once the
/// portrait is baked we RE-FORGE this exact rti to swap the checker for the portrait on the live screen.
pub(crate) static LOADING_BG_FIRST_RTI: AtomicUsize = AtomicUsize::new(0);
pub(crate) static LOADING_BG_FIRST_ENCODING: AtomicUsize = AtomicUsize::new(0);
pub(crate) static LOADING_BG_FIRST_TEX_NAME: std::sync::Mutex<Option<String>> =
    std::sync::Mutex::new(None);
/// (Retired one-shot guard; the reforge is now version-gated via LOADING_BG_REFORGE_VERSION for live
/// re-upload.) Kept allocated to avoid churn; not read.
#[allow(dead_code)]
pub(crate) static LOADING_BG_REFORGE_DONE: AtomicUsize = AtomicUsize::new(0);
/// `CS::TexRepositoryImp::GetResCap(repo, wchar_t* name) -> TexResCap*` (dump 0x140b80a90 -> deobf, shift
/// -0xf0). The TexResCap's `gxTexture` (+TITLE_CUSTOM_COVER_TEX_RESCAP_GX_TEXTURE_OFFSET = +0x78) is the
/// EXACT CSGxTexture the Scaleform now-loading sprite samples by name -- distinct from the forge's source
/// container GX, so we upload the captured portrait into THIS one to actually update the screen.
pub(crate) const TEX_REPOSITORY_GET_RES_CAP_RVA: usize = 0xb809a0;
/// Same RGB/ALPHA-max stats but from a readback of the texture actually BOUND into the now-loading
/// container (what GFx samples), not the renderer's offscreen RT. If the RT (above) has content but this
/// reads black, the sampleable CSGxTexture is a separate/unresolved resource from the render target.
/// 0xffff sentinel = readback did not run / found no resource this sample.
pub(crate) static PROFILE_BOUND_GX_RGB_MAX: AtomicUsize = AtomicUsize::new(0xffff);
pub(crate) static PROFILE_BOUND_GX_ALPHA_MAX: AtomicUsize = AtomicUsize::new(0xffff);
pub(crate) static PROFILE_LOOKAT_RT_LASTHASH: AtomicUsize = AtomicUsize::new(0);
/// Last slot the oracle sampled (the present-model slot cycles), so "changed" is only counted when two
/// consecutive samples are the SAME slot -- otherwise a slot switch (different character) would look like
/// motion. usize::MAX = none yet.
pub(crate) static PROFILE_LOOKAT_RT_LASTSLOT: AtomicUsize = AtomicUsize::new(usize::MAX);
/// Cached selftest flag (the draw task reads this atomic; the FrameBegin diag tick refreshes it from the
/// file throttled, so the draw path never does a per-frame file stat).
pub(crate) static PROFILE_LOOKAT_SELFTEST_ON: AtomicBool = AtomicBool::new(false);
/// Cached cursor-sweep PROOF flag (same latch pattern as selftest). When set, the draw task self-drives
/// the OS cursor through held L/C/R positions and drives the head from the read-back cursor.
pub(crate) static PROFILE_CURSOR_SWEEP_ON: AtomicBool = AtomicBool::new(false);
/// One-shot latch so the cursor-sweep helper logs only its first `SetCursorPos` warp + result.
pub(crate) static PROFILE_CURSOR_SWEEP_FIRST_WARP: AtomicBool = AtomicBool::new(false);
/// Cursor-sweep proof: draw-frames held at each cursor position (~24 frames ≈ 1s at ~23fps), and the
/// per-hold cursor X target as a fraction of the ER window width (left / center / right). Y is held at
/// mid-height. `SetCursorPos(rect.left + fx*w, rect.top + 0.5*h)`.
// Hold of 6 draw-frames per position: a full L/C/R cycle is ~18 frames (<1s), so every position is
// visited several times within the (short) post-Continue live-render window -> all three one-shot bucket
// dumps fill before the menu renderer winds down. The bone drive is instant (no interpolation), so each
// captured frame's pose exactly matches that frame's cursor even at this cadence.
pub(crate) const CURSOR_SWEEP_HOLD_FRAMES: usize = 6;
pub(crate) const CURSOR_SWEEP_TARGETS_X: [f32; 3] = [0.10, 0.50, 0.90];
/// Selftest sinusoid: angular step per draw-frame and yaw/pitch amplitudes (same units as the normalized
/// cursor, so the downstream Head/Neck/Spine2 gains apply identically). ~150-frame period -> ~2.5 s sweep.
pub(crate) const LOOKAT_SELFTEST_W: f32 = 0.0419; // 2*pi/150
pub(crate) const LOOKAT_SELFTEST_YAW_AMP: f32 = 1.0;
pub(crate) const LOOKAT_SELFTEST_PITCH_AMP: f32 = 0.6;
/// RT-readback oracle throttle: sample every N draw-frames (readback is a GPU->CPU stall; don't do it
/// every frame). 8 -> ~7 samples/s, plenty to measure nonblack% and hash-change%.
pub(crate) const LOOKAT_RT_SAMPLE_INTERVAL: usize = 8;
/// DEFAULT-OFF gate for the ProfileSelect load flow (see `profile_select_load_flow_enabled`). When
/// false (default) `product_core_autoload_tick` takes the PROVEN native Continue commit, byte-for-byte
/// unchanged; the human flips this on only to probe-test the portrait-rendering ProfileSelect path
/// (fire the Load-Game row -> live ProfileLoadDialog -> hold for the portrait render -> STAGE2 commit).
pub(crate) const PROFILE_SELECT_LOAD_FLOW_ENABLED: bool = false; // proven Continue char-load is the default; ProfileSelect flow is blocked by the accept-byte open+drain coupling (the only reliable menu-open commits Continue), so it can't get a window to navigate Load-Game -- left gated-off for the record
/// `MarkProfileIndexAsUsed` (deobf 0x140262250): sets `ProfileSummary->saveSlotsStates[slot] = true`
/// (the `bool[10]` at `ProfileSummary+0x8` that the refresh `FUN_1409aa680` gates each slot's portrait
/// render on). `fn(summary, slot)`. NOT called by the ProfileSelect flow by default -- the live
/// ProfileLoadDialog's own header-read marks the slots; wire a call only if a runtime probe shows the
/// target slot stays unmarked (`saveSlotsStates[slot]==0`) inside the open dialog.
pub(crate) const PROFILE_MARK_SLOT_USED_RVA: usize = 0x262250;
/// Target save slot for the menu-phase `force_profile_render` manual diagnostic (the staged
/// single-profile gold save's character is slot 0). The autoload path passes its own target slot
/// instead of this constant.
pub(crate) const FORCE_PROFILE_RENDER_MANUAL_SLOT: i32 = 0;
/// Latched once the portrait render window (hold-the-load-commit-until-the-portrait-renders) has
/// released -- either the portrait was captured or the hold timed out -- so the load commits exactly
/// once thereafter.
pub(crate) static PORTRAIT_RENDER_WINDOW_DONE: AtomicUsize = AtomicUsize::new(0);
/// Passive observer for native Scaleform image-symbol -> system texture bindings.
/// Dump `FUN_1407452c0` maps to live/deobf `0x1407451c0`. It receives an owning resource/list field
/// in rcx and a pair of DLString<char> values in rdx. Do not call it from product code; observe native
/// calls to learn valid owner/resource contexts for SYSTEX-backed surfaces.
pub(crate) const TITLE_SCALEFORM_BIND_OBSERVER_RVA: usize = 0x7451c0;
pub(crate) static TITLE_SCALEFORM_BIND_OBSERVER_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static TITLE_SCALEFORM_BIND_OBSERVER_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub(crate) static TITLE_SCALEFORM_BIND_OBSERVER_HITS: AtomicUsize = AtomicUsize::new(0);
pub(crate) static TITLE_SCALEFORM_BIND_OBSERVER_SYSTEX_HITS: AtomicUsize = AtomicUsize::new(0);
pub(crate) static TITLE_SCALEFORM_BIND_OBSERVER_LAST_OWNER: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_SCALEFORM_BIND_OBSERVER_LAST_PAIR: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_SCALEFORM_BIND_OBSERVER_LAST_SYMBOL_PTR: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_SCALEFORM_BIND_OBSERVER_LAST_TARGET_PTR: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
/// Experimental visible-surface bind rewrite for the replayed ProfileSelect cover: the native
/// SYSTEX profile texture normally targets `MENU_DummyProfileFace_01`; rewrite slot0 to the
/// visibly placed `MENU_FL_40135_Profile` surface and expose it as a distinct oracle.
pub(crate) const TITLE_PROFILE_VISIBLE_SURFACE_SYMBOL: &str = "MENU_FL_40135_Profile";
pub(crate) static TITLE_PROFILE_VISIBLE_SURFACE_BIND_REWRITES: AtomicUsize = AtomicUsize::new(0);
pub(crate) static TITLE_PROFILE_VISIBLE_SURFACE_BIND_LAST_OWNER: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_PROFILE_VISIBLE_SURFACE_BIND_LAST_PAIR: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_PROFILE_VISIBLE_SURFACE_BIND_LAST_SYMBOL_PTR: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);

