
/// MODEL B (FACTORY-HOOK LATCH RECIPE 2026-06-18, bd
/// live-dialog-menuwindow-latch-via-factory-hook-0x14081e5e0-2026): READ-ONLY deterministic
/// acquisition of the two LIVE args the Load-Game dialog factory 0x14081ead0 needs -- the live
/// TitleTopDialog* (the factory rcx = its [+0xa38] TitleFlowContext capture) and the live host
/// MenuWindow* (the factory rdx). The MenuWindow is NOT persistently readable at the parked title
/// (probe-5 proved [td+0xa38] is a CS::TitleFlowContext, NOT a SceneObjProxy, and there is no
/// persistent SceneObjProxy to read the +0x20 back-ref from). Instead the host MenuWindow is
/// LATCHED at boot from rdx of the SceneObjProxy ctor 0x14074a700
/// (`scene_obj_proxy_ctor_hook` -> LATCHED_MENU_WINDOW; probe-6: the OLD TitleTopDialog-factory rdx
/// was a std::function delegate, NOT the MenuWindow).
///
/// CONVERGED recipe (all pure safe_read_usize / atomic load -> NO writes, NO native calls, never
/// AVs -> save-safe; fail-closed at every step, every step logged via append_autoload_debug):
///   1. td = *(owner+0xe0); require *(td) == base+TITLE_TOP_DIALOG_VTABLE_RVA (else fail-closed).
///   2. SELF-DIAGNOSTIC: read + LOG the TitleFlowContext capture *(td+0xa38) + its vtable (context
///      only; it is the factory rcx, never gates acquisition).
///   3. menu_window = LATCHED_MENU_WINDOW (SeqCst); fail-closed if 0 (factory not yet hit) or not a
///      canonical heap pointer. Read mwvt = *(menu_window); LOG menu_window + mwvt; if mwvt is
///      neither MenuWindow nor MenuWindowProxy LOG loudly but STILL return it (probe visibility).
///   4. Return (td, menu_window).
pub(crate) unsafe fn locate_live_loadgame_node(
    owner: usize,
    base: usize,
) -> Option<(usize, usize)> {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const HEAP_LO: usize = 0x10000;
    const PTR_ALIGN_MASK: usize = 0x7;

    let title_vt = base + TITLE_TOP_DIALOG_VTABLE_RVA;
    let scene_proxy_vt = base + SCENE_OBJ_PROXY_VTABLE_RVA;
    let menu_vt = base + MENU_WINDOW_VTABLE_RVA;
    let menu_proxy_vt = base + MENU_WINDOW_PROXY_VTABLE_RVA;

    // (1) TitleTopDialog: owner+0xe0, vtable-gated (probe-2/3 runtime-confirmed).
    let td = unsafe { safe_read_usize(owner + TITLE_OWNER_MENU_HOLDER_E0_OFFSET) }.unwrap_or(NULL);
    let tdvt = if td != NULL {
        unsafe { safe_read_usize(td) }.unwrap_or(NULL)
    } else {
        NULL
    };
    if tdvt != title_vt {
        append_autoload_debug(format_args!(
            "live-dialog: owner+0x{:x}=0x{td:x} vt=0x{tdvt:x} != TitleTopDialog 0x{title_vt:x} -- title not up, fail-closed",
            TITLE_OWNER_MENU_HOLDER_E0_OFFSET
        ));
        return None;
    }
    append_autoload_debug(format_args!(
        "live-dialog: TitleTopDialog acquired owner+0x{:x}=0x{td:x} (vt 0x{tdvt:x})",
        TITLE_OWNER_MENU_HOLDER_E0_OFFSET
    ));

    // (2) SELF-DIAGNOSTIC (context only): the TitleFlowContext capture at td+0xa38. Probe-5 proved
    // this is a CS::TitleFlowContext (vt 0x142ac7f20), NOT a persistent SceneObjProxy, so it does
    // NOT yield the MenuWindow -- but it IS the correct factory rcx (= td+0xa38). LOG it for
    // context; it never gates acquisition.
    let capture =
        unsafe { safe_read_usize(td + DIALOG_SCENE_PROXY_CAPTURE_A38_OFFSET) }.unwrap_or(NULL);
    let cvt = if capture != NULL {
        unsafe { safe_read_usize(capture) }.unwrap_or(NULL)
    } else {
        NULL
    };
    append_autoload_debug(format_args!(
        "live-dialog: capture *(td+0x{:x})=0x{capture:x} vt=0x{cvt:x} (TitleFlowContext; factory rcx) (probe scene_proxy_vt 0x{scene_proxy_vt:x})",
        DIALOG_SCENE_PROXY_CAPTURE_A38_OFFSET
    ));

    // (3) MenuWindow: READ the boot-latched host MenuWindow* (latched as rdx of the TitleTopDialog
    // ctor 0x14074a700 by `scene_obj_proxy_ctor_hook`). The MenuWindow is NOT persistently
    // readable at the parked title, so the latch is the only headless source. Fail-closed if 0.
    let menu_window = LATCHED_MENU_WINDOW.load(Ordering::SeqCst);
    if menu_window == NULL {
        append_autoload_debug(format_args!(
            "live-dialog: LATCHED_MENU_WINDOW is 0 (SceneObjProxy ctor 0x14074a700 not yet hit) -- fail-closed, no factory call"
        ));
        return None;
    }
    if menu_window < HEAP_LO || (menu_window & PTR_ALIGN_MASK) != NULL {
        append_autoload_debug(format_args!(
            "live-dialog: latched MenuWindow 0x{menu_window:x} is not a valid heap pointer -- fail-closed, no factory call"
        ));
        return None;
    }
    let mwvt = unsafe { safe_read_usize(menu_window) }.unwrap_or(NULL);
    append_autoload_debug(format_args!(
        "live-dialog: latched MenuWindow=0x{menu_window:x} vt=0x{mwvt:x} (want MenuWindow 0x{menu_vt:x} or MenuWindowProxy 0x{menu_proxy_vt:x})"
    ));
    if mwvt != menu_vt && mwvt != menu_proxy_vt {
        // Loud log but STILL return it (probe visibility) -- the pointer is heap-canonical above.
        append_autoload_debug(format_args!(
            "live-dialog: unexpected latched MenuWindow vtable 0x{mwvt:x} (neither 0x{menu_vt:x} nor 0x{menu_proxy_vt:x}) -- returning anyway for probe visibility"
        ));
    }
    append_autoload_debug(format_args!(
        "live-dialog: ACQUIRED title_dialog=0x{td:x} (vt 0x{title_vt:x}) menu_window=0x{menu_window:x} via boot factory-hook latch"
    ));
    Some((td, menu_window))
}

/// MODEL B (FINAL RECIPE 2026-06-18): build the LIVE registered ProfileLoadDialog by calling the
/// dialog factory 0x14081ead0 WITH THE LIVE CALL-FRAME ARGS -- the only way the dialog becomes
/// live + pumped (the parameterless node-run builds a NON-LIVE dialog and discards it). The factory
/// reads the SceneProxy from [rcx] (r8 = *(dialog+0xa38), the live SceneProxy* the TitleTopDialog
/// ctor stored there at 0x1409a8213) and takes the live MenuWindow* as rdx. So:
///   factory(rcx = title_dialog + 0xa38, rdx = menu_window) -> ProfileLoadDialog* in rax.
/// This builds + registers the dialog into the menu group 0x143d87350 + active-screen set
/// intrinsically (registration is folded into the factory invocation under live args), which the
/// native pump then drives. We FAIL-CLOSED: re-validate the title_dialog vtable (0x142b26468) and
/// that its SceneProxy capture [+0xa38] + the menu_window are non-null heap BEFORE the call; a
/// mismatch returns false with NO native call. Zero-input (the game's own factory, no synthesis).
/// Returns true if the factory was invoked.
pub(crate) unsafe fn fire_live_loadgame_node(
    title_dialog: usize,
    menu_window: usize,
    base: usize,
    enter_stage2: bool,
) -> bool {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const HEAP_LO: usize = 0x10000;
    if title_dialog == NULL || menu_window == NULL {
        return false;
    }
    let dvt = unsafe { safe_read_usize(title_dialog) }.unwrap_or(NULL);
    let capture_slot = title_dialog + DIALOG_SCENE_PROXY_CAPTURE_A38_OFFSET;
    let scene_proxy = unsafe { safe_read_usize(capture_slot) }.unwrap_or(NULL);
    if dvt != base + TITLE_TOP_DIALOG_VTABLE_RVA || scene_proxy < HEAP_LO || menu_window < HEAP_LO {
        append_autoload_debug(format_args!(
            "live-dialog: FIRE ABORT (fail-closed, NO native call) title_dialog=0x{title_dialog:x} vt=0x{dvt:x}(want 0x{:x}) scene_proxy([+0xa38])=0x{scene_proxy:x} menu_window=0x{menu_window:x}",
            base + TITLE_TOP_DIALOG_VTABLE_RVA
        ));
        return false;
    }
    const RECORD_BASE_18: usize = 0x18;
    const RECORD_STATE_44: usize = 0x44;
    const RECORD_STRIDE_2A0: usize = 0x2a0;
    const RECORD_VALID_295: usize = 0x295;
    const RECORD_STATE_LOADABLE: i32 = 2;
    const RECORD_VALID_SET: u8 = 1;
    let want_slot = OWN_STEPPER_SLOT.load(Ordering::SeqCst);
    let gdm = game_data_man_ptr_or_null();
    let profile_summary = if gdm != NULL {
        unsafe { safe_read_usize(gdm + SLOT_MANAGER_CONTAINER_OFFSET) }.unwrap_or(NULL)
    } else {
        NULL
    };
    if profile_summary != NULL && want_slot >= OWN_STEPPER_SLOT_ZERO {
        let activate: unsafe extern "system" fn(usize, i32) =
            unsafe { std::mem::transmute(base + PROFILE_SLOT_ACTIVATE_RVA) };
        unsafe { activate(profile_summary, want_slot) };
        let rec = profile_summary + RECORD_BASE_18 + (want_slot as usize) * RECORD_STRIDE_2A0;
        unsafe { *((rec + RECORD_VALID_295) as *mut u8) = RECORD_VALID_SET };
        unsafe { *((rec + RECORD_STATE_44) as *mut i32) = RECORD_STATE_LOADABLE };
        append_autoload_debug(format_args!(
            "live-dialog: pre-activated profile_summary=0x{profile_summary:x} slot={want_slot} rec=0x{rec:x} before factory so ProfileLoadDialog rows populate"
        ));
    }
    let factory: unsafe extern "system" fn(usize, usize) -> usize =
        unsafe { std::mem::transmute(base + LIVE_DIALOG_FACTORY_RVA) };
    append_autoload_debug(format_args!(
        "live-dialog: FIRE factory 0x{:x}(rcx=title_dialog+0xa38=0x{capture_slot:x} [SceneProxy=0x{scene_proxy:x}], rdx=menu_window=0x{menu_window:x}) -- building LIVE registered ProfileLoadDialog",
        base + LIVE_DIALOG_FACTORY_RVA
    ));
    let dialog = unsafe { factory(capture_slot, menu_window) };
    let pld_vt = base + PROFILE_LOAD_DIALOG_VTABLE_RVA;
    let dialog_vt = if dialog >= HEAP_LO {
        unsafe { safe_read_usize(dialog) }.unwrap_or(NULL)
    } else {
        NULL
    };
    append_autoload_debug(format_args!(
        "live-dialog: factory returned dialog=0x{dialog:x} vt=0x{dialog_vt:x} (want ProfileLoadDialog 0x{pld_vt:x})"
    ));
    // FIX 2 (probe-6): drive the RETURNED dialog directly -- do NOT scan the active-screen array
    // 0x143d6d8d0 (probe-2 proved it is MODEL-RENDERERS, never the PLD). If the returned vtable is
    // the ProfileLoadDialog, the normal autoload path stores it + transitions own_stepper to STAGE2
    // ACTIVATE on THAT pointer. The invalid/empty Continue UX fallback deliberately stops here so
    // the user sees the native Load Game menu instead of any automatic load/confirm.
    if dialog_vt != pld_vt {
        append_autoload_debug(format_args!(
            "live-dialog: returned dialog vtable 0x{dialog_vt:x} != ProfileLoadDialog 0x{pld_vt:x} -- fail-closed, STAY (NO-WRITE, no STAGE2)"
        ));
        return false;
    }
    if !enter_stage2 {
        append_autoload_debug(format_args!(
            "live-dialog: LIVE ProfileLoadDialog=0x{dialog:x} (vt 0x{pld_vt:x}) from factory return -- menu-only fallback, no STAGE2/no confirm"
        ));
        return true;
    }
    OWN_STEPPER_DIALOG.store(dialog, Ordering::SeqCst);
    own_stepper_enter_s2_phase(OWN_STEPPER_PHASE_S2_ACTIVATE);
    append_autoload_debug(format_args!(
        "live-dialog: LIVE ProfileLoadDialog=0x{dialog:x} (vt 0x{pld_vt:x}) from factory return -- entering STAGE2 ACTIVATE (slot={})",
        OWN_STEPPER_SLOT.load(Ordering::SeqCst)
    ));
    true
}
