//! Spawning the game's own visual effects at world positions.
//!
//! Everything else in this DLL draws with imgui, in screen space, over the finished frame. That
//! is why the line looks painted on the glass: it does not occlude behind a hill, it takes no
//! light, and it is unmistakably not part of the game. An FXR spawned through the engine's own
//! SFX manager is a real effect in the world -- correct depth, correct lighting, the look the
//! game already has.
//!
//! # This is the one thing here that changes the game
//!
//! The rest of this crate reads. This spawns. It is a real engine object with a real lifetime,
//! and the module-level claim that this DLL does nothing to the game stops being true the moment
//! `marker_fxr_id` is set to something other than zero. That is why it is off by default.
//!
//! Whether a Seamless Co-op session replicates one of these to other players is **not known**. If
//! it does, a trail of markers pointing at an invader is a trail pointing back at you. Until
//! somebody watches a second client while these spawn, treat that as an open question rather than
//! a solved one.

#![cfg(windows)]

/// The fire-and-forget wrapper `FUN_140d929f0` is deliberately not used.
///
/// It resolves the singleton, spawns, and then throws the control block away -- which is why the
/// first version of this feature could place stones and never take them back. Everything here
/// goes through [`SPAWN_FFX_INSTANCE_RVA`] and keeps the block.
///
const GLOBAL_CSSFX_RVA: u32 = 0x3d8_39b8;

/// A world transform with no rotation and no scale, positioned at `position`.
///
/// Row-major with the translation in row 3, which is the layout the engine reads everywhere else
/// in this crate -- `CSCam`'s camera-to-world matrix puts the eye in row 3 and its basis in rows
/// 0..2, and the spawn-location list `CS::ChrIns::SpawnOneShotSfx` walks is the same 0x40-byte
/// shape.
///
/// Rotation is left as identity deliberately. An upright marker reads the same from every angle,
/// and deriving a rotation from the path direction would make each marker's orientation depend on
/// a navmesh answer that changes between refreshes -- markers that visibly twitch as the route is
/// recomputed.
fn transform_at(position: [f32; 3]) -> WorldTransform {
    WorldTransform([
        1.0,
        0.0,
        0.0,
        0.0,
        0.0,
        1.0,
        0.0,
        0.0,
        0.0,
        0.0,
        1.0,
        0.0,
        position[0],
        position[1],
        position[2],
        1.0,
    ])
}

/// The spawn transform, **16-byte aligned**, because the engine loads it with `MOVAPS`.
///
/// This alignment is not a precaution, it is the fix for a crash. `SpawnFfxInstance` forwards the
/// pointer to `FUN_141c9eeb0`, whose first act is to transpose the matrix in place:
///
/// ```text
/// 141c9eeb9  MOV    RAX, qword ptr [RSP + 0xb0]      ; <- this pointer
/// 141c9eedb  MOVAPS XMM4, xmmword ptr [RAX]
/// 141c9eede  MOVAPS XMM2, xmmword ptr [RAX + 0x20]
/// 141c9eee5  SHUFPS XMM3, xmmword ptr [RAX + 0x10], 0x44
/// 141c9eeed  SHUFPS XMM1, xmmword ptr [RAX + 0x30], 0x44
/// ```
///
/// `MOVAPS` and a memory-operand `SHUFPS` both **fault** on an address that is not a multiple of
/// 16. A bare `[f32; 16]` local has alignment 4, so whether the spawn worked came down to where
/// the compiler happened to place it on the stack -- and on 2026-08-26 it placed it badly and the
/// game died inside the first marker self-check spawn, no log line, no fault handler, from a
/// caller whose only sin was passing a correct matrix at the wrong address.
#[repr(C, align(16))]
struct WorldTransform([f32; 16]);

/// Resolve a game function by RVA for the running build, refusing what it cannot place.
///
/// The doc comment here used to make this claim while the body only added the base -- it checked
/// nothing. It now delegates to the shared resolver, which returns the translated address on a
/// build that moved the code and `None` when no mapping is verified. That matters more here than
/// almost anywhere: these addresses are transmuted into function pointers and called, so a stale
/// one transfers control into whatever now occupies those bytes.
fn function(rva: u32) -> Option<usize> {
    let module_base = er_game_base::mem::game_module_base().ok()?;
    er_game_base::game_build::resolve_game_address(module_base + rva as usize, "invasion-path sfx")
}

/// Bytes of `UnkSfxCtrlStruct`, the control block `SpawnFfxInstance` writes into.
///
/// Ghidra lays it out as `0x30 + 0x3d8 = 0x408`, and `FUN_140d929f0`'s own stack spaces two of
/// them `0x838 - 0x428 = 0x410` apart -- while `FUN_1420b6ac0` is handed a `0x3e0` block starting
/// at `+0x30`, i.e. through `0x410`. `0x440` is over-allocated on purpose: every byte is zeroed,
/// the slack costs nothing, and being short here would let the engine write past the end.
const CTRL_BYTES: usize = 0x440;

/// `CS::CSSfxImp::SpawnFfxInstance` -- RVA `0xd95280`. Called directly rather than through
/// `FUN_140d929f0`, because the wrapper discards the control block this returns and a discarded
/// control block is an effect that can never be removed.
const SPAWN_FFX_INSTANCE_RVA: u32 = 0xd9_5280;

/// `FUN_1420b6370(ctrl) -> bool` -- the engine's own liveness check, and it is two levels deep:
/// the instance pointer at `ctrl+0x08` must be non-null and `FUN_1420b6280` must report that
/// instance still alive. That second level is why this crate no longer reads `+0x08` itself and
/// has no constant for it.
///
/// Reading `ctrl+0x08` alone -- which is what this module did first -- passes for a control block
/// whose effect has already finished, and the teardown then pushes parameters into a dead object.
/// The engine's sign cleanup calls this before deciding anything, and so does this now.
const CTRL_IS_ALIVE_RVA: u32 = 0x20b_6370;

/// `FUN_1420b6ac0(ctrl, params, len, arg4)` -- push the parameter block to the live instance.
const CTRL_PUSH_PARAMS_RVA: u32 = 0x20b_6ac0;
/// `FUN_1420b63c0(ctrl)` -- finalise: acts on the instance at `ctrl+0x08` if there is one.
const CTRL_FINALISE_RVA: u32 = 0x20b_63c0;
/// `FUN_141c92f30(ctrl + 0x20)` -- tear down the control's second sub-object.
const CTRL_SUBOBJECT_RELEASE_RVA: u32 = 0x1c9_2f30;
/// `FUN_1420b5c40(ctrl)` -- unlink the control from the instance's observer list. Not a kill:
/// on its own this only unregisters, which is why the stop above has to happen first.
const CTRL_UNLINK_RVA: u32 = 0x20b_5c40;

/// Offsets inside the control block, all from the same base -- the pointer handed to
/// `SpawnFfxInstance` as its out-parameter.
///
/// Deriving them from one base is not pedantry. The sign code this recipe came from reaches them
/// through `FXHGSfxCtrl_Sign.super_FXHGSfxCtrl.field2_0x10`, so its printed offsets are relative
/// to that inner field and transcribing them directly would be wrong by `0x10` -- into the live
/// observer list.
mod ctrl {
    /// Sub-object released during teardown.
    pub(super) const SUBOBJECT: usize = 0x20;
    /// Start of the parameter block.
    pub(super) const PARAMS: usize = 0x30;
    /// Length of that block, as `FUN_1420b6ac0` is told.
    pub(super) const PARAMS_LEN: i32 = 0x3e0;
    /// The stop flag (block offset `0x3d4`).
    ///
    /// One byte below the AUTO-release flag at `0x405` that `FUN_141c93450` sets. Setting that
    /// one instead hands the effect to the engine to manage and it can never be removed -- which
    /// is exactly the bug being fixed here, so the two are named rather than spelled inline.
    pub(super) const STOP_FLAG: usize = 0x404;
    /// Three fields the sign teardown clears before pushing (block `0x18`/`0x20`/`0x28`).
    pub(super) const CLEARED: [usize; 3] = [0x48, 0x50, 0x58];
}

type SpawnInstanceFn = unsafe extern "C" fn(
    usize,
    *mut u8,
    *const u32,
    *const WorldTransform,
    u64,
    i32,
    i16,
    i16,
    i32,
) -> *mut u8;
type CtrlPushFn = unsafe extern "C" fn(*mut u8, *mut u8, i32, usize) -> u64;
type CtrlAliveFn = unsafe extern "C" fn(*mut u8) -> u64;
type CtrlVoidFn = unsafe extern "C" fn(*mut u8);

/// The three trailing arguments `SpawnFfxInstance` forwards to `FUN_140d94af0`, which builds the
/// FXR's **external value** table.
///
/// This was documented as an unproven lead that fed "time-of-day and weather, not colour". That
/// was wrong in a way worth spelling out: they are that table, and the mapping is exact.
/// `FUN_140d94af0` (RVA `0xd94af0`) builds nine entries with keys
/// `{0, 1, 2, 1000, 2000, 2100, 2200, 3000, 10000}` — key 1 is the current hour and key 2 the
/// wetness from the active `WEATHER_PARAM`, which is where the earlier description came from, but
/// the three arguments below land on three other keys:
///
/// | field | spawn arg | external value |
/// |---|---|---|
/// | `a` | `param_7`, `i16` | **2100** |
/// | `b` | `param_8`, `i16` | **2000** |
/// | `c` | `param_9`, `i32` | **2200** |
///
/// So they change an effect's appearance **if and only if** that FXR wired a node to one of those
/// three external values. That is a real mechanism with a real limit, not a guess: an effect that
/// references none of them will ignore all three no matter what is passed. `-1` is what the engine
/// passes for a one-shot, and remains the default.
///
/// A table with arbitrary keys can be built with `FUN_1420b7840`/`FUN_1420b7c20` and handed to
/// `FUN_141c9ee60` directly, bypassing `FUN_140d94af0` — still bounded by what the FXR references.
#[derive(Clone, Copy)]
pub(crate) struct SpawnVariant {
    pub(crate) a: i16,
    pub(crate) b: i16,
    pub(crate) c: i32,
}

/// A spawned effect, kept so it can be removed again.
///
/// The control block is boxed and 16-byte aligned: the engine writes a `FloatMatrix4x4` worth of
/// state through it, and it must not move once the instance holds a pointer back to it -- which
/// it does, through the observer list `FUN_1420b5c40` unlinks from.
#[repr(C, align(16))]
struct CtrlBlock([u8; CTRL_BYTES]);

pub(crate) struct Marker {
    ctrl: Box<CtrlBlock>,
    /// Whether the engine bound an instance when this was spawned. False means the id was
    /// rejected or is not resident -- not that the effect is invisible.
    bound_at_spawn: bool,
}

impl Marker {
    /// Did the engine accept the effect id and build an instance?
    pub(crate) fn bound(&self) -> bool {
        self.bound_at_spawn
    }
}

impl Marker {
    fn field(&mut self, offset: usize) -> *mut u8 {
        // SAFETY: every offset used is inside CTRL_BYTES, checked by the const assertion below.
        unsafe { self.ctrl.0.as_mut_ptr().add(offset) }
    }
}

const _: () = {
    assert!(ctrl::STOP_FLAG < CTRL_BYTES);
    assert!(ctrl::PARAMS + ctrl::PARAMS_LEN as usize <= CTRL_BYTES);
};

/// Spawn an effect and keep the handle, so it can be despawned later.
///
/// # Safety
///
/// Must be called on the game thread.
pub(crate) unsafe fn spawn_tracked(
    fxr_id: u32,
    position: [f32; 3],
    variant: SpawnVariant,
) -> Option<Marker> {
    if fxr_id == 0 || !position.iter().all(|axis| axis.is_finite()) {
        return None;
    }
    // SAFETY: game thread; a fault-tolerant read of one global.
    let singleton = unsafe { sfx_singleton() }?;
    let spawn = function(SPAWN_FFX_INSTANCE_RVA)?;
    // SAFETY: a validated address inside the game image.
    let spawn: SpawnInstanceFn = unsafe { std::mem::transmute::<usize, SpawnInstanceFn>(spawn) };

    let mut marker = Marker {
        ctrl: Box::new(CtrlBlock([0u8; CTRL_BYTES])),
        bound_at_spawn: false,
    };
    let transform = transform_at(position);
    let id = fxr_id;
    // SAFETY: the callee constructs the block it is given before writing to it, and the trailing
    // arguments are the ones the engine's own call site passes for a one-shot.
    unsafe {
        spawn(
            singleton,
            marker.ctrl.0.as_mut_ptr(),
            &raw const id,
            &raw const transform,
            0,
            8,
            variant.a,
            variant.b,
            variant.c,
        );
    }
    // Did the id actually resolve? `FUN_1420dda60` returns 0 and spawns nothing when an id is
    // unresolvable or not resident, so an unusable id is indistinguishable from a usable one that
    // happens to be invisible -- unless the control block is asked. It is asked here, once, at
    // spawn: bound means the engine accepted the id and built an instance.
    //
    // That turns "does effect N exist and spawn" into a log line instead of a frame somebody has
    // to look at, which matters because the candidate colour ids are chosen from file sizes and
    // resource lists, not from having been seen.
    marker.bound_at_spawn = unsafe { ctrl_is_alive(marker.field(0)) };
    Some(marker)
}

/// Ask the engine whether a control block still has a live instance.
///
/// Two levels deep on purpose -- see [`CTRL_IS_ALIVE_RVA`].
///
/// # Safety
///
/// Must be called on the game thread with a constructed control block.
unsafe fn ctrl_is_alive(ctrl: *mut u8) -> bool {
    let Some(is_alive) = function(CTRL_IS_ALIVE_RVA) else {
        return false;
    };
    // SAFETY: a validated address in the game image.
    let is_alive: CtrlAliveFn = unsafe { std::mem::transmute::<usize, CtrlAliveFn>(is_alive) };
    // SAFETY: the block was constructed by `SpawnFfxInstance`.
    (unsafe { is_alive(ctrl) } & 0xff) != 0
}

/// Remove a spawned effect, following the sequence `CS::SosSignMan` uses for summon signs.
///
/// # Safety
///
/// Must be called on the game thread, with a marker this module spawned.
pub(crate) unsafe fn despawn(mut marker: Marker) {
    let (Some(push), Some(finalise), Some(subobject), Some(unlink)) = (
        function(CTRL_PUSH_PARAMS_RVA),
        function(CTRL_FINALISE_RVA),
        function(CTRL_SUBOBJECT_RELEASE_RVA),
        function(CTRL_UNLINK_RVA),
    ) else {
        return;
    };
    // SAFETY: four validated addresses inside the game image.
    let (push, finalise, subobject, unlink): (CtrlPushFn, CtrlVoidFn, CtrlVoidFn, CtrlVoidFn) = unsafe {
        (
            std::mem::transmute::<usize, CtrlPushFn>(push),
            std::mem::transmute::<usize, CtrlVoidFn>(finalise),
            std::mem::transmute::<usize, CtrlVoidFn>(subobject),
            std::mem::transmute::<usize, CtrlVoidFn>(unlink),
        )
    };

    // Ask the engine whether this control still has a live instance, rather than reading the
    // pointer and hoping. A marker held for a few seconds -- which every real trail marker is --
    // can have its effect finish on its own in the meantime, and pushing parameters into a
    // finished instance is how this crashed a live session on 2026-08-25.
    // Without the predicate this returns false and the instance is left alone: leaking one effect
    // is cosmetic, writing into a dead one is not.
    // SAFETY: game thread; our own constructed block.
    let alive = unsafe { ctrl_is_alive(marker.field(0)) };
    if alive {
        // SAFETY: every write below is inside our own zeroed block, at offsets the const
        // assertion above bounds.
        unsafe {
            marker.field(ctrl::STOP_FLAG).write(1);
            for offset in ctrl::CLEARED {
                marker.field(offset).cast::<u64>().write(0);
            }
        }
        let base = marker.field(0);
        let params = marker.field(ctrl::PARAMS);
        // SAFETY: the block is constructed and still bound to a live instance.
        unsafe {
            push(base, params, ctrl::PARAMS_LEN, 0);
            finalise(base);
        }
    }
    let base = marker.field(0);
    let subobject_ptr = marker.field(ctrl::SUBOBJECT);
    // SAFETY: teardown of our own block, in the order the engine's own sign cleanup uses.
    unsafe {
        subobject(subobject_ptr);
        unlink(base);
    }
}

/// The `CSSfxImp` singleton, or `None` before it exists.
///
/// # Safety
///
/// Must be called on the game thread.
unsafe fn sfx_singleton() -> Option<usize> {
    let module_base = er_game_base::mem::game_module_base().ok()?;
    // SAFETY: fault-tolerant read; None rather than a fault if the page is not mapped.
    //
    // Resolved, not `module_base + RVA`. Every `.data` global moved between 1.16.2 and 1.17 --
    // this one 0x3d839b8 -> 0x3d87a28 -- and a stale read does not announce itself the way a
    // stale call does: `safe_read_usize` succeeds and hands back whatever now occupies the old
    // slot, which the heap-alignment screen below cannot tell from a real CSSfx pointer.
    let singleton = unsafe {
        er_game_base::mem::safe_read_usize(er_game_base::mem::game_data_addr(
            module_base,
            GLOBAL_CSSFX_RVA as usize,
            "GLOBAL_CSSFX_RVA",
        ))
    }?;
    // SAFETY: a plausibility screen on the raw value; reads nothing through the pointer.
    (singleton != 0 && unsafe { er_game_base::mem::is_heap_aligned_ptr(singleton) })
        .then_some(singleton)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every offset the teardown writes through must land inside the block we allocate. These are
    /// all compile-time constants, so they are checked at compile time -- a runtime assertion here
    /// would fire after the wild write it was meant to prevent.
    #[test]
    fn every_control_offset_lands_inside_the_block_we_allocate() {
        const {
            assert!(ctrl::SUBOBJECT < CTRL_BYTES);
            assert!(ctrl::STOP_FLAG < CTRL_BYTES);
            assert!(ctrl::PARAMS + ctrl::PARAMS_LEN as usize <= CTRL_BYTES);
            let mut index = 0;
            while index < ctrl::CLEARED.len() {
                assert!(ctrl::CLEARED[index] + 8 <= CTRL_BYTES);
                index += 1;
            }
        }
    }

    /// The stop flag and the auto-release flag are adjacent bytes. Setting the wrong one leaves
    /// the effect running forever under the engine's own management, which is the exact bug the
    /// despawn exists to fix, so the distance between them is asserted rather than trusted.
    #[test]
    fn the_stop_flag_is_not_the_auto_release_flag() {
        const AUTO_RELEASE_FLAG: usize = 0x405;
        assert_eq!(ctrl::STOP_FLAG + 1, AUTO_RELEASE_FLAG);
    }

    #[test]
    fn the_control_block_is_aligned_for_the_matrix_the_engine_writes_through_it() {
        assert_eq!(align_of::<CtrlBlock>(), 16);
        assert_eq!(size_of::<CtrlBlock>(), CTRL_BYTES);
    }

    #[test]
    fn a_transform_carries_the_position_in_row_three_and_is_otherwise_identity() {
        let transform = transform_at([10.0, -2.5, 7.0]);
        assert_eq!(&transform.0[12..15], &[10.0, -2.5, 7.0]);
        assert_eq!(transform.0[15], 1.0);
        // Rows 0..2 are the identity basis.
        assert_eq!(&transform.0[0..4], &[1.0, 0.0, 0.0, 0.0]);
        assert_eq!(&transform.0[4..8], &[0.0, 1.0, 0.0, 0.0]);
        assert_eq!(&transform.0[8..12], &[0.0, 0.0, 1.0, 0.0]);
    }

    #[test]
    fn a_transform_is_the_sixteen_floats_the_engine_reads() {
        assert_eq!(size_of::<WorldTransform>(), 0x40);
    }

    /// The engine transposes this matrix with `MOVAPS`, which faults on an address that is not a
    /// multiple of 16. A plain `[f32; 16]` is alignment 4 and killed the game; this asserts the
    /// alignment that fixed it, so nobody can quietly drop the `align(16)` again.
    #[test]
    fn the_transform_is_aligned_for_the_movaps_the_engine_loads_it_with() {
        assert_eq!(align_of::<WorldTransform>(), 16);
        let transform = transform_at([0.0, 0.0, 0.0]);
        assert_eq!((&raw const transform).addr() % 16, 0);
    }
}
