//! Handing the build's items to the player, through the game's own inventory call.
//!
//! # Why not the community `ItemGib` table
//!
//! The Cheat Engine workflow this replaces writes a 16-byte record array and calls a routine
//! found by signature scan. The game already exports the operation:
//! `CS::EquipGameData::AddInventoryEquipByItemId` is what the engine itself uses, it resolves
//! the gaitem handle through `CSGaitemImp`, and it takes the same category-tagged item id the
//! catalog produces. Calling it means the engine owns the mutation, not us.
//!
//! # Why an armament does not go through that call
//!
//! Because two thirds of an armament are not in its item id, and that call has nowhere to put
//! them:
//!
//! * `AddInventoryEquipByItemId` -> `GetGaitemHandleByItemId` mints a bare weapon gaitem. There
//!   is no gem in it, so the weapon carries whatever skill its `EquipParamWeapon` row names --
//!   for an infused row, usually none at all.
//! * `AddInventoryEquip` -> `EquipInventoryData::InsertItem(..., reinforcement = 0)` passes a
//!   hard-coded zero, and `InsertItem` writes it straight onto the instance through
//!   `GaitemLookupResult::SetReinforcement`. Every weapon it inserts is +0 until something puts
//!   the level back.
//!
//! The level is put back in both places the engine keeps it: folded into the minted item id
//! (`er_build_import_core::plan::armament_item_id`, the half the player actually sees) and written to
//! the instance field afterwards, since `InsertItem` has just zeroed that one.
//!
//! So an armament is minted the way the engine mints one that drops out of the world already
//! carrying an ash -- `EquipParamCustomWeapon` goes through
//! `CSGaitemImp::GetGaItemHandleWeaponWithGem(csGaitem, &handle, weaponId, gemId)` -- and its
//! reinforcement is written onto the instance afterwards through the same virtual the engine's
//! own `InsertItem` uses. That is why [`Grant::weapon_skill`] and [`Grant::reinforce_lv`] finally
//! have somewhere to go; before this they were computed, logged as resolved, and dropped.
//!
//! # Why this runs on the game thread
//!
//! It mutates the player's inventory and touches the `CSGaitemImp` singleton. Doing that from
//! the fetch worker would race the game's own inventory code. The caller must invoke
//! [`grant_all`] from a `CSTaskImp` task.
//!
//! # Why every grant is read back
//!
//! `AddInventoryEquipByItemId` returns an int this code does not pretend to understand, and a
//! call that "succeeded" is not evidence the item exists. Each grant is confirmed with
//! `EquipInventoryData::GetQuantityByItemId`, so the log reports what the inventory actually
//! holds rather than how many calls were made. Armaments get a second read-back, because
//! quantity cannot see an ash: the minted instance is asked for its own arts id and reinforcement
//! through `GetSwordArtsParamForWeapon` / `GaitemLookupResult::GetReinforcement`.

use er_build_import_core::plan::{Grant, NO_SKILL, armament_item_id};

/// `CS::EquipGameData::AddInventoryEquipByItemId(egd, int *itemId, u32 amount,
/// bool updateTrophyStats, bool updateAutoEquip) -> int`.
const ADD_INVENTORY_EQUIP_BY_ITEM_ID_RVA: usize = 0x246840;
/// `CS::EquipGameData::AddInventoryEquip(egd, uint *gaItemHandle, u32 amount,
/// bool updateTrophyStats, bool updateAutoEquip) -> int` -- returns the inventory index.
///
/// The half of the call above that takes a handle somebody else minted, which is the only way to
/// hand the inventory an instance that already carries a gem.
const ADD_INVENTORY_EQUIP_RVA: usize = 0x246480;
/// `CS::CSGaitemImp::GetGaItemHandleWeaponWithGem(CSGaitemImp*, uint *out, int weaponId,
/// int gemId)` (Ghidra `FUN_140671ce0`).
///
/// Mints a weapon gaitem and, when `gemId >= 0`, a gem gaitem mounted in its slot 0 -- the exact
/// pair `EquipParamCustomWeapon` drops go through. A negative `gemId` skips the gem entirely, so
/// this one entry point covers armaments with and without an ash.
const GET_GAITEM_HANDLE_WEAPON_WITH_GEM_RVA: usize = 0x671ce0;
/// `GaItemHandle::~GaItemHandle(uint *handle)`.
///
/// Releases the reference the mint took. `CSGaitemImp` is a bounded refcounted table (0x1400
/// entries) and every native caller of the mint destructs its local handle once the inventory has
/// taken its own reference; skipping it leaks table entries until the free queue is exhausted,
/// which this repository has already crashed on once.
const GAITEM_HANDLE_DTOR_RVA: usize = 0x682480;
/// `CS::GaitemLookupResult::SetReinforcement(GaitemLookupResult*, int)` -- `vtable + 0x20` on the
/// resolved instance, the same virtual `EquipInventoryData::InsertItem` calls.
const SET_REINFORCEMENT_RVA: usize = 0x672e00;
/// `CS::GaitemLookupResult::GetReinforcement(GaitemLookupResult*) -> int` -- `vtable + 0x18`.
const GET_REINFORCEMENT_RVA: usize = 0x672740;
// Declared once in `er-game-base::rva`; `er-better-refills` and `equip_native` read the
// same values from there.
use er_game_base::rva::{
    GET_EQUIP_INVENTORY_DATA_RVA as GET_EQUIP_INVENTORY_DATA,
    GET_QUANTITY_BY_ITEM_ID_RVA as GET_QUANTITY_BY_ITEM_ID, GLOBAL_CSGAITEM_RVA,
};

use crate::gaitem::GaitemLookupResult;

/// `GameDataMan::main_player_game_data`, read as a raw pointer.
///
/// The typed upstream field is an `OwnedPtr`, which asserts non-null; before a character is
/// loaded this slot really is null, so it is read as a plain pointer and checked.
///
/// The offset itself is measured, not read off that declaration: `GetMainPlayerGameData`
/// (`0x140e9fc30`) is `mov rax,[rip+GLOBAL_GameDataMan] ; mov rax,[rax+0x8] ; ret`, and the
/// `GameDataMan` constructor pairs 351/351 with zero moved offsets across 1.16.2/1.17. Full
/// witness in `storage::GAME_DATA_MAN_PLAYER_OFFSET`.
const GAME_DATA_MAN_PLAYER_OFFSET: usize = 0x08;
/// `PlayerGameData::equipGameData`, held by value.
const PLAYER_GAME_DATA_EQUIP_OFFSET: usize = 0x2b0;

/// The high nibble every item id carries its category in.
const ITEM_CATEGORY_MASK: u32 = 0xF000_0000;
/// The row id under that nibble.
const ITEM_ROW_MASK: u32 = 0x0FFF_FFFF;
/// The category nibble that means "armament" -- the only kind that carries a gem or a level.
const WEAPON_CATEGORY: u32 = 0x0000_0000;

type AddInventoryFn = unsafe extern "system" fn(usize, *const i32, u32, bool, bool) -> i32;
type AddInventoryByHandleFn = unsafe extern "system" fn(usize, *mut u32, u32, bool, bool) -> i32;
type MintWeaponFn = unsafe extern "system" fn(usize, *mut u32, i32, i32) -> *mut u32;
type HandleDtorFn = unsafe extern "system" fn(*mut u32);
type GetInventoryFn = unsafe extern "system" fn(usize) -> usize;
type GetQuantityFn = unsafe extern "system" fn(usize, *const i32) -> i32;
type SetReinforcementFn = unsafe extern "system" fn(*mut GaitemLookupResult, i32);
type GetReinforcementFn = unsafe extern "system" fn(*mut GaitemLookupResult) -> i32;

/// What one armament actually came out as, read off the instance the grant minted.
///
/// The whole point of this record is that it is measured, not assumed: `arts_id` is what
/// `GetSwordArtsParamForWeapon` says the new weapon holds, not what the plan asked for.
#[derive(Debug, Clone)]
pub struct ArmamentOutcome {
    /// The armament's display name.
    pub label: String,
    /// Category-tagged armament id, affinity included.
    pub item_id: u32,
    /// `EquipParamGem` row the plan asked to mount, or `None` for "no ash".
    pub wanted_gem: Option<u32>,
    /// `SwordArtsParam` row the minted instance reports, or `None` when it reports nothing.
    pub arts_id: Option<u32>,
    /// Upgrade level actually requested of the game: the build's level, clamped to one this
    /// armament has a `ReinforceParamWeapon` row for.
    pub wanted_level: u16,
    /// Upgrade level read back off the instance.
    pub level: i32,
    /// Inventory index the game filed it under; `-1` when the insert was refused.
    pub inventory_index: i32,
    /// The `GaItemHandle` this armament was minted as, or `0` when nothing usable was minted.
    ///
    /// The only name that distinguishes this copy from its twins. An ash lives on the gaitem
    /// instance, not in the item id, so several copies of one armament carrying different ashes
    /// all share an item id -- and `EquipInventoryData::GetItemInventoryIdx` (0x14024c560) keys
    /// purely on that id: its body is `if (*itemId != -1) GetItemIndex(itemsData, itemId)`, and
    /// `InventoryItemsData::InsertItemIntoLookupMap` keeps the lowest index for a repeated id.
    /// One id, one answer, forever the same copy. Carrying the handle forward is what lets the
    /// equip ask `GetItemIndexByGaitemHandle` (0x14024c460) instead, which scans the entries for
    /// the one whose handle matches and therefore can separate them.
    ///
    /// # Why the number outlives our own reference
    ///
    /// `grant_armament` destructs its local handle before returning, as every native caller
    /// does. That is a refcount decrement, not an invalidation: `AddInventoryEquip` took the
    /// inventory's own reference first, so the entry stays live for exactly as long as the item
    /// stays in the inventory -- which is longer than the equip pass. When the insert is refused
    /// the inventory took no reference and our release frees the table entry, so this is left
    /// zero rather than left dangling; `RemoveCSGaitemIns` bumps the handle's generation bits on
    /// free, so even a stale handle fails the lookup closed rather than naming a later item.
    pub handle: u32,
}

impl ArmamentOutcome {
    /// Whether the instance came out carrying an ash at all.
    pub fn has_ash(&self) -> bool {
        self.arts_id.is_some()
    }
}

/// One grant that landed, but at fewer than the number the build asked for.
///
/// A category of its own because it is neither of the two the report used to have. `missing`
/// means the item is not there at all -- a refusal, and loud. This is the quiet failure: the add
/// ran, the engine reported nothing, and the inventory holds three of the five that were asked
/// for. `EquipInventoryData::InsertItem` and `UpdateQuantity` both clamp with a bare
/// `if (max < amount) amount = max;`, so the shortfall exists only in the difference between two
/// numbers, and it exists only if something keeps both.
#[derive(Debug, Clone)]
pub struct Short {
    /// The id the read-back asked about.
    pub item_id: u32,
    /// What the item is, for the log line.
    pub label: String,
    /// How many the build asked for.
    pub requested: u32,
    /// How many the inventory actually holds, counting every id under the same name.
    pub held: u32,
    /// The pot group that capped this, when one did.
    ///
    /// Short and short-because-of-POTS are different reports, and only one of them is a defect.
    /// A build asking for a Fire Pot's declared maximum on a character carrying three Cracked
    /// Pots will come up short every single time, correctly and permanently -- the group's
    /// ceiling is the number of vessels, and no importer can raise it past what the player owns.
    /// Without this the line is indistinguishable from a grant the engine refused, and a reader
    /// chasing the second would keep finding the first.
    pub pot_group: Option<u8>,
}

/// One grant's demand, kept so the read-back can compare rather than merely look.
struct Requested {
    /// The id the inventory will file it under, which for an armament includes its level.
    item_id: u32,
    /// Other ids the same name resolves to; empty for an armament (see the call site).
    also_known_as: Vec<u32>,
    /// How many the build asked for.
    requested: u32,
    /// What the item is, for the log line.
    label: String,
    /// The pot group capping it, carried so a shortfall can name its cause. See [`Short`].
    pot_group: Option<u8>,
}

impl Requested {
    /// The demand a minted armament satisfies, keyed on the id it actually came out under.
    fn armament(outcome: &ArmamentOutcome, requested: u32) -> Self {
        Self {
            item_id: outcome.item_id,
            also_known_as: Vec::new(),
            requested,
            label: outcome.label.clone(),
            // An armament is not a goods row and has no pot group; the mint path never sets one.
            pot_group: None,
        }
    }
}

/// `EquipGameData.lastItemAddResult`, at `+0x3fc` in a `0x4b0`-byte `EquipGameData`.
///
/// Confirmed on both images rather than carried over: the 1.16.2 dump names the field at that
/// offset, and `AddInventoryEquip`'s entry sequence writes it as
/// `XOR ESI,ESI / MOV dword ptr [RCX + 0x3fc],ESI` -- the bytes `33 f6 89 b1 fc 03 00 00`, which
/// occur at `0x1402464b8` in `eldenring-deobf.bin` (1.16.2) and at the same address in
/// `eldenring-deobf-1.17.bin`.
const LAST_ITEM_ADD_RESULT_OFFSET: usize = 0x3fc;

/// `lastItemAddResult` when the engine flagged nothing.
///
/// Not a delivery receipt. `AddInventoryEquip` zeroes the field on entry and only ever writes it
/// again to refuse; the pot-group clamp happens further in, inside `InsertItem` /
/// `UpdateQuantity`, which reduce the amount and return normally. So a zero here means "no
/// refusal", and the quantity read-back remains the only evidence that the requested number
/// arrived.
const ADD_RESULT_OK: i32 = 0;
/// `lastItemAddResult` when the stackable merge was refused: the item is unique and one is
/// already held, or `EquipInventoryData::AdjustQuantityBy` reported failure. Nothing was added.
const ADD_RESULT_MERGE_REFUSED: i32 = 2;
/// `lastItemAddResult` when `EquipInventoryData::InsertItem` returned a negative index -- no free
/// entry, or the insert was rejected. Nothing was added.
const ADD_RESULT_INSERT_FAILED: i32 = 4;

/// Read `EquipGameData.lastItemAddResult`.
///
/// # Safety
///
/// `egd` must be a live `EquipGameData*`; the read itself is fault-checked.
unsafe fn last_item_add_result(egd: usize) -> Option<i32> {
    // Safety: a fault-checked four-byte read at a verified offset inside a live object.
    unsafe { er_game_base::mem::safe_read_i32(egd + LAST_ITEM_ADD_RESULT_OFFSET) }
}

/// The engine's meaning for a `lastItemAddResult` value, for the log.
fn describe_add_result(result: i32) -> &'static str {
    match result {
        ADD_RESULT_OK => "no refusal recorded",
        ADD_RESULT_MERGE_REFUSED => {
            "the stack merge was refused -- unique item already held, or AdjustQuantityBy failed"
        }
        ADD_RESULT_INSERT_FAILED => "InsertItem refused: no inventory entry was taken",
        _ => "an undocumented result code",
    }
}

/// What actually ended up in the inventory.
#[derive(Debug, Default)]
pub struct GrantOutcome {
    /// Grants the plan asked for.
    pub attempted: usize,
    /// Grants confirmed present at the requested quantity afterwards, by reading the inventory
    /// back.
    ///
    /// The comparison is against `Grant::quantity`, not against zero. Before 2026-08-31 this
    /// counted anything the inventory held at all, so a build that asked for five Fire Pots and
    /// received three -- the ordinary result of a full pot group -- reported
    /// `GRANTED: n/n confirmed present, 0 missing`.
    pub confirmed: usize,
    /// Item ids that were requested but could not be found afterwards at all.
    pub missing: Vec<u32>,
    /// Grants that landed at fewer than the requested number. See [`Short`].
    pub short: Vec<Short>,
    /// Items moved out of the storage box back into the inventory, rather than minted anew.
    pub pulled_from_storage: u32,
    /// Items moved into the storage box to free a pot group's capacity.
    pub deposited_to_storage: u32,
    /// Consumables destroyed to free a pot group, because the storage box refused them.
    ///
    /// The only irreversible number this importer produces, kept apart from
    /// [`GrantOutcome::deposited_to_storage`] for exactly that reason: a deposit and a discard
    /// both read as "the item is no longer in your pockets", and only one of them can be walked
    /// back. A report that folded them together would let the destructive case hide inside the
    /// harmless one.
    pub discarded_to_free_pots: u32,
    /// Whether the storage box was reachable at all this run. `false` means both storage rungs
    /// were inert, which a reader has to know before concluding anything from the two counts.
    pub storage_available: bool,
    /// Grants skipped because the character already had at least the quantity asked for.
    ///
    /// Counted rather than inferred: this is the difference between "the build was already
    /// satisfied here" and "the grant silently did nothing", which the report must not blur.
    pub already_held: usize,
    /// Per-armament read-back, in grant order.
    pub armaments: Vec<ArmamentOutcome>,
    /// How many of those were already in the pockets and were adopted rather than minted.
    ///
    /// The number that says a re-import is idempotent. It used to be structurally zero: every
    /// armament in the build was minted on every import, so importing one build twice handed out
    /// every weapon in it twice.
    pub armaments_adopted: usize,
}

impl GrantOutcome {
    /// Armaments that asked for an ash and came out without one.
    pub fn armaments_missing_their_ash(&self) -> impl Iterator<Item = &ArmamentOutcome> {
        self.armaments
            .iter()
            .filter(|arm| arm.wanted_gem.is_some() && !arm.has_ash())
    }
}

/// The live `EquipGameData*`, or `None` before a character is loaded.
///
/// # Safety
///
/// Must be called on the game thread; the pointers it walks are engine-owned.
pub unsafe fn equip_game_data() -> Option<usize> {
    use eldenring::cs::GameDataMan;
    use fromsoftware_shared::FromStatic;

    let gdm = GameDataMan::instance_ptr().ok()? as usize;
    if gdm == 0 {
        return None;
    }
    // Safety: the layout is `GameDataMan + 0x08 -> PlayerGameData*`, and the value is checked
    // for null rather than assumed present -- it genuinely is null at the title screen.
    let pgd = unsafe { *((gdm + GAME_DATA_MAN_PLAYER_OFFSET) as *const usize) };
    if pgd == 0 {
        return None;
    }
    Some(pgd + PLAYER_GAME_DATA_EQUIP_OFFSET)
}

/// `WorldChrMan::mainPlayerIns`.
const WORLD_CHR_MAN_MAIN_PLAYER_INS: usize = 0x1e508;

/// Whether the player exists **in the world**, not merely in save data.
///
/// This is deliberately stricter than "PlayerGameData is non-null". That pointer goes live
/// during the loading screen, long before a `PlayerIns` exists -- and the equip path's own
/// gate (`FUN_140788a90`) dereferences `WorldChrMan->mainPlayerIns` unconditionally. Running
/// the import on the weaker check crashed the game with a null-deref inside that gate
/// (access violation at game+0x788adb, 2026-08-22). Granting only needs `EquipGameData`, but
/// there is no reason to grant into a character that cannot yet wear anything, so the whole
/// import waits for this.
///
/// # Safety
///
/// Game thread only.
pub unsafe fn player_present() -> bool {
    use eldenring::cs::WorldChrMan;
    use fromsoftware_shared::FromStatic;

    // Safety: delegated; the helper is itself fault-checked.
    if unsafe { equip_game_data() }.is_none() {
        return false;
    }
    let Ok(wcm) = WorldChrMan::instance_ptr() else {
        return false;
    };
    let wcm = wcm as usize;
    if wcm == 0 {
        return false;
    }
    // Safety: reading one pointer at a verified offset inside a live singleton, and checking
    // it for null rather than assuming it is populated.
    let player = unsafe { *((wcm + WORLD_CHR_MAN_MAIN_PLAYER_INS) as *const usize) };
    player != 0
}

/// Whether a grant is an armament, i.e. the only kind that can carry a gem or a level.
///
/// # The category nibble is necessary and not sufficient
///
/// This used to be the nibble test alone, and that was correct only while ammunition was
/// unimplemented. Arrows and bolts are `EquipParamWeapon` rows and carry the same
/// [`WEAPON_CATEGORY`] nibble, so the nibble by itself now answers "armament" for a quiver of
/// Bone Arrows -- and that answer sends them down [`grant_armament`], which mints one
/// `GaItemHandle`, writes an upgrade level into the instance and mounts a gem. A stack of 99
/// arrows is not one instance, and none of those three things exist for ammunition.
///
/// So the deciding fact travels on the grant ([`Grant::armament`]), set where the catalog kind is
/// still known. The nibble is kept as the second half of the test because the two must agree: a
/// grant flagged as an armament whose id is not in the weapon category is a plan bug, and taking
/// the mint path on it would mint from the wrong table.
fn is_armament(grant: &Grant) -> bool {
    grant.armament && grant.item_id & ITEM_CATEGORY_MASK == WEAPON_CATEGORY
}

/// The `EquipParamGem` row a grant's `weapon_skill` names, or `None` for "no ash".
///
/// The sentinel and the category nibble are both checked: a `weapon_skill` that is neither
/// [`NO_SKILL`] nor a gem-tagged id is a value this code does not understand, and mounting the
/// low bits of it anyway is how a wrong-table id gets silently mounted as a gem.
fn gem_row(grant: &Grant) -> Option<u32> {
    gem_row_of(grant.weapon_skill)
}

/// The `EquipParamGem` row a `weapon_skill` field names, or `None` for "no ash".
///
/// Public because the equip side asks the same question of the same encoding when it decides
/// which minted copy belongs in a slot -- and two copies of that rule would be two chances for
/// the grant and the equip to disagree about what an armament is, which is precisely the
/// ambiguity the handle threading exists to remove.
pub fn gem_row_of(weapon_skill: u32) -> Option<u32> {
    if weapon_skill == NO_SKILL {
        return None;
    }
    if weapon_skill & ITEM_CATEGORY_MASK != er_build_import_core::plan::GEM_ITEM_CATEGORY {
        return None;
    }
    Some(weapon_skill & ITEM_ROW_MASK)
}

/// Give the player every item in `grants`, then read the inventory back to confirm.
///
/// # Safety
///
/// Must run on the game thread, with a character loaded (see [`player_present`]) and
/// `module_base` the loaded image base.
pub unsafe fn grant_all(module_base: usize, grants: &[Grant]) -> GrantOutcome {
    let mut outcome = GrantOutcome {
        attempted: grants.len(),
        ..GrantOutcome::default()
    };

    let Some(egd) = (unsafe { equip_game_data() }) else {
        outcome.missing = grants.iter().map(|grant| grant.item_id).collect();
        return outcome;
    };

    // Resolved for the running build. All three are direct calls into game code at 1.16.2
    // addresses; on 1.17 an unresolved one transfers control into whatever moved there. Refusing
    // takes the same path a null EquipGameData already takes -- every item reported missing,
    // which is exactly what happens when nothing is granted.
    let natives = crate::native::resolve_all(
        module_base,
        [
            (
                ADD_INVENTORY_EQUIP_BY_ITEM_ID_RVA,
                "AddInventoryEquipByItemId",
            ),
            (
                GET_EQUIP_INVENTORY_DATA,
                "CS::EquipGameData::GetEquipInventoryData",
            ),
            (GET_QUANTITY_BY_ITEM_ID, "GetQuantityByItemId"),
        ],
    );
    let Ok([add, get_inventory, get_quantity]) = natives else {
        outcome.missing = grants.iter().map(|grant| grant.item_id).collect();
        return outcome;
    };
    // Safety: all three addresses were resolved for the running build immediately above.
    let add: AddInventoryFn = unsafe { core::mem::transmute(add) };
    // Safety: as above.
    let get_inventory: GetInventoryFn = unsafe { core::mem::transmute(get_inventory) };
    // Safety: as above.
    let get_quantity: GetQuantityFn = unsafe { core::mem::transmute(get_quantity) };

    // Read once: the clamp needs the whole `ReinforceParamWeapon` id set, and every armament in
    // the plan asks it the same question.
    let levels = crate::catalog::ReinforceLevels::read();

    // What each grant asked for, and the IDS to ask about afterwards.
    //
    // The id is not always `grant.item_id`: for an armament the upgrade level lives in the id's
    // last two digits, so a +25 weapon is a different id from the +0 one the plan names, and a
    // read-back that asked about the planned id would report every armament missing.
    //
    // The requested quantity is carried alongside it because the confirmation is a comparison,
    // not a presence test. Until 2026-08-31 the check was `if held > 0 { confirmed += 1 }`, which
    // reports `GRANTED: n/n confirmed, 0 missing` for a build that asked for five Fire Pots and
    // got three -- the exact case the pot cap produces, silently, on a character whose Cracked
    // Pots are already spoken for. A number that cannot go down is not an instrument.
    let mut confirms: Vec<Requested> = Vec::with_capacity(grants.len());

    // Read the inventory before granting, so a grant can ask what is already held.
    //
    // A build says "this character HAS these items", not "add these items". Those read the same
    // on an empty character and differently on every other one: importing twice, or importing
    // onto a character that already owns something, used to add a second copy every time. For a
    // stackable that merely inflates a count; for the Flask of Wondrous Physick -- unique, one
    // per character -- the player ends up carrying two flasks, which is what surfaced this.
    //
    // A zero here is not fatal: it means the quantity cannot be measured, and granting
    // unconditionally is the old behaviour, which is wrong less often than granting nothing.
    // Safety: game thread, live EquipGameData.
    let inventory_before = unsafe { get_inventory(egd) };

    // The storage box, and what the game POT-caps. Both are optional and both fail the same way:
    // the ladder below loses a rung and says so, rather than the grant failing.
    //
    // Safety: game thread, `egd` live, `inventory_before` the carried inventory it owns.
    let storage = if inventory_before == 0 {
        None
    } else {
        unsafe { crate::storage::Storage::open(module_base, egd, inventory_before) }
    };
    outcome.storage_available = storage.is_some();
    let pots = crate::catalog::PotGroups::read();
    if storage.is_none() {
        crate::log_line(
            "[build-import] no storage box this session: an item already in the box cannot be \
             moved back, so it will be granted as a new copy, and a pot-group conflict cannot be \
             cleared",
        );
    }

    // Every ID the build wants, so the pot rung never deposits something the build asked for.
    // Alternates included: a name that resolves to several rows wants all of them left alone.
    let wanted_ids: std::collections::BTreeSet<u32> = grants
        .iter()
        .flat_map(|grant| std::iter::once(grant.item_id).chain(grant.also_known_as.iter().copied()))
        .collect();

    // Armaments the character already carries, so a re-import adopts them instead of minting a
    // second copy of each. Everything else in this loop reconciles to a target -- "the build wants
    // three Fire Pots, you have two, here is one" -- and armaments alone did not: they were minted
    // unconditionally, so importing the same build twice handed out every weapon in it twice.
    // Reported 2026-09-10 after repeated imports while tuning one build.
    //
    // Safety: game thread, character in the world; the pool is read once and only from entries
    // the inventory already holds.
    let mut pool = unsafe { CarriedArmaments::read(module_base, storage.as_ref()) };

    for grant in grants {
        let armament = is_armament(grant);
        if armament {
            let full_id = armament_item_id(
                grant.item_id,
                levels.game_level_for(
                    grant.item_id & ITEM_ROW_MASK,
                    grant.reinforce_lv,
                    grant.upgrade_is_character_default,
                ),
            );
            // The skill the build asks this armament to carry, as the row the instance will
            // report. `None` means the build named no ash, and then any copy at the right id
            // qualifies -- the build expressed no opinion about the skill.
            let wanted_arts =
                gem_row_of(grant.weapon_skill).and_then(crate::catalog::arts_row_for_gem);
            if let Some(adopted) = pool.claim(module_base, full_id, wanted_arts) {
                let outcome_of_adoption = ArmamentOutcome {
                    label: grant.label.clone(),
                    item_id: full_id,
                    wanted_gem: gem_row_of(grant.weapon_skill),
                    arts_id: adopted.arts,
                    wanted_level: grant.reinforce_lv,
                    level: (full_id % er_build_import_core::plan::ARMAMENT_LEVEL_STEP) as i32,
                    inventory_index: adopted.index,
                    handle: adopted.handle,
                };
                confirms.push(Requested::armament(&outcome_of_adoption, grant.quantity));
                outcome.armaments.push(outcome_of_adoption);
                outcome.armaments_adopted += 1;
                continue;
            }
            // Safety: same context; the armament path is documented on the helper.
            if let Some(outcome_of_mint) =
                unsafe { grant_armament(module_base, egd, grant, &levels) }
            {
                confirms.push(Requested::armament(&outcome_of_mint, grant.quantity));
                outcome.armaments.push(outcome_of_mint);
                continue;
            }
            // The mint declined (no `CSGaitemImp`, or the table is full). Fall through to the
            // plain call so the player at least gets a bare weapon rather than nothing -- still
            // at the right level, which is the one part of an armament that id alone can carry.
        }
        let full_id = if armament {
            armament_item_id(
                grant.item_id,
                levels.game_level_for(
                    grant.item_id & ITEM_ROW_MASK,
                    grant.reinforce_lv,
                    grant.upgrade_is_character_default,
                ),
            )
        } else {
            grant.item_id
        };
        // An armament'S ALTERNATES are not its upgrade rows. They are other weapons sharing its
        // name, offset by the same affinity but not by the level folded into `full_id`, so
        // counting them would credit a different weapon at a different level. Only the plain
        // categories, whose ids carry no level, can use them.
        let also_known_as: Vec<u32> = if armament {
            Vec::new()
        } else {
            grant.also_known_as.clone()
        };
        confirms.push(Requested {
            item_id: full_id,
            also_known_as: also_known_as.clone(),
            requested: grant.quantity,
            label: grant.label.clone(),
            pot_group: grant.pot_group,
        });
        // Reconcile to the target rather than adding to whatever is there. The build names a
        // quantity the character should end up with, so only the shortfall is granted, and an
        // item already held in sufficient number is left completely alone.
        // Count every ID the name resolves to, not just the one the catalog happened to pick.
        //
        // Elden Ring gives each upgrade level of a flask its own goods row, and every row carries
        // the same name -- so "Flask of Wondrous Physick" is several ids. Asking about one of them
        // and getting zero does not mean the player has no physick; it means they have a
        // different row of it. That is what handed out a second flask beside the one already in
        // the inventory, and the same collision made the Crimson and Cerulean flasks report "not
        // in the inventory" while sitting in the player's belt.
        //
        // `None` is "the inventory could not be read", which is not the same fact as "holds
        // none" and must not be printed as it.
        let held: Option<u32> = (inventory_before != 0).then(|| {
            let mut held: i32 = 0;
            for candidate in std::iter::once(full_id).chain(also_known_as.iter().copied()) {
                let candidate = candidate as i32;
                // Safety: engine-owned inventory pointer, read only; the id outlives the call.
                // A negative answer means "cannot say", not "owns a negative number".
                held = held.saturating_add(
                    unsafe { get_quantity(inventory_before, &raw const candidate) }.max(0),
                );
            }
            held.unsigned_abs()
        });
        let mut shortfall: u32 = grant.quantity.saturating_sub(held.unwrap_or(0));
        // What was asked for and why, for the grants where that is a real question. A grant of one
        // needs no explanation; a grant of ninety-nine does, and the number's provenance is the
        // difference between the importer honouring the item's own limit and the importer
        // emptying a gib table into the player's pockets. Printed before the ladder runs, so a
        // reader can tell an ask that was clamped from an ask that was never made.
        if grant.quantity > 1 {
            crate::log_line(&format!(
                "[build-import]   STACK {:?} item 0x{full_id:08X}: build wants {} ({}), character \
                 holds {}, granting {shortfall}{}",
                grant.label,
                grant.quantity,
                // Which field the number came from, because there are two and they live in
                // different tables. A consumable's ceiling is `EquipParamGoods.maxNum` clamped to
                // 99; an arrow's is `EquipParamWeapon.maxArrowQuantity`, which the engine reads
                // instead and which needs no clamp. A line that named the wrong one would send
                // the next reader to a field that has no row for this item.
                if grant.item_id & ITEM_CATEGORY_MASK == WEAPON_CATEGORY {
                    "the item's own EquipParamWeapon.maxArrowQuantity"
                } else {
                    "the item's own EquipParamGoods.maxNum, capped at 99"
                },
                match held {
                    Some(count) => count.to_string(),
                    None => "an unreadable number of".to_owned(),
                },
                match grant.pot_group {
                    Some(group) =>
                        format!(" -- pot group {group} will clamp this to the group's headroom"),
                    None => String::new(),
                }
            ));
        }
        if shortfall == 0 {
            // Say so. A skipped grant and a grant that never ran look identical in a log that
            // only records what it added, and "why do I have two flasks" is exactly the question
            // that needs this line to answer it.
            outcome.already_held += 1;
            crate::log_line(&format!(
                "[build-import]   ALREADY HELD, not granted: item 0x{full_id:08X}{} \
                 (build wants {}, character has at least that many)",
                if also_known_as.is_empty() {
                    String::new()
                } else {
                    format!(" (+{} id(s) under the same name)", also_known_as.len())
                },
                grant.quantity
            ));
            continue;
        }

        // The ladder. Everything above this point decides how many are missing; everything below
        // decides where they come from, cheapest and least destructive first:
        //
        //   1 ask       -- how many would the carried inventory actually accept? A pot cap is
        //                  the only thing that answers "fewer than you asked for" while every
        //                  other signal says the add will work.
        //   2 pull      -- the player's own copy, out of the storage box.
        //   3 make room -- deposit pot-group members the build does not want, which is what
        //                  raises the group's ceiling. Only for a pot-capped item.
        //   4 GIB       -- mint the rest.
        //
        // Nothing here removes an item from the game. A deposit puts it in the box the player can
        // walk to; a pull moves their own copy back into their pockets.
        if !armament && let Some(storage) = storage.as_ref() {
            // Safety for every call in this block: game thread, and `storage` holds only
            // engine-owned pointers resolved for the running build.
            let headroom = unsafe { storage.carried_headroom(full_id, shortfall as i32) };
            if headroom < shortfall as i32 {
                crate::log_line(&format!(
                    "[build-import]   CAPPED {:?} item 0x{full_id:08X}: the inventory will accept \
                     {headroom} of the {shortfall} still needed{}",
                    grant.label,
                    match grant.pot_group {
                        Some(group) => format!(" (pot group {group})"),
                        None => String::new(),
                    }
                ));
            }

            // RUNG 2 -- Pull. Run whenever the box holds one, not only when rung 1 said the
            // inventory is full. The build is a statement about the character, and minting a
            // second copy beside one the player already owns contradicts it just as surely as
            // minting a second Flask of Wondrous Physick did; the shortfall calculation above
            // cannot see into the box, so without this the boxed copy is invisible and duplicated.
            let mut pulled = 0u32;
            for candidate in std::iter::once(full_id).chain(also_known_as.iter().copied()) {
                if shortfall == 0 {
                    break;
                }
                // Safety: as above.
                if unsafe { storage.stored_quantity(candidate) } <= 0 {
                    continue;
                }
                // Safety: as above; `pull` re-resolves the index itself and re-measures.
                let got = unsafe { storage.pull(candidate, shortfall as i32) }.max(0) as u32;
                if got > 0 {
                    pulled += got;
                    shortfall = shortfall.saturating_sub(got);
                    crate::log_line(&format!(
                        "[build-import]   FROM STORAGE {:?} item 0x{candidate:08X}: {got} moved \
                         back into the inventory, {shortfall} still needed",
                        grant.label
                    ));
                }
            }
            outcome.pulled_from_storage += pulled;

            // RUNG 3 -- Make room. Only a pot-capped item has a group whose ceiling can be
            // raised, and only members the build does not want may be moved. The box has no pot
            // cap (`unlimitedConsumables`), so a deposit really does free the group.
            if shortfall > 0
                && let Some(group) = grant.pot_group
                // Safety: as above.
                && unsafe { storage.carried_headroom(full_id, shortfall as i32) } < shortfall as i32
            {
                let mut deposited = 0u32;
                let mut discarded = 0u32;
                for other in pots.members(group) {
                    if wanted_ids.contains(other) {
                        continue;
                    }
                    // Move as few as the group needs, not everything in it. A group's headroom is
                    // `potItemsCapacity[g] - potItemsCount[g]`, and every consumable deposited
                    // decrements the count by one -- so the deficit is the number to move, and
                    // emptying a player's thirty Poison Pots into the box to make room for one
                    // Fire Pot would be a correct result reached by an obnoxious route.
                    // Safety: as above.
                    let deficit = shortfall as i32
                        - unsafe { storage.carried_headroom(full_id, shortfall as i32) };
                    if deficit <= 0 {
                        break;
                    }
                    // Safety: as above.
                    let carried = unsafe { storage.carried_quantity(*other) };
                    if carried <= 0 {
                        continue;
                    }
                    // Safety: as above. `deposit` asks the box what it will take, refuses an
                    // equipped entry, re-resolves the index, and passes reassignQuickSlot=false.
                    let wanted_out = carried.min(deficit);
                    let moved = unsafe { storage.deposit(*other, wanted_out) }.max(0) as u32;
                    if moved > 0 {
                        deposited += moved;
                        crate::log_line(&format!(
                            "[build-import]   TO STORAGE item 0x{other:08X}: {moved} deposited to \
                             free pot group {group} for {:?}",
                            grant.label
                        ));
                    }

                    // Rung 3b -- destroy, and only here.
                    //
                    // The box is not always an escape. Its own per-item ceiling
                    // (`EquipParamGoods.maxRepositoryNum`) is what `ChangeAmountInBox` answers
                    // with, so a player who already has a full box of Hefty Rock Pots cannot
                    // deposit another one -- and a pot group is shared, so those Rock Pots are
                    // exactly what stops a Hefty Fire Pot from being carried. Every other rung has
                    // run and the group is still full: the item the build does not want is
                    // physically in the way and has nowhere to go.
                    //
                    // Discarding is what the player would do in the menu, and it is deliberately
                    // the narrowest thing this importer is allowed to destroy. All four conditions
                    // hold at this line and none of them is incidental: the id is not in
                    // `wanted_ids`, so the build does not ask for it; it is a member of the pot
                    // group being freed, so it is the specific obstruction; the deposit above has
                    // already been refused for the amount still needed; and the count is the
                    // group's deficit, never the stack. An equipped or quick-slotted copy needs no
                    // special handling -- `CS::EquipGameData::RemoveItem` unequips it and clears
                    // its quickbar reference before removing the entry.
                    let still_out = wanted_out - moved.min(wanted_out as u32) as i32;
                    if still_out > 0 {
                        // Safety: as above. `discard` re-resolves the index, decrements rather
                        // than deleting when part of a stack will do, and measures the result.
                        let destroyed = unsafe { storage.discard(*other, still_out) }.max(0) as u32;
                        if destroyed > 0 {
                            discarded += destroyed;
                            crate::log_line(&format!(
                                "[build-import]   DISCARDED item 0x{other:08X}: {destroyed} \
                                 DESTROYED to free pot group {group} for {:?}. The storage box \
                                 refused them -- it is already at this item's maxRepositoryNum -- \
                                 and the group had no other way to make room. This is the one \
                                 thing the importer does that the player cannot undo",
                                grant.label
                            ));
                        } else if !storage.can_discard() {
                            crate::log_line(&format!(
                                "[build-import]   STUCK item 0x{other:08X}: the box refused it and \
                                 the discard natives have no verified mapping for this build, so \
                                 pot group {group} cannot be freed for {:?}",
                                grant.label
                            ));
                        }
                    }
                }
                outcome.deposited_to_storage += deposited;
                outcome.discarded_to_free_pots += discarded;

                // The box may still hold the wanted item and there may now be room for it, so
                // the pull is worth one more try before anything is minted. A discard frees the
                // group just as a deposit does, so it counts here too -- otherwise a run that had
                // to destroy something would skip the retry and mint a copy of an item the player
                // already owns in the box.
                if (deposited > 0 || discarded > 0) && shortfall > 0 {
                    for candidate in std::iter::once(full_id).chain(also_known_as.iter().copied()) {
                        if shortfall == 0 {
                            break;
                        }
                        // Safety: as above.
                        let got =
                            unsafe { storage.pull(candidate, shortfall as i32) }.max(0) as u32;
                        if got > 0 {
                            outcome.pulled_from_storage += got;
                            shortfall = shortfall.saturating_sub(got);
                            crate::log_line(&format!(
                                "[build-import]   FROM STORAGE {:?} item 0x{candidate:08X}: \
                                 {got} moved back after freeing pot group {group}, {shortfall} \
                                 still needed",
                                grant.label
                            ));
                        }
                    }
                }
            }
        }

        if shortfall == 0 {
            continue;
        }

        // RUNG 4 -- GIB.
        // `updateTrophyStats` and `updateAutoEquip` both true: the same arguments the engine's
        // own pickup path uses, so achievements and auto-equip behave as if the item were
        // found in the world.
        let id = full_id as i32;
        // Safety: game thread, live EquipGameData, and `id` outlives the call.
        unsafe { add(egd, &raw const id, shortfall, true, true) };
        // And read what the game thought of it. The call's own return is an inventory index the
        // caller has no use for; the verdict is written to `EquipGameData.lastItemAddResult`.
        // Safety: a fault-checked read at a verified offset inside a live object.
        if let Some(result) = unsafe { last_item_add_result(egd) }
            && result != ADD_RESULT_OK
        {
            crate::log_line(&format!(
                "[build-import]   ADD REFUSED {:?} item 0x{full_id:08X}: \
                 EquipGameData.lastItemAddResult = {result} ({})",
                grant.label,
                describe_add_result(result)
            ));
        }
    }

    // Safety: same context; the inventory pointer is engine-owned and only read.
    let inventory = unsafe { get_inventory(egd) };
    if inventory == 0 {
        outcome.missing = grants.iter().map(|grant| grant.item_id).collect();
        return outcome;
    }
    for want in &confirms {
        let mut held: i32 = 0;
        for candidate in std::iter::once(want.item_id).chain(want.also_known_as.iter().copied()) {
            let candidate = candidate as i32;
            // Safety: as above.
            held = held
                .saturating_add(unsafe { get_quantity(inventory, &raw const candidate) }.max(0));
        }
        let held = held.unsigned_abs();
        // Confirmed means "AT THE QUANTITY THE BUILD ASKED FOR", not "present". The three
        // outcomes are distinct on purpose: five of five is a success, three of five is a
        // silently clamped add, and zero of five is a refusal -- and the middle one used to be
        // reported as the first.
        if held >= want.requested {
            outcome.confirmed += 1;
        } else if held == 0 {
            outcome.missing.push(want.item_id);
        } else {
            outcome.short.push(Short {
                item_id: want.item_id,
                label: want.label.clone(),
                requested: want.requested,
                held,
                pot_group: want.pot_group,
            });
        }
    }

    // The pot-group sweep, which runs after everything else because it is about what the character
    // should not be carrying rather than what it should.
    //
    // # The case it exists for, reported 2026-09-10
    //
    // A pot group is one shared allowance -- `potItemsCapacity[g]` is the player's Cracked Pot
    // count -- and every member spends from it. `Roped Fetid Pot` (row 430) and `Sleep Pot`
    // (row 640) are both `potGroupId 1`, so a character carrying ten Sleep Pots has spent half the
    // group on an item the build never mentions, and there is no rung above that will notice:
    // rungs 3 and 3b only fire while something is blocking the wanted item, and ten Roped Fetid
    // Pots fit alongside ten Sleep Pots perfectly well. The import finishes reporting complete
    // success and the player's pot allowance is half full of the wrong pot.
    //
    // # What it does not fix, and cannot
    //
    // It does not get more than `EquipParamGoods.maxNum` of the wanted pot into the pockets. That
    // field is 10 for every pot in the family, and it is a per-item ceiling the engine clamps in
    // `GetMaxQuantityForItemEntry` -- so "fill the whole group with the one pot the build names"
    // is not a thing the game permits, however much room the sweep makes. What the sweep buys is a
    // character whose pot allowance holds only what the build asked for.
    //
    // # Why it is allowed to move a player's belongings at all
    //
    // Only inside a group the build has an opinion about. A build that names no member of group
    // `g` leaves group `g` completely alone, which is what keeps this from being a licence to
    // tidy someone's inventory. Within such a group, a member the build does not name is
    // competing for a capped resource the build is trying to allocate -- and it is deposited
    // first, with a discard only where the box refuses, exactly as rungs 3 and 3b decide.
    if let Some(storage) = storage.as_ref() {
        // Groups the build has an opinion about: the ones at least one granted item belongs to.
        let mut claimed: Vec<u8> = grants.iter().filter_map(|grant| grant.pot_group).collect();
        claimed.sort_unstable();
        claimed.dedup();
        for group in claimed {
            for other in pots.members(group) {
                if wanted_ids.contains(other) {
                    continue;
                }
                // Safety: game thread, engine-owned inventory pointers.
                let carried = unsafe { storage.carried_quantity(*other) };
                if carried <= 0 {
                    continue;
                }
                // Safety: as above.
                let moved = unsafe { storage.deposit(*other, carried) }.max(0) as u32;
                if moved > 0 {
                    outcome.deposited_to_storage += moved;
                    crate::log_line(&format!(
                        "[build-import]   TO STORAGE item 0x{other:08X}: {moved} deposited -- the                          build claims pot group {group} and does not name this item, so it was                          spending the group's capacity on something unasked for"
                    ));
                }
                let left = carried - moved as i32;
                if left <= 0 {
                    continue;
                }
                // Safety: as above.
                let destroyed = unsafe { storage.discard(*other, left) }.max(0) as u32;
                if destroyed > 0 {
                    outcome.discarded_to_free_pots += destroyed;
                    crate::log_line(&format!(
                        "[build-import]   DISCARDED item 0x{other:08X}: {destroyed} destroyed --                          the build claims pot group {group}, does not name this item, and the                          storage box is already at its maxRepositoryNum. Irreversible"
                    ));
                }
            }
        }
    }

    outcome
}

/// Grant one armament, with its ash mounted and its upgrade level set, and read both back.
///
/// Returns `None` when the game declined to mint a handle at all, which is the caller's cue to
/// fall back to the plain by-item-id call.
///
/// # Safety
///
/// Game thread, `egd` a live `EquipGameData*`, `module_base` the loaded image base.
/// One armament the character already carries, as a candidate for a build that names it.
#[derive(Clone, Copy)]
struct Adopted {
    index: i32,
    handle: u32,
    item_id: u32,
    /// The `SwordArtsParam` row this instance reports, or `None` when it could not be read.
    arts: Option<u32>,
    claimed: bool,
}

/// The armaments already in the pockets, so a re-import does not mint a second of each.
///
/// # Why this is the only category that needed one
///
/// Every other grant reconciles to a target: it measures what is held and adds the shortfall.
/// Armaments were minted unconditionally, because an ash of war lives on the gaitem instance
/// rather than in the item id, so "do I hold this item" was thought not to answer "do I hold this
/// armament as the build wants it". It does answer it, as long as the skill is compared too --
/// and reading the skill off the instance is one call.
///
/// Without this, importing one build twice hands out every weapon in it twice. Reported
/// 2026-09-10 by a player tuning a build across repeated imports, with many copies of Leontiel's
/// Greatsword to show for it.
struct CarriedArmaments {
    entries: Vec<Adopted>,
}

impl CarriedArmaments {
    /// Read every carried armament and the skill its instance reports.
    ///
    /// Empty when the storage box could not be opened, which costs adoption and nothing else: the
    /// grant then mints, exactly as it did before.
    ///
    /// # Safety
    ///
    /// Game thread, character in the world.
    unsafe fn read(module_base: usize, storage: Option<&crate::storage::Storage>) -> Self {
        let Some(storage) = storage else {
            return Self {
                entries: Vec::new(),
            };
        };
        // Safety: game thread, read only.
        let entries = unsafe { storage.carried_entries() }
            .into_iter()
            .filter(|entry| {
                entry.item_id & ITEM_CATEGORY_MASK == 0 && entry.item_id != UNARMED_ITEM_ID
            })
            .map(|entry| {
                // Safety: the handle is the engine's own, read out of a live entry.
                let arts = unsafe {
                    crate::gaitem::GaitemLookupResult::from_handle(module_base, entry.handle)
                }
                // Safety: game thread; the lookup owns the record it reads.
                .and_then(|mut lookup| unsafe { lookup.sword_arts_id(module_base) });
                Adopted {
                    index: entry.index,
                    handle: entry.handle,
                    item_id: entry.item_id,
                    arts,
                    claimed: false,
                }
            })
            .collect();
        Self { entries }
    }

    /// Take a carried copy that satisfies this grant, if there is one left.
    ///
    /// `wanted_arts` of `None` means the build named no ash, so any copy at the right id will do
    /// -- the build expressed no opinion about the skill and adopting one is what it asked for.
    /// A copy is claimed at most once, so a build naming the same armament twice still mints the
    /// second.
    fn claim(
        &mut self,
        _module_base: usize,
        full_id: u32,
        wanted_arts: Option<u32>,
    ) -> Option<Adopted> {
        let found = self.entries.iter_mut().find(|entry| {
            !entry.claimed
                && entry.item_id == full_id
                && wanted_arts.is_none_or(|wanted| entry.arts == Some(wanted))
        })?;
        found.claimed = true;
        Some(*found)
    }
}

/// The Unarmed fist, which a cleared hand holds and which is never a build's armament.
const UNARMED_ITEM_ID: u32 = 0x0001_ADB0;

unsafe fn grant_armament(
    module_base: usize,
    egd: usize,
    grant: &Grant,
    levels: &crate::catalog::ReinforceLevels,
) -> Option<ArmamentOutcome> {
    // Safety: a fault-checked read of one pointer-sized slot in the loaded image.
    let gaitem = unsafe {
        er_game_base::mem::safe_read_usize(er_game_base::mem::game_data_addr(
            module_base,
            GLOBAL_CSGAITEM_RVA,
            "GLOBAL_CSGAITEM_RVA",
        ))
    }?;
    if gaitem == 0 {
        return None;
    }

    // Resolved for the running build. The three move together and MUST: the mint takes a
    // reference on the `CSGaitemImp` table and the release gives it back, so minting without
    // being able to release would leak a table entry on every armament in the build. `None`
    // means this armament is not minted at all, which the caller already handles.
    let [mint, add_by_handle, release] = crate::native::resolve_all(
        module_base,
        [
            (
                GET_GAITEM_HANDLE_WEAPON_WITH_GEM_RVA,
                "GetGaitemHandleWeaponWithGem",
            ),
            (ADD_INVENTORY_EQUIP_RVA, "AddInventoryEquip"),
            (GAITEM_HANDLE_DTOR_RVA, "GaItemHandle destructor"),
        ],
    )
    .ok()?;
    // Safety: all three addresses were resolved for the running build immediately above.
    let mint: MintWeaponFn = unsafe { core::mem::transmute(mint) };
    // Safety: as above.
    let add_by_handle: AddInventoryByHandleFn = unsafe { core::mem::transmute(add_by_handle) };
    // Safety: as above.
    let release: HandleDtorFn = unsafe { core::mem::transmute(release) };

    let wanted_gem = gem_row(grant);
    // `-1` is the engine's own "no gem": the mint tests `-1 < gemId` before touching the slot.
    let gem_argument = wanted_gem.map_or(-1i32, |row| row as i32);

    // The mint writes a `GaItemHandle` here. Four words rather than the two the struct needs,
    // because the engine's own callers reserve sixteen bytes for it and over-reserving on our
    // own stack costs nothing.
    let mut handle = [0u32; 4];

    // The level is part of the item ID, and it has to be decided before the mint rather than
    // after it. The build's level is what the author asked for, on the planner's scale, not
    // necessarily a level this armament has -- somber armaments stop at +10 while the planner
    // still counts them in regular smithing-stone levels, and a build's `weaponUpgrade` is one
    // number for the whole character -- so the game is asked which `reinforceTypeId + level` rows
    // exist and the request is translated onto them.
    let wanted_level = levels.game_level_for(
        grant.item_id & ITEM_ROW_MASK,
        grant.reinforce_lv,
        grant.upgrade_is_character_default,
    );
    let minted_id = armament_item_id(grant.item_id, wanted_level);

    // Safety: our own buffer, the live singleton, and ids that are plain integers.
    unsafe { mint(gaitem, handle.as_mut_ptr(), minted_id as i32, gem_argument) };
    if handle[0] == 0 {
        return None;
    }

    // Safety: the inventory takes its own reference to the handle; ours is released below.
    let inventory_index =
        unsafe { add_by_handle(egd, handle.as_mut_ptr(), grant.quantity, true, true) };

    // Set the instance field too. `AddInventoryEquip` -> `InsertItem` has just written a
    // hard-coded 0 into it, and the reference exporter sets both halves; this is the half that
    // survives that zeroing.
    // Safety: game thread; the helper only reads and writes this one instance.
    let level = unsafe { apply_reinforcement(module_base, &handle, wanted_level) };
    // Safety: same context; a pure read through the instance's own gem slot.
    let arts_id = unsafe { read_arts_id(module_base, &handle) };

    // The handle is kept only when the inventory actually took its own reference. A negative
    // return means `EquipInventoryData::InsertItem` refused (`AddInventoryEquip` sets
    // `lastItemAddResult = 4` and returns -1), so the release below is the last reference and
    // the table entry goes back on the free queue -- a number that no longer names this item.
    let owned_handle = if inventory_index < 0 { 0 } else { handle[0] };

    // Safety: releases exactly the reference the mint took, exactly as every native caller does.
    unsafe { release(handle.as_mut_ptr()) };

    Some(ArmamentOutcome {
        handle: owned_handle,
        label: grant.label.clone(),
        item_id: minted_id,
        wanted_gem,
        arts_id,
        wanted_level,
        level,
        inventory_index,
    })
}

/// Set an armament instance's upgrade level and read it back. Returns the level the game reports.
///
/// # Safety
///
/// Game thread; `handle` must name a live weapon gaitem.
unsafe fn apply_reinforcement(module_base: usize, handle: &[u32; 4], level: u16) -> i32 {
    // Safety: delegated; the shared record is the length the engine writes.
    let Some(mut lookup) = (unsafe { GaitemLookupResult::from_handle(module_base, handle[0]) })
    else {
        return -1;
    };
    // Resolved for the running build; both are one-line forwarders to the instance's own virtual.
    // `-1` on refusal is this function's own "the level could not be read", which the caller
    // already reports rather than presenting as a level.
    let Ok([set, get]) = crate::native::resolve_all(
        module_base,
        [
            (SET_REINFORCEMENT_RVA, "SetReinforcement"),
            (GET_REINFORCEMENT_RVA, "GetReinforcement"),
        ],
    ) else {
        return -1;
    };
    // Safety: both addresses were resolved for the running build immediately above.
    let set: SetReinforcementFn = unsafe { core::mem::transmute(set) };
    // Safety: as above.
    let get: GetReinforcementFn = unsafe { core::mem::transmute(get) };
    // Safety: the lookup resolved a live instance, which is what both forwarders dereference.
    unsafe { set(&raw mut lookup, i32::from(level)) };
    // Safety: as above.
    unsafe { get(&raw mut lookup) }
}

/// The `SwordArtsParam` row an armament instance actually holds, through its own equipped gem.
///
/// # Safety
///
/// Game thread; `handle` must name a live weapon gaitem.
unsafe fn read_arts_id(module_base: usize, handle: &[u32; 4]) -> Option<u32> {
    // Safety: delegated to the shared record, which owns both hops.
    let mut lookup = unsafe { GaitemLookupResult::from_handle(module_base, handle[0]) }?;
    // Safety: as above.
    unsafe { lookup.sword_arts_id(module_base) }
}
