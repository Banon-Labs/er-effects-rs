//! The live SIDE: read the creature's size, patch a free `LockCamParam` row, and point
//! `ChrExFollowCam+0x468` at it. Everything reversible, everything reversed on release.
//!
//! # It resolves no game address and installs no detour
//!
//! Same property layers 1 and 2 have and layer 3 spends exactly once. Every reach here is either a
//! DLRF-name-resolved singleton (`WorldChrMan`, `SoloParamRepository` -- build-independent by
//! construction, there is no RVA to go stale) or a struct-field offset out of
//! [`crate::camera::layout`]. There is no call, no prologue, no hook.
//!
//! It is the first layer of this crate to write a param row, which is a different kind of write
//! from a struct field -- see `scripts/me3-dll-conflicts.toml` for why that is still co-loadable.
//!
//! # Why patching the row is the mechanism, rather than writing camera state
//!
//! `CS::ChrExFollowCam::ApplyZoomLerp` (1.17 `0x1403b7570`) runs every frame from
//! `ChrExFollowCam::Update`, calls `LookupLockCamParam` afresh each time, and re-derives distance,
//! pivot height, pitch minimum, FOV and chase rate from the row it gets back. Writing the derived
//! camera state directly would be overwritten before the next frame drew; writing the row is read
//! by the same code every frame with no fight and no per-frame work of ours. That the lookup is
//! per-frame is also what makes the patch live: there is no load-time copy to miss.
//!
//! # The three ways this can go wrong, and what each does instead
//!
//! * **A missing row id is worse than no override.** `LookupLockCamParam` returns a NULL row for
//!   an id that is not in the table, and `ApplyZoomLerp` then does *nothing at all* -- the camera
//!   freezes at whatever it was last frame, no crash, no fallback. So the row is looked up before
//!   `+0x468` is written, and a missing one refuses.
//! * **A row somebody else uses.** Patching a row that a `NpcParam.lockCameraParamId` or
//!   `RideParam.rideCamParamId` names would move that character's camera too. The check is done
//!   against the live regulation rather than a table generated offline, so a player running a
//!   regulation mod gets a refusal naming the row instead of a silently wrong camera.
//! * **A rebuilt `ChrExFollowCam`.** The constructor writes `+0x464|+0x468 = -1`, so a camera
//!   rebuilt mid-possession (a warp, a map load) would silently drop the override.
//!   [`Session::reassert`] costs one compare per frame and puts it back, the same shape as the
//!   camera-override re-assert the driver already does for `WorldChrManDbg+0xb8`.

use eldenring::cs::{
    CSPersCam, ChrCam, LockCamParam, NpcParam, RideParam, SoloParam, SoloParamRepository,
    WorldChrMan,
};
use eldenring::param::LOCK_CAM_PARAM_ST;
use er_game_base::game_build::game_file_version;
use er_game_base::mem::{safe_read_f32, safe_read_i32, safe_read_usize};
use fromsoftware_shared::FromStatic;

use crate::camera::derived;
use crate::camera::geometry::{Refusal, Report, shape};
use crate::camera::layout::{self, Offsets};
use crate::log::possess_log;
use crate::possess::layout::{chr_ins, modules};
use crate::settings::CameraSettings;

/// Compile-time cross-check, same idea as [`crate::possess::game`]'s.
///
/// `ChrCam.chrExFollowCam` is a private field in the crate, so it cannot be named by
/// `offset_of!`. It is the field immediately after `ChrCam`'s `CSPersCam` superclass, though, so
/// the superclass's size is its offset -- and that the crate and this table agree on where the
/// follow camera lives is the thing worth failing the build over.
const _: () = {
    assert!(core::mem::offset_of!(WorldChrMan, chr_cam) == layout::world_chr_man::CHR_CAM);
    assert!(core::mem::size_of::<CSPersCam>() == layout::chr_cam::EX_FOLLOW_CAM);
};

/// `LockCamParam` row 0 -- the player's own camera row, and the fallback base when the id the
/// camera resolved last frame is unreadable. Its `camDistTarget` 3.8 and `camFovY` 48 are the
/// modal values across the whole 166-row table.
const PLAYER_LOCK_CAM_ROW: u32 = 0;

/// Everything that has to be put back, captured before anything was written.
struct Installed {
    /// The row that was patched.
    row: u32,
    /// `ChrExFollowCam+0x468`.
    override_slot: usize,
    /// What that slot held before -- `-1` on a camera the game constructed and nothing has
    /// touched, which is every camera, because nothing in the game writes this field.
    original_param_id: i32,
    /// The patched row's contents before the patch, restored whole rather than field by field:
    /// the install writes the whole row, so the exact inverse is writing the whole row back.
    original_row: LOCK_CAM_PARAM_ST,
}

/// One possession's worth of camera adaptation.
///
/// Constructed even when nothing is installed, because the report is the product too: a possession
/// whose camera did not change has to say why in the derived file.
pub(crate) struct Session {
    report: Report,
    installed: Option<Installed>,
    /// The subject, kept so a config reload can rebuild the patch without the driver handing it
    /// back.
    chr_ins: usize,
    /// ...and its `NpcParam` id / 10000, for the per-character `camera_distance_scale` lookup.
    chr_id: u32,
    /// The `[camera]` table this was built from.
    settings: CameraSettings,
    /// `[chr.cNNNN].camera_distance_scale` at the same moment.
    distance_scale: f32,
    /// `crate::config::generation()` when the two above were read. See [`Self::refresh`].
    generation: usize,
}

impl Session {
    /// Adapt the camera to `chr_ins`, reading the settings that decide how.
    pub(crate) fn begin(chr_ins: usize, chr_id: u32) -> Self {
        let generation = crate::config::generation();
        let (settings, distance_scale) = live_settings(chr_id);
        Self::install(chr_ins, chr_id, settings, distance_scale, generation, None)
    }

    /// Re-apply after a config reload moved something, so `[camera]` is live the way
    /// `[movement].speed_scale` is: save the file, watch the framing change.
    ///
    /// Rebuilds rather than edits the row in place, because that is the same code path a fresh
    /// possession takes -- including the free-row check, which a reload can invalidate by pointing
    /// `param_row` somewhere new. Answers whether anything was re-applied.
    pub(crate) fn refresh(&mut self) -> bool {
        let generation = crate::config::generation();
        if generation == self.generation {
            return false;
        }
        self.generation = generation;
        let (settings, distance_scale) = live_settings(self.chr_id);
        if settings == self.settings && distance_scale == self.distance_scale {
            return false;
        }
        // The base row is carried over, not RE-read. `ChrExFollowCam+0x460` still holds our row id
        // -- `ApplyZoomLerp` will not refresh it until next frame -- so re-reading it would copy
        // the untouched fields out of the row this reload is abandoning. That is invisible while
        // `param_row` stays put (the mirror equals the target and the fallback catches it) and
        // silently wrong the moment a reload moves `param_row`, which is exactly the edit somebody
        // makes when row 1000 turned out to be taken.
        let base = self.report.base_row;
        self.restore();
        *self = Self::install(
            self.chr_ins,
            self.chr_id,
            settings,
            distance_scale,
            generation,
            base,
        );
        true
    }

    /// Adapt the camera to `chr_ins`, or work out why not.
    ///
    /// Never panics and never leaves half a patch behind: the follow camera is resolved before the
    /// row is touched, and the one write that can still fail after the row is patched rolls the
    /// row back.
    fn install(
        chr_ins: usize,
        chr_id: u32,
        settings: CameraSettings,
        distance_scale: f32,
        generation: usize,
        base_override: Option<u32>,
    ) -> Self {
        let row = settings.param_row;
        let refuse = |reason: Refusal| Self {
            report: Report::refused(row, distance_scale, reason),
            installed: None,
            chr_ins,
            chr_id,
            settings,
            distance_scale,
            generation,
        };

        if !settings.enabled {
            return refuse(Refusal::Disabled);
        }
        let Some(offsets) = layout::offsets(game_file_version()) else {
            return refuse(Refusal::UnmeasuredBuild);
        };
        let Some((height, radius)) = hit_extents(chr_ins, offsets) else {
            return refuse(Refusal::NoHeight);
        };
        let Some(shape) = shape(height, radius, distance_scale, settings) else {
            return refuse(Refusal::NoHeight);
        };
        // Resolved before the row is patched, so the commonest failure needs no rollback.
        let Some(follow_cam) = follow_cam(offsets) else {
            return refuse(Refusal::NoFollowCam);
        };
        let override_slot = follow_cam + offsets.lock_cam_param_override;
        let Some(original_param_id) = (unsafe { safe_read_i32(override_slot) }) else {
            return refuse(Refusal::WriteFailed);
        };

        // Safety: the singleton reference is only handed out when the pointer is populated, and
        // every row read or written below is inside the param heap that reference owns. The
        // holders are checked first because `rows`/`get_mut` PANIC on a param that has not
        // streamed in -- `get_param_file` does `holder.get_res_cap(0).expect(..)`.
        let Ok(repo) = (unsafe { SoloParamRepository::instance_mut() }) else {
            return refuse(Refusal::ParamsNotReady);
        };
        if !holder_ready::<LockCamParam>(repo)
            || !holder_ready::<NpcParam>(repo)
            || !holder_ready::<RideParam>(repo)
        {
            return refuse(Refusal::ParamsNotReady);
        }
        if let Some(user) = row_user(repo, row) {
            return refuse(Refusal::RowInUse(user));
        }

        // The fields the size law does not decide -- FOV, the pitch minimum, the lock vertical
        // offset, the chase rate, the lock-on radii -- come from whatever row the camera resolved
        // a frame ago rather than from the target row's own values, which belong to some unrelated
        // creature. Row 1000 ships `camFovY = 54.5` and `chrTransChaseRateForNormal = 0.2` against
        // the player row's 48.0 and -1.0, so leaving them alone would silently widen the FOV and
        // override the chase rate the moment anything was possessed.
        //
        // `rotRangeMinX` is on that list deliberately. It used to be written, lerped towards -15
        // degrees for a large subject on the theory that a tall creature needs more overhead --
        // but it is the limit on how far below the subject the camera may drop, not above (see
        // `crate::camera::geometry`), so that bought nothing and cost a shot. Copying it means a
        // map region that narrowed the pitch range keeps its narrowing through a possession.
        let base_row = base_override
            .filter(|base| *base != row)
            .unwrap_or_else(|| base_row(follow_cam, offsets, row));
        let Some(mut patched) = repo.get::<LockCamParam>(base_row).cloned() else {
            return refuse(Refusal::BaseRowMissing);
        };
        patched.set_cam_dist_target(shape.distance);
        patched.set_chr_org_offset_y(shape.pivot_height);

        let Some(target) = repo.get_mut::<LockCamParam>(row) else {
            return refuse(Refusal::RowMissing(row));
        };
        let original_row = target.clone();
        *target = patched;

        if !write_i32(override_slot, i32::try_from(row).unwrap_or(-1)) {
            // Put the row back rather than leaving a patched row nothing points at. It would be
            // harmless -- the row is unreferenced -- but "nothing we wrote outlives the attempt"
            // is a cheaper invariant to keep than to reason about.
            if let Some(target) = repo.get_mut::<LockCamParam>(row) {
                *target = original_row;
            }
            return refuse(Refusal::WriteFailed);
        }

        Self {
            report: Report {
                hit_height: Some(height),
                hit_radius: Some(radius),
                row,
                base_row: Some(base_row),
                distance_scale,
                applied: Some(shape),
                refusal: None,
            },
            installed: Some(Installed {
                row,
                override_slot,
                original_param_id,
                original_row,
            }),
            chr_ins,
            chr_id,
            settings,
            distance_scale,
            generation,
        }
    }

    /// Put the override back if something reset it.
    ///
    /// Nothing in the game writes `+0x468`, but the `ChrExFollowCam` CONSTRUCTOR sets it to -1 as
    /// half of one qword store, so a camera rebuilt mid-possession loses the override. One read
    /// and a compare per frame.
    pub(crate) fn reassert(&self) {
        let Some(installed) = self.installed.as_ref() else {
            return;
        };
        let Ok(wanted) = i32::try_from(installed.row) else {
            return;
        };
        if unsafe { safe_read_i32(installed.override_slot) } != Some(wanted) {
            write_i32(installed.override_slot, wanted);
        }
    }

    /// Undo everything, in the reverse order it was done. Answers whether it all took.
    ///
    /// Safe to call when nothing was installed, which is what makes it a teardown step the state
    /// machine can run unconditionally.
    pub(crate) fn restore(&mut self) -> bool {
        let Some(installed) = self.installed.take() else {
            return true;
        };
        // Both writes land in one call, so no frame can see a half-restored camera. The order is
        // about which half survives a partial failure: with the slot cleared first, a row that
        // will not write back is unreferenced garbage and the camera is already vanilla. The other
        // way round leaves `+0x468` naming a row that now holds the base row's numbers, which is a
        // camera nobody asked for and nothing will fix.
        let slot = write_i32(installed.override_slot, installed.original_param_id);
        let row = restore_row(installed.row, installed.original_row);
        if !slot || !row {
            possess_log(format_args!(
                "camera: RESTORE INCOMPLETE -- ChrExFollowCam+0x468 {}, LockCamParam row {} {}. \
                 The follow camera may keep the possessed creature's framing until the next map \
                 load rebuilds it.",
                if slot { "restored" } else { "NOT restored" },
                installed.row,
                if row { "restored" } else { "NOT restored" },
            ));
        }
        slot && row
    }

    /// What to print in `er-npc-possess.derived.toml`.
    pub(crate) fn derived_block(&self, chr_id: u32) -> String {
        derived::render(chr_id, &self.report)
    }

    /// One line for `er-npc-possess.log`, said once at possession start.
    pub(crate) fn log_line(&self) -> String {
        match (self.report.applied, self.report.refusal) {
            (Some(shape), _) => {
                let height = self.report.hit_height.unwrap_or_default();
                let framing = crate::camera::geometry::framing(shape, height, 0.0);
                format!(
                    "camera: hitHeight {height:.2} m -> LockCamParam row {} patched (dist \
                     {:.2} m, pivot {:.2} m) and ChrExFollowCam+0x468 pointed at it. That frames \
                     the top of the body at {:+.4} half-screen-heights with {:.2} body-heights \
                     above it; the player's own is +0.0296 / 1.09.",
                    self.report.row,
                    shape.distance,
                    shape.pivot_height,
                    framing.head_screen_y,
                    framing.headroom_heights,
                )
            }
            (None, Some(reason)) => format!(
                "camera: NOT adapted -- {}. The creature is framed with the parameters your own \
                 body would have used, which on anything large puts the camera inside the model. \
                 See {}.",
                reason.describe(),
                crate::config::DERIVED_CONFIG_FILE_NAME,
            ),
            (None, None) => "camera: not adapted, and for no recorded reason".to_owned(),
        }
    }
}

/// The two settings the size law needs, read together so they describe one instant of the file.
fn live_settings(chr_id: u32) -> (CameraSettings, f32) {
    let scale = crate::config::chr_override(chr_id)
        .and_then(|over| over.camera_distance_scale)
        .unwrap_or(1.0);
    (crate::config::camera(), scale)
}

/// Is `P`'s res cap streamed in? `rows` and `get_mut` panic when it is not.
fn holder_ready<P: SoloParam>(repo: &SoloParamRepository) -> bool {
    repo.solo_param_holders
        .get(P::INDEX as usize)
        .and_then(|holder| holder.get_res_cap(0))
        .is_some()
}

/// The row id whose fields the patch is built on: whatever the camera resolved last frame, read
/// out of the `ChrExFollowCam+0x460` mirror `ApplyZoomLerp` writes for free.
///
/// Falls back to the player row when the mirror is unreadable, negative, or -- the case that
/// matters -- already names the row about to be patched, which would compound our own numbers on a
/// second possession after a failed restore.
fn base_row(follow_cam: usize, offsets: Offsets, target: u32) -> u32 {
    let mirrored = unsafe { safe_read_i32(follow_cam + offsets.resolved_lock_cam_param) };
    match mirrored.and_then(|id| u32::try_from(id).ok()) {
        Some(id) if id != target => id,
        _ => PLAYER_LOCK_CAM_ROW,
    }
}

/// The first param row in the live regulation that names `row`, or `None` when it is free.
///
/// `NpcParam.lockCameraParamId` and `RideParam.rideCamParamId` are the only two fields in any of
/// the 179 paramdefs that reference a `LockCamParam` id, so these two scans are the whole search.
/// In the shipped regulation 73 of the 166 rows come back free, including all of 1000-1099.
fn row_user(repo: &SoloParamRepository, row: u32) -> Option<u32> {
    let Ok(wanted) = i32::try_from(row) else {
        return None;
    };
    if repo
        .rows::<NpcParam>()
        .any(|(_, npc)| npc.lock_camera_param_id() == wanted)
    {
        return Some(row);
    }
    repo.rows::<RideParam>()
        .any(|(_, ride)| ride.ride_cam_param_id() == wanted)
        .then_some(row)
}

/// Put one `LockCamParam` row back. Separated so the restore path re-resolves the singleton rather
/// than holding a reference across the whole possession.
fn restore_row(row: u32, original: LOCK_CAM_PARAM_ST) -> bool {
    // Safety: as in `begin`; the holder is checked before the row is reached.
    let Ok(repo) = (unsafe { SoloParamRepository::instance_mut() }) else {
        return false;
    };
    if !holder_ready::<LockCamParam>(repo) {
        return false;
    }
    match repo.get_mut::<LockCamParam>(row) {
        Some(target) => {
            *target = original;
            true
        }
        None => false,
    }
}

/// `WorldChrMan.chrCam -> +0x60 chrExFollowCam`, or `None` when either link has not come up.
fn follow_cam(offsets: Offsets) -> Option<usize> {
    // Safety: `instance()` yields a reference only when the singleton is populated; the field read
    // below goes through the typed pointer the crate models, and the one raw read after it is
    // fault-tolerant.
    let world_chr_man = unsafe { WorldChrMan::instance() }.ok()?;
    let chr_cam: *const ChrCam = world_chr_man.chr_cam?.as_ptr();
    let follow = unsafe { safe_read_usize(chr_cam as usize + offsets.ex_follow_cam) }?;
    // A follow camera that is not a plausible heap pointer means the ChrCam is half-constructed;
    // `+0x468` on a bad base is a write into whatever happens to be there.
    unsafe { er_game_base::mem::is_heap_aligned_ptr(follow) }.then_some(follow)
}

/// Where the player is LOOKING: the camera's own yaw, or `None` when the camera is not up.
///
/// One `f32` at `ChrExFollowCam+0x154 anglesEuler.y`, which the engine derives as
/// `atan2(look.x, look.z)` -- see [`layout::chr_ex_follow_cam::ANGLES_EULER_YAW`] for the four
/// lines of `Update` that compute it and the byte window that pins it on both builds. This layer
/// still resolves no game function address.
///
/// **The sign is not a character heading.** A body facing this direction has yaw
/// `anglesEuler.y + PI`; [`crate::possess::intent::aim`] is the only place that conversion is
/// made, so there is one copy of it.
///
/// # Why this answers the lock-on case too
///
/// Lock-on drives this camera -- `ChrExFollowCam::Update` derives these angles from the point the
/// camera is aimed at, and a held lock is what moves that point. So "where the camera looks"
/// already points at the locked target, which is why [`crate::possess::intent::aim`] treats the
/// camera as the always-available answer and the subject's own `ChrIns+0xd0 lockOnTargetPos` as a
/// refinement it accepts only when the two agree.
pub(crate) fn look_yaw() -> Option<f32> {
    let offsets = layout::offsets(game_file_version())?;
    let follow_cam = follow_cam(offsets)?;
    // Safety: the read goes through `ReadProcessMemory`, so an unmapped address answers `None`
    // instead of faulting.
    let yaw = unsafe { safe_read_f32(follow_cam + offsets.angles_euler_yaw) }?;
    yaw.is_finite().then_some(yaw)
}

/// `[[ChrIns+0x190] + 0x68] + 0x340/0x344` -- the physics capsule's height and radius, in metres.
///
/// This is what `CS::ChrIns::GetPhysicsHitHeight` returns, reached by the two loads the function
/// itself performs rather than by calling it, which is why this layer resolves no address.
fn hit_extents(chr_ins: usize, offsets: Offsets) -> Option<(f32, f32)> {
    let modules = unsafe { safe_read_usize(chr_ins + chr_ins::MODULES) }?;
    let physics = unsafe { safe_read_usize(modules + modules::PHYSICS) }?;
    let height = unsafe { safe_read_f32(physics + offsets.hit_height) }?;
    let radius = unsafe { safe_read_f32(physics + offsets.hit_radius) }?;
    Some((height, radius))
}

/// Write an `i32`, but only after proving the address reads. Same contract as the writers in
/// [`crate::possess::game`]: a read faults harmlessly, a write does not get the chance.
fn write_i32(at: usize, value: i32) -> bool {
    if unsafe { safe_read_i32(at) }.is_none() {
        return false;
    }
    unsafe { (at as *mut i32).write(value) };
    true
}
