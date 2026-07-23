// ---------------------------------------------------------------------------------------------------
// CAMERA LEVER (custom profile-portrait viewport). VERIFIED RE 2026-06-29 -- bd
// `camera-lever-RE-VERIFIED-offsets-and-call-addrs-2026-06-29`. The interactive-face roadmap's camera
// function addresses were garbled (dump-vs-deobf space confusion); these are ground-truthed against the
// Ghidra runtime dump (`pc_eldenring_runtime.1.16.1.exe`) + `scripts/dump-deobf-shift.py`.
//
// The `CSMenuProfModelRend` ctor (dump 0x140bbe010) sets the orbit camera ONCE from `MenuOffscrRendParam`
// via `FUN_140bbe190`, which (a) writes the orbit fields below, (b) builds a view matrix into `+0x9e0`
// via `FUN_140bbe480`, (c) pushes the CSPersCam (`+0x9d0`) into the offscreen render via `FUN_140bba550`.
// We replicate steps (b)+(c) AFTER writing our own orbit fields, and never call `FUN_140bbe190` itself
// (it re-reads the param and clobbers the orbit fields).
//
// All offsets are BYTE offsets from the renderer (CSMenuProfModelRend) base.
/// Orbit target point, `Vec3` (x@+0x9b4, y@+0x9b8, z@+0x9bc); `w`@+0x9c0 is 1.0.
pub(crate) const PROFILE_CAM_TARGET_OFFSET: usize = 0x9b4;
pub(crate) const PROFILE_CAM_TARGET_W_OFFSET: usize = 0x9c0;
/// Orbit distance (f32). Consumed sign-flipped by the matrix builder (camera sits behind the target);
/// a SMALLER value = closer.
pub(crate) const PROFILE_CAM_DISTANCE_OFFSET: usize = 0x9c4;
/// Orbit yaw (f32, radians) -- horizontal turn (Y-axis rotation in the matrix builder). Confirmed by
/// the 2026-06-29 runtime smoke: a large delta on the OTHER field (+0x9cc) shifted the framing
/// vertically, so +0x9c8 is yaw and +0x9cc is pitch (corrects the initial swapped labels).
pub(crate) const PROFILE_CAM_YAW_OFFSET: usize = 0x9c8;
/// Orbit pitch (f32, radians) -- vertical tilt (X-axis rotation in the matrix builder).
pub(crate) const PROFILE_CAM_PITCH_OFFSET: usize = 0x9cc;
/// The embedded `CSPersCam` subobject (the `rdx` argument to the push). Its view matrix lives at
/// CSCam+0x10 == renderer+0x9e0; `fov`@+0xa20, `aspectRatio`@+0xa24 (far=10000, near=0.05 defaults).
pub(crate) const PROFILE_CAM_PERSCAM_OFFSET: usize = 0x9d0;
/// The computed 4x4 view matrix (16 f32 = 64 bytes), == the CSPersCam view matrix.
pub(crate) const PROFILE_CAM_VIEW_MATRIX_OFFSET: usize = 0x9e0;
/// Field-of-view (f32, radians) == CSPersCam.fov.
pub(crate) const PROFILE_CAM_FOV_OFFSET: usize = 0xa20;
/// Aspect ratio (f32) == CSPersCam.aspectRatio.
pub(crate) const PROFILE_CAM_ASPECT_OFFSET: usize = 0xa24;
/// View-matrix builder `FUN_140bbe480` (dump) -> deobf 0x140bbe390 (shift -0xf0, content-unique).
/// `fn(renderer /rcx/, out: *mut f32[16] /rdx/) -> *mut f32`. Pure orbit->view-matrix math (sinf/cosf
/// of pitch/yaw, target, -distance); reads renderer+0x9b4/+0x9c4/+0x9c8/+0x9cc; no render context,
/// allocation, or lock.
pub(crate) const PROFILE_CAM_BUILD_MATRIX_RVA: usize = 0xbbe390;
/// Camera push `FUN_140bba550` (dump) -> deobf 0x140bba460 (shift -0xf0, content-unique).
/// `fn(renderer /rcx/, persCam = renderer+0x9d0 /rdx/)`. Copies the cam matrix+projection into the
/// offscreen render's view-state (`*(renderer+0xa8)`) and recomputes derived matrices/viewport. Verified
/// pure CPU state (no GPU submit / allocation / lock) -- safe on the CSTaskImp game thread; it is the
/// exact path the engine runs at renderer construction.
pub(crate) const PROFILE_CAM_PUSH_RVA: usize = 0xbba460;
/// Custom-viewport transform applied to the engine's latched baseline orbit. Produces a visibly closer,
/// tilted portrait framing vs the engine's straight-on default. These exact values are the framing the
/// user approved in the 2026-06-29 runtime smoke (a tight zoom with a strong upward pitch into the
/// face); the deltas are correctly named after the pitch/yaw fix and remain free knobs to retune.
// ZOOM-OUT (2026-06-30, user: loading-screen face was way too zoomed -- only forehead/eyes visible).
// Pull the camera BACK past the engine baseline (>1.0) to a head-and-shoulders product shot, and drop the
// strong upward pitch that framed the forehead. Free knobs -- retune from the user's image.
// ZOOM-OUT AGAIN (2026-07-06, user): a character with a massive-head helmet FILLED the entire frame at
// 1.7, leaving NO background -- so the depth mask cut nothing (unkeyed) and every frame was rejected, i.e.
// the render was fine but the framing starved the keyer. Pull back generously so even the biggest head
// leaves background margin for the depth key. The overlay aspect-covers the keyed RT, so a smaller head
// in the RT still composites; a head with no surrounding background does not.
pub(crate) const PROFILE_CAM_DISTANCE_SCALE: f32 = 6.0;
/// Slight vertical tilt only (was 0.40 = a forehead close-up).
pub(crate) const PROFILE_CAM_PITCH_DELTA_RAD: f32 = 0.05;
/// Head-on by default (2026-06-30, user): zero horizontal turn so the camera faces the character
/// straight-on (was -0.06, a slight off-axis turn).
pub(crate) const PROFILE_CAM_YAW_DELTA_RAD: f32 = 0.0;
pub(crate) const PROFILE_CAM_FOV_SCALE: f32 = 1.0;
/// Per-slot latched baseline orbit, captured ONCE (before the first override write) so every per-tick
/// override is derived from an immutable baseline -- drift-free and clobber-proof even if a refresh
/// re-runs the engine camera setup. `Copy` so the array-repeat initializer below is const.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ProfileCamBaseline {
    pub target: [f32; 3],
    pub distance: f32,
    pub pitch: f32,
    pub yaw: f32,
    pub fov: f32,
}
pub(crate) static PROFILE_CAM_BASELINE: std::sync::Mutex<[Option<ProfileCamBaseline>; 10]> =
    std::sync::Mutex::new([None; 10]);
/// Camera-override telemetry (RAM semaphores): total applies (matrix build + push), bit-per-slot
/// latched-baseline mask, last applied slot, and whether the last built view matrix was all-finite.
pub(crate) use er_telemetry::counters::PROFILE_CAM_APPLY_CALLS;
pub(crate) use er_telemetry::counters::PROFILE_CAM_LATCHED_MASK;
pub(crate) static PROFILE_CAM_LAST_SLOT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) use er_telemetry::counters::PROFILE_CAM_LAST_MATRIX_OK;
/// Offscreen render camera-params POD (the ~0xc4-byte block `FUN_140cca450` blits, dump 0x140cca450).
/// VERIFIED RE 2026-06-29. Reached via the camera push: `FUN_140bba550` -> `FUN_140bb7da0` ->
/// `FUN_141ad94e0` -> `FUN_140cca450(dst = *(offscreenRend+0x20) + 0xd0, src = *(offscreenRend+0x28))`.
/// The leading 4x4 view matrix at +0x00 is written by `FUN_141a536b0` (copies exactly 0x40 bytes); the
/// 1280x720 (0x500x0x2d0) viewport rects and the fov/aspect copies are written by `FUN_140b12260`.
/// Fields named where the RE is confident; the rest are kept as offset-named `u32`/`f32` so the exact
/// layout is preserved and editable as future RE resolves them. This represents the 0xc4 bytes
/// `FUN_140cca450` copies; the containing allocation may be larger. `#[repr(C)]` with all-4-byte fields
/// keeps every field naturally aligned at its true offset (the engine reads some as unaligned u64).
/// Documentary/layout type: never constructed at runtime (the engine populates the real block) -- kept
/// for future view/use/edit, with the size/align asserts below as the compile-time layout guard.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub(crate) struct OffscreenRenderCamParams {
    /// +0x00: 4x4 view matrix (row-major as the engine stores it). Written by `FUN_141a536b0`.
    pub view_matrix: [f32; 16],
    /// +0x40: inferred camera position / extra row (set outside the copy path; unconfirmed).
    pub field_0x40: [f32; 4],
    /// +0x50: field-of-view (copied from view-state+0x50 by `FUN_140b12260`).
    pub fov: f32,
    /// +0x54, +0x58: inferred near/far plane (copied from view-state+0x58/+0x5c).
    pub field_0x54: f32,
    pub field_0x58: f32,
    /// +0x5c/+0x60: primary viewport width/height (set to 1280/720 by `FUN_140b12260`).
    pub viewport_width_a: u32,
    pub viewport_height_a: u32,
    /// +0x64, +0x68: unknown.
    pub field_0x64: u32,
    pub field_0x68: u32,
    /// +0x6c: aspect ratio (copied from view-state+0x54 by `FUN_140b12260`).
    pub aspect_ratio: f32,
    /// +0x70: unknown (NOT copied by `FUN_140cca450`; present in the layout).
    pub field_0x70: u32,
    /// +0x74..+0x9c: unknown.
    pub field_0x74: u32,
    pub field_0x78: u32,
    pub field_0x7c: u32,
    pub field_0x80: u32,
    pub field_0x84: u32,
    pub field_0x88: u32,
    pub field_0x8c: u32,
    pub field_0x90: u32,
    pub field_0x94: u32,
    pub field_0x98: u32,
    pub field_0x9c: u32,
    /// +0xa0..+0xb7: three more viewport width/height rects (also 1280/720; scissor/full/etc.).
    pub viewport_width_b: u32,
    pub viewport_height_b: u32,
    pub viewport_width_c: u32,
    pub viewport_height_c: u32,
    pub viewport_width_d: u32,
    pub viewport_height_d: u32,
    /// +0xb8..+0xc3: unknown (tail of the copied region).
    pub field_0xb8: u32,
    pub field_0xbc: u32,
    pub field_0xc0: u32,
}
const _: () = assert!(core::mem::size_of::<OffscreenRenderCamParams>() == 0xc4);
const _: () = assert!(core::mem::align_of::<OffscreenRenderCamParams>() == 4);
