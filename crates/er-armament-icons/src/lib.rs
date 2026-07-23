//! er-armament-icons-dll -- armament tile Ash-of-War badge (bd er-effects-rs-pe98).
//!
//! GOAL: every non-empty highlightable armament/ranged/catalyst/shield tile, in every
//! menu that renders the shared tile widget, gets the weapon's skill (Ash of War) icon
//! as a vanilla-style corner badge in the tile's BOTTOM-LEFT corner. Pure DLL-driven:
//! no regulation.bin changes, no loose packed files; icons resolve by Scaleform symbol
//! name from the game's own texture repositories at runtime.
//!
//! MECHANISM (static RE 2026-07-23, bd er-effects-rs-pe98 comments): the game populates
//! each tile natively -- `TilePopulate(SceneObjProxy* tile, MenuGaitem*)` (dump
//! FUN_1408ff560) fills the tile's named child proxies and pushes icons through the
//! universal icon setter (dump FUN_14074bdb0), which draws a bitmap-fill quad via the
//! Scaleform Drawing API from a symbol like `MENU_ItemIcon_%05d`. Vanilla corner badges
//! (affinity/infusion) are driven the same way. We post-hook TilePopulate and drive a
//! badge child with the game's own primitives.
//!
//! MILESTONE 1 (this build): diagnostic only -- install the TilePopulate MinHook, count
//! fires, log sample (tile, gaitem) pointers. Proves the hook target and call volume
//! across menus before any badge drawing.

#![allow(non_snake_case)]

use std::{
    fmt,
    path::PathBuf,
    sync::atomic::{AtomicU64, AtomicUsize, Ordering},
};

use er_game_base::log::{append_line, game_directory_path};

const DLL_PROCESS_ATTACH: u32 = 1;
const DLL_MAIN_SUCCESS: i32 = 1;

const LOG_FILE_NAME: &str = "er-armament-icons.log";

// -- Ground-truthed deobf RVAs (scripts/dump-deobf-shift.py 2026-07-23, all content-unique;
//    dump VA -> deobf VA recorded on bd er-effects-rs-pe98) --

/// Per-tile populate `TilePopulate(SceneObjProxy* tile, MenuGaitem*)`.
/// dump FUN_1408ff560 -> deobf 0x1408ff470 (shift -0xf0). HOOK target.
const TILE_POPULATE_RVA: usize = 0x8ff470;
/// Universal icon setter `SetIcon(SceneObjProxy*, iconInfo*)` (iconInfo +0x38 category
/// byte, +0x3c i32 iconId). dump FUN_14074bdb0 -> deobf 0x14074bcc0 (-0xf0). CALL target
/// for the badge draw (milestone 2).
const ICON_SETTER_RVA: usize = 0x74bcc0;
/// Menu-side MenuGaitem -> swordArtsParamId resolver `int f(MenuGaitem*)` (dump
/// FUN_140849970): returns -1 for non-weapons; honors the socketed Ash-of-War gem
/// override (gemParamId +0x7c, gaitem handle +0x50) before falling back to the
/// EquipParamWeapon row's swordArtsParamId.
/// dump 0x140849970 -> deobf 0x140849880 (-0xf0). CALL target.
const MENU_GAITEM_SWORD_ARTS_RESOLVER_RVA: usize = 0x849880;
/// `LookupSwordArtsParam(SwordArtsParamLookupResult* out, uint id)` -- POD result
/// `{ u32 paramId @0x0, SwordArtsParam* row @0x8 }`, row null when the id misses.
/// dump 0x140d50d70 -> deobf 0x140d50cc0 (-0xb0). CALL target.
const LOOKUP_SWORD_ARTS_PARAM_RVA: usize = 0xd50cc0;
/// `FUN_1408487d0(MenuGaitem*) -> u32 iconId` -- the tile's own item icon id (the main
/// ItemIcon). Used by the mirror-item-icon diagnostic to draw a guaranteed-visible glyph
/// into the badge for oracle rect-location. dump 0x1408487d0 -> deobf 0x1408486e0 (-0xf0).
const MENU_GAITEM_ICON_ID_RVA: usize = 0x8486e0;
/// SwordArtsParam row: skill iconId is the u16 at row +0x1A. Ground-truthed to the
/// game's OWN HUD skill-icon builder CS::CSFeManImp::UpdatePlayerComponents (dump
/// 0x140772b70): it reads `*(u16*)(swordArtsRow + offsetof(_EQUIP_PARAM_GOODS_ST,
/// behaviorId=0x18) + 2)` = row+0x1A and feeds it to the iconInfo builder for the
/// equipped-weapon skill icon. Reading the same offset makes the badge match the
/// game's own skill icon exactly.
const SWORD_ARTS_PARAM_ICON_ID_OFFSET: usize = 0x1a;
/// iconInfo builder `FUN_14073d4e0(iconInfo* out, uint iconId)`: zero-fills the
/// 0x40-byte iconInfo, writes category (+0x38, from the iconId range table) and
/// iconId (+0x3c). dump 0x14073d4e0 -> deobf 0x14073d3e0 (-0x100). CALL target.
const ICON_INFO_BUILDER_RVA: usize = 0x73d3e0;
/// `SceneObjProxy::assignComponentWithName(SceneObjProxy* parent, SceneObjProxy* out,
/// const char* nameFmt, ...)` -- constructs into raw `out` (no pre-init needed),
/// returns `out`. dump 0x14074a3e0 -> deobf 0x14074a2f0 (-0xf0). CALL target.
const ASSIGN_COMPONENT_WITH_NAME_RVA: usize = 0x74a2f0;
/// `bool FUN_140733250(SceneObjProxy*)` -- did the named resolve bind a real GFx
/// display object. dump 0x140733250 -> deobf 0x140733150 (-0x100). CALL target.
const PROXY_IS_BOUND_RVA: usize = 0x733150;
/// `FUN_140733440(SceneObjProxy*, u8 visible)` -- GFx SetDisplayInfo(visible);
/// stateful in the movie, no native per-frame re-hide.
/// dump 0x140733440 -> deobf 0x140733340 (-0x100). CALL target.
const PROXY_SET_VISIBLE_RVA: usize = 0x733340;
/// `CS::CSScaleformValue::~CSScaleformValue` -- MANDATORY after every
/// assignComponentWithName resolve (the proxy holds a ref-counted GFx Value).
/// dump 0x140d7f900 -> deobf 0x140d7f850 (-0xb0). CALL target.
const SCALEFORM_VALUE_DTOR_RVA: usize = 0xd7f850;
/// `CSScaleformValue scaleformValue` lives at SceneObjProxy +0x28 (Ghidra
/// get_structure CS/SceneObjProxy: size 0x60, scaleformValue @40, len 0x38).
const PROXY_SCALEFORM_VALUE_OFFSET: usize = 0x28;
/// SceneObjProxy size; assignComponentWithName constructs into a raw buffer of
/// this size (the game reuses one uninitialized stack slot per resolve).
const PROXY_SIZE: usize = 0x60;
/// iconInfo size for the universal icon setter (zero-filled by the builder).
const ICON_INFO_SIZE: usize = 0x40;

/// How many initial TilePopulate calls get a per-call sample log line.
const SAMPLE_LOG_CALLS: u64 = 16;
/// After sampling, log a heartbeat every this many TilePopulate fires.
const HEARTBEAT_EVERY_FIRES: u64 = 512;

static LOG_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static TILE_POPULATE_FIRES: AtomicU64 = AtomicU64::new(0);
static ORIG_TILE_POPULATE: AtomicUsize = AtomicUsize::new(0);
static HOOK_ACTIVE: AtomicUsize = AtomicUsize::new(0);

// -- Badge oracle counters (the machine-checkable acceptance signal) --
/// Tiles whose weapon resolved to a skill and had the ArtsIcon badge drawn.
static BADGE_DRAWN: AtomicU64 = AtomicU64::new(0);
/// Tiles skipped: resolver returned -1 (non-weapon tile or no skill).
static BADGE_NOT_WEAPON: AtomicU64 = AtomicU64::new(0);
/// Tiles skipped: SwordArtsParam row missing for a resolved id.
static BADGE_NO_ROW: AtomicU64 = AtomicU64::new(0);
/// Tiles where "ArtsIcon/IconImage" failed to bind in the tile template.
static BADGE_UNBOUND: AtomicU64 = AtomicU64::new(0);
/// Weapon tiles that reached the icon stage (badge-draw attempts). Sampling ordinal --
/// the raw TilePopulate fire count is dominated by early non-weapon/empty tiles.
static BADGE_ATTEMPTS: AtomicU64 = AtomicU64::new(0);
/// Sentinel: no forced icon (use the real skill icon).
const FORCE_ICON_NONE: u32 = u32::MAX;
/// Sentinel (ER_ARMAMENT_ICONS_FORCE_ICON=mirror): draw the tile's own item icon into the badge.
const FORCE_ICON_MIRROR: u32 = u32::MAX - 1;
/// Diagnostic forced icon id (ER_ARMAMENT_ICONS_FORCE_ICON), or FORCE_ICON_NONE / _MIRROR.
static FORCE_ICON_ID: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(FORCE_ICON_NONE);

fn log_message(args: fmt::Arguments<'_>) {
    let path = game_directory_path()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        .join(LOG_FILE_NAME);
    let seq = LOG_SEQUENCE.fetch_add(1, Ordering::SeqCst) + 1;
    append_line(&path, format_args!("[{seq:06}] {args}"));
}

#[cfg(windows)]
static START: std::sync::Once = std::sync::Once::new();

#[cfg(windows)]
#[unsafe(no_mangle)]
/// # Safety
///
/// Called by the Windows loader. Do not call directly.
pub unsafe extern "system" fn DllMain(
    _module: *mut core::ffi::c_void,
    reason: u32,
    _reserved: *mut core::ffi::c_void,
) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        START.call_once(spawn_install_thread);
    }
    DLL_MAIN_SUCCESS
}

#[cfg(not(windows))]
#[unsafe(no_mangle)]
pub extern "C" fn er_armament_icons_host_stub() -> i32 {
    DLL_MAIN_SUCCESS
}

#[cfg(windows)]
fn spawn_install_thread() {
    let _ = std::thread::Builder::new()
        .name("er-armament-icons".to_owned())
        .spawn(|| {
            use eldenring::cs::CSTaskImp;
            use fromsoftware_shared::FromStatic;

            if let Ok(v) = std::env::var("ER_ARMAMENT_ICONS_FORCE_ICON") {
                let v = v.trim();
                if v.eq_ignore_ascii_case("mirror") {
                    FORCE_ICON_ID.store(FORCE_ICON_MIRROR, Ordering::Relaxed);
                } else if let Ok(id) = v.parse::<u32>() {
                    FORCE_ICON_ID.store(id, Ordering::Relaxed);
                }
            }
            let forced = FORCE_ICON_ID.load(Ordering::Relaxed);
            log_message(format_args!(
                "attach: milestone-2 badge draw (TilePopulate hook -> ArtsIcon un-hide + icon set); \
                 force_icon={} icon_setter_rva=0x{ICON_SETTER_RVA:x} \
                 resolver_rva=0x{MENU_GAITEM_SWORD_ARTS_RESOLVER_RVA:x} \
                 lookup_sword_arts_param_rva=0x{LOOKUP_SWORD_ARTS_PARAM_RVA:x}",
                match forced {
                    FORCE_ICON_NONE => "off".to_owned(),
                    FORCE_ICON_MIRROR => "mirror".to_owned(),
                    id => id.to_string(),
                }
            ));
            // Wait for the game's task manager the way the sibling DLLs do (yield, no sleep):
            // its readiness implies the game image and its statics are mapped.
            loop {
                match unsafe { CSTaskImp::instance() } {
                    Ok(_) => break,
                    Err(_) => std::thread::yield_now(),
                }
            }
            let Ok(base) = er_game_base::mem::game_module_base() else {
                log_message(format_args!(
                    "install: game_module_base unresolved; aborting"
                ));
                return;
            };
            install_tile_populate_hook(base);
        });
}

#[cfg(windows)]
fn install_tile_populate_hook(base: usize) {
    use std::ffi::c_void;

    use er_hook::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook};

    if HOOK_ACTIVE.load(Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            log_message(format_args!("install: MH_Initialize failed: {status:?}"));
            return;
        }
    }
    let target = base + TILE_POPULATE_RVA;
    let hook =
        match unsafe { MhHook::new(target as *mut c_void, tile_populate_hook as *mut c_void) } {
            Ok(hook) => hook,
            Err(status) => {
                log_message(format_args!(
                    "install: MhHook::new(tile_populate @0x{target:x}) failed: {status:?}"
                ));
                return;
            }
        };
    ORIG_TILE_POPULATE.store(hook.trampoline() as usize, Ordering::SeqCst);
    if let Err(status) = unsafe { hook.queue_enable() } {
        log_message(format_args!(
            "install: queue_enable(tile_populate) failed: {status:?}"
        ));
        return;
    }
    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => {
            HOOK_ACTIVE.store(1, Ordering::SeqCst);
            log_message(format_args!(
                "install: tile_populate hook ACTIVE @0x{target:x} (deobf rva 0x{TILE_POPULATE_RVA:x})"
            ));
        }
        status => log_message(format_args!("install: MH_ApplyQueued failed: {status:?}")),
    }
}

/// Post-hook on `TilePopulate(SceneObjProxy* tile, MenuGaitem*)`. The original runs
/// first, so vanilla tile state -- including the binder's force-hide of the dormant
/// `ArtsIcon` child -- is already applied; our un-hide afterwards wins (GFx
/// SetDisplayInfo is stateful, no native per-frame re-hide exists).
///
/// `tile` is slot 0 of the consecutive 0x60-sized child-proxy array (slot 1 =
/// ItemIcon, 7 = AutoReplenish, 9 = AttributeIcon, ...); `ArtsIcon` is bound into no
/// slot, so it is re-resolved by name per populate, exactly like the game's binders.
#[cfg(windows)]
unsafe extern "system" fn tile_populate_hook(tile: usize, gaitem: usize) -> usize {
    let orig = ORIG_TILE_POPULATE.load(Ordering::SeqCst);
    let ret = if orig != 0 {
        let original: unsafe extern "system" fn(usize, usize) -> usize =
            unsafe { std::mem::transmute(orig) };
        unsafe { original(tile, gaitem) }
    } else {
        0
    };
    let fires = TILE_POPULATE_FIRES.fetch_add(1, Ordering::SeqCst) + 1;
    if fires % HEARTBEAT_EVERY_FIRES == 0 {
        log_message(format_args!(
            "tile_populate heartbeat: fires={fires} drawn={} not_weapon={} no_row={} unbound={}",
            BADGE_DRAWN.load(Ordering::SeqCst),
            BADGE_NOT_WEAPON.load(Ordering::SeqCst),
            BADGE_NO_ROW.load(Ordering::SeqCst),
            BADGE_UNBOUND.load(Ordering::SeqCst),
        ));
    }
    const HEAP_LO: usize = 0x10000;
    if tile < HEAP_LO || gaitem < HEAP_LO {
        return ret;
    }
    unsafe { draw_arts_badge(tile, gaitem, fires) };
    ret
}

/// Resolve the tile's weapon skill and drive the dormant `ArtsIcon/IconImage` child
/// with the game's own icon primitives (bd er-effects-rs-pe98 RE #2/#3).
#[cfg(windows)]
unsafe fn draw_arts_badge(tile: usize, gaitem: usize, fires: u64) {
    let Ok(base) = er_game_base::mem::game_module_base() else {
        return;
    };

    type ResolverFn = unsafe extern "system" fn(usize) -> i32;
    type LookupFn = unsafe extern "system" fn(*mut SwordArtsLookupResult, u32);
    type IconInfoBuilderFn = unsafe extern "system" fn(*mut u8, u32);
    type AssignFn = unsafe extern "system" fn(usize, *mut u8, *const u8) -> *mut u8;
    type IsBoundFn = unsafe extern "system" fn(*const u8) -> bool;
    type IconSetterFn = unsafe extern "system" fn(*mut u8, *const u8);
    type SetVisibleFn = unsafe extern "system" fn(*mut u8, u8);
    type ScaleformValueDtorFn = unsafe extern "system" fn(*mut u8);

    #[repr(C)]
    struct SwordArtsLookupResult {
        param_id: u32,
        _pad: u32,
        row: usize,
    }

    let resolver: ResolverFn =
        unsafe { std::mem::transmute(base + MENU_GAITEM_SWORD_ARTS_RESOLVER_RVA) };
    let arts_id = unsafe { resolver(gaitem) };
    if arts_id < 0 {
        BADGE_NOT_WEAPON.fetch_add(1, Ordering::SeqCst);
        return;
    }

    let lookup: LookupFn = unsafe { std::mem::transmute(base + LOOKUP_SWORD_ARTS_PARAM_RVA) };
    let mut lookup_result = SwordArtsLookupResult {
        param_id: 0,
        _pad: 0,
        row: 0,
    };
    unsafe { lookup(&mut lookup_result, arts_id as u32) };
    if lookup_result.row == 0 {
        BADGE_NO_ROW.fetch_add(1, Ordering::SeqCst);
        if fires <= SAMPLE_LOG_CALLS {
            log_message(format_args!(
                "badge sample #{fires}: arts_id={arts_id} has no SwordArtsParam row"
            ));
        }
        return;
    }
    let real_icon_id =
        unsafe { *((lookup_result.row + SWORD_ARTS_PARAM_ICON_ID_OFFSET) as *const u16) } as u32;
    // DIAGNOSTIC-ONLY override (ER_ARMAMENT_ICONS_FORCE_ICON=<u16 menu icon id>): draw a fixed,
    // guaranteed-visible icon into every badge instead of the skill icon. Used to (a) locate the
    // badge's on-screen rect via a locator-vs-vanilla pixel diff and (b) prove the pixel path flips
    // the diff oracle to SUCCESS. NOT product behavior -- product uses the real skill icon.
    let forced = FORCE_ICON_ID.load(Ordering::Relaxed);
    let icon_id = if forced == FORCE_ICON_MIRROR {
        // Mirror the tile's own item icon (guaranteed visible) into the badge -- locator for the
        // pixel oracle's rect discovery and a proof that the pixel path is visible.
        type IconIdFn = unsafe extern "system" fn(usize) -> u32;
        let resolver: IconIdFn = unsafe { std::mem::transmute(base + MENU_GAITEM_ICON_ID_RVA) };
        unsafe { resolver(gaitem) }
    } else if forced != FORCE_ICON_NONE {
        forced
    } else {
        real_icon_id
    };
    // Badge-attempt ordinal (weapon tiles that reached the icon stage). The first N raw
    // TilePopulate fires are non-weapon/empty tiles that return early, so sampling on the
    // raw fire count never captures a weapon tile -- sample on this instead.
    let attempt = BADGE_ATTEMPTS.fetch_add(1, Ordering::SeqCst) + 1;

    let build_icon_info: IconInfoBuilderFn =
        unsafe { std::mem::transmute(base + ICON_INFO_BUILDER_RVA) };
    let mut icon_info = [0u8; ICON_INFO_SIZE];
    unsafe { build_icon_info(icon_info.as_mut_ptr(), icon_id) };

    let assign: AssignFn = unsafe { std::mem::transmute(base + ASSIGN_COMPONENT_WITH_NAME_RVA) };
    let is_bound: IsBoundFn = unsafe { std::mem::transmute(base + PROXY_IS_BOUND_RVA) };
    let icon_setter: IconSetterFn = unsafe { std::mem::transmute(base + ICON_SETTER_RVA) };
    let set_visible: SetVisibleFn = unsafe { std::mem::transmute(base + PROXY_SET_VISIBLE_RVA) };
    let value_dtor: ScaleformValueDtorFn =
        unsafe { std::mem::transmute(base + SCALEFORM_VALUE_DTOR_RVA) };

    // Transient resolve, exactly the game's own pattern: construct into raw storage,
    // act, then run the CSScaleformValue dtor (the proxy holds a ref-counted GFx
    // Value; skipping the dtor leaks movie-object references).
    let mut proxy = [0u8; PROXY_SIZE];
    let mut drawn = false;

    // BIND PROBE (run-2/3 diagnostic: every tile UNBOUND): for the first few WEAPON tiles,
    // log which known child names actually bind under this tile proxy. ItemIcon is a
    // KNOWN-PRESENT control -- if it fails too, the assign call/parent is wrong; if only
    // ArtsIcon fails, this tile template lacks that child.
    if attempt <= SAMPLE_LOG_CALLS {
        let mut bound_names = String::new();
        for name in [
            c"ItemIcon",
            c"ItemIcon/IconImage",
            c"AutoReplenish",
            c"AttributeIcon",
            c"AttributeIcon/IconImage",
            c"ArtsIcon",
            c"ArtsIcon/IconImage",
            c"New",
        ] {
            unsafe { assign(tile, proxy.as_mut_ptr(), name.as_ptr().cast()) };
            let bound = unsafe { is_bound(proxy.as_ptr()) };
            unsafe { value_dtor(proxy.as_mut_ptr().add(PROXY_SCALEFORM_VALUE_OFFSET)) };
            if bound {
                if !bound_names.is_empty() {
                    bound_names.push(',');
                }
                bound_names.push_str(name.to_str().unwrap_or("?"));
            }
        }
        log_message(format_args!(
            "bind probe #{attempt}: fires={fires} tile=0x{tile:x} arts_id={arts_id} bound=[{bound_names}]"
        ));
    }

    // Draw into the "ArtsIcon" CONTAINER directly. Bind probe (run-4 20260723-132050)
    // proved the equipped-slot tiles bind "ArtsIcon" but NOT "ArtsIcon/IconImage" -- the
    // container clip exists, the nested image child does not. The icon setter recurses
    // into an "IconImage" child if present, else draws the bitmap-fill quad into the clip
    // itself, so the container is the right target. (Grid-selection cells bind no ArtsIcon
    // at all -- they need a GFX-template child add, tracked separately.)
    unsafe { assign(tile, proxy.as_mut_ptr(), c"ArtsIcon".as_ptr().cast()) };
    if unsafe { is_bound(proxy.as_ptr()) } {
        unsafe {
            icon_setter(proxy.as_mut_ptr(), icon_info.as_ptr());
            set_visible(proxy.as_mut_ptr(), 1);
        }
        drawn = true;
    }
    unsafe { value_dtor(proxy.as_mut_ptr().add(PROXY_SCALEFORM_VALUE_OFFSET)) };

    if drawn {
        let drawn_total = BADGE_DRAWN.fetch_add(1, Ordering::SeqCst) + 1;
        if drawn_total <= SAMPLE_LOG_CALLS {
            log_message(format_args!(
                "badge sample: DRAWN #{drawn_total} arts_id={arts_id} icon_id={icon_id} tile=0x{tile:x}"
            ));
        }
    } else {
        let unbound_total = BADGE_UNBOUND.fetch_add(1, Ordering::SeqCst) + 1;
        if unbound_total <= SAMPLE_LOG_CALLS {
            log_message(format_args!(
                "badge sample: UNBOUND #{unbound_total} arts_id={arts_id} icon_id={icon_id} tile=0x{tile:x}"
            ));
        }
    }
}
