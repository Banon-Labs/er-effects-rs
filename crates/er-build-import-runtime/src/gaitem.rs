//! `CS::GaitemLookupResult` -- the record every question about a live item instance goes through.
//!
//! # Why the record has a module rather than a local array of words
//!
//! It is twenty bytes, and the only field the ash-of-war answer depends on is the last one:
//! `itemId` at `+0x10`. Both readers of a worn armament ask exactly that field --
//! `GaitemLookupResult::canGemBeChanged` (0x1406741b0, reached through
//! `GetGemGaitemHandleFromWeapon`) decides whether the weapon may carry a gem at all, and
//! `GetSwordArtsParamIdForWeapon` turns it into an `EquipParamWeapon` row whose
//! `swordArtsParamId` is the armament's default skill.
//!
//! So a caller that reserves sixteen bytes hands the engine four bytes of somebody else's stack
//! as the item id -- and the resulting failure is silent, and constant. Zero passes both of the
//! engine's validity tests (`itemId & 0xF0000000 == 0`, and `itemId & 0x0FFFFFFF != 0x0FFFFFFF`),
//! so a zero there does not read as "nothing": it reads as `EquipParamWeapon` **row 0**, unarmed.
//! Every slot then resolves to the same row, `canGemBeChanged` says no, the gem override never
//! runs, and every slot reports one skill. Measured 2026-08-23: eight equipped armaments across
//! five distinct `ChrAsmSlot`s, one answer, `SwordArtsParam` 503 "Kick".
//!
//! Two callers in this crate were building this record independently and only one of them was
//! right. Declaring it once, with its size and its `itemId` offset asserted at compile time, is
//! what makes that class of mistake impossible rather than merely fixed.
//!
//! # The initial state is not zeroes
//!
//! [`GaitemLookupResult::from_handle`] calls the engine's own constructor instead of filling the
//! record here, because the constructor is where the initial state is defined and that state is
//! `gaItemIns = nullptr; itemId = -1` before the lookup runs. The `-1` carries as much weight as
//! the size: it is the sentinel both readers test for, so a handle that resolves to nothing
//! reports nothing -- whereas a zero left in that field names a param row.

use er_game_base::rva::{
    GET_SWORD_ARTS_PARAM_FOR_WEAPON_RVA as GET_SWORD_ARTS_PARAM_FOR_WEAPON,
    GET_WEAPON_GAITEM_HANDLE_BY_SLOT_RVA as GET_WEAPON_GAITEM_HANDLE_BY_SLOT,
};

/// `CS::GaitemLookupResult::GaitemLookupResult(GaitemLookupResult *out, uint *gaItemHandle)`.
///
/// Eleven instructions: `out->gaItemHandle = *handle`, `out->gaItemIns = nullptr`,
/// `out->itemId = -1`, then `GetGaitemInsByHandle(out, handle)`. Calling it rather than
/// reimplementing those four steps means the engine keeps owning what a fresh record looks like.
///
/// It takes no reference and there is nothing to release: the matching
/// `~GaitemLookupResult` (0x140672730) is a single `RET`, so a record is plain data and may be
/// dropped. That is worth stating, because the sibling `GaItemHandle` this crate mints in
/// `grant` does hold one and leaks the `CSGaitemImp` table if it is not destructed.
const GAITEM_LOOKUP_RESULT_CTOR_RVA: usize = 0x6726c0;

type LookupCtorFn = unsafe extern "system" fn(*mut GaitemLookupResult, *mut u32);
type GaitemHandleBySlotFn = unsafe extern "system" fn(usize, *mut u32, i32) -> *mut u32;
type SwordArtsForWeaponFn =
    unsafe extern "system" fn(*mut GaitemLookupResult, *mut SwordArtsParamLookupResult);

/// The engine's `GaitemLookupResult`: a handle, the instance it resolves to, and the
/// category-tagged item id that instance holds.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct GaitemLookupResult {
    /// `+0x00` -- the handle the record was built from.
    pub handle: u32,
    /// `+0x04` -- read by nothing this crate calls; present so the later offsets are real.
    _unknown_04: u32,
    /// `+0x08` -- `CSGaitemIns*`. Zero means the handle named nothing.
    pub instance: u64,
    /// `+0x10` -- the category-tagged item id, or `u32::MAX` when the lookup found nothing.
    pub item_id: u32,
}

// The three facts the whole module exists to hold: `itemId` really is the last field, it really
// is at +0x10, and the record we hand the engine really is at least as long as the one it writes.
const _: () = assert!(core::mem::offset_of!(GaitemLookupResult, instance) == 0x08);
const _: () = assert!(core::mem::offset_of!(GaitemLookupResult, item_id) == 0x10);
const _: () = assert!(core::mem::size_of::<GaitemLookupResult>() >= 0x14);

/// The engine's `SwordArtsParamLookupResult`: a row id and the row itself.
///
/// Sixteen bytes with the row pointer at `+0x08`, which is why it is a typed record rather than
/// four `u32`s -- the four-word form happened to be long enough, and being accidentally long
/// enough is what the sibling record above proves is not a property worth relying on.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct SwordArtsParamLookupResult {
    param_id: u32,
    _pad_04: u32,
    param_row: u64,
}

const _: () = assert!(core::mem::size_of::<SwordArtsParamLookupResult>() == 0x10);
const _: () = assert!(core::mem::offset_of!(SwordArtsParamLookupResult, param_row) == 0x08);

impl GaitemLookupResult {
    /// Resolve `handle` into a record, or `None` when it names no live instance.
    ///
    /// # Safety
    ///
    /// Game thread; `module_base` the loaded image base.
    pub unsafe fn from_handle(module_base: usize, handle: u32) -> Option<Self> {
        if handle == 0 || handle == u32::MAX {
            return None;
        }
        // Resolved for the running build, not added blind: this is a direct call into game code
        // at a 1.16.2 address, and on a build that moved it that is a control transfer into
        // whatever now occupies those bytes. `None` means the handle cannot be resolved, which is
        // the answer this function already has a shape for.
        let ctor = crate::native::resolve(
            module_base,
            GAITEM_LOOKUP_RESULT_CTOR_RVA,
            "GaitemLookupResult constructor",
        )?;
        // Safety: resolved for the running build immediately above.
        let ctor: LookupCtorFn = unsafe { core::mem::transmute(ctor) };
        // The constructor reads the handle through a pointer, so it needs a place to live. The
        // fields below are overwritten by the constructor; they are named rather than zeroed
        // wholesale so a future field cannot be added without a value being chosen for it.
        let mut handle = handle;
        let mut record = Self {
            handle: 0,
            _unknown_04: 0,
            instance: 0,
            item_id: u32::MAX,
        };
        // Safety: both destinations are ours and outlive the call, and the record is the length
        // the engine writes (asserted above).
        unsafe { ctor(&raw mut record, &raw mut handle) };
        (record.instance != 0).then_some(record)
    }

    /// The `SwordArtsParam` row this instance actually carries, gem included.
    ///
    /// `GetSwordArtsParamForWeapon` reads the armament's own `EquipParamWeapon.swordArtsParamId`
    /// first and then OVERRIDES it from the gem mounted in the instance's slot 0 -- but only when
    /// `EquipParamWeapon::canGemBeChanged` says the armament takes ashes at all. So a `Some` here
    /// is "what this weapon's skill is", not "what gem is on it": a unique armament reports its
    /// built-in skill, and a gem stored on a weapon that cannot change gems is ignored.
    ///
    /// # Safety
    ///
    /// Game thread; `module_base` the loaded image base and this record freshly resolved.
    pub unsafe fn sword_arts_id(&mut self, module_base: usize) -> Option<u32> {
        // Resolved for the running build; `None` is "this armament's skill is unknown", which
        // is already this function's answer for an armament that has none.
        let arts_for_weapon = crate::native::resolve(
            module_base,
            GET_SWORD_ARTS_PARAM_FOR_WEAPON,
            "GetSwordArtsParamForWeapon",
        )?;
        // Safety: resolved for the running build immediately above.
        let arts_for_weapon: SwordArtsForWeaponFn =
            unsafe { core::mem::transmute(arts_for_weapon) };
        let mut arts = SwordArtsParamLookupResult::default();
        // Safety: both records are ours and are the length the engine writes.
        unsafe { arts_for_weapon(&raw mut *self, &raw mut arts) };
        let param_id = arts.param_id;
        // Zero is an answer, not a blank. `SwordArtsParam` row 0 is `No Skill`, the ash that
        // takes a weapon's innate skill away, so folding it into `None` reports a shield that
        // carries it as having no ash at all -- and the import then scores its own correct mount
        // as a failure. Only `u32::MAX` means the engine had nothing to say.
        (param_id != u32::MAX).then_some(param_id)
    }
}

/// `GaitemLookupResult::GetGemGaitemHandleFromWeapon` -- the handle of the gem mounted on this
/// armament, or zero when it has none.
///
/// Reached through the engine rather than by walking `CSGemSlotTable` here: the function checks
/// `canGemBeChanged` and the instance pointer first, so an armament that takes no gem answers zero
/// instead of reading a slot table that is not there.
const GET_GEM_HANDLE_FROM_WEAPON_RVA: usize = 0x673e30;

type GemHandleFromWeaponFn =
    unsafe extern "system" fn(*mut GaitemLookupResult, *mut u32) -> *mut u32;

impl GaitemLookupResult {
    /// The `EquipParamGem` row of the gem mounted on this armament, or `None` for none.
    ///
    /// The gem's own item id carries the goods-style category nibble, so the row is its low 28
    /// bits -- the same mask every other id in this crate is read through.
    ///
    /// # Safety
    ///
    /// Game thread; `module_base` the loaded image base, and `self` a record the engine filled.
    pub unsafe fn mounted_gem_row(&mut self, module_base: usize) -> Option<u32> {
        // Resolved for the running build; `None` is "no gem", which is this function's own
        // answer for an armament that carries none.
        let from_weapon = crate::native::resolve(
            module_base,
            GET_GEM_HANDLE_FROM_WEAPON_RVA,
            "GaitemLookupResult::GetGemGaitemHandleFromWeapon",
        )?;
        // Safety: resolved for the running build immediately above.
        let from_weapon: GemHandleFromWeaponFn = unsafe { core::mem::transmute(from_weapon) };
        let mut handle = 0u32;
        // Safety: our own destination; the engine writes one `uint` through it and zeroes it when
        // the armament carries no gem.
        unsafe { from_weapon(&raw mut *self, &raw mut handle) };
        if handle == 0 || handle == u32::MAX {
            return None;
        }
        // Safety: the handle is the engine's own.
        let gem = unsafe { GaitemLookupResult::from_handle(module_base, handle) }?;
        let row = gem.item_id & 0x0FFF_FFFF;
        (row != 0 && row != 0x0FFF_FFFF).then_some(row)
    }
}

/// The gaitem handle of the armament worn in `slot`, or `None` when the slot is empty.
///
/// `slot` is a `ChrAsmSlot` and the engine validates it as `(uint)(slot + 6) < 0x12`, i.e. the
/// signed range `-6..=11`: the six weapon positions, the four ammo positions, and the negative
/// "whichever hand is active" selectors. It bottoms out in
/// `ChrAsm::GetEquipmentGaitemHandleBySlot`, a single `chrAsm->equipmentGaItemHandles[slot]` --
/// so this is genuinely per-slot, and the same numbering `GetParamIdInSlot` answers in.
///
/// # Safety
///
/// Game thread; `module_base` the loaded image base and `player` a live `PlayerIns*`.
pub unsafe fn worn_weapon_handle(module_base: usize, player: usize, slot: i32) -> Option<u32> {
    // Resolved for the running build; `None` is "the slot is empty", which is this function's
    // own answer for a hand holding nothing.
    let handle_by_slot = crate::native::resolve(
        module_base,
        GET_WEAPON_GAITEM_HANDLE_BY_SLOT,
        "ChrAsm::GetEquipmentGaitemHandleBySlot",
    )?;
    // Safety: resolved for the running build immediately above.
    let handle_by_slot: GaitemHandleBySlotFn = unsafe { core::mem::transmute(handle_by_slot) };
    let mut handle = 0u32;
    // Safety: our own destination; the engine writes exactly one `uint` through it, and zeroes
    // it first when the slot is out of its own range.
    unsafe { handle_by_slot(player, &raw mut handle, slot) };
    (handle != 0 && handle != u32::MAX).then_some(handle)
}
