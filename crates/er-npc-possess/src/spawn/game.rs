//! The two game functions this layer calls, and the field reads that decide when to call them.
//!
//! # Why these two and not the two the brief named
//!
//! The obvious spawn entry is `CS::ChrSet::SpawnSummonBuddy` (1.16.2 `0x140492cb0`), and it is the
//! wrong one. Its decompilation is explicit about why: it takes the slot from the caller, as a
//! `FieldInsHandle`, and then does
//!
//! ```text
//! uVar1 = HandleToIndex(&handle);
//! chrSetEntry = param_1->entries + (int)uVar1;      // no bound of any kind
//! if (chrSetEntry->chrIns != NULL) return chrSetEntry->chrIns;
//! ```
//!
//! -- an unchecked index into a heap array, and a silent return of somebody else'S character when
//! the slot is taken. Every bound on that path would have had to be ours, on an array Seamless
//! Co-op also lives in.
//!
//! [`SPAWN_DYNAMIC_CHR_RVA`] is the entry the game uses for exactly this job.
//! `CSTalkDynamicChrCtrl` -- the runtime spawner behind dynamically-placed NPCs -- calls it, and it
//! forwards to `FUN_140492a90`, which opens `index = 6`, loops `while (index < 0x14)` and takes the
//! first entry whose `chrIns` is null. **The index is never ours, so there is no index of ours to
//! be wrong**, and a full band returns NULL instead of overwriting anybody. It also does the
//! registration `SpawnSummonBuddy` skips: the `ChrSet` vtable slot, the `eventEntityIdMap` insert
//! (only when the entity id is nonzero, which is why ours is zero) and `AddChrInsToGroupIdMap`.
//!
//! What is left for this module to bound is the array, because that loop consults no capacity at
//! all: see [`spawn`], which reads `chrInsCapacity` at runtime and refuses when it does not cover
//! the band the game is about to walk.
//!
//! # And the four readiness predicates, which cost no address at all
//!
//! The brief's readiness predicate is four native calls. Three of them turned out to be field reads
//! wearing a function, and the fourth is a pointer walk this crate already had:
//!
//! | brief | what it actually is |
//! |---|---|
//! | `chrSet->entries[slot].chrIns == returned` | a read, unchanged |
//! | `ChrIns::GetEneDat != NULL` | `3 <= *(i32*)(chrIns->chrRes + 0x40) < 6` |
//! | `FUN_1404ca4a0(eneDat) != NULL` | two loads and a `== 4` byte test |
//! | `chrModelIns`/`chrCtrl` non-null | [`Chr::chr_ctrl`] + [`Chr::real_manipulator`] |
//!
//! The last row is a deliberate substitution rather than a transcription. `chrCtrl` non-null is a
//! weaker statement than what possession needs and than what this crate already checks:
//! [`Chr::chr_ctrl`] additionally proves `ChrCtrl.owner` points back at the same `ChrIns`, and
//! [`Chr::real_manipulator`] proves the `ComManipulator` the thunk will forward to exists. Those
//! two are the actual preconditions for the next thing that happens. `chrModelIns` is not read
//! anywhere in this crate, so adding an offset for it -- one this session did not verify on 1.17 --
//! would have been cost with no benefit.

use core::ffi::c_void;

use eldenring::cs::{ChrIns as CsChrIns, ChrSet, WorldChrMan};
use er_game_base::mem::{
    game_rva_named, is_heap_aligned_ptr, safe_read_i32, safe_read_u8, safe_read_usize,
};
use fromsoftware_shared::FromStatic;

use crate::possess::game::Chr;
use crate::possess::layout::{chr_ins, chr_res, chr_set, chr_spawn_request, file_cap};
use crate::spawn::readiness::Gate;
use crate::spawn::request::{SpawnRequest, SpawnSpec};

/// Compile-time cross-check, same contract as `crate::possess::game`'s: the left side is this
/// crate's reverse engineering, the right is `fromsoftware-rs`'s independently derived model, and a
/// failure here is a build error rather than a wrong read.
const _: () = {
    assert!(core::mem::offset_of!(CsChrIns, chr_set_entry) == chr_ins::CHR_SET_ENTRY);
    // `chr_res` is private in the crate's `ChrIns`, so it cannot be cross-checked the way the rest
    // are. Its evidence is byte-level instead and is stronger for it: `ChrIns::GetEneDat` is
    // byte-identical between the two builds and opens `48 8b 43 28`, i.e. `MOV RAX,[RBX+0x28]`.
    // See `crate::possess::layout::chr_res`.
    assert!(core::mem::offset_of!(ChrSet<CsChrIns>, capacity) == chr_set::CAPACITY);
    assert!(core::mem::offset_of!(ChrSet<CsChrIns>, entries) == chr_set::ENTRIES);
    // The buddy `ChrSet` is the one the spawn entry hard-codes as `RCX + 0x10f90`. Asserting the
    // crate's offset against that immediate is what ties the pointer this module reads capacity
    // from to the array the game is about to index.
    assert!(core::mem::offset_of!(WorldChrMan, summon_buddy_chr_set) == 0x10f90);
};

/// `CS::WorldChrManImp::SpawnDynamicChr(WorldChrManImp*, ChrSpawnRequest*) -> ChrIns*`,
/// 1.16.2 RVA `0x506f30`.
///
/// Ghidra leaves it `FUN_140506f30`; the name here is what it does. Its whole body is
/// `FUN_140492a90(&this->buddyChrSet, request)` followed by `chrIns->vft[0x610](chrIns,
/// this->field_0x1e610)` on a non-null result -- so calling it rather than `FUN_140492a90` directly
/// is what keeps the post-spawn step the game performs.
///
/// Verified for 1.17 rather than assumed, twice over. The sweep row in
/// `docs/recon/rva-map-1162-to-1170.verified.tsv` is `IDENTICAL-WHOLE` over 18 instructions with
/// `.pdata` extents `0x43/0x43` in both images; and independently, a masked byte search with the
/// two struct immediates pinned (`0x10f90` for the buddy `ChrSet`, `0x1e610` for the vtable call's
/// argument) matches exactly one place in each image -- 1.16.2 `0x140506f3a`, 1.17 `0x140507d0a`,
/// both `+0xa` into the function. Pinning those immediates is what separates it from
/// `WorldChrManImp::CreatePlayer`, its byte-for-byte twin on `playerChrSet` `+0x10ee0`, which a
/// fully wildcarded pattern cannot tell it from (two hits per image).
pub(crate) const SPAWN_DYNAMIC_CHR_RVA: u32 = 0x0050_6f30;

/// `CS::WorldChrManImp::RemoveChrIns(WorldChrManImp*, ChrIns*)`, 1.16.2 RVA `0x50a570`.
///
/// The only thing DESPAWN may call. The refcount needs nothing from us: this hands the character to
/// `CSDelayDeleteMan` and the game balances the `EneDat` reference itself, exactly as its own
/// `SummonBuddyManager::RemoveChrIns` does. Calling the release helper `FUN_1404cd870` alongside it
/// -- which an earlier reading of the brief suggested -- would double-free.
///
/// It also covers the `ChrSet` this layer spawns into: it tries `debugChrSet`, `playerChrSet`,
/// `replayGhostChrSet` and then `buddyChrSet`, and nulls the entry in whichever one holds the
/// character. Two of its side effects matter here and are relied on rather than repeated: it clears
/// `WorldChrManDbg+0xb8` when that names the character being removed, and it removes it from the
/// `+0x1e630..+0x1e638` vector.
///
/// **ordering, and it is a crash if it moves.** The delayed delete eventually destroys the
/// `ChrIns`, which runs `ChrCtrl::Unref`, which DLPanics on a non-null `ChrCtrl+0x3b0`. So this
/// must never run before the manipulator override has been cleared; see
/// `crate::possess::teardown::Step`, where that ordering is the enum's own discriminants.
///
/// Verified for 1.17 as `IDENTICAL-WHOLE` over 126 instructions, `.pdata` `0x233/0x233`, and
/// independently by a masked byte search unique in both images (1.16.2 `0x14050a570`, 1.17
/// `0x14050b340`).
pub(crate) const REMOVE_CHR_INS_RVA: u32 = 0x0050_a570;

/// `ChrIns* SpawnDynamicChr(WorldChrManImp*, ChrSpawnRequest*)`.
type SpawnDynamicChrFn = unsafe extern "system" fn(*mut c_void, *const u8) -> usize;

/// `void RemoveChrIns(WorldChrManImp*, ChrIns*)`.
type RemoveChrInsFn = unsafe extern "system" fn(*mut c_void, usize);

/// A creature this mod created, and where in the roster it lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Spawned {
    pub(crate) creature: Chr,
    /// Its index in the buddy `ChrSet`. Derived from the character's own `chrSetEntry`
    /// back-pointer and proved to be inside `BAND_FIRST..capacity`, so every later read of
    /// `entries[slot]` is in bounds by construction.
    pub(crate) slot: u32,
    /// `&entries[slot]`, cached so the liveness read is one load rather than a re-walk.
    pub(crate) entry: usize,
}

/// What a spawn needs to know about the roster before it touches it: the manager to call, and the
/// capacity and entry array of the `ChrSet` that manager is about to walk.
///
/// All three from one `WorldChrMan::instance()`, on purpose. Two lookups would leave a window --
/// however small, and however unlikely on the game's own thread -- in which the capacity was read
/// from one manager and the spawn performed against another.
fn buddy_roster() -> Result<(usize, u32, usize), String> {
    let Ok(world_chr_man) = (unsafe { WorldChrMan::instance() }) else {
        return Err("WorldChrMan is not up yet".to_owned());
    };
    let manager = core::ptr::from_ref(world_chr_man) as usize;
    let chr_set = core::ptr::from_ref(&world_chr_man.summon_buddy_chr_set) as usize;
    let Some(capacity) = (unsafe { safe_read_i32(chr_set + chr_set::CAPACITY) }) else {
        return Err("the buddy roster's capacity did not read".to_owned());
    };
    let Ok(capacity) = u32::try_from(capacity) else {
        return Err(format!(
            "the buddy roster reports a negative capacity ({capacity})"
        ));
    };
    // The bound, on the array the game is about to walk. `FUN_140492a90` scans `entries[6..20)`
    // without consulting this field, so a roster shorter than the band would have the game read
    // past its own allocation -- and nothing about the call we are about to make would look wrong.
    if capacity < chr_set::BAND_END {
        return Err(format!(
            "the buddy roster holds {capacity} slots and the game's own spawner scans up to {}, \
             so calling it would read past the end of that array -- refusing rather than \
             resizing, because Seamless Co-op lives in this same ChrSet",
            chr_set::BAND_END
        ));
    }
    let Some(entries) = (unsafe { safe_read_usize(chr_set + chr_set::ENTRIES) }) else {
        return Err("the buddy roster's entry array did not read".to_owned());
    };
    if !unsafe { is_heap_aligned_ptr(entries) } {
        return Err("the buddy roster's entry array is not a plausible pointer".to_owned());
    }
    Ok((manager, capacity, entries))
}

/// Create the creature `spec` describes, on the game thread.
///
/// Returns the character and the roster slot it landed in, or the reason it did not happen.
/// A refusal has changed nothing; an `Ok` means a `ChrIns` now exists that this mod owns and must
/// eventually hand to [`despawn`].
pub(crate) fn spawn(spec: &SpawnSpec) -> Result<Spawned, String> {
    let (world_chr_man, capacity, entries) = buddy_roster()?;
    let Some(mut request) = SpawnRequest::new(spec) else {
        return Err(format!(
            "chr_id {} cannot be spelled as c%04d, so there is no model name to ask for",
            spec.chr_id
        ));
    };
    // The name pointer is into the block itself, so it can only be written once the block is where
    // the game will read it. This is that moment, and the check below is what makes "the block did
    // not move" a fact rather than an assumption about codegen.
    request.bind();
    let expected =
        request.as_ptr() as usize + chr_spawn_request::MODEL + chr_spawn_request::MODEL_BUFFER;
    if request.bound_name_pointer() != expected {
        return Err("the spawn request moved after its name pointer was bound".to_owned());
    }

    let Ok(address) = game_rva_named(SPAWN_DYNAMIC_CHR_RVA, "SPAWN_DYNAMIC_CHR_RVA") else {
        return Err(format!(
            "the spawn entry has no verified mapping for {} -- refusing rather than calling \
             whatever now occupies those bytes",
            er_game_base::game_build::describe_build()
        ));
    };
    // Safety: the address was resolved for the running build immediately above, and the signature
    // is the one the disassembly shows -- `MOV RDI,RCX; ADD RCX,0x10f90; CALL ...` with RDX passed
    // straight through, i.e. two pointer arguments in the Win64 order.
    let spawn_dynamic_chr: SpawnDynamicChrFn = unsafe { core::mem::transmute(address) };
    let returned = unsafe { spawn_dynamic_chr(world_chr_man as *mut c_void, request.as_ptr()) };
    // Nothing retains the block, which is the question worth answering because it holds a pointer
    // into ITSELF: `FUN_140492a90` formats the name into its own `DLAllocatedStr` and
    // `CreateCharacter` copies it into the `ChrInitData`, both synchronously, before this call
    // returns. So `request` is dead from here and nothing below may read it.
    //
    // There is deliberately no `drop(request)` saying so. `SpawnRequest` is a plain byte array with
    // no `Drop` impl, so the call would be a no-op that `clippy::drop_non_drop` rejects -- and a
    // reader who saw one would reasonably infer a destructor that does not exist. The comment
    // carries the fact; the code has nothing to do.

    if returned == 0 {
        return Err(format!(
            "the game's own spawner found no free slot: it scans buddy roster entries {}..{} and \
             all of them are taken",
            chr_set::BAND_FIRST,
            chr_set::BAND_END
        ));
    }
    if !unsafe { is_heap_aligned_ptr(returned) } {
        // Nothing to undo: a return value that is not a pointer means we cannot identify what, if
        // anything, was created, and removing a guess is worse than leaking one.
        return Err(format!(
            "the spawn returned 0x{returned:x}, which is not a plausible ChrIns"
        ));
    }
    let creature = Chr::new(returned);

    match locate(creature, capacity, entries) {
        Ok((slot, entry)) => Ok(Spawned {
            creature,
            slot,
            entry,
        }),
        Err(reason) => {
            // It exists and we cannot track it, which is the one case where despawning immediately
            // is better than keeping it: an untracked creature is one nothing will ever remove.
            despawn(creature);
            Err(reason)
        }
    }
}

/// Find the roster slot a freshly created character occupies, and prove it is in bounds.
///
/// Derived from the character's own `chrSetEntry` back-pointer rather than by scanning: the entry
/// is what `CreateCharacter` was handed and what the `ChrIns` constructor stores, so this is the
/// game's own answer. The three checks are the bounds proof -- the pointer is inside the array, it
/// is on a stride boundary, and the index is inside the band the game scans and below the capacity
/// that was read at runtime.
fn locate(creature: Chr, capacity: u32, entries: usize) -> Result<(u32, usize), String> {
    let Some(entry) = (unsafe { safe_read_usize(creature.address() + chr_ins::CHR_SET_ENTRY) })
    else {
        return Err("the spawned character's roster entry did not read".to_owned());
    };
    let Some(offset) = entry.checked_sub(entries) else {
        return Err("the spawned character's roster entry is below the entry array".to_owned());
    };
    if offset % chr_set::ENTRY_STRIDE != 0 {
        return Err(format!(
            "the spawned character's roster entry is {offset:#x} into the array, which is not a \
             multiple of the {:#x}-byte stride",
            chr_set::ENTRY_STRIDE
        ));
    }
    let Ok(slot) = u32::try_from(offset / chr_set::ENTRY_STRIDE) else {
        return Err("the spawned character's roster slot does not fit in a u32".to_owned());
    };
    // Against the band, not merely against the capacity. `capacity` (checked to be at least
    // `BAND_END` before the call) is the bound that makes later reads of `entries[slot]` safe; the
    // band is the bound the game's own loop promises, and it is the stronger of the two. A slot
    // outside 6..20 would be safe to read and would still mean this crate's model of
    // `FUN_140492a90` is wrong, which is worth refusing over rather than tracking.
    if !(chr_set::BAND_FIRST..chr_set::BAND_END).contains(&slot) {
        return Err(format!(
            "the spawned character landed in roster slot {slot}, outside the {}..{} band the \
             game's own spawner scans (the roster itself holds {capacity})",
            chr_set::BAND_FIRST,
            chr_set::BAND_END
        ));
    }
    if unsafe { safe_read_usize(entry + chr_set::ENTRY_CHR_INS) } != Some(creature.address()) {
        return Err(format!(
            "roster slot {slot} does not hold the character the spawn returned"
        ));
    }
    Ok((slot, entry))
}

/// Hand a creature this mod created back to the game.
///
/// Only ever for a creature this mod spawned, and only once. `RemoveChrIns` passes it to
/// `CSDelayDeleteMan`; calling it twice, or on a character the game already removed, queues a freed
/// `ChrIns` for a second destruction.
///
/// **`ChrCtrl+0x3b0` must already be clear.** The delayed delete runs `ChrCtrl::Unref`, which
/// DLPanics on a non-null override slot. The teardown ordering enforces that; this function does
/// not re-check it, because a despawn that silently declined would leave an orphan and say nothing.
pub(crate) fn despawn(creature: Chr) -> bool {
    let Ok(world_chr_man) = (unsafe { WorldChrMan::instance() }) else {
        return false;
    };
    let world_chr_man = core::ptr::from_ref(world_chr_man) as usize;
    let Ok(address) = game_rva_named(REMOVE_CHR_INS_RVA, "REMOVE_CHR_INS_RVA") else {
        return false;
    };
    // Prove the pointer still reads before handing it to a function that will dereference it and
    // call a virtual on it.
    if unsafe { safe_read_usize(creature.address()) }.is_none() {
        return false;
    }
    // Safety: resolved for the running build above; the signature is from the disassembly, whose
    // first two instructions are `TEST RDX,RDX; JZ` -- the character in RDX, the manager in RCX.
    let remove: RemoveChrInsFn = unsafe { core::mem::transmute(address) };
    unsafe { remove(world_chr_man as *mut c_void, creature.address()) };
    true
}

/// Evaluate one readiness gate against live memory.
///
/// `caps` is `layout::ene_dat_cap_offsets` for the running build; `None` makes
/// [`Gate::AssetsResident`] undecidable, which [`crate::spawn::readiness::Readiness::observe`]
/// treats as a skip rather than a failure.
///
/// Answers `Some(false)` for a read that did not work, not `None`: a pointer chain that stopped
/// resolving is a gate that is genuinely not satisfied, and reporting it as "cannot be decided on
/// this build" would let a broken spawn wait out its deadline reporting the wrong reason.
pub(crate) fn evaluate(
    spawned: &Spawned,
    caps: Option<(usize, usize)>,
    gate: Gate,
) -> Option<bool> {
    match gate {
        Gate::Registered => Some(
            unsafe { safe_read_usize(spawned.entry + chr_set::ENTRY_CHR_INS) }
                == Some(spawned.creature.address()),
        ),
        Gate::ChrResLoaded => Some(chr_res_loaded(spawned.creature)),
        Gate::AssetsResident => {
            let (primary, fallback) = caps?;
            Some(assets_resident(spawned.creature, primary, fallback))
        }
        // Both accessors carry their own structural proof: `chr_ctrl` checks that `ChrCtrl.owner`
        // points back at this `ChrIns`, and `real_manipulator` reads `ChrCtrl+0x18` and requires a
        // heap-aligned result. That is the pointer the thunk will forward to.
        Gate::Drivable => Some(spawned.creature.real_manipulator().is_some()),
    }
}

/// `ChrIns::IsInLoadedState`, as the field read it is: `3 <= chrRes->step < 6`.
fn chr_res_loaded(creature: Chr) -> bool {
    let Some(chr_res) = (unsafe { safe_read_usize(creature.address() + chr_ins::CHR_RES) }) else {
        return false;
    };
    if !unsafe { is_heap_aligned_ptr(chr_res) } {
        return false;
    }
    unsafe { safe_read_i32(chr_res + chr_res::STEP) }
        .is_some_and(|step| (chr_res::STEP_LOADED_FIRST..chr_res::STEP_LOADED_END).contains(&step))
}

/// `FUN_1404ca4a0(GetEneDat(chrIns)) != NULL`, inlined as the four loads it is.
///
/// The `EneDat` pointer is read through `safe_read_usize` and null-checked here, so this is safe to
/// evaluate on its own -- but it is still ordered after [`Gate::ChrResLoaded`], because the game's
/// own version does not null-check its argument and the gate order is the contract that keeps a
/// future caller from copying half of this.
fn assets_resident(creature: Chr, primary: usize, fallback: usize) -> bool {
    let Some(chr_res) = (unsafe { safe_read_usize(creature.address() + chr_ins::CHR_RES) }) else {
        return false;
    };
    let Some(ene_dat) = (unsafe { safe_read_usize(chr_res + chr_res::ENE_DAT) }) else {
        return false;
    };
    if !unsafe { is_heap_aligned_ptr(ene_dat) } {
        return false;
    }
    // The primary cap wins when it is there; the fallback is consulted only when it is null, which
    // is the order `FUN_1404ca4a0` itself uses.
    for cap_offset in [primary, fallback] {
        let Some(cap) = (unsafe { safe_read_usize(ene_dat + cap_offset) }) else {
            continue;
        };
        if !unsafe { is_heap_aligned_ptr(cap) } {
            continue;
        }
        if unsafe { safe_read_u8(cap + file_cap::LOAD_STATE) } != Some(file_cap::LOADED) {
            // A cap that exists but has not finished is the answer, not a reason to try the other
            // one: the game stops here too.
            return false;
        }
        return unsafe { safe_read_usize(cap + file_cap::FLVER_RES_CAP) }
            .is_some_and(|flver| flver != 0);
    }
    false
}
