//! Every touch of live game memory the engine makes, and nothing that decides anything.
//!
//! # What is deliberately not here
//!
//! **Almost no native calls: One game function address in the whole module, and no detour.** Every
//! lever the possession itself needs turned out to be a field: `EnableInvincible`'s whole body is
//! `chrFlags1c5 = (chrFlags1c5 & 0xef) | (state << 4)`, `SetDisableFallDamage`'s is
//! `MOV [RCX+0x1B],DL; RET`, the move is the engine's own per-frame proxy drain rather than a
//! teleport call, and firing an attack is a write to `CSChrEventModule+0x18`.
//!
//! The single exception is [`PLAY_ANIMATION_BY_BEHAVIOR_NAME_RVA`], and it was not a free choice.
//! The field write formats `W_Event%04d` and nothing else, and `W_Event` is a broad alias layer
//! rather than a total one -- so every dodge in the game (2,252 of them, spelled `W_Step`) is
//! unreachable without it. Keeping the property would have meant shipping a possession that cannot
//! roll. That address is registered as a verified 1.16.2 -> 1.17 pair and resolved through
//! `game_rva_named`, so on an unrecognised build the fallback goes quiet rather than jumping into
//! whatever now occupies those bytes.
//!
//! Everything else here is still translation-proof; what it can be broken by is a struct offset,
//! which is why [`crate::possess::layout`] is one module and why the assertions below exist.
//!
//! # The two singletons, and why neither is an address either
//!
//! `WorldChrMan` and `WorldChrManDbg` come from `fromsoftware-rs`'s `FromStatic`, which resolves
//! them through the game's own DLRF reflection data by name. That is build-independent by
//! construction: there is no RVA to go stale.
//!
//! # Reads fault, writes do not get the chance
//!
//! Every read of game memory goes through `er_game_base::mem::safe_read_*`, which is
//! `ReadProcessMemory` against the current-process pseudo-handle -- a despawned character or a
//! half-constructed pointer chain answers `None` instead of raising an access violation. A write
//! cannot be made fault-tolerant that way, so every write here reads the same address first and
//! skips when the read fails. That turns a stale pointer into a missed frame rather than a crash.

use eldenring::cs::{
    CSChrActionRequestModule, CSChrBehaviorModule, CSChrDataModule, CSChrEventModule,
    CSChrPhysicsModule, CSChrTimeActModule, CSChrTimeActModuleAnim, ChrCtrl as CsChrCtrl,
    ChrIns as CsChrIns, ChrInsModuleContainer, ChrSet, ChrType, WorldChrMan, WorldChrManDbg,
};
use er_game_base::game_build::game_file_version;
use er_game_base::mem::{
    game_rva_named, is_heap_aligned_ptr, safe_read_f32, safe_read_i32, safe_read_u8, safe_read_u16,
    safe_read_usize,
};
use fromsoftware_shared::FromStatic;

use crate::possess::fall::{self, FallState};
use crate::possess::intent::{IntentWrite, Stick};
use crate::possess::layout::{self, packet15_receive};
use crate::possess::layout::{
    ai_ins, ai_path_data, chr_action_request_module, chr_behavior_module, chr_ctrl,
    chr_ctrl_modifier, chr_data_module, chr_event_module, chr_ins, chr_physics_module,
    chr_time_act_module, com_think_owner, manipulator, modules, world_chr_man_dbg,
};
use crate::possess::netdamage;
use crate::settings::{TargetMode, TargetSettings};

/// One frame of the creature's TimeAct queue: what it is animating, and where in the clip.
///
/// Only ever built from an entry the engine pushed this frame -- see
/// [`Chr::current_anim_frame`], which answers `None` rather than handing back a stale one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct AnimFrame {
    /// `animQueue[readIdx].animId`.
    pub(crate) animation: i32,
    /// `localTime`, seconds into the clip. `None` when it did not read or was not finite.
    pub(crate) local_time: Option<f32>,
    /// `animLength`, how long the clip runs. `None` when it did not read, was not finite, or was
    /// not positive -- a zero length would make every clip look finished.
    pub(crate) anim_length: Option<f32>,
}

/// Compile-time cross-check of two independently derived layouts.
///
/// The left side is this crate's reverse engineering; the right is `fromsoftware-rs`'s model of
/// the same struct. They were derived separately, so an assertion failing here means one of them
/// moved and somebody has to go and find out which -- which is a build error, and exactly the
/// noise that is wanted. Only the crate's `pub` fields can be checked; the constants it spells
/// `unkNNN` (`ChrCtrl+0x100/+0x110/+0x3b0`, `CSChrPhysicsModule+0x150`) carry no assertion and are
/// marked RE-only in `layout`.
const _: () = {
    assert!(core::mem::offset_of!(CsChrIns, chr_ctrl) == chr_ins::CHR_CTRL);
    assert!(core::mem::offset_of!(CsChrIns, team_type) == chr_ins::TEAM_TYPE);
    assert!(
        core::mem::offset_of!(CsChrIns, lock_on_target_position) == chr_ins::LOCK_ON_TARGET_POS
    );
    assert!(core::mem::offset_of!(CsChrIns, modules) == chr_ins::MODULES);
    assert!(core::mem::offset_of!(CsChrIns, chr_flags1c5) == chr_ins::CHR_FLAGS_1C5);
    assert!(core::mem::offset_of!(CsChrIns, chr_flags1c8) == chr_ins::CHR_FLAGS_1C8);
    assert!(
        core::mem::offset_of!(CsChrIns, tint_alpha_multiplier) == chr_ins::TINT_ALPHA_MULTIPLIER
    );
    assert!(
        core::mem::offset_of!(CsChrIns, tint_alpha_multiplier_modifier)
            == chr_ins::TINT_ALPHA_MULTIPLIER_MODIFIER
    );
    // The sibling mirror models the 1.17 position of `debugFlags` since it was refreshed on
    // 2026-09-08 (upstream `b080915`, adopted at `16d2661`); before that it modelled 1.16.2 and
    // this line named the other constant. The engine still picks 1.16.2 or 1.17 at runtime, and
    // asserting the mirror against whichever build it models is what pins the pair -- so the day
    // this assertion needs flipping is the day the mirror moved, which is precisely when every
    // `offset_of!` answer in the crate changed and somebody has to look. Measured, not assumed:
    // 0x538 came out of a type error, the same way the `PlayerGameData` size did.
    assert!(core::mem::offset_of!(CsChrIns, debug_flags) == chr_ins::DEBUG_FLAGS_1170);
    assert!(core::mem::offset_of!(CsChrCtrl, owner) == chr_ctrl::OWNER);
    assert!(core::mem::offset_of!(CsChrCtrl, manipulator) == chr_ctrl::MANIPULATOR);
    assert!(core::mem::offset_of!(CsChrCtrl, modifier) == chr_ctrl::MODIFIER);
    assert!(core::mem::offset_of!(CsChrCtrl, chr_proxy_flags) == chr_ctrl::CHR_PROXY_FLAGS);
    assert!(core::mem::offset_of!(ChrInsModuleContainer, data) == modules::DATA);
    assert!(core::mem::offset_of!(ChrInsModuleContainer, physics) == modules::PHYSICS);
    assert!(core::mem::offset_of!(ChrInsModuleContainer, time_act) == modules::TIME_ACT);
    assert!(core::mem::offset_of!(ChrInsModuleContainer, behavior) == modules::BEHAVIOR);
    assert!(core::mem::offset_of!(ChrInsModuleContainer, event) == modules::EVENT);
    assert!(
        core::mem::offset_of!(ChrInsModuleContainer, action_request) == modules::ACTION_REQUEST
    );
    assert!(
        core::mem::offset_of!(CSChrActionRequestModule, tae_cancels)
            == chr_action_request_module::TAE_CANCELS
    );
    assert!(
        core::mem::offset_of!(CSChrEventModule, request_animation_id)
            == chr_event_module::REQUEST_ANIMATION_ID
    );
    assert!(
        core::mem::offset_of!(CSChrTimeActModule, anim_queue) == chr_time_act_module::ANIM_QUEUE
    );
    assert!(core::mem::offset_of!(CSChrTimeActModule, read_idx) == chr_time_act_module::READ_IDX);
    assert!(core::mem::offset_of!(CSChrTimeActModule, write_idx) == chr_time_act_module::WRITE_IDX);
    assert!(
        core::mem::offset_of!(CSChrTimeActModuleAnim, anim_length)
            == chr_time_act_module::ANIM_LENGTH
    );
    assert!(core::mem::size_of::<CSChrTimeActModuleAnim>() == chr_time_act_module::ANIM_STRIDE);
    // `localTime` is the third field, and upstream keeps it private under the name `play_time2`
    // -- so it cannot be reached by `offset_of!` and is pinned by arithmetic against the two
    // public neighbours instead. `anim_length` is at +0xC and the entry is 0x10 wide, so the
    // private field between `play_time` (+0x4) and `anim_length` sits at +0x8; if upstream ever
    // repacks the entry, this stops matching and says so at compile time rather than reading a
    // float out of the wrong quarter of the record.
    assert!(
        chr_time_act_module::ANIM_LENGTH == chr_time_act_module::ANIM_LOCAL_TIME + size_of::<f32>()
    );
    assert!(
        core::mem::offset_of!(CSChrBehaviorModule, root_motion) == chr_behavior_module::ROOT_MOTION
    );
    assert!(core::mem::offset_of!(CSChrDataModule, hp) == chr_data_module::HP);
    assert!(core::mem::offset_of!(CSChrPhysicsModule, position) == chr_physics_module::POSITION);
    assert!(
        core::mem::offset_of!(CSChrPhysicsModule, standing_on_solid_ground)
            == chr_physics_module::STANDING_ON_SOLID_GROUND
    );
    // Not `orientation_euler`, which upstream places at `+0x2d0` and which this crate read until
    // 2026-09-02. That name is a guess -- the named 1.16.2 dump types the same bytes as
    // `ChrPhysicsModuleInitData.initialOrientation`, and it reads `(0,0,0,0)` on a live character
    // -- so asserting one guessed name against another proved nothing. This pins the live
    // quaternion instead, whose offset is byte-proven on both builds.
    assert!(
        core::mem::offset_of!(CSChrPhysicsModule, interpolated_orientation)
            == chr_physics_module::Q_INTERPOLATED_ORIENTATION
    );
    assert!(
        core::mem::offset_of!(WorldChrManDbg, cam_override_chr_ins)
            == world_chr_man_dbg::CAM_OVERRIDE_CHR_INS
    );
};

/// `CS::PlayAnimationByBehaviorName`, 1.16.2 RVA `0xc14370`.
///
/// The only game function address this crate resolves. Layers 1 and 2 resolve none at all -- every
/// lever they need turned out to be a field -- and layer 3 keeps that for everything the
/// `W_Event%04d` field write can spell -- 4,667 of the 6,921 shipped moves. This exists for the
/// remainder, which is almost entirely the 2,252 dodges: they are spelled `W_Step` and have no
/// `W_Event` name at all, so without this they are simply not in the game.
///
/// Verified for 1.17 rather than assumed. The function is 80 bytes and byte-identical between
/// 1.16.2 `0x140c14370` and 1.17 `0x140c15a40`, matching uniquely in both images with no rel32
/// wildcarding needed -- both of its callees moved by the same `+0x16d0` as the function itself.
/// The pair is registered in `docs/recon/rva-map-1162-to-1170.verified.tsv`, which is what makes
/// [`game_rva_named`] able to translate it; without that row it refuses and the fallback goes
/// quiet rather than jumping into whatever now occupies those bytes.
const PLAY_ANIMATION_BY_BEHAVIOR_NAME_RVA: u32 = 0x00c1_4370;

/// `void PlayAnimationByBehaviorName(hkbCharacter** slot, const wchar_t* name)`.
///
/// The first argument is a pointer to the slot, not the character -- the function opens with
/// `CMP qword ptr [RCX],0x0` and later `MOV RCX,[RBX]`. Ghidra types it `hkbCharacter*`, which is
/// wrong and would have been a null-deref on the first call.
type PlayAnimationByBehaviorNameFn = unsafe extern "system" fn(*mut usize, *const u16);

/// A `FloatVector4` in the engine's layout, `align(16)` because `ForceSetPosition` loads its
/// argument with `MOVAPS`, which `#GP`s on an unaligned address.
///
/// Shared with `er-invasion-warp-core`, which carries the alignment test and discovered the
/// requirement; the dependency exists for this one type rather than duplicating a `repr` whose
/// whole point is a property nobody would notice was missing until the game died.
pub(crate) use er_invasion_warp_core::warp::FloatVector4;

/// Read all four floats of a `FloatVector4`.
///
/// `w` is not decoration here. `[vt+0x50]` computes it as `dot(direction, modelMatrix.row4)` --
/// row4 being the translation row -- so on a body at `(5.04, 5.06, 0.44)` moving along `-X` it is
/// `-5.04`, a number that changes as the creature walks. That looks exactly like a bug and is not:
/// it is what the engine itself writes, and matching it is the point. It was invisible until now
/// because the telemetry printed three components.
fn read_vec4(at: usize) -> Option<[f32; 4]> {
    let [x, y, z] = read_vec3(at)?;
    let w = unsafe { safe_read_f32(at + 12) }?;
    w.is_finite().then_some([x, y, z, w])
}

/// Read three floats as a position. `None` when the address will not read.
fn read_vec3(at: usize) -> Option<[f32; 3]> {
    let x = unsafe { safe_read_f32(at) }?;
    let y = unsafe { safe_read_f32(at + 4) }?;
    let z = unsafe { safe_read_f32(at + 8) }?;
    (x.is_finite() && y.is_finite() && z.is_finite()).then_some([x, y, z])
}

/// Write a `u32`, but only after proving the address reads.
fn write_u32(at: usize, value: u32) -> bool {
    if unsafe { safe_read_i32(at) }.is_none() {
        return false;
    }
    unsafe { (at as *mut u32).write(value) };
    true
}

/// Write a signed 32-bit field, but only after proving the address reads.
///
/// Separate from [`write_u32`] because `AiIns.turnTarget` is an `AiTargetPointType`, whose
/// `TARGET_SELF` is `-1`. Spelling that as a `u32` at every call site is how a sign gets lost.
fn write_i32(at: usize, value: i32) -> bool {
    if unsafe { safe_read_i32(at) }.is_none() {
        return false;
    }
    unsafe { (at as *mut i32).write(value) };
    true
}

/// Write a byte, but only after proving the address reads.
fn write_u8(at: usize, value: u8) -> bool {
    if unsafe { safe_read_u8(at) }.is_none() {
        return false;
    }
    unsafe { (at as *mut u8).write(value) };
    true
}

/// Read-modify-write one flag byte.
fn set_bits_u8(at: usize, set: u8, clear: u8) -> bool {
    let Some(current) = (unsafe { safe_read_u8(at) }) else {
        return false;
    };
    write_u8(at, (current | set) & !clear)
}

/// Write an `f32`, but only after proving the address reads.
fn write_f32(at: usize, value: f32) -> bool {
    if unsafe { safe_read_f32(at) }.is_none() {
        return false;
    }
    unsafe { (at as *mut f32).write(value) };
    true
}

/// Write a 16-byte vector, after proving both halves read.
fn write_vec4(at: usize, value: FloatVector4) -> bool {
    if unsafe { safe_read_usize(at) }.is_none() || unsafe { safe_read_usize(at + 8) }.is_none() {
        return false;
    }
    // The address is a field of a 16-aligned game struct at a 16-aligned offset, so this is a
    // sound aligned store; `FloatVector4` is `repr(C, align(16))` for the same reason.
    unsafe { (at as *mut FloatVector4).write(value) };
    true
}

/// One live `ChrIns`, addressed by number rather than by reference.
///
/// A `usize` and not a `&ChrIns`: the possessed creature can despawn between frames, and a Rust
/// reference to it would be a claim of validity this code cannot make. Every accessor re-walks the
/// pointer chain through `safe_read_*` and answers `None` when a link has gone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Chr(usize);

impl Chr {
    pub(crate) const fn new(address: usize) -> Self {
        Self(address)
    }

    pub(crate) const fn address(self) -> usize {
        self.0
    }

    /// The `ChrCtrl`, verified to point back at this `ChrIns`.
    ///
    /// The back-check is not ceremony: `ChrCtrl+0x3b0` is a write into a structure this code found
    /// by following two pointers, and `owner` is a free proof that the second one landed where it
    /// was supposed to.
    pub(crate) fn chr_ctrl(self) -> Option<usize> {
        let ctrl = unsafe { safe_read_usize(self.0 + chr_ins::CHR_CTRL) }?;
        if !unsafe { is_heap_aligned_ptr(ctrl) } {
            return None;
        }
        let owner = unsafe { safe_read_usize(ctrl + chr_ctrl::OWNER) }?;
        (owner == self.0).then_some(ctrl)
    }

    fn module(self, slot: usize) -> Option<usize> {
        let container = unsafe { safe_read_usize(self.0 + chr_ins::MODULES) }?;
        if !unsafe { is_heap_aligned_ptr(container) } {
            return None;
        }
        let module = unsafe { safe_read_usize(container + slot) }?;
        unsafe { is_heap_aligned_ptr(module) }.then_some(module)
    }

    fn physics(self) -> Option<usize> {
        self.module(modules::PHYSICS)
    }

    /// Physics-space position. The same field `ForceSetPosition` writes, so reading one
    /// character's and writing another's needs no conversion at all.
    pub(crate) fn position(self) -> Option<[f32; 3]> {
        read_vec3(self.physics()? + chr_physics_module::POSITION)
    }

    /// Heading in radians, derived from the body's live orientation quaternion.
    ///
    /// The quaternion, not a euler field: the euler field this used to read
    /// (`+0x2d0`) is `ChrPhysicsModuleInitData.initialOrientation`, a spawn-time constant that
    /// reads all-zero on a live character, so this answered `0.0` for every creature and every
    /// player. `+0x60` is what `CS::ChrCtrl::GetPhysicsOrientation` -- the body of `GetForward` --
    /// reads, and the offset is byte-proven on 1.16.2 and 1.17 alike; see
    /// [`chr_physics_module::Q_INTERPOLATED_ORIENTATION`].
    ///
    /// `None` when the sixteen bytes are not a unit quaternion, which doubles as a liveness check
    /// on the physics module.
    pub(crate) fn yaw(self) -> Option<f32> {
        let at = self.physics()? + chr_physics_module::Q_INTERPOLATED_ORIENTATION;
        let x = unsafe { safe_read_f32(at) }?;
        let y = unsafe { safe_read_f32(at + 4) }?;
        let z = unsafe { safe_read_f32(at + 8) }?;
        let w = unsafe { safe_read_f32(at + 12) }?;
        crate::possess::intent::yaw_of_quaternion([x, y, z, w])
    }

    /// The engine-maintained last position this character was standing on.
    pub(crate) fn last_grounded_position(self) -> Option<[f32; 3]> {
        read_vec3(self.physics()? + chr_physics_module::LAST_GROUNDED_POSITION)
    }

    /// Is it on solid ground right now? The field `ChrIns::IsStandingOnSolidGround` reads.
    pub(crate) fn standing_on_solid_ground(self) -> Option<bool> {
        let at = self.physics()? + chr_physics_module::STANDING_ON_SOLID_GROUND;
        unsafe { safe_read_u8(at) }.map(|byte| byte != 0)
    }

    /// Current HP, from `CSChrDataModule+0x138`.
    pub(crate) fn hp(self) -> Option<i32> {
        let data = self.module(modules::DATA)?;
        unsafe { safe_read_i32(data + chr_data_module::HP) }
    }

    /// Fire an animation, which for this crate means writing one `int`.
    ///
    /// `CSChrEventModule::RequestAnimation`'s entire lasting effect is this store; see
    /// [`chr_event_module`] for why the other half of it does not need doing. The engine picks the
    /// request up in its next `CSChrEventModule::Update`, turns the id into the behaviour-graph
    /// event `W_Event%04d`, and resets the field to -1.
    ///
    /// Returns whether the store landed -- Not whether the animation played. Nothing in the
    /// process can answer the second question: an id the graph does not consume resolves cleanly
    /// and does nothing. That is what the offline fireability gate and the watchdog are between
    /// them for.
    pub(crate) fn request_animation(self, animation: i32) -> bool {
        let Some(event) = self.module(modules::EVENT) else {
            return false;
        };
        write_u32(
            event + chr_event_module::REQUEST_ANIMATION_ID,
            animation as u32,
        )
    }

    /// Is a request still waiting to be consumed?
    ///
    /// Used to avoid stacking two requests in one frame: the field holds one id, so a second
    /// write before `Update` runs silently discards the first.
    pub(crate) fn animation_request_pending(self) -> bool {
        let Some(event) = self.module(modules::EVENT) else {
            return false;
        };
        unsafe { safe_read_i32(event + chr_event_module::REQUEST_ANIMATION_ID) }
            .is_some_and(|pending| pending != -1)
    }

    /// What the creature is animating this frame, or `None` when it is animating nothing.
    ///
    /// `None` is a real answer and not a failure. `CS::ChrIns::PreBehaviorSafe` calls
    /// `ResetAnimQueque` (`readIdx = writeIdx`) once per frame and `RunTaeAndUpdateAnimQueue`
    /// pushes one entry per animation actually driven, advancing `writeIdx`; so an equal pair
    /// means nothing was driven and the entry sitting at `readIdx` belongs to an earlier frame.
    ///
    /// Reading it anyway is the bug this function was rewritten to remove. In the 2026-09-02 run
    /// a possessed c4604 kept answering "3000" long after that swing had ended, so every press for
    /// five and a half minutes was buffered as mid-attack and dropped. The stale value does not
    /// fail open; it fails permanently closed, which is indistinguishable from the mod not
    /// working.
    pub(crate) fn current_anim_frame(self) -> Option<AnimFrame> {
        let time_act = self.module(modules::TIME_ACT)?;
        let read = unsafe { safe_read_i32(time_act + chr_time_act_module::READ_IDX) }?;
        let write = unsafe { safe_read_i32(time_act + chr_time_act_module::WRITE_IDX) }?;
        if read == write {
            // Nothing was driven this frame. Not an error, and not "still playing whatever was
            // there before" -- the creature is animating nothing.
            return None;
        }
        let index = u32::try_from(read).ok()?;
        if index >= chr_time_act_module::ANIM_QUEUE_LEN {
            // The cursor is a `u32` the engine wraps itself, so an out-of-range value means the
            // pointer chain landed somewhere wrong. Reading past the ring would be reading
            // whatever follows it.
            return None;
        }
        let entry = time_act
            + chr_time_act_module::ANIM_QUEUE
            + (index as usize) * chr_time_act_module::ANIM_STRIDE;
        Some(AnimFrame {
            animation: unsafe { safe_read_i32(entry) }?,
            local_time: unsafe { safe_read_f32(entry + chr_time_act_module::ANIM_LOCAL_TIME) }
                .filter(|value| value.is_finite()),
            anim_length: unsafe { safe_read_f32(entry + chr_time_act_module::ANIM_LENGTH) }
                .filter(|value| value.is_finite() && *value > 0.0),
        })
    }

    /// Did the animation request this crate wrote actually get dispatched?
    ///
    /// The engine's own answer rather than "we wrote the field last frame".
    /// `CS::CSChrEventModule::Update` clears bit 0 of `+0x24` at entry
    /// (`and dword [rcx+0x24], 0xfffffffe`, 1.16.2 `0x14043a597`) and sets it only on the branch
    /// that actually reaches the behaviour world (`or dword [rdi+0x24], 1`, unique in
    /// `eldenring-deobf.bin` at `0x14043a688` and in `eldenring-deobf-1.17.bin` at `0x14043abe8`).
    /// So a set bit means the request became a graph event, and a clear bit after a write means
    /// one of `Update`'s four gates dropped it silently -- which nothing in this crate could see
    /// before.
    pub(crate) fn animation_request_dispatched(self) -> bool {
        let Some(event) = self.module(modules::EVENT) else {
            return false;
        };
        unsafe { safe_read_i32(event + chr_event_module::DISPATCH_FLAGS) }
            .is_some_and(|flags| flags as u32 & chr_event_module::DISPATCHED != 0)
    }

    /// May the animation being played be cancelled into another attack right now?
    ///
    /// The engine's own answer, not this crate's: the two tests are exactly the ones
    /// `CS::CSAiFunc::IsEnableCancelAttack` applies before letting a creature's AI chain out of a
    /// swing. `None` means the module chain did not read, which is a different answer from "no"
    /// and the caller treats it as such -- see [`crate::moveset::chain`].
    ///
    /// One frame late, and that is fine. The possession ticks in `CSTaskGroupIndex::FrameBegin`,
    /// while `PreBehaviorSafe` clears the transient bits and the TimeAct events re-set them
    /// during the behaviour update -- both later in the same frame. So this reads the window the
    /// previous frame established, 16 ms of latency on a window whose median width is 800 ms.
    pub(crate) fn attack_cancel_allowed(self) -> Option<bool> {
        let module = self.module(modules::ACTION_REQUEST)?;
        // `safe_read_u32` is not in `er_game_base::mem`; the field is a `u32` and the cast is
        // the whole difference, so the bits are the same bits either way.
        let flags =
            unsafe { safe_read_i32(module + chr_action_request_module::TAE_CANCELS) }? as u32;
        Some(
            flags & chr_action_request_module::CANCEL_ATTACK != 0
                && flags & chr_action_request_module::CANCEL_DISABLE == 0,
        )
    }

    /// The creature's `hkbCharacter`, by two loads.
    ///
    /// `CSChrEventModule::Update` gets this by calling `FUN_14041aef0(behaviorModule, &slot)`,
    /// which calls `FUN_140c07a40(behaviorModule->field_0x10, &slot)`, whose whole body is
    /// `slot = *(field_0x10 + 0x30)` behind a null check. Both calls are pure loads, so this does
    /// the loads and resolves neither address -- the moveset layer ends up needing exactly one
    /// game function address, and this is not it.
    fn hkb_character(self) -> Option<usize> {
        let behavior = self.module(modules::BEHAVIOR)?;
        let owner = unsafe { safe_read_usize(behavior + chr_behavior_module::HKB_OWNER) }?;
        if !unsafe { is_heap_aligned_ptr(owner) } {
            return None;
        }
        let character = unsafe { safe_read_usize(owner + chr_behavior_module::HKB_CHARACTER) }?;
        unsafe { is_heap_aligned_ptr(character) }.then_some(character)
    }

    /// Fire an animation by name -- the fallback for ids the field write cannot spell.
    ///
    /// `requestAnimationId` formats `W_Event%04d` and nothing else, and `W_Event` is a broad alias
    /// layer rather than a total one: c2120's dodges exist only as `W_Step6000`..`W_Step6011` and
    /// have no `W_Event` spelling at all. This builds the name the graph actually declares --
    /// `<prefix><id>` at `%04d` -- and hands it to `PlayAnimationByBehaviorName`, which resolves it
    /// through the same behaviour world and reaches the same `fireHkbEvent_C`.
    ///
    /// **this is the one game function address the whole crate resolves**, and it is on this path
    /// only. Everything the `W_Event` spelling can reach goes through
    /// [`Self::request_animation`], which is a field write. Refusing costs the dodges; calling an
    /// unresolved address costs the session, so a refusal returns `false` and the caller logs it.
    pub(crate) fn play_animation_by_name(self, name: &[u16]) -> bool {
        debug_assert_eq!(
            name.last(),
            Some(&0),
            "the game expects a NUL-terminated wide string"
        );
        let Some(mut character) = self.hkb_character() else {
            return false;
        };
        let Ok(address) = game_rva_named(
            PLAY_ANIMATION_BY_BEHAVIOR_NAME_RVA,
            "PLAY_ANIMATION_BY_BEHAVIOR_NAME_RVA",
        ) else {
            return false;
        };
        // Safety: the address was resolved for the running build immediately above, and the
        // signature is read out of the function's own disassembly rather than inferred --
        // `CMP qword ptr [RCX],0x0` proves the first argument is a pointer to the slot holding the
        // `hkbCharacter`, not the character itself, which is why `character` is passed by address.
        let play: PlayAnimationByBehaviorNameFn = unsafe { core::mem::transmute(address) };
        unsafe { play(&raw mut character, name.as_ptr()) };
        true
    }

    /// The creature's chr id -- `4500` for a Flying Dragon -- from its `NpcParam` row.
    ///
    /// `NpcParam` rows are `<chr><4 digits>`, so the id is the row divided by ten thousand. That
    /// is the same arithmetic `scripts/er-moveset-table-gen.py` uses to key the shipped table, and
    /// it is the reason the table can be keyed by an integer at all.
    ///
    /// The offset comes from `offset_of!` rather than from [`crate::possess::layout`], unlike
    /// every other field here. The layout module exists for offsets the `eldenring` crate does
    /// not model, where a hand-derived constant is the only option and a cross-check is the best
    /// available guard. This field is modelled, so taking the offset from the type is strictly
    /// better than writing the number down twice and asserting they match.
    pub(crate) fn npc_param_id(self) -> Option<u32> {
        let at = self.0 + core::mem::offset_of!(CsChrIns, npc_param_id);
        let row = unsafe { safe_read_i32(at) }?;
        u32::try_from(row).ok().map(|row| row / 10_000)
    }

    /// `CSChrBehaviorModule+0x30 rootMotion`, squared magnitude -- telemetry only.
    ///
    /// Restored after being deleted as dead code. It was dead for one merge: the chaining layer
    /// stopped using it when the watchdog's liveness test moved from "did the BODY move" to "did
    /// the animation advance" -- which was the right change, because with locomotion broken every
    /// attack translated nothing and the old test denied the whole moveset one clip at a time.
    ///
    /// The locomotion layer then needed the same field for a different question, and it is the
    /// discriminator that separates the three surviving movement hypotheses. Read beside `staged`
    /// and `published`: staged-but-not-published means `[vt+0x50]` never ran; published with zero
    /// root motion means the vector never became a locomotion clip; published with root motion
    /// while the position does not change means a clip is playing and something is holding the
    /// body, which is the co-located player capsule the user suspected.
    ///
    /// **that last case is live, and it was wrongly closed once.** An earlier note eliminated the
    /// capsule on the grounds that this field read zero -- so no clip was playing, so nothing
    /// could be holding anything. A later run read `0.0031356404` here with the creature's
    /// position identical to five decimals across thirteen seconds. One zero sample is not the
    /// property "always zero", and treating it as one closed the user's own hypothesis on
    /// evidence that did not support it.
    ///
    /// What still separates the two readings is which CLIP: |rootMotion| of `0.056` per frame is
    /// walking pace at 60 fps, but an idle sway or a turn-in-place also translates a little. That
    /// is why `anim` now prints on the same line -- a locomotion id there makes the body held,
    /// an idle id makes it merely swaying, and neither needs another argument to settle.
    ///
    /// Squared, because nothing here needs the magnitude itself and a square root would be a
    /// wasted instruction sixty times a second in service of a log line.
    pub(crate) fn root_motion_squared(self) -> Option<f32> {
        let behavior = self.module(modules::BEHAVIOR)?;
        let [x, y, z] = read_vec3(behavior + chr_behavior_module::ROOT_MOTION)?;
        Some(z.mul_add(z, x.mul_add(x, y * y)))
    }

    /// `CS::ChrIns::IsDead` -- `chrCtrl->ctrlModifier->ChrCtrlModifierData._1cFlags & 1`.
    pub(crate) fn is_dead_flag(self) -> Option<bool> {
        let ctrl = self.chr_ctrl()?;
        let modifier = unsafe { safe_read_usize(ctrl + chr_ctrl::MODIFIER) }?;
        if !unsafe { is_heap_aligned_ptr(modifier) } {
            return None;
        }
        let at = modifier + chr_ctrl_modifier::DATA + chr_ctrl_modifier::FLAGS_1C;
        unsafe { safe_read_i32(at) }
            .map(|flags| (flags as u32) & chr_ctrl_modifier::FLAG_IS_DEAD != 0)
    }

    /// Dead by either signal. HP is the direct one and is checked first; the flag is the engine's
    /// own predicate and catches a death that has not yet zeroed the counter.
    pub(crate) fn is_dead(self) -> bool {
        if self.hp().is_some_and(|hp| hp <= 0) {
            return true;
        }
        self.is_dead_flag().unwrap_or(false)
    }

    /// Does the whole pointer chain the engine needs still resolve?
    pub(crate) fn is_live(self) -> bool {
        self.chr_ctrl().is_some() && self.position().is_some()
    }

    /// The creature's real `ComManipulator`, read from `ChrCtrl+0x18` -- never from `+0x3b0`,
    /// which is where our thunk goes.
    pub(crate) fn real_manipulator(self) -> Option<usize> {
        let ctrl = self.chr_ctrl()?;
        let manip = unsafe { safe_read_usize(ctrl + chr_ctrl::MANIPULATOR) }?;
        unsafe { is_heap_aligned_ptr(manip) }.then_some(manip)
    }

    /// Point the creature's real `ComManipulator` at our patched vtable.
    ///
    /// # Why this replaced the `ChrCtrl+0x3b0` override
    ///
    /// The override worked -- `[vt+0x48]` really was no-oped through it -- but it created a second
    /// object, and the engine does not only dispatch through that slot. Fourteen sites resolve
    /// `chrManipulator ?? manipulator` and then use the answer as an object. The publish and the
    /// consume are a matched pair, and they end up on opposite sides of that split:
    ///
    /// ```text
    /// publish:  FUN_1403cdc20(manip, vec)   writes ChrManipulator +0x10 AND +0x70
    /// consume:  FUN_1403cd770(manip, out)   MOVUPS XMM0,[RCX+0x10] ; MOVAPS [RDX],XMM0 ; RET
    /// caller:   FUN_1403cbff0(ChrCtrl*)     resolves chrManipulator ?? manipulator,
    ///                                       then calls FUN_1403cd770 on the result
    /// ```
    ///
    /// The old forwarding stub swapped `rcx` to the real manipulator, so `[vt+0x50]` published
    /// into that object while `FUN_1403cbff0` read `+0x10` off the override's zeroes.
    /// `FUN_1403cd4c0`, called on the same pointer two lines later, reads `+0x20`..`+0x50` and is
    /// starved the same way.
    ///
    /// **A correction to an earlier version of this note**, kept because the mistake is repeatable:
    /// it cited `FUN_1403cbff0` as reading `+0x70` at `0x1403cc0b9`. That load is `ChrCtrl+0x70` --
    /// `FUN_1403cbff0` takes a `ChrCtrl*`, not a manipulator. The citation came from a displacement
    /// scan, which is exactly the inference `find-deobf-field-access.py` warns about in its own
    /// help text: a displacement is not a struct. The mechanism survived the correction; the
    /// evidence for it had to be re-derived through the getter above.
    ///
    /// One object cannot diverge from itself. `ChrCtrl+0x3b0` is now left NULL -- which also
    /// retires the `ChrCtrl::Unref` DLPanic hazard the old design had to tiptoe around.
    ///
    /// # What this refuses
    ///
    /// The manipulator must still be the one this creature owns, and must still hold the exact
    /// vptr the copy was taken from. Anything else means either a second copy of this mod or an
    /// assumption that has stopped holding, and both are worse to overwrite than to decline.
    pub(crate) fn install_manipulator_vtable(
        self,
        real_com: usize,
        original: usize,
        patched: usize,
    ) -> bool {
        if self.real_manipulator() != Some(real_com) {
            return false;
        }
        // `ChrCtrl+0x3b0` must be empty. This design deliberately leaves it null, so a non-null
        // slot means something else owns this creature -- an older build of this mod, or another
        // mod using the override the same way this one used to. Swizzling underneath that would
        // give the creature two owners and one of them a stale vtable.
        let Some(ctrl) = self.chr_ctrl() else {
            return false;
        };
        if unsafe { safe_read_usize(ctrl + chr_ctrl::MANIPULATOR_OVERRIDE) } != Some(0) {
            return false;
        }
        if unsafe { safe_read_usize(real_com) } != Some(original) {
            return false;
        }
        // A pointer-sized store into a live game object, at an address just proven readable and
        // proven to hold the value we expect. The engine reads it on its next virtual dispatch.
        unsafe { (real_com as *mut usize).write(patched) };
        true
    }

    /// Put the creature's original vtable pointer back. The step that must happen.
    ///
    /// Our patched table lives in a page we free, so a creature left pointing at it dispatches
    /// through unmapped memory on its next tick. [`crate::possess::teardown`] orders this before
    /// the `Thunk` is dropped for exactly that reason.
    ///
    /// An associated function, addressed by the recorded `real_com` rather than through the
    /// `ChrIns`: the creature can be freed while possessed -- 23 release/retake cycles happened in
    /// one session -- and a pointer chain that no longer resolves must not stop us putting the
    /// vptr back. Four outcomes, and only one is a failure:
    ///
    /// * the address will not read -- the object is gone, nothing to undo, success;
    /// * it holds our table -- restore it, success;
    /// * it holds the original already -- somebody beat us to it, success;
    /// * it holds anything else -- another owner has it, and stomping that is worse than leaving
    ///   it alone.
    pub(crate) fn restore_manipulator_vtable(
        real_com: usize,
        original: usize,
        patched: usize,
    ) -> bool {
        match unsafe { safe_read_usize(real_com) } {
            None => true,
            Some(current) if current == patched => {
                unsafe { (real_com as *mut usize).write(original) };
                true
            }
            Some(current) => current == original,
        }
    }

    /// Co-locate: ask the engine to move this character to `position` facing `yaw`, on its own
    /// next `updatePos`.
    ///
    /// **Nothing is called.** This fills the same request buffer the engine already drains every
    /// frame for network ghosts -- `ChrCtrl::UpdatePositions` reads `chrProxyFlags`, calls
    /// `ForceSetPosition` and `SetOrientation`, syncs the havok proxies, zeroes the accumulated
    /// delta matrix and clears the flags. Two consequences worth naming:
    ///
    /// * **No fall damage.** `ForceSetPosition` snaps `prevUpdatePosition` as well as `position`,
    ///   so the per-frame fall delta is zero rather than the distance we just moved.
    /// * **Other players see it.** The same drain pushes the result into
    ///   `WorldChrManImp::GetNetChrSyncPositionUpdateBuffer`, so an UNMODDED client sees the
    ///   possessing player genuinely standing at the creature.
    ///
    /// `yaw` is EULER RADIANS in `.y`, not a quaternion: the drain feeds `SetOrientation`, which
    /// feeds `EulerToQuat`.
    pub(crate) fn request_move(self, position: [f32; 3], yaw: Option<f32>) -> bool {
        let Some(ctrl) = self.chr_ctrl() else {
            return false;
        };
        if !position.iter().all(|v| v.is_finite()) {
            return false;
        }
        let mut flags = chr_ctrl::CHR_PROXY_FLAG_POSITION;
        if !write_vec4(
            ctrl + chr_ctrl::RAGDOLL_POSITION,
            FloatVector4::new(position[0], position[1], position[2], 1.0),
        ) {
            return false;
        }
        if let Some(yaw) = yaw.filter(|v| v.is_finite())
            && write_vec4(
                ctrl + chr_ctrl::RAGDOLL_ROTATION,
                FloatVector4::new(0.0, yaw, 0.0, 0.0),
            )
        {
            flags |= chr_ctrl::CHR_PROXY_FLAG_ROTATION;
        }
        let at = ctrl + chr_ctrl::CHR_PROXY_FLAGS;
        let existing = unsafe { safe_read_i32(at) }.unwrap_or(0) as u32;
        write_u32(at, existing | flags)
    }

    /// `ChrIns+0x6c teamType`, or `None` when the byte does not read.
    pub(crate) fn team_type(self) -> Option<u8> {
        // Safety: as everywhere else here -- the read goes through `ReadProcessMemory`, so an
        // unmapped address answers `None` instead of faulting.
        unsafe { safe_read_u8(self.0 + chr_ins::TEAM_TYPE) }
    }

    /// Put this character on `team`, which is what decides who its lock-on can and cannot see.
    ///
    /// One byte, and `CS::ChrIns::GetTeamType` reads it directly -- but with two overrides above
    /// it, and one of them can make this write invisible. A `SpecialEffect` carrying state info
    /// `0x84` forces [`chr_ins::TEAM_TYPE_CHARMED`] anyway, which agrees with what possession
    /// writes; a `ChrSlotSys` `TemporaryTeamType` slot, which is network state, wins outright. On
    /// an ordinary offline enemy there is no such slot and this byte is the answer.
    pub(crate) fn set_team_type(self, team: u8) -> bool {
        write_u8(self.0 + chr_ins::TEAM_TYPE, team)
    }

    /// `ChrIns+0xd0 lockOnTargetPos` -- where the lock-on system last put this character's lock
    /// point, in physics space.
    ///
    /// Meaningful on the possessed creature because `camOverrideChrIns` has made it the lock-on
    /// subject, so `FUN_140716260` writes this field on it every frame a lock is held. **Nothing
    /// clears it when the lock is dropped**, so the value alone is not evidence that a lock exists;
    /// [`crate::possess::intent::aim`] is where that is dealt with.
    pub(crate) fn lock_on_target_pos(self) -> Option<[f32; 3]> {
        read_vec3(self.0 + chr_ins::LOCK_ON_TARGET_POS)
    }

    /// Invincibility, exactly as `CS::ChrIns::EnableInvincible` writes it.
    pub(crate) fn set_invincible(self, on: bool) -> bool {
        let at = self.0 + chr_ins::CHR_FLAGS_1C5;
        if on {
            set_bits_u8(at, chr_ins::INVINCIBLE, 0)
        } else {
            set_bits_u8(at, 0, chr_ins::INVINCIBLE)
        }
    }

    /// Set the tint alpha and zero its modifier.
    ///
    /// Both, because the modifier is a per-frame DECAY: `SetFadeInOut` writes `-alpha/duration`
    /// into it, which is why calling that function fades rather than holds. Writing the alpha
    /// alone would work for one frame and then drift.
    pub(crate) fn set_alpha(self, alpha: f32) -> bool {
        let held = write_f32(self.0 + chr_ins::TINT_ALPHA_MULTIPLIER_MODIFIER, 0.0);
        write_f32(self.0 + chr_ins::TINT_ALPHA_MULTIPLIER, alpha) && held
    }

    /// Silence this character, and optionally stop what it is already playing.
    ///
    /// `stop_playing` is a one-shot the engine consumes in `PostPhysicsSafe`, so it is passed
    /// `true` once at possession start and `false` on every per-frame re-assert.
    pub(crate) fn set_muted(self, muted: bool, stop_playing: bool) -> bool {
        let at = self.0 + chr_ins::CHR_FLAGS_1C8;
        if muted {
            let set = if stop_playing {
                chr_ins::MUTE_SOUND | chr_ins::STOP_PLAYING_SOUNDS
            } else {
                chr_ins::MUTE_SOUND
            };
            set_bits_u8(at, set, 0)
        } else {
            set_bits_u8(at, 0, chr_ins::MUTE_SOUND)
        }
    }

    /// Set or clear the no-attack debug flag, and always clear no-move.
    ///
    /// `offset` comes from `layout::debug_flags_offset`, which answers `None` on a build nobody
    /// measured -- so an unrecognised build never reaches this function and never writes eight
    /// bytes off target into a live field.
    ///
    /// No-move is cleared unconditionally rather than left alone: the body is driven by
    /// [`Self::request_move`], and a zeroed move vector would fight that write every frame.
    pub(crate) fn set_no_attack(self, offset: usize, on: bool) -> bool {
        let at = self.0 + offset;
        let Some(current) = (unsafe { safe_read_i32(at) }) else {
            return false;
        };
        let mut flags = current as u32 & !chr_ins::DEBUG_FLAG_NO_MOVE;
        if on {
            flags |= chr_ins::DEBUG_FLAG_NO_ATTACK;
        } else {
            flags &= !chr_ins::DEBUG_FLAG_NO_ATTACK;
        }
        write_u32(at, flags)
    }

    /// The creature's `AiIns`, proven to be the live AI object of this body.
    ///
    /// **the layout canary, and it is an identity proof rather than a plausibility screen.** The
    /// version this replaced checked only that `pathData` was a heap-aligned pointer to three
    /// finite floats. That passed on every frame of a possession in which nothing moved, and it
    /// was cited as evidence the offsets were right -- a canary that cannot fail is worse than no
    /// canary, because it launders a guess into a fact.
    ///
    /// What makes an exact check possible is that `ComManipulator` **embeds** its
    /// `CSComThinkOwner` at `+0xc8` instead of pointing at one. So the pointer `AiIns+0x0` holds
    /// is not merely "a live object": it must equal `manipulator + 0xc8` to the byte. Walking on
    /// from there, `CSComThinkOwner+0x10` must be the `ChrCtrl` we started from, and
    /// `ChrManipulator+0xa8` must be the `ChrIns` we started from. Four addresses, three exact
    /// equalities, one closed loop:
    ///
    /// ```text
    /// ChrIns --+0x58--> ChrCtrl --+0x18--> ComManipulator --+0xc0--> AiIns
    ///    ^                  ^                     |  ^                  |
    ///    +------------------|---------------------+  |    +0x0 (comThinkOwner)
    ///     ChrManipulator+0xa8|                       |                  |
    ///                        +-- CSComThinkOwner+0x10 +--- == manip+0xc8 <-+
    /// ```
    ///
    /// The manipulator is the real one, from `ChrCtrl+0x18`, never the thunk this crate installs
    /// at `+0x3b0`: `ChrIns::GetAiInsFromManipulator` and `EnemyIns::GetChrManipulator` do not
    /// consult the override, so the AI side of the engine still sees the creature's own
    /// `ComManipulator` -- which is exactly what lets us write intent into fields the forwarded
    /// `[vt+0x50]` then consumes. The thunk would fail leg three anyway, since it is a byte copy
    /// whose embedded `comManipOwner` still names the object it was copied from.
    ///
    /// Every offset in that loop comes from the named 1.16.2 dump, and a build that moved any of
    /// them cannot close it by accident -- which is exactly the failure mode
    /// [`Self::write_move_intent`] must not have, because `AiIns` is `0xf0d0` bytes and a stray
    /// write inside it faults on nothing and reports success.
    ///
    /// The `walkType` range check is the one soft conjunct: `CSAiFunc::MoveTo` writes `2 - walk`
    /// and `ClearMoveRequest` writes `0`, so a live field holds `0..=2` and sixteen thousand other
    /// values say the offset is not the field.
    fn validated_ai_ins(self) -> Option<usize> {
        let ctrl = self.chr_ctrl()?;
        let manip = self.real_manipulator()?;
        // Leg one: the manipulator we reached from this ChrIns names it back.
        if unsafe { safe_read_usize(manip + manipulator::OWNING_CHR) }? != self.0 {
            return None;
        }
        let ai = unsafe { safe_read_usize(manip + manipulator::AI_INS) }?;
        if !unsafe { is_heap_aligned_ptr(ai) } {
            return None;
        }
        // Leg two: the AI object's owner is the manipulator's own inline member, exactly.
        let owner = unsafe { safe_read_usize(ai + ai_ins::COM_THINK_OWNER) }?;
        if owner != manip + manipulator::COM_THINK_OWNER {
            return None;
        }
        // Leg three: the member's own back-pointer names the manipulator it is embedded in...
        if unsafe { safe_read_usize(owner + com_think_owner::COM_MANIP_OWNER) }? != manip {
            return None;
        }
        // ...and leg four, it points back at this body.
        if unsafe { safe_read_usize(owner + com_think_owner::CHR_CTRL) }? != ctrl {
            return None;
        }
        // ...and the field the gate is written into currently holds a value the engine writes.
        let walk = unsafe { safe_read_i32(ai + ai_ins::WALK_TYPE) }?;
        if !(ai_ins::WALK_TYPE_STOP..=ai_ins::WALK_TYPE_RUN).contains(&walk) {
            return None;
        }
        Some(ai)
    }

    /// The address of `pathData->target`, or `None` when the chain does not check out.
    pub(crate) fn ai_path_target(self) -> Option<usize> {
        let path_data = unsafe { safe_read_usize(self.validated_ai_ins()? + ai_ins::PATH_DATA) }?;
        if !unsafe { is_heap_aligned_ptr(path_data) } {
            return None;
        }
        let target = path_data + ai_path_data::TARGET;
        read_vec3(target).map(|_| target)
    }

    /// Read `AiIns.walkType` back, for telemetry only.
    ///
    /// A read-back is the half of the instrument that a write-side check cannot supply: a write
    /// that "succeeded" and a field that holds the value are different claims, and the difference
    /// is exactly the difference between "the engine ignored us" and "we never wrote". `None`
    /// means the AI object did not validate this frame, which is itself the answer.
    pub(crate) fn read_walk_type(self) -> Option<i32> {
        let ai = self.validated_ai_ins()?;
        unsafe { safe_read_i32(ai + ai_ins::WALK_TYPE) }
    }

    /// Read `AiIns.wantToMoveTo` back, for telemetry only. See [`Self::read_walk_type`].
    pub(crate) fn read_want_to_move_to(self) -> Option<[f32; 3]> {
        let ai = self.validated_ai_ins()?;
        read_vec3(ai + ai_ins::WANT_TO_MOVE_TO)
    }

    /// Write this frame's movement intent -- All four fields `CSAiFunc::MoveTo` writes.
    ///
    /// `wantToMoveTo` and `pathData->target` get the same point, because neither wins in every
    /// branch of `AiIns::UpdateMovement`. That point is [`IntentWrite::aim`], which is the walk
    /// target until something has called [`IntentWrite::aiming_at`] -- the two fields are what the
    /// body faces. `walkType` is the gate that decides whether the engine builds a move vector
    /// from them at all, and `turnTarget` is what makes the facing follow them; see
    /// [`crate::possess::intent`] for the direction those point in and why it was backwards until
    /// 2026-09-02.
    ///
    /// Gated on [`Self::validated_ai_ins`], so a build whose `AiIns` layout has moved gets no
    /// writes at all rather than four wrong ones. It returns whether the stores happened, not
    /// whether they survived: see [`Self::read_move_intent`] for why surviving is not this
    /// function's business any more.
    pub(crate) fn write_move_intent(self, write: IntentWrite) -> bool {
        let Some(ai) = self.validated_ai_ins() else {
            return false;
        };
        // The aim point, not the walk target, and the two are the same value until something has
        // called `IntentWrite::aiming_at`. Both fields get it because neither wins in every branch
        // of `AiIns::UpdateMovement`, and because what the body faces is derived from
        // `wantToMoveTo` -- see [`crate::possess::intent::aim`]. The walk target reaches the engine
        // by a different route entirely: the driver measures the move direction from
        // `IntentWrite::target` and stages it at `ComManipulator+0x140`.
        let value = FloatVector4::new(write.aim[0], write.aim[1], write.aim[2], 1.0);
        // `pathData->target` is a bonus and is no longer a precondition. It is the answer to
        // branch two of `AiIns::UpdateMovement` and costs one store, but the request is
        // `wantToMoveTo` + `walkType` + `turnTarget`; gating all three on the optional fourth is
        // how one bad frame becomes permanent. `CS::AiIns::ClearMoveRequest` calls
        // `FUN_1402e7640(pathData)` -> `AiPathData::ClearFollowPathParams`, and the crate never
        // rebuilds a path (that is `FUN_1402c65e0`, the third thing `CSAiFunc::MoveTo` does and
        // the one this crate does not reproduce) -- so anything that made this resolve `None` used
        // to silence the whole move request for the rest of the possession.
        let path = self
            .ai_path_target()
            .is_some_and(|target_at| write_vec4(target_at, value));
        let want = write_vec4(ai + ai_ins::WANT_TO_MOVE_TO, value);
        // Every one of the four, every frame, and none of them short-circuited: a gait written
        // without a target walks the body somewhere stale, and a target written without a gait is
        // the failure this function shipped with.
        let walk = write_i32(ai + ai_ins::WALK_TYPE, write.walk_type);
        let turn = write_i32(ai + ai_ins::TURN_TARGET, write.turn_target);
        // `path` is deliberately not in the verdict: it is the optional store above, and a
        // creature whose path data has been cleared still takes a perfectly good move request.
        let _ = path;
        want && walk && turn
    }

    /// What `walkType` and `wantToMoveTo.x` hold right now -- the read-back instrument, kept
    /// deliberately separate from [`Self::write_move_intent`]'s verdict.
    ///
    /// It used to be part of that verdict, on the reasoning that the game thread cannot write
    /// between our store and our load. That reasoning was wrong and the instrument is what proved
    /// it: on roughly half of all frames the pair reads back as `walkType = 0` with
    /// `wantToMoveTo` equal to the body's own position, which is `AiIns::ClearMoveRequest`'s exact
    /// signature. The goal machine is a genuine competing writer and losing that race is now
    /// expected, not a fault -- so folding it into a boolean made the caller log "the layout is
    /// wrong" about a healthy possession.
    ///
    /// It stays because it is the only instrument this problem has ever had, and because it is how
    /// anyone checks that the manipulator path in [`Self::write_manipulator_move`] is carrying the
    /// movement the AI path keeps losing.
    pub(crate) fn read_move_intent(self) -> Option<(i32, f32)> {
        let ai = self.validated_ai_ins()?;
        let walk = unsafe { safe_read_i32(ai + ai_ins::WALK_TYPE) }?;
        let target_x = unsafe { safe_read_f32(ai + ai_ins::WANT_TO_MOVE_TO) }?;
        Some((walk, target_x))
    }

    /// What sits in the staging slot the engine publishes from, all four components.
    ///
    /// This crate no longer writes it -- the manual stage was a workaround for the starvation the
    /// vtable swizzle removed, and leaving it in was what made movement unnatural. Reading it is
    /// still the cheapest proof that `[vt+0x50]` is computing a move vector of its own.
    pub(crate) fn staged_move_vector(self) -> Option<[f32; 4]> {
        read_vec4(self.real_manipulator()? + manipulator::PENDING_MOVE_VECTOR)
    }

    /// What the engine has published at `ChrManipulator+0x10`, all four components.
    ///
    /// The consume side is `FUN_1403cd770` -- `MOVUPS XMM0,[RCX+0x10]; MOVAPS [RDX],XMM0; RET` --
    /// called by `FUN_1403cbff0` on `chrManipulator ?? manipulator`. With the swizzle that is one
    /// object, so this is the same field `FUN_1403cdc20` publishes into and the divergence that
    /// starved locomotion cannot recur.
    ///
    /// `w` is `dot(direction, modelMatrix.row4)`, row4 being the translation row, so it tracks the
    /// creature's position and is meant to. `[vt+0x50]` computes the same value.
    pub(crate) fn published_move_vector(self) -> Option<[f32; 4]> {
        read_vec4(self.real_manipulator()? + manipulator::PUBLISHED_MOVE_VECTOR)
    }

    /// `ChrCtrl.chrProxyFlags` -- kept as a logged control, not a suspect.
    ///
    /// `CS::ChrCtrl::ShouldUpdateAi` returns false when this is non-zero, and `[vt+0x50]` returns
    /// on it before publishing anything, so a latched value would starve movement while every
    /// write reported success. It reads `0` on a possessed creature, measured. It stays visible so
    /// nobody re-derives the suspicion.
    pub(crate) fn chr_proxy_flags(self) -> Option<u32> {
        let at = self.chr_ctrl()? + chr_ctrl::CHR_PROXY_FLAGS;
        unsafe { safe_read_i32(at) }.map(|flags| flags as u32)
    }

    /// Stop the creature, the way `CS::AiIns::ClearMoveRequest` stops one.
    ///
    /// Called on release rather than left to the AI. Goal selection comes back the moment
    /// `ChrCtrl+0x3b0` is cleared and its next `Activate` writes `walkType` itself -- but "next"
    /// is a frame or more away, and until then a released creature would still be carrying our
    /// last run order toward a point the player picked, which looks exactly like the mod failing
    /// to let go.
    pub(crate) fn stop_move_intent(self) -> bool {
        let Some(at) = self.position() else {
            return false;
        };
        self.write_move_intent(IntentWrite::stopped(at))
    }

    /// The physics capsule's horizontal half-extent, in metres -- `CSChrPhysicsModule+0x344`.
    ///
    /// The field that decides overlap, which is why the spawn layer reads it and why the camera
    /// layer (which cares how tall the subject is) reads its neighbour instead. Both offsets are
    /// the camera layer's, deliberately: they are one pair of numbers with one proof, and a second
    /// copy here would be a second thing to be wrong about.
    ///
    /// Populated by `CSChrPhysicsModule::InitForEnemy` from `NpcParam.hitRadius`, so it does not
    /// read until the character is built -- which is exactly why the spawn layer places the
    /// creature after readiness rather than at creation.
    pub(crate) fn hit_radius(self) -> Option<f32> {
        let at = self.physics()? + crate::camera::layout::chr_physics_module::HIT_RADIUS;
        unsafe { safe_read_f32(at) }.filter(|v| v.is_finite())
    }

    /// The capsule height, `CSChrPhysicsModule+0x340` -- what `ChrIns::GetPhysicsHitHeight`
    /// returns.
    ///
    /// Named in [`crate::camera::layout`] alongside the radius above, for the same reason: the
    /// two are one pair of numbers with one proof, and this crate keeps exactly one copy of them.
    /// The camera layer reads it to size the framing; [`crate::possess::body_size`] reads it on
    /// both characters, because what it needs is the ratio.
    pub(crate) fn hit_height(self) -> Option<f32> {
        let at = self.physics()? + crate::camera::layout::chr_physics_module::HIT_HEIGHT;
        unsafe { safe_read_f32(at) }.filter(|v| v.is_finite())
    }

    /// The render scale on this character's `ChrCtrl`, `{X, Y, Z}`.
    ///
    /// Read at possession start so release can put back what was actually there rather than
    /// assuming `1.0`. That is the constructor's value and what a stock body carries, but it is
    /// not what a body some other mod has already scaled carries.
    pub(crate) fn body_scale(self) -> Option<[f32; 3]> {
        read_vec3(self.chr_ctrl()? + chr_ctrl::SCALE_SIZE)
    }

    /// Set the render scale, writing both copies the way `ChrCtrl::SetScaleSize` does.
    ///
    /// **Nothing is called**, and that is the same trade this crate makes everywhere else: the
    /// function's entire body is two three-float stores, one into `ChrCtrl` and one into the
    /// `CSChrDataModule` it reaches through `owner->modules[0]`, so writing the fields is calling
    /// it -- and the crate spends no game address for it. The byte proof, including that both
    /// displacements are identical on 1.16.2 and 1.17, is on
    /// [`super::layout::chr_ctrl::SCALE_SIZE`].
    ///
    /// This is the render transform only. It does not move the physics capsule, so the body's
    /// collision and hurtbox stay exactly where they were -- see [`crate::possess::body_size`].
    pub(crate) fn set_body_scale(self, scale: [f32; 3]) -> bool {
        if !scale.iter().all(|v| v.is_finite() && *v > 0.0) {
            return false;
        }
        let Some(ctrl) = self.chr_ctrl() else {
            return false;
        };
        let Some(data) = self.module(modules::DATA) else {
            return false;
        };
        let mut wrote = true;
        for (index, value) in scale.iter().enumerate() {
            let step = index * size_of::<f32>();
            wrote &= write_f32(ctrl + chr_ctrl::SCALE_SIZE + step, *value);
            wrote &= write_f32(data + chr_data_module::SCALE_SIZE + step, *value);
        }
        wrote
    }

    /// Tell the engine this character was last standing here.
    ///
    /// **this is a fall-death SAFETY write and not a cosmetic one.** `CSChrFallModule`'s landing
    /// handler computes the fall it is about to charge for as
    /// `lastGroundedPosition.y - GetPosition().y` -- byte-proven in both images, uniquely, at
    /// 1.16.2 `0x14044dd1f` and 1.17 `0x14044e27f`:
    ///
    /// ```text
    /// 48 8b 88 90 01 00 00   MOV RCX,[RAX+0x190]     ; ChrIns->modules
    /// 48 8b 59 68            MOV RBX,[RCX+0x68]      ; ->physics
    /// e8 ?? ?? ?? ??         CALL GetPosition
    /// 0f 10 b3 50 01 00 00   MOVUPS XMM6,[RBX+0x150] ; lastGroundedPosition
    /// 0f c6 f6 55            SHUFPS XMM6,XMM6,0x55   ; .y
    /// f3 0f 5c 70 04         SUBSS  XMM6,[RAX+0x4]   ; - position.y
    /// ```
    ///
    /// -- and above `DAT_14329e658` (`0.30 m`) `CSChrFallModule::Update` dispatches the landing
    /// notification through the character's manipulator vtable slot `+0x80`, and above the damage
    /// thresholds it charges `hpMax * ratio` through `CSChrDataModule::ChangeHP` with
    /// `deathType = Fall`.
    ///
    /// **corrected 2026-09-02: the damage half does consult `IsImmuneToAttack`, and this doc said
    /// the opposite.** `FUN_14044e730` gates it, and at `0x14044e866` it is
    /// `MOV RCX,RAX ; MOV R9,[RAX] ; CALL [R9+0x1d8] ; TEST AL,AL ; JNZ 0x14044e79a`, where
    /// `0x14044e79a` is `XOR EAX,EAX; RET` -- and `[vtable+0x1d8]` is the slot
    /// [`chr_ins::INVINCIBLE`] already documents as `ChrIns::IsImmuneToAttack`. So the bit alone
    /// stops a possessed body being charged for a fall, and this write is not what keeps the body
    /// alive while a possession is running.
    ///
    /// It is what keeps it alive on the frames either side of one, when the bit is off and the
    /// field still names whatever height the possession left it at -- which is why the value
    /// written is clamped rather than copied. See [`fall`].
    ///
    /// [`Self::request_move`] snaps `position` and `prevUpdatePosition` and leaves this field
    /// alone, so a body carried through a leaping creature's arc lands reading a fall it never
    /// took. Writing it alongside the move is not a lie to the engine: the body is being put here,
    /// so here is where it last stood.
    ///
    /// # The clamp, which is the part that survives a body the world has moved
    ///
    /// `Y` is pinned at the co-location target or at the body's own current height, whichever is
    /// lower -- [`fall::grounded_pin`], which carries the reasoning and the tests. The body keeps
    /// its own full-size character proxy while it is being carried (`ChrCtrl::updatePos` runs a
    /// whole `CSChrPhysicsModule::doUpdates` immediately after draining our teleport), so it can
    /// end up below the point we placed it on. Pinning the field at the creature's height in that
    /// state stores a fall the body never took, and the engine charges for it the moment the bit
    /// that currently hides it comes off.
    pub(crate) fn pin_last_grounded(self, position: [f32; 3]) -> bool {
        let Some(physics) = self.physics() else {
            return false;
        };
        if !position.iter().all(|v| v.is_finite()) {
            return false;
        }
        // The body's own `Y`, read straight out of `position.y` rather than through
        // `Self::position` so the module pointer chain is walked once.
        let body_y = unsafe { safe_read_f32(physics + chr_physics_module::POSITION + 4) };
        let pinned = fall::grounded_pin(position, body_y);
        write_vec4(
            physics + chr_physics_module::LAST_GROUNDED_POSITION,
            FloatVector4::new(pinned[0], pinned[1], pinned[2], 1.0),
        )
    }

    /// Everything the fall path reads off this character, in one pointer walk.
    ///
    /// For the co-location telemetry, and for nothing else: no decision in this crate is taken on
    /// it. A body whose physics module does not resolve reports every field as `None` rather than
    /// failing the caller, because the line is worth writing either way -- "the body did not read"
    /// is the single most useful thing it can say on the frame a possession goes wrong.
    pub(crate) fn fall_state(self) -> FallState {
        let Some(physics) = self.physics() else {
            return FallState::default();
        };
        let flag = |offset: usize| unsafe { safe_read_u8(physics + offset) }.map(|byte| byte != 0);
        FallState {
            position: read_vec3(physics + chr_physics_module::POSITION),
            last_grounded: read_vec3(physics + chr_physics_module::LAST_GROUNDED_POSITION),
            on_ground: flag(chr_physics_module::STANDING_ON_SOLID_GROUND),
            falling: flag(chr_physics_module::FALLING),
            touching_ground: flag(chr_physics_module::IS_TOUCHING_GROUND),
            max_step_height: unsafe {
                safe_read_f32(physics + chr_physics_module::MAX_STEP_HEIGHT)
            },
            capsule_half_height: unsafe {
                safe_read_f32(physics + chr_physics_module::CAPSULE_HALF_HEIGHT)
            },
        }
    }
}

/// The real local player, as a `ChrIns` address.
///
/// `PlayerIns.chr_ins` is the struct's first field, so the `PlayerIns` pointer is the `ChrIns`
/// pointer -- the same identity `RespawnPlayer` relies on.
pub(crate) fn main_player() -> Option<Chr> {
    let world_chr_man = unsafe { WorldChrMan::instance() }.ok()?;
    let player = world_chr_man.main_player.as_ref()?;
    Some(Chr::new(core::ptr::from_ref(&player.chr_ins) as usize))
}

/// Point the camera and lock-on at `chr`, or hand them back with `None`.
///
/// This writes `WorldChrManDbg+0xb8 camOverrideChrIns`, which only
/// `WorldChrManImp::GetMainPlayerIns` and four siblings read. `PlayerIns::IsMainPlayerIns`
/// compares against the raw `WorldChrManImp+0x1e508` and ignores the override, so the ~670
/// identity, damage and save consumers keep pointing at the real player while the ~40 camera and
/// lock-on consumers follow the creature. **That split is the safety property**; repointing
/// `mainPlayerIns` instead is a proven hard blocker (`PlayerIns` is `0x740` and `EnemyIns` is
/// `0x5e0`, and `PlayerIns.playerGameData@+0x580` aliases `EnemyIns.manipulator@+0x580`, so null
/// guards never fire).
///
/// Nothing has to be cleaned up on despawn either: `WorldChrManImp::RemoveChrIns` nulls this field
/// when the character being removed is the one it names.
pub(crate) fn set_camera_override(chr: Option<Chr>) -> bool {
    let Ok(dbg) = (unsafe { WorldChrManDbg::instance() }) else {
        return false;
    };
    let at = core::ptr::from_ref(dbg) as usize + world_chr_man_dbg::CAM_OVERRIDE_CHR_INS;
    if unsafe { safe_read_usize(at) }.is_none() {
        return false;
    }
    unsafe { (at as *mut usize).write(chr.map_or(0, Chr::address)) };
    true
}

/// Is the camera currently pointed at `chr`?
pub(crate) fn camera_override_is(chr: Chr) -> bool {
    let Ok(dbg) = (unsafe { WorldChrManDbg::instance() }) else {
        return false;
    };
    let at = core::ptr::from_ref(dbg) as usize + world_chr_man_dbg::CAM_OVERRIDE_CHR_INS;
    let current = unsafe { safe_read_usize(at) };
    current == Some(chr.address())
}

/// Metres from the possessed creature to the nearest other live enemy.
///
/// What the moveset dispatcher bands on. Not a lock-on distance: `CSLockTgtMan` is not modelled by
/// this crate and reaching it would need an offset nobody here has verified on 1.17, so this is
/// the nearest hostile instead and the module docs say so rather than implying otherwise.
///
/// The player's own body is excluded for free -- it is `ChrType::Player`, and co-located with the
/// creature anyway, so including it would report every attack as point-blank.
///
/// `None` means nothing is within the fifty-unit search radius, which the dispatcher reads as
/// [`crate::moveset::dispatch::Band::Far`]. Called only on a frame with a press, because it walks
/// every loaded `ChrSet`.
pub(crate) fn nearest_hostile_distance(creature: Chr) -> Option<f32> {
    let settings = TargetSettings {
        mode: TargetMode::Nearest,
        ..TargetSettings::default()
    };
    // `pick_target` excludes the character it is given by address, so passing the creature makes
    // it "nearest enemy to the creature, that is not the creature".
    let (target, _) = pick_target(settings, creature);
    let (from, to) = (creature.position()?, target?.position()?);
    let squared = (to[0] - from[0]).powi(2) + (to[1] - from[1]).powi(2) + (to[2] - from[2]).powi(2);
    squared.is_finite().then(|| squared.sqrt())
}

/// Why a target could not be found. Carried into the refusal so the player is told which of the
/// four modes looked and what it saw, rather than "no target".
pub(crate) fn describe_no_target(mode: TargetMode, candidates: usize) -> String {
    match mode {
        TargetMode::LockOn => {
            format!("nothing locked on ({candidates} enemies loaded) -- lock on and press again")
        }
        TargetMode::Nearest | TargetMode::Crosshair => {
            format!("no enemy within reach ({candidates} loaded)")
        }
        TargetMode::ChrId => format!("no loaded enemy matches chr_id ({candidates} loaded)"),
        // UNREACHABLE in practice, and spelled out rather than swept under a `_` arm. The engine
        // returns into `begin_spawn` before any search happens, so `spawn` never reaches a
        // target-not-found path -- but a wildcard here would also silently swallow the next mode
        // somebody adds, and this is the message that would be printed if the interception ever
        // stopped working.
        TargetMode::Spawn => {
            "spawn mode creates a character rather than searching for one, so this refusal means \
             the spawn path was not taken"
                .to_owned()
        }
    }
}

/// How far a `nearest`/`crosshair` search will reach, in physics-space units squared.
///
/// Bounded rather than global: possessing something on the other side of the map because it
/// happened to be the closest thing loaded is not a feature, it is a bug the player cannot see the
/// cause of. Fifty units is comfortably past melee range and short of the next encounter.
const MAX_TARGET_DISTANCE_SQUARED: f32 = 50.0 * 50.0;

/// Find the character `settings` selects, and count what was considered.
///
/// Returns `(target, candidates_seen)`.
pub(crate) fn pick_target(settings: TargetSettings, player: Chr) -> (Option<Chr>, usize) {
    let Ok(world_chr_man) = (unsafe { WorldChrMan::instance() }) else {
        return (None, 0);
    };
    // The lock-on handle lives on the `PlayerIns`, and `main_player` is an `OwnedPtr`, so the
    // dereference is explicit rather than inferred -- a `&OwnedPtr<PlayerIns>` and a `&PlayerIns`
    // are different things and the compiler is right to say so.
    let locked_on = world_chr_man
        .main_player
        .as_ref()
        .map(|player| player.locked_on_enemy);

    // The four ChrSets that hold no enemies, excluded by address rather than by index -- an index
    // would be one more reverse-engineered constant to be wrong about. This is the same walk
    // `er-enemynpc-effects` uses, which is the only enemy enumeration in this repo with runtime
    // evidence behind it.
    let excluded = [
        core::ptr::from_ref(&world_chr_man.player_chr_set) as usize,
        core::ptr::from_ref(&world_chr_man.ghost_chr_set) as usize,
        core::ptr::from_ref(&world_chr_man.summon_buddy_chr_set) as usize,
        core::ptr::from_ref(&world_chr_man.debug_chr_set) as usize,
    ];
    let mut chr_sets: Vec<usize> = Vec::with_capacity(16);
    chr_sets.push(core::ptr::from_ref(&world_chr_man.open_field_chr_set.base) as usize);
    for chr_set in world_chr_man.chr_sets.iter().flatten() {
        let address = chr_set.as_ptr() as usize;
        if excluded.contains(&address) || chr_sets.contains(&address) {
            continue;
        }
        chr_sets.push(address);
    }

    let player_position = player.position();
    let mut candidates = 0usize;
    let mut best: Option<(f32, Chr)> = None;
    for address in chr_sets {
        let chr_set = unsafe { &*(address as *const ChrSet<CsChrIns>) };
        for chr_ins in chr_set.characters() {
            let at = core::ptr::from_ref(chr_ins) as usize;
            if at == player.address() || chr_ins.chr_type != ChrType::Npc {
                continue;
            }
            // The game leaves `special_effect` null on a character that is still being
            // constructed, and reading through it is a live crash this repo has already captured.
            // Read it as a raw pointer so a half-built character is a skip.
            let special_effect =
                unsafe { *(core::ptr::from_ref(&chr_ins.special_effect).cast::<usize>()) };
            if special_effect == 0 {
                continue;
            }
            let chr = Chr::new(at);
            if chr.is_dead() || !chr.is_live() {
                continue;
            }
            candidates += 1;

            match settings.mode {
                TargetMode::ChrId => {
                    if chr_ins.npc_param_id == settings.chr_id || chr_ins.npc_id == settings.chr_id
                    {
                        return (Some(chr), candidates);
                    }
                }
                TargetMode::LockOn => {
                    if locked_on.is_some_and(|handle| {
                        !handle.is_empty() && handle == chr_ins.field_ins_handle
                    }) {
                        return (Some(chr), candidates);
                    }
                }
                // CROSSHAIR is nearest today, and says so rather than pretending. Picking what the
                // camera centre is pointed at needs the camera's own forward vector, which is a
                // piece of reverse engineering this layer does not have; `nearest` is the honest
                // fallback and the mode is kept so the config does not have to change when it
                // lands.
                TargetMode::Nearest | TargetMode::Crosshair => {
                    let (Some(from), Some(to)) = (player_position, chr.position()) else {
                        continue;
                    };
                    let distance = (to[0] - from[0]).powi(2)
                        + (to[1] - from[1]).powi(2)
                        + (to[2] - from[2]).powi(2);
                    if distance <= MAX_TARGET_DISTANCE_SQUARED
                        && best.is_none_or(|(best_distance, _)| distance < best_distance)
                    {
                        best = Some((distance, chr));
                    }
                }
                // Spawn mode has no search to do: the engine intercepts it before this walk. The
                // arm is explicit rather than a wildcard so that adding a mode that does search is
                // a compile error here instead of a mode that silently matches nothing.
                TargetMode::Spawn => {}
            }
        }
    }
    (best.map(|(_, chr)| chr), candidates)
}

/// Read the left thumbstick as a movement request. `None` when no pad is connected, or when the
/// stick is inside the deadzone.
pub(crate) fn read_move_stick(movement: &crate::settings::MovementSettings) -> Option<Stick> {
    // The pad wins when it is deflected, so a player holding both does not fight themselves.
    // `read_left_stick` answering `None` means no pad is attached at all; a resting stick answers
    // `Some` and then falls inside the deadzone, and both end up here.
    if let Some(stick) = crate::input::read_left_stick().and_then(|(x, y)| Stick::from_xinput(x, y))
    {
        return Some(stick);
    }
    let (x, y) = crate::input::read_move_keys(
        movement.forward,
        movement.back,
        movement.left,
        movement.right,
    )?;
    Stick::from_axes(x, y)
}

/// Everyone else in the session, reduced to what the net-damage ledger needs.
///
/// Walks `WorldChrMan.player_chr_set` -- the set that holds player characters, which is precisely
/// the population `FUN_14044a1b0` sorts into categories 1 and 2. Nothing here resolves a game
/// address: the main player is identified by pointer against `WorldChrMan.main_player`, and
/// everything else in that set is another human's character.
///
/// The set holds a handful of entries at most, but the caller still samples on a cadence rather
/// than per frame -- see [`crate::possess::netdamage::Ledger::due`].
pub(crate) fn players_in_world() -> Vec<netdamage::Seen> {
    let Ok(world_chr_man) = (unsafe { WorldChrMan::instance() }) else {
        return Vec::new();
    };
    let main_player = world_chr_man
        .main_player
        .as_ref()
        .map(|player| core::ptr::from_ref(&player.chr_ins) as usize);
    let mut seen = Vec::new();
    for player in world_chr_man.player_chr_set.characters() {
        let at = core::ptr::from_ref(&player.chr_ins) as usize;
        // Same guard the target search uses: the game leaves `special_effect` null on a character
        // that is still being constructed, and reading through a half-built one is a live crash
        // this repo has already captured.
        let special_effect =
            unsafe { *(core::ptr::from_ref(&player.chr_ins.special_effect).cast::<usize>()) };
        if special_effect == 0 {
            continue;
        }
        let chr = Chr::new(at);
        seen.push(netdamage::Seen {
            address: at,
            is_main_player: main_player == Some(at),
            chr_type: player.chr_ins.chr_type as i32,
            team: chr.team_type(),
            hp: chr.hp(),
        });
    }
    seen
}

/// The last player-versus-player damage packet this process received.
///
/// `WorldChrManImp`'s packet pump dequeues every arriving `Packet15` into one function-static
/// buffer and never clears it, so reading that buffer answers the question no amount of local
/// telemetry could: when nobody's HP moves, did the other player's damage arrive?
///
/// Read-only, and the only game address this module resolves for the net-damage watch -- one
/// per-build data RVA, refused rather than guessed on an unmeasured build. See
/// [`crate::possess::layout::packet15_receive`] for the pump's own instruction sequence and for
/// why the damage is an `i16`.
///
/// `None` means the build is unmeasured or the read faulted, which the ledger reports as such
/// rather than as an absence of packets.
pub(crate) fn last_received_damage_packet() -> Option<netdamage::ReceivedPacket> {
    let rva = layout::packet15_receive_rva(game_file_version())?;
    let base = er_game_base::mem::game_module_base().ok()? + rva as usize;
    // Handles are two `u32`s side by side; read them as one `usize` so a partial read is a miss
    // rather than half a handle. `safe_read_usize` is `ReadProcessMemory`, so an address that has
    // somehow gone bad answers `None` instead of raising.
    let dealer = unsafe { safe_read_usize(base + packet15_receive::DEALER_HANDLE) }? as u64;
    let victim = unsafe { safe_read_usize(base + packet15_receive::VICTIM_HANDLE) }? as u64;
    let damage = unsafe { safe_read_u16(base + packet15_receive::DAMAGE) }? as i16;
    let stamina = unsafe { safe_read_u16(base + packet15_receive::STAMINA) }? as i16;
    Some(netdamage::ReceivedPacket {
        dealer,
        victim,
        damage,
        stamina,
    })
}
