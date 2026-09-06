//! Tier A: stable singleton RVA / offset table (game 1.16.x, image base
//! 0x140000000). These are version-anchored facts shared by all three DLLs;
//! they were previously re-declared under ~4 different aliases in the product
//! `constants/*` tree and hand-copied verbatim into the two mini-DLLs. This is
//! the single source of truth.
//!
//! Feature-specific / experiment-local offsets do NOT belong here — only the
//! cross-cutting singleton globals + their generic field offsets.

/// `GameDataMan` singleton global (aliased as GAME_DATA_MAN_GLOBAL_RVA /
/// CONTINUE_MANAGER_GLOBAL_RVA in the product tree).
pub const GAME_DATA_MAN_GLOBAL_RVA: usize = 0x3d5df38;
/// `CSMenuMan` singleton global (aliased GLOBAL_CSMENUMAN_RVA /
/// CS_MENU_MAN_GLOBAL_RVA / SELECTBOT_INPUT_MANAGER_GLOBAL_RVA /
/// TITLE_INPUT_MANAGER_RVA).
pub const CS_MENU_MAN_GLOBAL_RVA: usize = 0x3d6b7b0;
/// `GameMan` singleton global (save-slot owner).
pub const GAME_MAN_SINGLETON_RVA: usize = 0x3d69918;
/// `CS::CSPcKeyConfig` singleton global (Ghidra: `GLOBAL_CSPcKeyConfig`) -- the PLAYER'S OWN
/// keyboard, mouse and gamepad bindings. NULL until the game initialises its key configuration.
///
/// `+0x008` is the DEFAULT binding table loaded from `KeyAssignParam_TypeA` and `+0x440` is the
/// CURRENT one the player configured: `0x36` entries of `0x14` bytes each. Stride, count and
/// offset all come from the accessor `GetAssign` (`0x242ab0`), which is literally
/// `cmp r8d,0x35; ja fail; lea rcx,[rcx + idx*0x14 + 0x440]`.
///
/// `+0x000` is the input-device SOURCE byte: the title screen writes 0 for a pad press and 1 for a
/// key press (`FUN_1409b0800` / `FUN_1409b1260`), which is what flips the on-screen button glyphs.
/// `er-title-flow` aliases this global as `TITLE_MENU_TRANSITION_SINGLETON_RVA` and its narrow
/// title advance sets that byte -- correct behaviour, but it is the device-source byte and not a
/// "menu-open in progress" flag as the comment there used to say.
///
/// RE 2026-08-25 against the 1.16.2 dump; see bd
/// `er-keybinding-table-cspckeyconfig-1162-2026-08-25` for the full layout, the internal-key-id
/// enum and the scancode lookup table that goes with it.
pub const CS_PC_KEY_CONFIG_SINGLETON_RVA: usize = 0x3d5dea8;
/// The `.data` byte `Game.Debug.IsEnableControlOnDisactiveWindow` reads -- the single instruction
/// `movzx eax, byte ptr [0x144588af1]` at `0x1402e6853` (1.16.2 RVA; the DATA map carries it to
/// 0x458cb71 on 1.17, bracketed by five anchors that all move +0x4080).
///
/// DATA, NOT A HOOK TARGET, and the name says so on purpose: nothing detours this address.
/// `er-focus-input` WRITES the byte so an unfocused window still feeds input;
/// `er-quickload`'s `can_move_probe` READS it as one of the three bytes that decide whether the
/// game reads any input this frame. Declared here because two ME3 shells naming one literal is the
/// drift `scripts/check-rva-alias-drift.py` exists to catch, and because `_DATA_` in the name is
/// what keeps `scripts/check-shared-hook-rvas.py` from reading the pair as two MinHook instances on
/// one prologue.
pub const GAME_DEBUG_ENABLE_CONTROL_ON_DISACTIVE_WINDOW_DATA_RVA: usize = 0x4588af1;
/// `CS::FieldArea**` singleton global -- 1.16.2 runtime VA `0x143d691d8`.
///
/// The 1.16.2 Ghidra dump has 264 reads of this global. `ConvertBlockCoordsToPhysicsCoords`
/// (`0x14061e120`) reads it as `GLOBAL_FieldArea`, then passes `FieldArea+0x18` to the typed
/// `WorldInfoOwner` methods.
pub const FIELD_AREA_PTR_RVA: usize = 0x3d691d8;
/// `FUN_14067b570` -- system-slot-only save dispatcher (GameMan b73 set, b72 clear).
pub const SAVE_DISPATCH_SYSTEM_RVA: usize = 0x67b570;
/// `FUN_14067b940` -- combined character+system save dispatcher (GameMan b72 and b73 set).
pub const SAVE_DISPATCH_COMBINED_RVA: usize = 0x67b940;
/// `InGameStep::STEP_MoveMap_LoadlistInit` -- builds the world-res loadlist.
pub const STEP_MOVEMAP_LOADLIST_INIT_RVA: usize = 0xaec570;
/// SaveLoad IO device request teardown/release function.
pub const SL_RELEASE_REQUEST_RVA: usize = 0xe6f200;
/// `FUN_140679180` -- the LOAD-side pump of the SaveLoad IO device. Polls `FUN_140e6e080` and writes
/// `GameMan.saveState` 3 (answer 0) or 0 (any answer but 0 or 1). `CS::MoveMapStep::DoSaveStuff`
/// calls it ONLY under `IsSaveState2()`, because `FUN_140e6e080` answers 4 without releasing
/// anything while `iodev+0x18 == 0` -- which is every frame a SAVE owns the device.
/// Aliased as B80_POLL_RVA (er-title-flow) / SAVE_STATE_LOAD_POLL_RVA (er-save-suppress); both
/// derive from here (2026-08-31).
pub const SL_LOAD_POLL_WRAPPER_RVA: usize = 0x679180;
/// `FUN_140679510` -- the SAVE-side pump of the same device. Polls `FUN_140e6e430` and writes
/// `GameMan.saveState = 0` for any answer but 1. `DoSaveStuff` calls it ONLY under `IsSaveState1()`.
/// Aliased as B80_LANE1_DRIVER_RVA (er-title-flow) / SAVE_STATE_SAVE_LANE_RVA (er-save-suppress).
pub const SL_SAVE_LANE_WRAPPER_RVA: usize = 0x679510;
/// `EzChildStepBase` child-step reset/teardown function.
pub const EZ_CHILDSTEP_RESET_RVA: usize = 0xeb54c0;
/// Scaleform LoaderImpl file-open wrapper.
pub const TITLE_SCALEFORM_FILE_OPEN_RVA: usize = 0x11ced80;
/// CS::TitleTopDialog vtable.
pub const TITLE_TOP_DIALOG_VTABLE_RVA: usize = 0x2b26468;
/// Scaleform::MemoryFile vtable.
pub const SCALEFORM_MEMORY_FILE_VTABLE_RVA: usize = 0x2ba4c80;
/// `CSSystemStep` singleton global.
pub const CS_SYSTEM_STEP_GLOBAL_RVA: usize = 0x3d85680;
/// `FD4::FD4StepTemplateBase::currentState` -- the state the stepper is EXECUTING this frame,
/// at `CSSystemStep + 0x48`. `requestedState` is the adjacent `+0x4c` and the "step done,
/// advance" bool is `+0x50`; the constructor zeroes the first two together with a single
/// `mov qword [this+0x48], 0`.
///
/// PROVENANCE, because this constant was WRONG (0x40) from its introduction until 2026-08-31 and
/// the wrong value looked exactly like the right one. `oracle_system_step_label` reported `"?"`
/// with `oracle_system_step_state = -95247096` = `0xfa52a508`, which is the low half of the
/// `FD4ComponentAttachSystem_Step::allocator` POINTER that actually lives at `+0x40` -- a legal
/// i32 out of a legal read, so nothing ever faulted.
///
/// The 0x40 came from back-solving the layout off a field NAME: the sibling `fromsoftware-rs`
/// `FD4StepTemplateBase` has a member spelled `unk48` directly after `requested_state`, and
/// "unk48 is at 0x48" puts `current_state` at 0x40. That member is misnamed -- it sits at 0x50 --
/// and the Rust struct's computed layout was right all along.
///
/// Measured instead of named: `scripts/pair-object-field-drift.py --pair 0x140dec6d0:226
/// 0x140dee4d0:226 --base rsi --base rcx` aligns the CSSystemStep step-template constructor's two
/// bodies 57/57 instructions with 13 field offsets -- 0x0, 0x10, 0x18, 0x48, 0x50, 0x58, 0x60,
/// 0x68, 0x69, 0x70, 0xa0, 0xa8, 0xac -- every one HELD across 1.16.2 -> 1.17 and 0x40 absent from
/// the set. The bytes are identical in both images: `48 89 5e 48 88 5e 50` at 0x140dec744 (1.16.2)
/// and 0x140dee544 (1.17). Frozen as a witness row in
/// `scripts/check-object-field-offsets-1170.py`, which is what now keeps this honest.
pub const CS_SYSTEM_STEP_CURRENT_STATE_OFFSET: usize = 0x48;
/// Native `MenuWindowJob::Run` close-with-Failed helper. It calls `SetResult(..., Failed=3, 0)`
/// and then invokes the receiver's own vtable slot +0x60.
pub const MENU_WINDOW_CLOSE_WITH_FAILED_RVA: usize = 0x7ac890;
/// Save-data subsystem gate global (submit path guard). Ghidra names it `GLOBAL_CSEventState`;
/// the guard role is the reason it is here. It is a 3-byte HeapAlloc read as a byte at
/// offset 0, not the 0x270-byte object allocated beside it (that one is 0x3d68448).
pub const SAVE_DATA_SUBSYSTEM_GATE_RVA: usize = 0x3d68078;
/// Main heap allocator singleton global (`GLOBAL_MainHeapAllocator`). Identified from the
/// 1.16.2 Ghidra dump: 1821 xrefs, readers spanning `CSTaskImp` / `CSWindowImp` / `CSEzWork`
/// / `BloodMessageInsMan`, and `GameMan::WriteSaveToSlot` (0x14067b750) derefs
/// `GLOBAL_MainHeapAllocator->_vfptr->AllocateAligned` while referencing this address.
/// Aliased in the tree as GLOBAL_MAIN_HEAP_ALLOCATOR_RVA / SLLOAD_SRC2_RVA (a wrong name) /
/// SAVE_BUFFER_ALLOCATOR_GLOBAL_RVA (a role name in er-save-loader; that crate now derives
/// from here, so the misnomer is documented at its declaration rather than duplicated).
pub const GLOBAL_MAIN_HEAP_ALLOCATOR_RVA: usize = 0x3d872e0;
/// Menu heap allocator singleton global (`GLOBAL_MenuHeapAllocator`, `DLAllocator*`), the
/// allocator every menu-owned allocation goes through. Sits 0x70 past the main heap allocator
/// above; the two are easy to transpose, which is the second reason it is pinned here.
/// Identified from the 1.16.2 Ghidra dump: 401 xrefs, and the accessor
/// `DLAllocator *GetMenuHeapAllocator(void)` (0x1407a72a0) is nothing but
/// `return GLOBAL_MenuHeapAllocator;` reading this address.
/// Aliased in the tree as GLOBAL_MENU_HEAP_ALLOCATOR_RVA (er-quickload save-picker path editor,
/// which passes it as the allocator argument of the SoftwareKeyboard job alloc) /
/// MENU_HEAP_ALLOCATOR_POINTER_RVA (er-player-name-filter, which passes it to
/// `DLString<wchar_t>::FromU16Array` exactly as `GetPlayerChrName` does).
pub const GLOBAL_MENU_HEAP_ALLOCATOR_RVA: usize = 0x3d87350;
/// SaveLoad IO device singleton global. Its lazy getter is 0x140e6e060, and
/// `GameMan::WriteSaveToSlot` fetches the device through that getter before submitting a save.
/// Aliased as IODEV_GLOBAL_RVA (er-title-flow) / SL_IODEV_GLOBAL_RVA (er-save-suppress); both
/// now derive from here (2026-08-01). "iodev" is the role the callers use -- the dump does not
/// name the class, so treat the name as descriptive rather than authoritative.
pub const SL_IODEV_GLOBAL_RVA: usize = 0x4589390;
/// `FUN_140e05fb0(CSDlcImp*, bool)` -- the DLC virtual-root REFILL: re-queries Steam DLC ownership
/// and calls `CSDlcImp::AddVirtualFileRoots`.
pub const DLC_ROOTS_REFILL_RVA: usize = 0x00e0_5fb0;
/// `GLOBAL_CSDlc` -- the `CSDlcImp` singleton.
pub const CSDLC_SINGLETON_RVA: usize = 0x03d8_6bd8;

/// `CS::ItemReplenishStateTracker::ShouldReplenishItem(tracker, int *itemId)` -- whether an item is
/// currently flagged to be topped back up from the storage box. Honours the per-type DEFAULT for an
/// item with no entry (`AUTO_REPLENISH_TYPE` 2 / Consumable defaults ON, type 1 defaults OFF), which
/// is why it, and not a read of the entry vector, is the right question to ask about current state.
pub const SHOULD_REPLENISH_ITEM_RVA: usize = 0x0023_d990;
/// `ReplanishItemsFromChest()` -- the native storage -> personal inventory transfer loop. Vanilla
/// runs it from `OnEvent_BonfireRespawn` and `MoveMapStep::UpdatePlayerInfo`. Marking replenish
/// state alone moves no items; this is what moves them.
pub const REPLANISH_ITEMS_FROM_CHEST_RVA: usize = 0x0024_dff0;

/// `GameDataMan` -> `PlayerGameData` pointer field offset.
/// `CS::GameMan::SetMoveMapStepBlockId(BlockId *out, BlockId *in)` -- writes
/// `GameMan.moveMapStepBlockId`, i.e. picks the destination block of the next map transition.
/// Byte-checked against `eldenring-deobf.bin` at shift 0 for 1.16.2 (`0x14067abd0`).
///
/// NOTE for callers: `param_1` is the OUT slot, and it is not always equal to `param_2` --
/// for areas 50..=88 the id is rewritten through `CalcGetReplaceMapIdByDisaster`. Read the
/// out slot back rather than assuming the requested block is the effective one.
pub const SET_MOVE_MAP_STEP_BLOCK_ID_RVA: usize = 0x67abd0;

pub const GAME_DATA_MAN_PLAYER_GAME_DATA_08_OFFSET: usize = 0x08;
/// `CSMenuMan` -> `menuData` pointer field offset.
pub const CS_MENU_MAN_MENU_DATA_OFFSET: usize = 0x8;

/// `CS::GetGR_System_Message(MenuString* rcx, int edx)` -- resolves a `GR_System_Message` FMG id
/// into a `MenuString`.
///
/// Cross-cutting because two DLLs reach it for opposite reasons: `er-quickload` hooks it READ-ONLY
/// to detect corrupted-save popups by message id, and `er-invasion-warp` calls it to build a
/// `MenuString` for the system-message banner. Declared twice with different names until 2026-08-06,
/// which is exactly the alias drift this module exists to stop.
///
/// CORRECTED HISTORY worth keeping: 0x762e30 is `GetTextEmbedImageName`, NOT this. Hooking that one
/// is why a corrupted-save oracle once stayed at zero forever.
pub const GR_SYSTEM_MESSAGE_RVA: usize = 0x762d50;

/// Shared `CS::MessageBoxDialog` function/vtable RVAs.
///
/// These are cross-cutting game identities consumed by title flow, telemetry, and the product's
/// startup/quit paths. Keep the values here so a version correction cannot drift between crates.
#[repr(usize)]
pub enum MsgBoxRva {
    ForceStop = 0x78dfd0,
    OkHandler = 0x78e030,
    MultiChoiceGetter = 0x7b0cf0,
    Builder = 0x9275b0,
    OnDecide = 0x927ba0,
    Update = 0x927d30,
    DialogVtable = 0x2b03550,
}

pub const MSGBOX_DIALOG_VTABLE_RVA: usize = MsgBoxRva::DialogVtable as usize;

// ---- inventory functions shared by more than one DLL ----
//
// These moved here when a second crate needed them: `er-better-refills` reads the player's
// inventory to decide what to replenish, and `er-build-import` reads it to confirm granted
// items and to resolve the inventory INDEX that the equip path requires. Two literal copies of
// one address is exactly the drift `scripts/check-rva-alias-drift.py` exists to stop.

/// `CS::EquipGameData::GetEquipInventoryData(equipGameData) -> EquipInventoryData*`.
pub const GET_EQUIP_INVENTORY_DATA_RVA: usize = 0x247b30;
/// `EquipInventoryData::GetQuantityByItemId(inventory, int *itemId) -> int`.
pub const GET_QUANTITY_BY_ITEM_ID_RVA: usize = 0x24c1b0;
// ---- the message repository singleton ----
//
// Moved here for the same reason as the inventory functions above: two crates now read it.
// `er-invasion-warp` resolves a `PlaceName`, and `er-build-import` builds its whole
// name -> item-id catalog out of the game's own strings.
//
// IT IS READ FROM THE GAME'S OWN GLOBAL, NOT from a typed upstream singleton. `fromsoftware-rs`
// at the revision CI pins (`FROMSOFTWARE_RS_REV` in .github/workflows/check.yml) has no
// `MsgRepositoryImp`; only a local fork does. Depending on that type builds on a developer's
// machine and fails in CI with `unresolved import`, which is exactly what happened. Reading the
// global keeps the address in this repo, where `scripts/check-rva-alias-drift.py` can see it.

/// `GLOBAL_MsgRepository` -- the `CS::MsgRepositoryImp*` singleton slot, read from
/// `MOV RCX, qword ptr [0x143d7d4f8]` inside `MsgRepository::GetAndFormat`.
///
/// The slot is null until the repository is constructed, so every caller must treat a zero as
/// "not up yet" rather than dereferencing it -- the engine itself DLPanics on that path.
pub const MSG_REPOSITORY_GLOBAL_RVA: usize = 0x3d7_d4f8;

/// `GLOBAL_SoloParamRepository` -- the `SoloParamRepositoryImp*` singleton slot, read as
/// `MOV RCX, qword ptr [0x143d81ee8]` at the head of every param-row lookup.
///
/// Null on a quit teardown, and the lookups do not tolerate that: `FUN_140d3d5f0`
/// (`LoadBalancerParam`) and `LookupMenuOffscrRendParam` both go straight to
/// `DLPanic("...FD4Singleton.h", 0xb4, "<uninitialised singleton accessed>")`, which then dies
/// writing through a null pointer. Treat a zero here as "already gone" rather than dereferencing
/// it. Observed killing a tester's game twice in two days, 2026-08-22 and 2026-08-23.
pub const SOLO_PARAM_REPOSITORY_GLOBAL_RVA: usize = 0x3d8_1ee8;

/// `EquipInventoryData::GetItemInventoryIdx(inventory, int *itemId) -> int`.
///
/// Returns the index the equip path needs; negative means the item is not held.
pub const GET_ITEM_INVENTORY_IDX_RVA: usize = 0x24c560;

// ---- the storage box, and moving items in and out of it ----
//
// These four shipped as private constants inside `er-better-refills`, whose deposit-back path
// has been calling them in production since the first-grace feature landed. The build importer
// now needs the same four for the opposite direction, so they move here rather than becoming a
// second literal copy each -- the drift `scripts/check-rva-alias-drift.py` exists to stop.
//
// THE STORAGE BOX IS A SECOND `EquipInventoryData`, NOT A DIFFERENT KIND OF THING. Every
// inventory function above takes an `EquipInventoryData*` and does not care which one, so
// `GetQuantityByItemId` / `GetItemInventoryIdx` answer about storage exactly as they answer about
// the carried inventory. The one difference that matters is a construction flag:
// `EquipInventoryData::EquipInventoryData(this, size, keySize, limitedPots, unlimitedConsumables)`
// (0x14024bbf0) has two callers, and the `EquipGameData` ctor at 0x140245485 passes
// `unlimitedConsumables = 0` while the `PlayerGameData` ctor at 0x14025d879 passes `1`. That flag
// makes `UpdatePotsStates` early-return and routes `GetMaxAmountForItem` to
// `GetMaxItemCountForUnlimitedConsumables`, so the box has NO pot-group cap -- which is the whole
// reason depositing a displaced pot raises the ceiling for the one being imported.

/// `GetMainPlayerStorageBoxInventory() -> EquipInventoryData*` -- `PlayerGameData + 0x8d0`.
///
/// TWO WAYS TO DIE, and neither is recoverable, so both are the caller's job to rule out first:
/// a null `GLOBAL_CSMenuMan` takes the `DLPanic("...FD4Singleton.h", 0xb4, ...)` path, and a
/// non-null `GLOBAL_GameDataMan` with a null `mainPlayerGameData` reads through the null pointer
/// at `+0x8d0`. Check both before calling.
pub const GET_MAIN_PLAYER_STORAGE_BOX_INVENTORY_RVA: usize = 0x786810;
/// `EquipInventoryData::ChangeAmountInBox(box, int *itemId, int requested) -> int accepted`.
///
/// A PURE QUERY DESPITE THE NAME: its whole body is `if (requested < 1) return 0;`, then the
/// `CanDepositItemToStorageBox` eligibility gate (only when `unlimitedConsumables`), then a
/// delegation to [`GET_ADD_OR_REMOVE_AMOUNT_RVA`]. It moves nothing. Ask it how many the box will
/// take, then move exactly that many with [`TRANSFER_ITEM_BETWEEN_INVENTORY_DATAS_RVA`].
pub const CHANGE_AMOUNT_IN_BOX_RVA: usize = 0x24e3d0;
/// `TransferItemBetweenInventoryDatas(u32 srcIdx, EquipInventoryData *src,
/// EquipInventoryData *dst, i32 quantity, bool reassignQuickSlot) -> bool`.
///
/// It takes an INDEX, not an item id, and the index is only valid until the next transfer: the
/// call ends in `AdjustQuantityBy` and then `RemoveItem` once the stack empties, which reindexes
/// the source. Re-resolve [`GET_ITEM_INVENTORY_IDX_RVA`] immediately before every call.
///
/// `reassignQuickSlot` IS DIRECTIONAL AND WILL CORRUPT A SAVE IF IT IS SET THE WRONG WAY. When
/// set, the tail writes the DESTINATION index into the main player's quick-slot table
/// (`EquipItemData::SetQuickSlotItem` on `GLOBAL_GameDataMan->mainPlayerGameData`). Passing
/// `true` while DEPOSITING therefore points the player's quickbar at a storage-box index -- a
/// dangling reference that persists into the save. Pass `false` when depositing, `true` when
/// retrieving.
pub const TRANSFER_ITEM_BETWEEN_INVENTORY_DATAS_RVA: usize = 0x24db90;
/// `CS::EquipGameData::UpdateTrophyStats(equipGameData, int *itemId)`.
///
/// What the engine's own acquisition path calls after an item lands, so achievement state matches
/// what the inventory now holds. A transfer that skips it leaves the two disagreeing.
pub const UPDATE_TROPHY_STATS_RVA: usize = 0x24a1a0;
/// `CS::EquipInventoryDat::GetAddOrRemoveAmount(inventory, uint *itemId, int delta) -> int`.
///
/// HOW MANY OF `itemId` THIS INVENTORY WOULD ACTUALLY ACCEPT (or, for a negative `delta`, give
/// up). A pure query: it reads the entry, asks `HasSpaceForItem` / `GetMaxAmountForItem` /
/// `GetMaxQuantityForItemEntry`, and returns the clamped number without touching anything.
///
/// This is the ONLY cheap way to see a pot-group cap coming. Both acquisition paths --
/// `InsertItem` (0x14024cfd0) and `UpdateQuantity` (0x14024d760) -- clamp with a silent
/// `if (max < amount) amount = max;`, so an add of five that delivers three returns no error and
/// sets no result code. Asking first is what turns that into a number a caller can report.
pub const GET_ADD_OR_REMOVE_AMOUNT_RVA: usize = 0x24c630;

// ---- the equipped loadout: read by the importer, the exporter and the HUD badge ----
//
// These moved here when the Generate Build Link row arrived and gave every one of them a THIRD
// declaration. Each address was already written out twice -- once in `er-build-import-runtime`
// where the importer verifies its own writes, once in `er-armament-icons` where the HUD badge
// resolves the equipped gem -- and an exporter that reads the same slots would have made three.
//
// `check-rva-alias-drift.py` puts the reason better than a comment can: divergent names for one
// address are divergent CLAIMS about what that address IS, and at least one of them is then a
// wrong reverse-engineering fact shipping in the DLL. Declaring the value once and deriving the
// aliases keeps the names -- which are useful, they say what the caller wants -- while leaving
// exactly one place a 1.16.x address correction has to land.

/// `CS::EquipGameData::GetParamIdInSlot(egd, ChrAsmSlot) -> int`.
///
/// The read-back oracle for equipment, in both directions. The importer needs it because
/// `EquipItemToChrAsmSlot` returns void and declines silently, so only the slot's contents
/// afterwards are evidence; the exporter needs it because those contents ARE the build.
pub const GET_PARAM_ID_IN_SLOT_RVA: usize = 0x2470e0;

/// `CS::EquipGameData::GetEquippedGreatrune(egd, int *out, int slot) -> int*`.
///
/// THREE arguments. The outer wrapper at `0x140247900` is only
/// `ADD RCX,0x288 / MOV RBX,RDX / CALL 0x14024f390 / MOV RAX,RBX` -- it never writes R8, so the
/// slot argument passes straight through to the inner function, whose body begins
/// `*out = -1; if (slot == 0 && ...)`. Calling it with two arguments leaves R8 holding whatever
/// the call site happened to have, the `slot == 0` test fails, and it reports -1 no matter what is
/// equipped. That produced three runs of "the rune will not equip" when the rune was fine.
pub const GET_EQUIPPED_GREATRUNE_RVA: usize = 0x247900;

/// `CS::EquipGameData::GetPhysicTearBySlot(egd, int *out, uint slot) -> int*`.
///
/// Another OUT-PARAMETER getter, like the great rune above: the second argument is a pointer the
/// callee writes through, not a scalar. Treating it as a scalar getter stores through whatever the
/// caller passed, which took the game down once.
pub const GET_PHYSIC_TEAR_BY_SLOT_RVA: usize = 0x247a20;

/// `CS::PlayerGameData::CopyChrName(PlayerGameData *pgd, const wchar_t *name)` -- the SOLE
/// writer of a character's name, and therefore the only sanctioned way to change one.
///
/// A character name is stored THREE times inside `PlayerGameData`, and only this function keeps
/// them in step (1.16.2 `0x1402610c0`, decompiled 2026-08-30):
///
///   * `+0x9c`, the raw `wchar_t[17]`. This is the one that is SERIALIZED: the save body is a
///     verbatim image of the PGD from `+0x08`, so the raw name lands at slot-body `+0x94`
///     (`er_profile_summary_core::SAVE_PGD_CHARACTER_NAME_OFFSET`).
///   * `+0x8e8`, a ref-counted `CSWordCheckedStringInternal` holding the raw AND word-checked
///     spellings. `FUN_14025f8e0` reads THIS one, and `FUN_14025f8e0` is what
///     `CS::ProfileSummary`'s per-slot update (`0x140262270`) and `CS::GetPlayerChrName` -- the
///     overhead nameplate -- both call. A raw `memcpy` into `+0x9c` would leave the save-slot
///     list and every other player still reading the OLD name out of here.
///   * `+0x8f8`, a second such string, refreshed with the word-check flag set when
///     `isMainPlayer`, and copied from `+0x8e8` otherwise.
///
/// TWO PROPERTIES OF THE INPUT ARE LOAD-BEARING. The function begins with a bare `wcslen` on the
/// argument, so an UNTERMINATED buffer is an out-of-bounds read before anything is validated; and
/// it then copies only `if (len < 0x11)`, so a name of 17 units or more is SILENTLY IGNORED --
/// the call returns having changed no name at all, while still refreshing the two string objects
/// from the unchanged `+0x9c`. Clamp to 16 units and terminate; never rely on a length check
/// inside the callee to report anything.
pub const PLAYER_GAME_DATA_COPY_CHR_NAME_RVA: usize = 0x2610c0;

/// `CS::EquipMagicData::GetMagicSlotsCount(emd, SpecialEffect*) -> uint`.
///
/// A null `SpecialEffect` means "derive it from the player", which accounts for Memory Stones and
/// talismans; the engine clamps the result to 14.
pub const GET_MAGIC_SLOTS_COUNT_RVA: usize = 0x250580;

/// `CS::EquipMagicData::GetEquipMagicId(emd, slot) -> int` -- the memorised spell in a slot.
pub const GET_EQUIP_MAGIC_ID_RVA: usize = 0x2506d0;

/// `GetWeaponGaitemHandleBySlot(PlayerIns*, u32 *out, ChrAsmSlot) -> u32*`.
///
/// First hop of the ash-of-war read: a `ChrAsmSlot` names a gaitem handle.
pub const GET_WEAPON_GAITEM_HANDLE_BY_SLOT_RVA: usize = 0x656920;

/// `GetGaitemInsByHandle(GaitemLookupResult *inout, GaitemLookupResult *handleSource)`.
///
/// Second hop. BOTH arguments are the SAME pointer in every native caller; passing two different
/// buffers reads a handle that was never written into the second one, and the failure is a silent
/// nothing rather than a fault.
pub const GET_GAITEM_INS_BY_HANDLE_RVA: usize = 0x672e40;

/// `GetSwordArtsParamForWeapon(GaitemLookupResult*, SwordArtsParamLookupResult *out)`.
///
/// Third hop, and the reason the chain is worth walking: it resolves through
/// `GetGemGaitemHandleFromWeapon` -> `GetGaitemInsGem`, i.e. the ACTUAL equipped gem. The menu
/// path's `arts_id * 100` heuristic misses every weapon whose gem id is not derived that way
/// (Igon's Drake Hunt: arts 4210 -> gem 548000).
pub const GET_SWORD_ARTS_PARAM_FOR_WEAPON_RVA: usize = 0x673f30;

/// `GLOBAL_CSGaitem`, the process-lifetime `CSGaitemImp` singleton pointer.
///
/// Constructed once at boot and never rebuilt per world-load. Every gaitem primitive in the game
/// reads it and `DLPanic`s on null, so a caller that mints or releases a handle must read it from
/// here rather than keep its own copy.
pub const GLOBAL_CSGAITEM_RVA: usize = 0x3d69890;

/// `WorldChrMan` singleton global; `+0x1e508` is the local `PlayerIns`.
///
/// Both dereferences are null-checked in the game's own code, so both must be null-checked here.
pub const WORLD_CHR_MAN_GLOBAL_RVA: usize = 0x3d65f88;

/// `WorldChrMan::mainPlayerIns`, the local player inside the singleton above.
pub const WORLD_CHR_MAN_PLAYER_INS_OFFSET: usize = 0x1e508;

// ---- Centralised by the #362 merge (move-vs-move) ------------------------------------------
// These seven addresses were each declared as a literal in TWO crates at once, because two
// branches independently moved the same constants into two different new crates: er-title-flow
// (the autoload/title-flow slice) and er-diag-harness / er-quit-menu-core (already on main).
// Neither branch was wrong; the merge is what made them duplicates, and
// `scripts/check-rva-alias-drift.py` reported all seven as NEW drift.
//
// Its own prescription is followed here: declare the value ONCE, cross-cutting singletons in
// this file, and derive every other name from it. One address, one literal.

/// `FUN_14021bbf0` -- the MSB filecap parse callback.
pub const MSB_FILECAP_PARSE_CALLBACK_RVA: usize = 0x0021_bbf0;
/// `CS::MenuViewer` pad-confirm-pressed predicate.
pub const MENU_VIEWER_PAD_CONFIRM_PRESSED_RVA: u32 = 0x0075_8a10;
/// `CS::MenuViewer` mouse-clicked predicate.
pub const MENU_VIEWER_PAD_MOUSE_CLICKED_RVA: u32 = 0x0075_8a70;
/// The DLC virtual-root refill JOB.
pub const DLC_ROOTS_JOB_RVA: usize = 0x0083_6f30;
/// `STEP_Loadlist_Wait`.
pub const STEP_LOADLIST_WAIT_RVA: usize = 0x00af_1800;
/// The DLC virtual-root BLANK path.
pub const DLC_ROOTS_BLANK_RVA: usize = 0x00e0_6490;
/// `DLIO::DLFileDeviceManager` singleton global -- deobf VA `0x1448464a8`.
///
/// SOLE DECLARATION of this address. `er-title-flow`'s `EBL_REGISTRY_GLOBAL_RVA` and
/// `er-reload-trace`'s `MOUNTED_ARCHIVE_REGISTRY_RVA` are aliases derived from this constant;
/// on 2026-08-30 both of them held an independently CORRUPTED copy of it (`0x84864a8`, a doubled
/// digit; `0x448464a8`, the deobf VA with only `0x14` stripped rather than the whole image base).
/// The image is `0x5e01800` bytes, so neither corruption is inside it and every read through them
/// silently returned nothing. That is the failure mode this centralisation exists to prevent.
///
/// The value is byte-proven, not inferred: `GetFileDeviceManager` @`0x141f48b40` is
/// `48 8b 05 61 d9 8f 02` = `mov rax, [rip+0x28fd961]`, and `0x141f48b47 + 0x28fd961` is
/// `0x1448464a8` and nothing else. Ghidra names that global `GLOBAL_DLFileDeviceManager` and its
/// lazy creator `FUN_141f49f60`. `0x48464a8` has reference sites in `.text` (6/6 agreeing on the
/// 1.17 carry to `0x484a528`); `0x84864a8` and `0x448464a8` have zero between them.
///
/// This global IS the manager pointer, NOT "the mounted-archive registry" -- the mount census
/// walks the registry hanging off it (`R+0x90`/`R+0x98`, stride `0x40`; archive name is an MSVC
/// wstring at `entry+0x08`, `Archive*` at `entry+0x30`, lock at `R+0xB8`). This corrects bd
/// `step3-census-registry-null-on-load2-mount-skip-confirmed-2026-07-17`: a genuinely null manager
/// would break every file read in the process, so that census reading `null` was a
/// deref-depth/timing artifact and the conclusion drawn from it does not follow.
pub const DL_FILE_DEVICE_MANAGER_SINGLETON_RVA: usize = 0x0484_64a8;

/// `GLOBAL_CSSessionManager` singleton global -- 1.16.2 runtime VA `0x143d7a4d0`.
///
/// SOLE DECLARATION of this address; `er-invasion-warp-core`'s `SESSION_MANAGER_GLOBAL_RVA` is an
/// alias derived from it. Byte-proven out of `1405f2935: mov 0x3787b94(%rip),%rcx`, and read a
/// second, independent way by `MoveMapStep`'s ending-request evaluator at `0x140afa86b`
/// (`MOV RAX,qword ptr [0x143d7a4d0]` immediately before `CMP dword ptr [RAX + 0x10],0x4`).
///
/// It is here rather than in the warp crate because that second reader is the product's, not the
/// sidecar's: the ending evaluator's `protocolState == WaitReload` term is one of the nine inputs
/// that decide whether the MoveMap child tears the world down (see
/// `er_title_flow::SESSION_PROTOCOL_STATE_WAIT_RELOAD`).
pub const CS_SESSION_MANAGER_GLOBAL_RVA: usize = 0x3d7_a4d0;

/// `GLOBAL_CSEventMan` singleton global -- 1.16.2 runtime VA `0x143d686f8`.
///
/// Byte-proven out of the same ending-request evaluator: `0x140afa8bd`
/// `MOV RCX,qword ptr [0x143d686f8]`, then `MOV RCX,qword ptr [RCX + 0x10]` (the `deadReset`
/// member, `CSEventManImp` field ordinal 2 in the named 1.16.2 dump) and `CALL 0x1405fef50`,
/// which is the one-line getter `return param_1->field1_0x8;`, compared against `2`.
pub const CS_EVENT_MAN_GLOBAL_RVA: usize = 0x3d6_86f8;
