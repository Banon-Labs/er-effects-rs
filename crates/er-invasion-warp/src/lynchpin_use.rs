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
/// Whether the row lookup has already said it could not resolve. Without this the tick says so
/// once a frame: the first build of this module wrote 2,685 identical refusal lines into one
/// session's log, which is the shape that makes a log unreadable rather than informative.
#[cfg(windows)]
static ROW_REFUSAL_SAID: AtomicUsize = AtomicUsize::new(0);
/// The same latch for the popup skip, for the same reason.
#[cfg(windows)]
static SKIP_REFUSAL_SAID: AtomicUsize = AtomicUsize::new(0);
/// Frames of use-state override still owed.
#[cfg(windows)]
static PIN_FRAMES_LEFT: AtomicUsize = AtomicUsize::new(0);
/// The inventory index the current override is pinning.
#[cfg(windows)]
static PINNED_ITEM_IDX: AtomicUsize = AtomicUsize::new(0);
/// Whether the pass-through branch has said so once. It was entirely silent until 2026-09-09,
/// which is how a log showing one skip and nothing else read as the feature working while every
/// use after the first showed the dialog.
#[cfg(windows)]
static PASS_REPORTED: AtomicUsize = AtomicUsize::new(0);
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
    let base = er_game_base::mem::game_module_base().ok()?;
    // The engine's own precondition, and the reason the first build of this module killed the
    // process 1140ms into boot. `EquipParamGoods::GetEntry` opens by loading
    // `GLOBAL_SoloParamRepository` and, when it is null, calls the assert at 1.17.1
    // `0x141ebb610` with file `0x1429caaa0` line 0xb4 -- and that assert path itself faults on a
    // null `rcx` this early, so the refusal is a `0xc0000005`, not a returned error. Reading the
    // slot first turns the engine's fatal precondition into this function's `None`.
    if er_game_base::mem::read_global_ptr(
        base,
        er_game_base::rva::SOLO_PARAM_REPOSITORY_GLOBAL_RVA,
        "SOLO_PARAM_REPOSITORY_GLOBAL_RVA",
    ) == 0
    {
        return None;
    }
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
        if ROW_REFUSAL_SAID.swap(1, Ordering::SeqCst) == 0 {
            crate::standalone_log(format_args!(
                "lynchpin: the goods row is not resolvable yet -- either the param tables are not \
                 up or the lookup address was refused for this build. Retried every tick; this \
                 line is printed once."
            ));
        }
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

/// `r14` as it stood when `OpenConversationChoicesMenu` was entered. The option-menu object is
/// `r14 - 0x120`.
///
/// # Measured, on the live game, not derived
///
/// A read-only Frida agent (`scripts/frida/ersc-owner-truth.js`) watched `ersc+0x241a0` and the
/// dialog opener in the same use of the item, on run `br-20260910-003108-c109`:
///
/// ```text
///   show:        osm=0x466ad518  session=0x466ac930  state=0x1  r12=0x7fde63
///   open_dialog: r12=0x45eae1c8  r14=0x466ad638  ret=0x18002438e
/// ```
///
/// `0x466ad638 - 0x120 == 0x466ad518`, exactly the object `show` was called with, and the return
/// address `0x18002438e` is inside `show` itself, so the opener is called from there directly. The
/// `0x120` is not a fitted constant either -- it is the third instruction of `show`:
///
/// ```text
///   ersc+0x241c8: mov  r12, rcx            <- rcx is the menu object
///   ersc+0x241cb: lea  r14, [rcx + 0x120]  <- and r14 keeps a fixed interior pointer to it
/// ```
///
/// Two earlier builds read `r12` instead, on the reasoning that `mov r12, rcx` makes it the menu
/// object. The register dump above is why that failed twice: at the OPENER `r12` is `0x45eae1c8`,
/// something else entirely, and at `show`'s own entry it is `0x7fde63` -- the Lynchpin's goods id,
/// arriving from show's caller. `r12` is only the menu object between `+0x241c8` and whatever
/// reuses it, which does not include this frame.
///
/// Nothing is written into `ersc.dll` for this: it is a register read in our own frame, so the
/// MinHook patch that faults a Themida-protected module at `0x140010043` is not involved.
///
/// A wrong read still cannot be believed -- `adopt_menu_object` requires `+0x58` to lead to an
/// object carrying a live session state before it stores anything.
#[cfg(windows)]
static SHOW_R14: AtomicUsize = AtomicUsize::new(0);

/// How far into the menu object `show` keeps `r14` pointing: `lea r14, [rcx + 0x120]`.
#[cfg(windows)]
const SHOW_R14_INTERIOR_OFFSET: usize = 0x120;

/// The registered detour: capture `r14`, then tail-jump to the real handler with the arguments
/// untouched.
///
/// It has to be naked. A plain Rust function may spill or reuse `r14` in its own prologue, so the
/// capture must be the very first instruction executed at the detour address.
///
/// # Safety
///
/// Installed as a bare detour on a byte-verified prologue; the jump preserves every argument
/// register.
#[cfg(windows)]
#[unsafe(naked)]
unsafe extern "system" fn open_choices_entry(a: usize, b: usize, c: usize, d: usize) -> usize {
    core::arch::naked_asm!(
        "mov qword ptr [rip + {slot}], r14",
        "jmp {handler}",
        slot = sym SHOW_R14,
        handler = sym open_choices_hook,
    )
}

/// The detour on `OpenConversationChoicesMenu`.
///
/// # Safety
///
/// Installed by the union on a byte-verified prologue; the ABI is `(dialog)`.
#[cfg(windows)]
unsafe extern "system" fn open_choices_hook(dialog: usize, b: usize, c: usize, d: usize) -> usize {
    // The Frida agent's `Interceptor.attach(ersc+0x241a0)` did this; the register read replaces it.
    // `adopt_menu_object` validates before storing, so a frame whose `r12` is not the menu object
    // is refused and the gate simply falls back.
    let menu_object = SHOW_R14
        .load(Ordering::SeqCst)
        .wrapping_sub(SHOW_R14_INTERIOR_OFFSET);
    let adopted = crate::local_invasion_filter::menu_object::adopt_menu_object(menu_object);
    let (idle, source) = crate::local_invasion_filter::popup_skip_gate_is_idle();
    if idle {
        POPUPS_SKIPPED.fetch_add(1, Ordering::SeqCst);
        // Inline, on this thread, which is the whole difference between this and the version that
        // hard-locked the game.
        //
        // Declining the dialog means the option is never chosen, so if nothing calls the invade
        // action no search starts at all -- the popup vanishes and the item does nothing. The
        // Frida prototype called `invadeAction(menuObject)` right here, from the replacement
        // itself, on the thread Seamless had already driven into: one entrant, no contention.
        //
        // `request_invade()` was used instead and it deadlocked, because arming moves the call to
        // the game task tick, and the player's next item use puts the game's own goods path inside
        // `ersc.dll` holding the session mutex at the same moment (run `br-20260909-234803-535c`:
        // `about to drive ERSC invade` with no successor line, 122 threads, 9 cpu ticks in 3s).
        //
        // `inside_ersc_callback()` does not refuse this: that guard is set by the ersc observers,
        // and this detour is on a game function and never enters it.
        // Through the real menu object when we have it, which is what Frida passed. The
        // synthesized-owner path is the fallback and it is the one that wedged the game, so it is
        // only reached when the register capture was refused.
        let started = if adopted {
            crate::local_invasion_filter::drive_invade_with_owner(
                menu_object,
                "the Lynchpin's own use",
            )
        } else {
            crate::local_invasion_filter::drive_invade_inline("the Lynchpin's own use")
        };
        crate::standalone_log(format_args!(
            "lynchpin: skipped Seamless's start-a-search popup and started the search inline \
             (started={started}, skipped {}, passed through {}) -- gate answered from {source}",
            POPUPS_SKIPPED.load(Ordering::SeqCst),
            POPUPS_PASSED.load(Ordering::SeqCst)
        ));
        return 0;
    }
    let passed = POPUPS_PASSED.fetch_add(1, Ordering::SeqCst) + 1;
    if PASS_REPORTED.swap(1, Ordering::SeqCst) == 0 {
        crate::standalone_log(format_args!(
            "lynchpin: a dialog was let through because the session does not read idle \
             (passed {passed}, skipped {}) -- gate answered from {source}. If this was \
             the item's own start-a-search prompt then the gate is wrong, not the \
             dialog. Printed once.",
            POPUPS_SKIPPED.load(Ordering::SeqCst)
        ));
    }
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
            if SKIP_REFUSAL_SAID.swap(1, Ordering::SeqCst) == 0 {
                crate::standalone_log(format_args!(
                    "lynchpin: refused {} -- {error}; the popup will appear as Seamless built it. \
                     This line is printed once, not once per tick.",
                    OPEN_CONVERSATION_CHOICES_MENU.name
                ));
            }
            return false;
        }
    };
    // A bare `MhHook`, deliberately NOT `register_union_hook`, and the reason is the register
    // capture above.
    //
    // The union does not put `open_choices_entry` at the detour address -- it installs
    // `union_dispatch`, an ordinary Rust function that loads its head handler and calls it. That
    // prologue runs before the naked shim does, and it does not preserve `r12` for a callee: on
    // run `br-20260910-000334-453e` the capture came back `0x1` and the log said `REFUSED an
    // option-menu object handed in at 0x1`. A bare hook puts the shim's first instruction AT the
    // detour address, which is the only place the register is still Seamless's.
    //
    // The cost is the union's one guarantee: if another feature ever hooks this same function,
    // MinHook binds one detour per address and one of the two is silently dropped. Nothing else
    // in this DLL touches `OpenConversationChoicesMenu` today, and a capture that reads `1` is
    // worth nothing at all, so the trade is made here and written down rather than assumed.
    let hook = match unsafe {
        er_hook::MhHook::new(
            address as *mut core::ffi::c_void,
            open_choices_entry as *mut core::ffi::c_void,
        )
    } {
        Ok(hook) => hook,
        Err(status) => {
            crate::standalone_log(format_args!(
                "lynchpin: FAILED to create the popup-skip detour @0x{address:x} -- {status:?}. \
                 The address resolved and its prologue matched, so this is MinHook refusing a \
                 verified address; the popup will appear as Seamless built it"
            ));
            return false;
        }
    };
    ORIG_OPEN_CHOICES.store(hook.trampoline() as usize, Ordering::SeqCst);
    // SAFETY: the hook was created above; enabling is MinHook's own queued path.
    if unsafe { hook.queue_enable() }.is_err() {
        ORIG_OPEN_CHOICES.store(0, Ordering::SeqCst);
        return false;
    }
    // SAFETY: applies the queue this function just added to.
    match unsafe { er_hook::MH_ApplyQueued() } {
        er_hook::MH_STATUS::MH_OK => {
            crate::standalone_log(format_args!(
                "lynchpin: armed the popup skip on {} @0x{address:x} as a bare detour, so `r14` \
                 still points 0x120 into Seamless's option-menu object when it runs (measured, \
                 not assumed: `lea r14,[rcx+0x120]` at ersc+0x241cb, and a live dump gave \
                 r14=0x466ad638 against show's own osm=0x466ad518) -- a dialog is declined only \
                 while that object's session reads idle, so the leave-invasion prompt still opens",
                OPEN_CONVERSATION_CHOICES_MENU.name
            ));
            true
        }
        status => {
            ORIG_OPEN_CHOICES.store(0, Ordering::SeqCst);
            crate::standalone_log(format_args!(
                "lynchpin: FAILED to enable the popup skip @0x{address:x} -- {status:?}"
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
    let base = er_game_base::mem::game_module_base().ok()?;
    let game_data_man = er_game_base::mem::read_global_ptr(
        base,
        er_game_base::rva::GAME_DATA_MAN_GLOBAL_RVA,
        "GAME_DATA_MAN_GLOBAL_RVA",
    );
    if game_data_man == 0 {
        return None;
    }
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
    // Null until the world is up. Resolved for the running build rather than read raw: every
    // `.data` global moved on 1.17, so a raw read succeeds and hands back whatever now occupies
    // the 1.16.2 slot -- and this pointer is written through.
    let world_chr_man = er_game_base::mem::read_global_ptr(
        base,
        er_game_base::rva::WORLD_CHR_MAN_GLOBAL_RVA,
        "WORLD_CHR_MAN_GLOBAL_RVA",
    );
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
    // Resolved rather than read raw, for the reason in `main_player_chr_ins`: this pointer is
    // written through, and a stale global would put those writes on whatever moved into the slot.
    let menu_man = er_game_base::mem::read_global_ptr(
        base,
        er_game_base::rva::CS_MENU_MAN_GLOBAL_RVA,
        "CS_MENU_MAN_GLOBAL_RVA",
    );
    if menu_man == 0 {
        return;
    }
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
    // No `install_menu_object_observer()` call here, and this is not an omission.
    //
    // Detouring `ersc.dll` to capture the menu object is what the local-invasion filter already
    // gates off, and installing it from here reproduced that crash exactly, measured on run
    // `br-20260909-234159-0a54`: `observing ersc show @0x1800241a0` at boot, then at +29493ms a
    // read fault at `game+0x11f42` -- the `0x140010043` page MinHook's ersc patch faults on --
    // followed by 215
    // recursive access violations with `rsp` marching down 0x1260 a frame until the thread was
    // gone. On screen that is a loading screen that never finishes. The filter's comment says
    // "~50s and 30.6s"; this was 29.5s.
    //
    // So the menu object has to arrive without a detour inside Seamless. `resolve_session` already
    // reaches the session that way -- "via a pointer in ersc's own writable data at 0x1806088f0"
    // -- and `er_invasion_warp_adopt_menu_object` takes one from outside the DLL. Until one of
    // those supplies `OSM`, `menu_object_session_is_idle` answers false and every dialog is let
    // through, which shows the popup rather than swallowing the prompt that leaves an invasion.
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
