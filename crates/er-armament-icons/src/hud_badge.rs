//! Ash-of-War badge on the in-game HUD weapon slots (`01_000_fe.gfx`).
//!
//! # Why this needs its own hooks
//!
//! The menu badge rides `TilePopulate` (`FUN_1408ff470`), which never reaches HUD tiles: across
//! 9216 populates in run 20260727-233703, ZERO bound `Dish`/`MpShortage`/`ReloadedIcon` -- the
//! HUD quick-slot child signature -- and the child probe already tests `Dish/Root`, so that is a
//! tested negative rather than an assumption.
//!
//! # The HUD slot map (1.16.2, read out of `PlayerHUDScene`)
//!
//! `PlayerHUDScene` builds seven `ItemPanel` slots; the per-frame driver `FUN_1408d0900`
//! updates each one. Component pointers are offsets into the scene object:
//!
//! ```text
//!   path                            component      ctor            updater
//!   PlayerHUD/ItemPanel/Magic       scene+0x0b70   FUN_1408d19b0   FUN_1408d2320
//!   PlayerHUD/ItemPanel/Item        scene+0x15d8   FUN_1408d19b0   FUN_1408d2320
//!   PlayerHUD/ItemPanel/LeftWep     scene+0x2040   FUN_1408d1d00   FUN_1408d2110
//!   PlayerHUD/ItemPanel/RightWep    scene+0x2848   FUN_1408d1d00   FUN_1408d2110
//!   PlayerHUD/ItemPanel/ArrowBolt   scene+0x3050   FUN_1408d1b90   FUN_1408d2410
//!   PlayerHUD/ItemPanel/Arts        scene+0x3ff8   FUN_1408d1d00   FUN_1408d2110
//!   PlayerHUD/ItemPanel2/Item_0..3  scene+0x48d0 + i*0x808         FUN_1408d2110
//! ```
//!
//! This table is the correction to an earlier wrong reading. `FUN_1408d19b0` was taken for "the
//! armament slot ctor called exactly twice", and it IS called exactly twice -- for **Magic** and
//! **Item**. So every earlier HUD run drove the spell and quick-item slots, which is precisely
//! what the user saw: green placeholder squares on the quick-item strip and never anything on
//! the weapons. `scripts/disas-annotate-strings.py 0x1408cf3c0` prints the table above straight
//! out of the image; `crates/er-gfx/tests/hud_tree_probe.rs` asserts the movie side of it.
//!
//! # The three hooks
//!
//! * `FUN_1408d0900(scene, alpha, viewModel)` -- the per-frame slot driver. Hooked ONLY to learn
//!   the scene pointer, which is what turns a component into a slot identity. Without it the two
//!   weapon components are indistinguishable except by address ordering, and ordering is a guess.
//!
//! * `FUN_1408d1d00(component, panelClip)` -- the weapon slot ctor (LeftWep, RightWep, Arts and
//!   the four `ItemPanel2` items). It resolves `Fade/Item` from the panel clip -- falling back to
//!   `Fade` when absent, which is how the icon-less `Arts` slot goes through the same ctor -- and
//!   hands that clip to the generic child binder. The ctor knows the COMPONENT, the binder knows
//!   the CLIP, and neither knows both, so the two are paired by a same-thread handshake.
//!
//! * `FUN_1408d1e30(component+0x68, tileClip, textClip)` -- the generic child binder, hooked for
//!   its `rdx` only. It binds `ItemIcon/IconImage`, `AttributeIcon/IconImage`, `Dish/Root`,
//!   `Grayout`, `Flash`, `MpShortage`, `inadequacy`, `ReloadedIcon/IconImage`, `Text/Name`,
//!   `Text/Stock` -- and finally resolves a child literally named `ArtsIcon` and hides it.
//!
//!   That last hide CANNOT reach this badge: the resolver (`FUN_140d7f9d0`) splits the requested
//!   name on `/` and walks one member per segment (`strchr(name, '/')` at `0x140d7fa19`), so it
//!   has no recursive search and a request for `ArtsIcon` never matches `ItemIcon/ArtsIcon`. The
//!   nested mount is therefore safe by construction, not by luck.
//!
//! * `FUN_1408d2110(component, alpha, slotData)` -- the weapon slot update. Shared by seven
//!   components, so the scene offset (above) is what selects the two that draw.
//!
//! # Float arguments
//!
//! Both `FUN_1408d0900` and `FUN_1408d2110` take a `float` in `xmm1` (each saves it immediately:
//! `movaps xmm6, xmm1`). A detour declared `fn(usize, usize, usize)` leaves `xmm1` as a volatile
//! register the detour body may clobber before it reaches the trampoline, which would corrupt the
//! HUD fade value. Declaring the parameter as `f32` in position 1 puts it in `xmm1` under the
//! win64 ABI and passes it through untouched.
//!
//! # Where the ash comes from
//!
//! NOT from the HUD slot struct: `FUN_1408d0900` takes its data from a HUD view-model, which is
//! filled elsewhere. The equipped weapon is read directly instead, through the same named chain
//! the game uses:
//!
//! ```text
//! WorldChrMan(+0x1e508)          -> PlayerIns
//! GetWeaponGaitemHandleBySlot    -> gaitem handle for a ChrAsmSlot
//! GetGaitemInsByHandle           -> GaitemLookupResult
//! GetSwordArtsParamForWeapon     -> SwordArtsParamLookupResult   (via the real equipped GEM)
//! row + 0x1A                     -> SwordArtsParam.iconId
//! ```
//!
//! `GetSwordArtsParamIdForWeapon` resolves through `GetGemGaitemHandleFromWeapon` ->
//! `GetGaitemInsGem`, i.e. the ACTUAL equipped gem, so unlike the menu path's `arts_id * 100`
//! heuristic it does not miss weapons whose gem id is not derived from the arts id (Igon's Drake
//! Hunt: arts 4210 -> gem 548000).
//!
//! Every RVA below was verified with `scripts/verify-hook-address.py`, which compares MNEMONIC
//! and instruction LENGTH between the 1.16.2 dump and `eldenring-deobf.bin` -- the dump is
//! authoritative for MEANING, the deobf binary for ADDRESSES, and they are not the same file.

#![cfg(windows)]

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use er_game_base::mem::safe_read_usize;

use crate::log_message;

// -- verified deobf RVAs (`scripts/verify-hook-address.py`, mnemonic + length vs the dump) --

/// `FUN_1408d0900(scene /*rcx*/, alpha /*xmm1*/, viewModel /*r8*/)` -- per-frame slot driver.
///
/// Hooked purely to capture `scene`. Its prologue does `mov rsi, rcx` and every slot update it
/// issues is `lea rcx, [rsi + <offset>]`, so `rcx` here is the base those offsets are relative
/// to. One call site only (`FUN_1408c8b70`).
const HUD_SCENE_UPDATE_RVA: usize = 0x8d0900;
/// `FUN_1408d1d00(component /*rcx*/, panelClip /*rdx*/)` -- weapon slot ctor.
const HUD_WEAPON_SLOT_CTOR_RVA: usize = 0x8d1d00;
/// `FUN_1408d1e30(subComponent /*rcx*/, tileClip /*rdx*/, textClip /*r8*/)` -- child binder.
///
/// Hooked for its `rdx` ONLY. The ctor does not forward its own `rdx`: it resolves `Fade/Item`
/// (or `Fade`) from the panel clip and passes THAT, so the ctor's argument is the panel, not the
/// tile. Its `rcx` is `component + 0x68`, which is why it cannot key the registry itself.
const HUD_CHILD_BINDER_RVA: usize = 0x8d1e30;
/// `FUN_1408d2110(component /*rcx*/, alpha /*xmm1*/, slotData /*r8*/)` -- weapon slot update.
const HUD_WEAPON_SLOT_UPDATE_RVA: usize = 0x8d2110;

/// `PlayerHUD/ItemPanel/LeftWep` and `/RightWep`, as offsets into the `PlayerHUDScene` object.
///
/// These two constants are the entire slot-identity mechanism, and they are exact rather than
/// inferred: `PlayerHUDScene` constructs at `lea rbx, [r13 + 0x2040]` / `[r13 + 0x2848]` and the
/// driver updates at `lea rcx, [rsi + 0x2040]` / `[rsi + 0x2848]`. Any other component reaching
/// the update hook -- `Arts`, the four `ItemPanel2` items -- fails both tests and never draws.
const HUD_SLOT_LEFT_WEP_OFFSET: usize = 0x2040;
const HUD_SLOT_RIGHT_WEP_OFFSET: usize = 0x2848;

/// `GetWeaponGaitemHandleBySlot(PlayerIns*, u32* out, ChrAsmSlot)`.
const GET_WEAPON_GAITEM_HANDLE_BY_SLOT_RVA: usize = 0x656920;
/// `GetGaitemInsByHandle(GaitemLookupResult* inout, GaitemLookupResult* handleSource)`.
///
/// Both arguments are the SAME pointer in every caller -- see [`GaitemLookupResult`] for why,
/// and for the silent-nothing failure mode of getting it wrong.
const GET_GAITEM_INS_BY_HANDLE_RVA: usize = 0x672e40;
/// `GetSwordArtsParamForWeapon(GaitemLookupResult*, SwordArtsParamLookupResult* out)`.
const GET_SWORD_ARTS_PARAM_FOR_WEAPON_RVA: usize = 0x673f30;

/// `WorldChrMan` singleton global; `+0x1e508` is the local `PlayerIns`. Both dereferences are
/// null-checked in the game's own code, so both are null-checked here.
const WORLD_CHR_MAN_GLOBAL_RVA: usize = 0x3d65f88;
const WORLD_CHR_MAN_PLAYER_INS_OFFSET: usize = 0x1e508;

/// `SwordArtsParam.iconId`.
const SWORD_ARTS_PARAM_ICON_ID_OFFSET: usize = 0x1a;

/// The nested badge path. The binder force-hides a child literally named `ArtsIcon`, so the
/// injected clip lives one level down inside the classless `ItemIcon` container -- out of reach
/// of a resolver that walks one path segment at a time.
const BADGE_PATH: &std::ffi::CStr = c"ItemIcon/ArtsIcon";

/// `ChrAsmSlot` selectors for "the weapon currently cycled into each hand".
///
/// Negative slots are SELECTORS, not indices: `FUN_1403be430` validates `slot + 6 <= 0x12` and
/// jumps through a table at `0x1403be508` to resolve the player's current cycle position into a
/// concrete ChrAsm equipment index. That resolution is what makes these the right thing to ask
/// for -- a raw index would ignore which of the three slots per hand the player has selected.
///
/// The table decodes as:
///
/// ```text
///   slot -2  ->  kind 0, index = sel*2      -> 0, 2, 4    (even)
///   slot -1  ->  kind 1, index = sel*2 + 1  -> 1, 3, 5    (odd)
///   slot -6/-5 -> index = sel*2 + 6         -> 6, 8       (arrows)
///   slot -4/-3 -> index = sel*2 + 7         -> 7, 9       (bolts)
/// ```
///
/// ChrAsm's weapon block interleaves the hands -- 0 = Right 1, 1 = Left 1, 2 = Right 2, 3 = Left
/// 2, 4 = Right 3, 5 = Left 3 -- so EVEN is the right hand and ODD is the left. The ammo rows
/// confirm the reading independently: `sel*2 + 6` lands on 6/8 (Arrow 1/2) and `sel*2 + 7` on
/// 7/9 (Bolt 1/2), exactly the known layout.
///
/// So **-2 is RIGHT and -1 is LEFT**. These two constants were previously assigned the other way
/// round, which would have drawn each hand's Ash of War on the opposite slot -- a swap that still
/// produces two plausible-looking badges and is invisible unless the hands hold different ashes.
const CHR_ASM_SLOT_RIGHT_ACTIVE: i32 = -2;
const CHR_ASM_SLOT_LEFT_ACTIVE: i32 = -1;

const PROXY_SIZE: usize = 0x60;
const PROXY_SCALEFORM_VALUE_OFFSET: usize = 0x28;
const ICON_INFO_SIZE: usize = 0x40;

type SceneUpdateFn = unsafe extern "system" fn(usize, f32, usize) -> usize;
type CtorFn = unsafe extern "system" fn(usize, usize) -> usize;
type BinderFn = unsafe extern "system" fn(usize, usize, usize) -> usize;
type SlotUpdateFn = unsafe extern "system" fn(usize, f32, usize) -> usize;
type AssignFn = unsafe extern "system" fn(usize, *mut u8, *const i8) -> *mut u8;
type IsBoundFn = unsafe extern "system" fn(*const u8) -> bool;
type IconSetterFn = unsafe extern "system" fn(*mut u8, *const u8) -> usize;
type SetVisibleFn = unsafe extern "system" fn(*mut u8, bool);
type ScaleformValueDtorFn = unsafe extern "system" fn(*mut u8);
type IconInfoBuilderFn = unsafe extern "system" fn(*mut u8, u32) -> *mut u8;
type GetWeaponHandleFn = unsafe extern "system" fn(usize, *mut u32, i32) -> *mut u32;
type GetGaitemInsFn =
    unsafe extern "system" fn(*mut GaitemLookupResult, *mut GaitemLookupResult) -> *mut u8;
type GetArtsForWeaponFn =
    unsafe extern "system" fn(*const GaitemLookupResult, *mut SwordArtsLookupResult);

/// `{ u32 paramId @0x0, SwordArtsParam* row @0x8 }` -- `row` is null when the id misses. Same
/// POD the menu path already uses (`LOOKUP_SWORD_ARTS_PARAM_RVA`).
#[repr(C)]
struct SwordArtsLookupResult {
    param_id: u32,
    _pad: u32,
    row: usize,
}

/// The gaitem lookup record. It is BOTH input and output, which is the whole trick:
///
/// ```text
///   +0x00  u32   gaitem handle          INPUT  -- caller fills this
///   +0x08  ptr   resolved GaitemIns*    output (`mov [rdi+8], rax`)
///   +0x10  u32   item category/id       output (`mov [rdi+0x10], ecx`)
/// ```
///
/// `GetGaitemInsByHandle` opens with `cmp dword ptr [rcx], 0; je <bare ret>` -- it reads the
/// handle out of the struct it is asked to fill, and returns having written NOTHING when that
/// field is zero. The game's own one-line thunk `FUN_1406743a0` is `mov rdx, rcx; jmp
/// GetGaitemInsByHandle`, i.e. it passes the SAME pointer as both arguments.
///
/// Getting this wrong is silent, not loud: passing a zeroed struct plus a separate handle
/// pointer takes the early return, leaves `ins`/`kind` at zero, and every icon lookup then
/// resolves nothing -- no crash, no log, just a badge that never appears.
///
/// `+0x10` is not incidental: `GetSwordArtsParamIdForWeapon` reads exactly that field
/// (`mov edx, dword ptr [rdi + 0x10]` at `0x140673fb6`).
#[repr(C)]
#[derive(Default)]
struct GaitemLookupResult {
    handle: u32,
    _pad0: u32,
    ins: usize,
    kind: u32,
    _pad1: u32,
}

static ORIG_SCENE_UPDATE: AtomicUsize = AtomicUsize::new(0);
static ORIG_CTOR: AtomicUsize = AtomicUsize::new(0);
static ORIG_CHILD_BINDER: AtomicUsize = AtomicUsize::new(0);
static ORIG_SLOT_UPDATE: AtomicUsize = AtomicUsize::new(0);

/// The live `PlayerHUDScene`, from the per-frame driver. `0` until the first HUD frame.
static SCENE_BASE: AtomicUsize = AtomicUsize::new(0);

/// Set by the weapon ctor for the duration of its call, consumed by the child binder.
///
/// The ctor calls the binder synchronously on the same thread, so a plain handshake pairs them
/// exactly -- and it also means only weapon-family slots ever get a badge bound, instead of every
/// HUD widget the generic binder runs for.
static PENDING_COMPONENT: AtomicUsize = AtomicUsize::new(0);
static INSTALLED: AtomicUsize = AtomicUsize::new(0);

// -- oracle counters --
static CTOR_FIRES: AtomicU64 = AtomicU64::new(0);
static BINDER_BOUND: AtomicU64 = AtomicU64::new(0);
static UPDATE_FIRES: AtomicU64 = AtomicU64::new(0);
static HUD_DRAWN: AtomicU64 = AtomicU64::new(0);
static HUD_HIDDEN: AtomicU64 = AtomicU64::new(0);
static HUD_NO_PROXY: AtomicU64 = AtomicU64::new(0);
static HUD_UNKNOWN_SLOT: AtomicU64 = AtomicU64::new(0);
const SAMPLE_LOGS: u64 = 24;

/// Per-component badge proxy registry.
///
/// The update hook only receives the COMPONENT; the parent clip is in scope solely inside the
/// binder. So the badge proxy is bound once at bind time and remembered here, keyed by the
/// component pointer.
///
/// Fixed-size and lock-free: this runs on the game thread inside a per-frame HUD update, so a
/// mutex here would be a frame-time hazard for no benefit. Sized well past the seven components
/// `FUN_1408d1d00` builds, with slack for the HUD scene being rebuilt.
const REGISTRY_SLOTS: usize = 16;

struct BadgeSlot {
    component: AtomicUsize,
    /// Raw proxy storage. Only touched by the game thread.
    proxy: std::cell::UnsafeCell<[u8; PROXY_SIZE]>,
    bound: AtomicUsize,
}

// SAFETY: every access happens on the game thread, inside either the binder or the update hook,
// both of which are called from the same HUD update chain.
unsafe impl Sync for BadgeSlot {}

static REGISTRY: [BadgeSlot; REGISTRY_SLOTS] = [const {
    BadgeSlot {
        component: AtomicUsize::new(0),
        proxy: std::cell::UnsafeCell::new([0u8; PROXY_SIZE]),
        bound: AtomicUsize::new(0),
    }
}; REGISTRY_SLOTS];

fn registry_find(component: usize) -> Option<&'static BadgeSlot> {
    REGISTRY.iter().find(|s| {
        s.component.load(Ordering::SeqCst) == component && s.bound.load(Ordering::SeqCst) != 0
    })
}

fn registry_claim(component: usize) -> Option<&'static BadgeSlot> {
    // Re-bind in place when this component is already known (the HUD scene can be rebuilt).
    if let Some(s) = REGISTRY
        .iter()
        .find(|s| s.component.load(Ordering::SeqCst) == component)
    {
        return Some(s);
    }
    REGISTRY
        .iter()
        .find(|s| s.component.load(Ordering::SeqCst) == 0)
}

/// Which hand this component drives, or `None` for every other slot.
///
/// Exact, not heuristic: the scene pointer comes from the driver and the two offsets are the
/// literals `PlayerHUDScene` and the driver both use. `Arts` and the four `ItemPanel2` items go
/// through the SAME ctor and the SAME updater, so this test is the only thing separating them --
/// and it fails closed, because an unrecognised component hides its badge rather than guessing a
/// hand. (The previous scheme picked the lower of two remembered addresses, which silently
/// mapped the Magic and Item slots onto the right and left hands.)
fn hand_for_component(component: usize) -> Option<i32> {
    let scene = SCENE_BASE.load(Ordering::SeqCst);
    if scene == 0 || component < scene {
        return None;
    }
    match component - scene {
        HUD_SLOT_LEFT_WEP_OFFSET => Some(CHR_ASM_SLOT_LEFT_ACTIVE),
        HUD_SLOT_RIGHT_WEP_OFFSET => Some(CHR_ASM_SLOT_RIGHT_ACTIVE),
        _ => None,
    }
}

/// `PlayerIns`, or `None` before the world exists. Both hops are null-checked exactly as the
/// game checks them.
unsafe fn player_ins(base: usize) -> Option<usize> {
    let world = unsafe { safe_read_usize(base + WORLD_CHR_MAN_GLOBAL_RVA) }.unwrap_or(0);
    if world == 0 {
        return None;
    }
    let player = unsafe { safe_read_usize(world + WORLD_CHR_MAN_PLAYER_INS_OFFSET) }.unwrap_or(0);
    (player != 0).then_some(player)
}

/// Ash-of-War icon id for the weapon in `slot`, or `None` when that hand holds no weapon or the
/// weapon has no ash.
unsafe fn arts_icon_for_slot(base: usize, slot: i32) -> Option<u32> {
    let player = unsafe { player_ins(base) }?;

    let get_handle: GetWeaponHandleFn =
        unsafe { std::mem::transmute(base + GET_WEAPON_GAITEM_HANDLE_BY_SLOT_RVA) };
    let mut handle: u32 = 0;
    unsafe { get_handle(player, &mut handle, slot) };
    if handle == 0 {
        return None; // empty hand -- the game writes 0 before validating the slot
    }

    let get_ins: GetGaitemInsFn =
        unsafe { std::mem::transmute(base + GET_GAITEM_INS_BY_HANDLE_RVA) };
    let mut gaitem = GaitemLookupResult {
        handle,
        ..Default::default()
    };
    // Same pointer for both arguments, exactly as the game's own thunk does.
    let g = &raw mut gaitem;
    unsafe { get_ins(g, g) };
    if gaitem.ins == 0 {
        return None; // handle did not resolve to a live gaitem
    }

    let get_arts: GetArtsForWeaponFn =
        unsafe { std::mem::transmute(base + GET_SWORD_ARTS_PARAM_FOR_WEAPON_RVA) };
    let mut result = SwordArtsLookupResult {
        param_id: 0,
        _pad: 0,
        row: 0,
    };
    unsafe { get_arts(&raw const gaitem, &mut result) };
    if result.row == 0 {
        return None; // no skill on this weapon
    }

    // The ICON lives on the GEM, not on `SwordArtsParam`.
    //
    // Measured live on both equipped weapons (`scripts/frida/hud-arts-chain.py`, run
    // 20260728-094220): the chain resolves perfectly -- arts 801 and 802, valid rows -- and
    // `SwordArtsParam + 0x1A` reads **0** for both, while `EquipParamGem` carries icons 8481
    // and 8482. That is why the first live run bound 6 badges and drew none.
    //
    // The menu badge has always read the gem, which is also why the menu shows icons the HUD
    // did not; going through the same resolver keeps the two surfaces showing the SAME icon for
    // the same weapon instead of two independently-derived answers. It verifies the gem row's
    // own `swordArtsParamId` matches, so a heuristic miss degrades to "no gem" rather than to
    // someone else's icon.
    let gem_icon = unsafe { crate::resolve_gem_icon_id(base, result.param_id as i32) };
    if gem_icon != 0 {
        return Some(gem_icon);
    }

    // Fallback for ashes with no icon-bearing gem row. Kept because it is the game's own HUD
    // skill-icon source (`UpdatePlayerComponents` reads this exact offset), so where it is
    // non-zero it is authoritative.
    let icon = unsafe { *((result.row + SWORD_ARTS_PARAM_ICON_ID_OFFSET) as *const u16) } as u32;
    (icon != 0).then_some(icon)
}

/// Bind our badge child under `parent_clip` and remember it for `component`.
unsafe fn bind_badge(base: usize, component: usize, parent_clip: usize) {
    let Some(entry) = registry_claim(component) else {
        return;
    };
    let assign: AssignFn =
        unsafe { std::mem::transmute(base + crate::ASSIGN_COMPONENT_WITH_NAME_RVA) };
    let is_bound: IsBoundFn = unsafe { std::mem::transmute(base + crate::PROXY_IS_BOUND_RVA) };
    let value_dtor: ScaleformValueDtorFn =
        unsafe { std::mem::transmute(base + crate::SCALEFORM_VALUE_DTOR_RVA) };

    // Release a previous binding for this component before overwriting it: the proxy owns a
    // ref-counted CSScaleformValue, and dropping the storage without the dtor leaks a movie
    // object reference every time the HUD is rebuilt.
    if entry.bound.swap(0, Ordering::SeqCst) != 0 {
        unsafe {
            value_dtor(
                (*entry.proxy.get())
                    .as_mut_ptr()
                    .add(PROXY_SCALEFORM_VALUE_OFFSET),
            )
        };
    }

    let storage = unsafe { &mut *entry.proxy.get() };
    *storage = [0u8; PROXY_SIZE];
    unsafe {
        assign(
            parent_clip,
            storage.as_mut_ptr(),
            BADGE_PATH.as_ptr().cast(),
        )
    };
    if unsafe { is_bound(storage.as_ptr()) } {
        entry.component.store(component, Ordering::SeqCst);
        entry.bound.store(1, Ordering::SeqCst);
        // Hide on bind. The movie already places the HUD badge with `visible = 0`, so this is
        // belt-and-braces -- but it costs one call and it means a slot that is bound and then
        // never updated (a HUD scene torn down mid-frame) cannot flash its un-set placeholder.
        let set_visible: SetVisibleFn =
            unsafe { std::mem::transmute(base + crate::PROXY_SET_VISIBLE_RVA) };
        unsafe { set_visible(storage.as_mut_ptr(), false) };
        let n = BINDER_BOUND.fetch_add(1, Ordering::SeqCst) + 1;
        if n <= SAMPLE_LOGS {
            let scene = SCENE_BASE.load(Ordering::SeqCst);
            log_message(format_args!(
                "hud-badge: bound #{n} component=0x{component:x} scene_off={} clip=0x{parent_clip:x}",
                if scene != 0 && component >= scene {
                    format!("0x{:x}", component - scene)
                } else {
                    "scene-unknown".to_owned()
                }
            ));
        }
    } else {
        // Not an error on its own: `Arts` goes through this same ctor with `Fade` as its clip,
        // and `Fade` has no `ItemIcon` child at all.
        unsafe { value_dtor(storage.as_mut_ptr().add(PROXY_SCALEFORM_VALUE_OFFSET)) };
    }
}

/// The per-frame slot driver. Hooked ONLY to publish the scene pointer.
unsafe extern "system" fn hud_scene_update_hook(
    scene: usize,
    alpha: f32,
    view_model: usize,
) -> usize {
    if scene != 0 && SCENE_BASE.swap(scene, Ordering::SeqCst) != scene {
        log_message(format_args!(
            "hud-badge: PlayerHUDScene = 0x{scene:x} (LeftWep=0x{:x} RightWep=0x{:x})",
            scene + HUD_SLOT_LEFT_WEP_OFFSET,
            scene + HUD_SLOT_RIGHT_WEP_OFFSET,
        ));
    }
    let orig = ORIG_SCENE_UPDATE.load(Ordering::SeqCst);
    if orig == 0 {
        return 0;
    }
    let f: SceneUpdateFn = unsafe { std::mem::transmute(orig) };
    unsafe { f(scene, alpha, view_model) }
}

/// The generic child binder. Hooked ONLY to capture its `rdx` -- the real tile clip -- and only
/// while a weapon slot ctor is on the stack above it.
unsafe extern "system" fn hud_child_binder_hook(
    sub_component: usize,
    tile_clip: usize,
    text_clip: usize,
) -> usize {
    let orig = ORIG_CHILD_BINDER.load(Ordering::SeqCst);
    let ret = if orig != 0 {
        let f: BinderFn = unsafe { std::mem::transmute(orig) };
        unsafe { f(sub_component, tile_clip, text_clip) }
    } else {
        0
    };
    // Bind AFTER the original: it populates the component's own children and force-hides the
    // tile-level `ArtsIcon`, so binding first would race its setup.
    let component = PENDING_COMPONENT.load(Ordering::SeqCst);
    if component != 0
        && tile_clip != 0
        && let Ok(base) = er_game_base::mem::game_module_base()
    {
        unsafe { bind_badge(base, component, tile_clip) };
    }
    ret
}

unsafe extern "system" fn hud_weapon_ctor_hook(component: usize, panel_clip: usize) -> usize {
    let n = CTOR_FIRES.fetch_add(1, Ordering::SeqCst) + 1;
    // Publish the identity for the child binder the original is about to call, and clear it
    // afterwards so unrelated binder calls (Magic, Item, every other HUD widget) never bind.
    PENDING_COMPONENT.store(component, Ordering::SeqCst);
    let orig = ORIG_CTOR.load(Ordering::SeqCst);
    let ret = if orig != 0 {
        let f: CtorFn = unsafe { std::mem::transmute(orig) };
        unsafe { f(component, panel_clip) }
    } else {
        0
    };
    PENDING_COMPONENT.store(0, Ordering::SeqCst);
    if n <= SAMPLE_LOGS {
        let scene = SCENE_BASE.load(Ordering::SeqCst);
        log_message(format_args!(
            "hud-badge: weapon ctor #{n} component=0x{component:x} scene_off={} \
             panel_clip=0x{panel_clip:x} bound={}",
            if scene != 0 && component >= scene {
                format!("0x{:x}", component - scene)
            } else {
                "scene-unknown".to_owned()
            },
            registry_find(component).is_some()
        ));
    }
    ret
}

unsafe extern "system" fn hud_weapon_update_hook(
    component: usize,
    alpha: f32,
    slot_data: usize,
) -> usize {
    let orig = ORIG_SLOT_UPDATE.load(Ordering::SeqCst);
    let ret = if orig != 0 {
        let f: SlotUpdateFn = unsafe { std::mem::transmute(orig) };
        unsafe { f(component, alpha, slot_data) }
    } else {
        0
    };
    let n = UPDATE_FIRES.fetch_add(1, Ordering::SeqCst) + 1;
    // Log the first fires UNCONDITIONALLY. A run with zero draws is otherwise indistinguishable
    // between "this hook never fired" and "it fired but the component was not recognised".
    if n <= SAMPLE_LOGS {
        let scene = SCENE_BASE.load(Ordering::SeqCst);
        log_message(format_args!(
            "hud-badge: update fire #{n} component=0x{component:x} scene_off={} \
             registered={} hand={:?}",
            if scene != 0 && component >= scene {
                format!("0x{:x}", component - scene)
            } else {
                "scene-unknown".to_owned()
            },
            registry_find(component).is_some(),
            hand_for_component(component),
        ));
    }

    let Ok(base) = er_game_base::mem::game_module_base() else {
        return ret;
    };
    let Some(entry) = registry_find(component) else {
        let miss = HUD_NO_PROXY.fetch_add(1, Ordering::SeqCst) + 1;
        if miss <= SAMPLE_LOGS {
            log_message(format_args!(
                "hud-badge: update for UNREGISTERED component=0x{component:x} \
                 (binder never bound a badge under it)"
            ));
        }
        return ret;
    };

    let set_visible: SetVisibleFn =
        unsafe { std::mem::transmute(base + crate::PROXY_SET_VISIBLE_RVA) };
    let storage = unsafe { &mut *entry.proxy.get() };

    // Not one of the two weapon slots: `Arts` and the `ItemPanel2` items bind a badge because
    // they share the ctor, so they must be actively hidden rather than merely skipped.
    let Some(slot) = hand_for_component(component) else {
        unsafe { set_visible(storage.as_mut_ptr(), false) };
        HUD_UNKNOWN_SLOT.fetch_add(1, Ordering::SeqCst);
        return ret;
    };

    match unsafe { arts_icon_for_slot(base, slot) } {
        Some(icon_id) => {
            let build_icon_info: IconInfoBuilderFn =
                unsafe { std::mem::transmute(base + crate::ICON_INFO_BUILDER_RVA) };
            let icon_setter: IconSetterFn =
                unsafe { std::mem::transmute(base + crate::ICON_SETTER_RVA) };
            let mut icon_info = [0u8; ICON_INFO_SIZE];
            unsafe { build_icon_info(icon_info.as_mut_ptr(), icon_id) };
            unsafe { icon_setter(storage.as_mut_ptr(), icon_info.as_ptr()) };
            unsafe { set_visible(storage.as_mut_ptr(), true) };
            let d = HUD_DRAWN.fetch_add(1, Ordering::SeqCst) + 1;
            if d <= SAMPLE_LOGS {
                log_message(format_args!(
                    "hud-badge: draw #{d} slot={slot} icon_id={icon_id} component=0x{component:x}"
                ));
            }
        }
        None => {
            // Hide on EVERY non-draw path. The badge clip is part of the movie and the HUD slot
            // persists across weapon swaps, so "do not draw" is not enough -- that is exactly how
            // empty rows kept a stale plate in the menus (run 20260727-233703).
            unsafe { set_visible(storage.as_mut_ptr(), false) };
            HUD_HIDDEN.fetch_add(1, Ordering::SeqCst);
        }
    }

    if n.is_multiple_of(512) {
        log_message(format_args!(
            "hud-badge heartbeat: ctors={} bound={} updates={} drawn={} hidden={} \
             no_proxy={} other_slot={}",
            CTOR_FIRES.load(Ordering::SeqCst),
            BINDER_BOUND.load(Ordering::SeqCst),
            UPDATE_FIRES.load(Ordering::SeqCst),
            HUD_DRAWN.load(Ordering::SeqCst),
            HUD_HIDDEN.load(Ordering::SeqCst),
            HUD_NO_PROXY.load(Ordering::SeqCst),
            HUD_UNKNOWN_SLOT.load(Ordering::SeqCst),
        ));
    }
    ret
}

/// Install the HUD hooks. Fail-closed and idempotent: a failure here leaves the HUD exactly as
/// vanilla, it does not crash or half-apply.
pub fn install(base: usize) {
    use std::ffi::c_void;

    use er_hook::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook};

    if INSTALLED.load(Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            log_message(format_args!("hud-badge: MH_Initialize failed: {status:?}"));
            return;
        }
    }

    // (rva, detour, where the trampoline is stored, label)
    let plan: [(usize, *mut c_void, &AtomicUsize, &str); 4] = [
        (
            HUD_SCENE_UPDATE_RVA,
            hud_scene_update_hook as *mut c_void,
            &ORIG_SCENE_UPDATE,
            "scene_update",
        ),
        (
            HUD_CHILD_BINDER_RVA,
            hud_child_binder_hook as *mut c_void,
            &ORIG_CHILD_BINDER,
            "child_binder",
        ),
        (
            HUD_WEAPON_SLOT_CTOR_RVA,
            hud_weapon_ctor_hook as *mut c_void,
            &ORIG_CTOR,
            "weapon_ctor",
        ),
        (
            HUD_WEAPON_SLOT_UPDATE_RVA,
            hud_weapon_update_hook as *mut c_void,
            &ORIG_SLOT_UPDATE,
            "weapon_update",
        ),
    ];

    // Create and enable ALL of them before applying: a half-installed set would bind badges that
    // nothing ever shows (or worse, show badges nothing ever hides).
    let mut hooks = Vec::with_capacity(plan.len());
    for (rva, detour, slot, label) in plan {
        let target = base + rva;
        let hook = match unsafe { MhHook::new(target as *mut c_void, detour) } {
            Ok(h) => h,
            Err(status) => {
                log_message(format_args!(
                    "hud-badge: MhHook::new({label} @0x{target:x}) failed: {status:?}"
                ));
                return;
            }
        };
        slot.store(hook.trampoline() as usize, Ordering::SeqCst);
        if let Err(status) = unsafe { hook.queue_enable() } {
            log_message(format_args!(
                "hud-badge: queue_enable({label}) failed: {status:?}"
            ));
            return;
        }
        hooks.push((label, target));
    }

    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => {
            INSTALLED.store(1, Ordering::SeqCst);
            for (label, target) in &hooks {
                log_message(format_args!("hud-badge: hook ACTIVE {label} @0x{target:x}"));
            }
        }
        status => log_message(format_args!("hud-badge: MH_ApplyQueued failed: {status:?}")),
    }
}
