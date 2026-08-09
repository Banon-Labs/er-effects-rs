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
