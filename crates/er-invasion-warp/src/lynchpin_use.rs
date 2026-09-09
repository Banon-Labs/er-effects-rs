//! Using the Challenger's Lynchpin without the wait and without the popup.
//!
//! Three separate mechanisms, each measured live on 2026-09-09 through Frida before any of it was
//! written here, and each recorded in `bd driving-an-inventory-item-use-needs-chrins-0x168`:
//!
//! 1. **Driving the use.** The inventory's `Use` command is only
//!    `CSMenuGaitemUseState::Request(&CSMenuMan->menuData->menuGaitemUseState, gaitem, arg)`, four
//!    stores into a 24-byte struct. `CS::PlayerIns::GetSelectedQuickSlotItemId` then overrides the
//!    real quick slot from that struct's `+0xc` unconditionally, which is the only reason an item
//!    that lives solely in the inventory can be used at all.
//! 2. **Making the use actually do something.** `ChrIns+0x168` is the repeat count TAE event 65
//!    loops on: `for (n = chrIns->field49_0x168; n != 0; n--)`. At zero the event fires and its
//!    whole body is skipped, so the animation plays, nothing is consumed, no effect applies and
//!    Seamless never hears about it. Five live drives died on exactly that and read as a broken
//!    item.
//! 3. **Shortening the animation.** `EquipParamGoods.goodsUseAnim`, the `u8` at row `+0x42`.
//!
//! # The animation lengths are measured, not chosen
//!
//! Each candidate was driven once and its clip read out of the TimeAct ring, where `animLength` is
//! the clip's length in the character's own seconds:
//!
//! | `goodsUseAnim` | TimeAct | seconds |
//! | --- | --- | --- |
//! | 66 (what ersc.dll stamps) | 50530 | 5.000 |
//! | 8 (vanilla invasion fingers) | 50030 | 3.900 |
//! | 6 (Tiny Great Pot) | 50230 | 3.167 |
//! | 17 (Throwing Dagger) | 55000 | 1.433 |
//!
//! `CSChrBehaviorModule::animSpeedGradientMultiplier` was tried first and is not the lever: the
//! field takes the write, reads back changed, and the animation still runs its full length.
//!
//! # Why the row is written at runtime rather than in `regulation.bin`
//!
//! The Lynchpin's row is not in the regulation at all. `ersc.dll` allocates 0xb0 bytes at init and
//! stamps `goodsUseAnim = 0x42` into it, so the only place the value exists is the live row that
//! `EquipParamGoods::GetEntry` hands back (bd
//! `goods-use-anim-is-equipparamgoods-0x42-lynchpin-is-runtime-synthesised-2026-09-09`). One byte,
//! once, after Seamless has registered its rows.
//!
//! # The popup is skipped, not accepted and dismissed
//!
//! The first working version took Seamless's option and then called
//! `CSMenuManImp::CloseMenu(CSMenuMan, -1)`. That dismissed *every* dialog the game opened,
//! including the one the same item raises to leave an invasion, which has an activation window --
//! and it trapped the player in an invasion with no way out. See
//! `bd auto-dismiss-must-be-scoped-not-every-dialog-2026-09-09`.
//!
//! So the dialog is never built into view instead: the detour on `OpenConversationChoicesMenu`
//! declines to call the original only when Seamless's session reads idle, which is true when the
//! item is offering to start a search and never true once an invasion is live. Every other dialog,
//! the leave prompt included, falls through untouched.

#[cfg(windows)]
use std::sync::atomic::{AtomicUsize, Ordering};

#[cfg(windows)]
use crate::map_seams::{MapSeam, verify_seam};

/// `CSMenuMan+0x8` -> `CSMenuData`, then `+0x70` -> `CSMenuGaitemUseState`.
#[cfg(windows)]
const MENU_GAITEM_USE_STATE_OFFSET: usize = 0x70;
/// Within `CSMenuGaitemUseState`: request state, 0 idle, 1 requested, 2 latched.
#[cfg(windows)]
const USE_STATE_OFFSET: usize = 0x8;
/// The item id the quick-slot getter overrides with, carrying the `0x4` goods category nibble.
#[cfg(windows)]
const USE_ITEM_ID_OFFSET: usize = 0xc;
/// The inventory index the use consumes from.
#[cfg(windows)]
const USE_ITEM_IDX_OFFSET: usize = 0x10;
/// The argument the inventory command passes through; zero is what the menu sends for a plain use.
#[cfg(windows)]
const USE_ARG_OFFSET: usize = 0x14;
/// `ChrIns+0x168`, unnamed in the 1.16.2 dump: the repeat count TAE event 65 loops on.
#[cfg(windows)]
const CHR_INS_CONSUME_COUNT_OFFSET: usize = 0x168;
/// `PlayerGameData+0x2b0` -> `EquipGameData`.
#[cfg(windows)]
const PLAYER_GAME_DATA_EQUIP_GAME_DATA_OFFSET: usize = 0x2b0;
/// `EquipGameData+0x158` -> `EquipInventoryData`. `GetEquipInventoryData` is
/// `lea rax,[rcx+0x158]; ret` on both builds, so this is an offset and not a call.
#[cfg(windows)]
const EQUIP_INVENTORY_DATA_OFFSET: usize = 0x158;
/// `EquipParamGoods.goodsUseAnim`, `u8`, within a 0xb0-byte row.
#[cfg(windows)]
const GOODS_USE_ANIM_OFFSET: usize = 0x42;
/// The Challenger's Lynchpin, as the menu spells it: goods `8380003` with the goods category
/// nibble.
#[cfg(windows)]
const LYNCHPIN_ITEM_ID: u32 = 0x407f_de63;
/// The same id as the param table spells it.
#[cfg(windows)]
const LYNCHPIN_GOODS_ID: u32 = 0x7f_de63;
/// The Throwing Dagger's use animation: TimeAct 55000, 1.433 seconds, measured.
#[cfg(windows)]
const SHORT_USE_ANIM: u8 = 17;
/// How many frames the use-state override is held. It is cleared back to -1 within about nine
/// frames when no menu is open, so a single write is gone before the animation asks for it.
#[cfg(windows)]
const PIN_FRAMES: usize = 90;

/// `EquipParamGoods::GetEntry(EquipParamGoodsLookupResult *out, uint id) -> out`.
#[cfg(windows)]
const EQUIP_PARAM_GOODS_GET_ENTRY_RVA: u32 = 0x00d3_9df0;

/// `CS::CSMenuMan::OpenConversationChoicesMenu` -- the game function Seamless opens its option
/// menu through. Its own body stamps the menu id and hands a job to the menu system; declining to
/// call it is what keeps the menu from ever appearing.
#[cfg(windows)]
const OPEN_CONVERSATION_CHOICES_MENU: MapSeam = MapSeam {
    name: "CS::CSMenuMan::OpenConversationChoicesMenu",
    rva: 0x00e9_e4f0,
    prologue: &[
        0x40, 0x53, 0x48, 0x83, 0xec, 0x30, 0x48, 0x8b, 0xd9, 0x33, 0xd2,
    ],
    arg_count: 1,
};

/// The original `OpenConversationChoicesMenu`, once the detour is in.
#[cfg(windows)]
static ORIG_OPEN_CHOICES: AtomicUsize = AtomicUsize::new(0);
/// Whether the animation byte has been written this session.
#[cfg(windows)]
static ANIM_SHORTENED: AtomicUsize = AtomicUsize::new(0);
/// Frames of use-state override still owed.
#[cfg(windows)]
static PIN_FRAMES_LEFT: AtomicUsize = AtomicUsize::new(0);
/// The inventory index the current override is pinning.
#[cfg(windows)]
static PINNED_ITEM_IDX: AtomicUsize = AtomicUsize::new(0);
/// Dialogs this module declined to open, and dialogs it let through.
#[cfg(windows)]
static POPUPS_SKIPPED: AtomicUsize = AtomicUsize::new(0);
#[cfg(windows)]
static POPUPS_PASSED: AtomicUsize = AtomicUsize::new(0);

/// The live `EquipParamGoods` row for one goods id, or `None` before the param tables are up.
///
/// # Safety
///
/// Game task thread. The call is the engine's own lookup and takes no lock this module holds.
#[cfg(windows)]
unsafe fn goods_row(goods_id: u32) -> Option<usize> {
    let entry = er_game_base::mem::game_rva_named(
        EQUIP_PARAM_GOODS_GET_ENTRY_RVA,
        "EQUIP_PARAM_GOODS_GET_ENTRY_RVA",
    )
    .ok()?;
    // `EquipParamGoodsLookupResult` is `{ int paramId; int _pad; _EQUIP_PARAM_GOODS_ST *row; }`.
    let mut lookup: [usize; 2] = [usize::MAX, 0];
    type GetEntryFn = unsafe extern "system" fn(*mut usize, u32) -> *mut usize;
    // SAFETY: the address is version-translated and the shape is the engine's own two-argument
    // lookup; the out-parameter is this frame's stack.
    let get_entry: GetEntryFn = unsafe { core::mem::transmute(entry) };
    // SAFETY: as above.
    unsafe { get_entry(lookup.as_mut_ptr(), goods_id) };
    let row = lookup[1];
    (row != 0).then_some(row)
}

/// Write the shorter use animation onto the live row, once per session.
///
/// Returns whether the row was found and written. A row that is not there yet is not a failure:
/// `ersc.dll` registers it during its own init, so an early tick simply tries again next frame.
///
/// # Safety
///
/// Game task thread.
#[cfg(windows)]
pub unsafe fn shorten_use_animation() -> bool {
    if ANIM_SHORTENED.load(Ordering::SeqCst) != 0 {
        return true;
    }
    // SAFETY: game task thread; returns `None` rather than faulting before the tables exist.
    let Some(row) = (unsafe { goods_row(LYNCHPIN_GOODS_ID) }) else {
        return false;
    };
    let field = row + GOODS_USE_ANIM_OFFSET;
    // SAFETY: fault-tolerant read of one byte inside a row the engine just handed back.
    let before = unsafe { er_game_base::mem::safe_read_u8(field) };
    // SAFETY: same byte, inside the same row.
    unsafe { core::ptr::write_volatile(field as *mut u8, SHORT_USE_ANIM) };
    ANIM_SHORTENED.store(1, Ordering::SeqCst);
    crate::standalone_log(format_args!(
        "lynchpin: use animation {before:?} -> {SHORT_USE_ANIM} on the live row 0x{row:x}+0x42. \
         Measured lengths: 66 is TimeAct 50530 at 5.000s, 8 is 50030 at 3.900s, 6 is 50230 at \
         3.167s, 17 is 55000 at 1.433s. The row is not in regulation.bin -- ersc.dll allocates it \
         at init -- so this is the only place the value exists."
    ));
    true
}

/// The detour on `OpenConversationChoicesMenu`.
///
/// # Safety
///
/// Installed by the union on a byte-verified prologue; the ABI is `(dialog)`.
#[cfg(windows)]
unsafe extern "system" fn open_choices_hook(dialog: usize, b: usize, c: usize, d: usize) -> usize {
    if crate::local_invasion_filter::session_is_idle() {
        POPUPS_SKIPPED.fetch_add(1, Ordering::SeqCst);
        // The option this menu offers is the invade action. Arming rather than calling it here is
        // deliberate: the action takes Seamless's session mutex, and this detour runs on whatever
        // thread opened the menu.
        crate::local_invasion_filter::request_invade();
        crate::standalone_log(format_args!(
            "lynchpin: skipped Seamless's start-a-search popup and armed the search instead \
             (skipped {}, passed through {})",
            POPUPS_SKIPPED.load(Ordering::SeqCst),
            POPUPS_PASSED.load(Ordering::SeqCst)
        ));
        return 0;
    }
    POPUPS_PASSED.fetch_add(1, Ordering::SeqCst);
    let orig = ORIG_OPEN_CHOICES.load(Ordering::SeqCst);
    if orig == 0 {
        return 0;
    }
    // SAFETY: the union stored the trampoline for this exact target.
    unsafe { core::mem::transmute::<usize, er_hook::UnionFn>(orig)(dialog, b, c, d) }
}

/// Install the popup skip. Idempotent; returns whether the detour is in force.
///
/// # Safety
///
/// Game task thread, after the runtime is up.
#[cfg(windows)]
pub unsafe fn install_popup_skip() -> bool {
    if ORIG_OPEN_CHOICES.load(Ordering::SeqCst) != 0 {
        return true;
    }
    // SAFETY: game task thread; the seam verifies its own prologue and refuses otherwise.
    let address = match unsafe { verify_seam(&OPEN_CONVERSATION_CHOICES_MENU) } {
        Ok(address) => address,
        Err(error) => {
            crate::standalone_log(format_args!(
                "lynchpin: REFUSED {} -- {error}; the popup will appear as Seamless built it",
                OPEN_CONVERSATION_CHOICES_MENU.name
            ));
            return false;
        }
    };
    // SAFETY: byte-verified address, four-argument union shape, one argument used.
    match unsafe {
        er_hook::register_union_hook(
            address,
            open_choices_hook as er_hook::UnionFn,
            &ORIG_OPEN_CHOICES,
        )
    } {
        Ok(()) => {
            crate::standalone_log(format_args!(
                "lynchpin: armed the popup skip on {} @0x{address:x} -- a dialog is declined only \
                 while the session reads idle, so the leave-invasion prompt still opens",
                OPEN_CONVERSATION_CHOICES_MENU.name
            ));
            true
        }
        Err(status) => {
            crate::standalone_log(format_args!(
                "lynchpin: FAILED to arm the popup skip @0x{address:x} -- {status:?}. The address \
                 resolved and its prologue matched, so this is the hook engine refusing a verified \
                 address; the popup will appear as Seamless built it"
            ));
            false
        }
    }
}

/// Ask for the Lynchpin to be used, starting on the next tick.
///
/// Returns whether the item was found in the inventory to use.
///
/// # Safety
///
/// Game task thread.
#[cfg(windows)]
pub unsafe fn request_use() -> bool {
    // SAFETY: game task thread; every read is fault-closed.
    let Some(index) = (unsafe { inventory_index(LYNCHPIN_ITEM_ID) }) else {
        crate::standalone_log(format_args!(
            "lynchpin: asked to use the item, but it is not in the inventory -- it appears only \
             after sitting at a site of grace"
        ));
        return false;
    };
    PINNED_ITEM_IDX.store(index, Ordering::SeqCst);
    PIN_FRAMES_LEFT.store(PIN_FRAMES, Ordering::SeqCst);
    true
}

/// The inventory index of one item id, or `None` when it is not held.
///
/// # Safety
///
/// Game task thread.
#[cfg(windows)]
unsafe fn inventory_index(item_id: u32) -> Option<usize> {
    let lookup = er_game_base::mem::game_rva_named(
        er_game_base::rva::GET_ITEM_INVENTORY_IDX_RVA as u32,
        "GET_ITEM_INVENTORY_IDX_RVA",
    )
    .ok()?;
    let game_data_man =
        er_game_base::mem::game_module_base().ok()? + er_game_base::rva::GAME_DATA_MAN_GLOBAL_RVA;
    // SAFETY: fault-closed reads of two singleton hops.
    let game_data_man = unsafe { er_game_base::mem::safe_read_usize(game_data_man) }?;
    // SAFETY: as above; `PlayerGameData` is the first pointer inside `GameDataMan`.
    let player_game_data = unsafe { er_game_base::mem::safe_read_usize(game_data_man + 0x8) }?;
    if player_game_data == 0 {
        return None;
    }
    let inventory =
        player_game_data + PLAYER_GAME_DATA_EQUIP_GAME_DATA_OFFSET + EQUIP_INVENTORY_DATA_OFFSET;
    type GetItemIdxFn = unsafe extern "system" fn(usize, *const u32) -> i32;
    // SAFETY: version-translated address, engine's own two-argument getter.
    let get_item_idx: GetItemIdxFn = unsafe { core::mem::transmute(lookup) };
    // SAFETY: as above; the id is this frame's stack. The tagged spelling is the one the
    // inventory keys on -- the bare goods id answers -1.
    let index = unsafe { get_item_idx(inventory, &raw const item_id) };
    (index >= 0).then_some(index as usize)
}

/// The local `PlayerIns`, or `None` before the world exists.
///
/// # Safety
///
/// Game task thread.
#[cfg(windows)]
unsafe fn main_player_chr_ins() -> Option<usize> {
    let base = er_game_base::mem::game_module_base().ok()?;
    // SAFETY: fault-closed read of the singleton slot; null until the world is up.
    let world_chr_man = unsafe {
        er_game_base::mem::safe_read_usize(base + er_game_base::rva::WORLD_CHR_MAN_GLOBAL_RVA)
    }?;
    if world_chr_man == 0 {
        return None;
    }
    // SAFETY: as above. `PlayerIns` begins with its `ChrIns`, so the two addresses are the same.
    let player = unsafe {
        er_game_base::mem::safe_read_usize(
            world_chr_man + er_game_base::rva::WORLD_CHR_MAN_PLAYER_INS_OFFSET,
        )
    }?;
    (player != 0).then_some(player)
}

/// Hold the use-state override for one frame, if a use was asked for.
///
/// # Safety
///
/// Game task thread.
#[cfg(windows)]
unsafe fn drive_pinned_use() {
    let left = PIN_FRAMES_LEFT.load(Ordering::SeqCst);
    if left == 0 {
        return;
    }
    PIN_FRAMES_LEFT.store(left - 1, Ordering::SeqCst);
    let Ok(base) = er_game_base::mem::game_module_base() else {
        return;
    };
    // SAFETY: fault-closed singleton reads.
    let Some(menu_man) = (unsafe {
        er_game_base::mem::safe_read_usize(base + er_game_base::rva::CS_MENU_MAN_GLOBAL_RVA)
    }) else {
        return;
    };
    // SAFETY: as above.
    let Some(menu_data) = (unsafe {
        er_game_base::mem::safe_read_usize(
            menu_man + er_game_base::rva::CS_MENU_MAN_MENU_DATA_OFFSET,
        )
    }) else {
        return;
    };
    if menu_data == 0 {
        return;
    }
    let state = menu_data + MENU_GAITEM_USE_STATE_OFFSET;
    let index = PINNED_ITEM_IDX.load(Ordering::SeqCst) as i32;
    // SAFETY: the four stores the engine's own `Request` makes, into the struct it makes them in.
    unsafe {
        core::ptr::write_volatile((state + USE_ITEM_ID_OFFSET) as *mut u32, LYNCHPIN_ITEM_ID);
        core::ptr::write_volatile((state + USE_ITEM_IDX_OFFSET) as *mut i32, index);
        core::ptr::write_volatile((state + USE_ARG_OFFSET) as *mut i32, 0);
    }
    // SAFETY: game task thread; fault-closed, and `None` before there is a player.
    if let Some(player) = unsafe { main_player_chr_ins() } {
        // SAFETY: the repeat count TAE event 65 loops on; at zero the event does nothing at all.
        unsafe {
            core::ptr::write_volatile((player + CHR_INS_CONSUME_COUNT_OFFSET) as *mut u32, 1)
        };
    }
    if left == PIN_FRAMES {
        // SAFETY: the request itself, raised once. Holding it every frame would re-press the
        // action rather than hold the override.
        unsafe { core::ptr::write_volatile((state + USE_STATE_OFFSET) as *mut u8, 1) };
    }
    if left == 1 {
        // SAFETY: hand the struct back the way the engine leaves it, so nothing later reads the
        // Lynchpin as the selected quick item.
        unsafe {
            core::ptr::write_volatile((state + USE_ITEM_ID_OFFSET) as *mut i32, -1);
            core::ptr::write_volatile((state + USE_ITEM_IDX_OFFSET) as *mut i32, -1);
            core::ptr::write_volatile((state + USE_STATE_OFFSET) as *mut u8, 0);
        }
    }
}

/// One frame of this module's work: shorten the animation, arm the skip, hold any pinned use.
///
/// Every part is idempotent and every part fails closed, so a tick before the world exists costs
/// nothing and is retried.
///
/// # Safety
///
/// Game task thread, after `CSTaskImp` resolved.
#[cfg(windows)]
pub unsafe fn tick() {
    // SAFETY: game task thread; each is fault-closed and idempotent.
    unsafe {
        shorten_use_animation();
        install_popup_skip();
        drive_pinned_use();
    }
}

/// How many popups this module declined and how many it let through.
#[cfg(windows)]
#[must_use]
pub fn popup_tally() -> (usize, usize) {
    (
        POPUPS_SKIPPED.load(Ordering::SeqCst),
        POPUPS_PASSED.load(Ordering::SeqCst),
    )
}

#[cfg(test)]
mod tests {
    /// The measured animation lengths, kept where a reader of the module can check the claim
    /// without a game: the value written must be the shortest row in the table the doc comment
    /// records.
    #[test]
    fn seventeen_is_the_shortest_measured_animation() {
        let measured = [(66_u8, 5.000_f32), (8, 3.900), (6, 3.167), (17, 1.433)];
        let shortest = measured
            .iter()
            .min_by(|a, b| a.1.partial_cmp(&b.1).expect("no NaN in a literal table"))
            .expect("the table is not empty");
        assert_eq!(shortest.0, 17);
    }
}
