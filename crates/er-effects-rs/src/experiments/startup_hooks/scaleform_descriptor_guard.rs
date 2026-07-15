// === Scaleform descriptor-heap null guard (er-effects-rs-y22i) ===================================
//
// Crash-report-driven, ALWAYS-ON defensive guard. With our DLL loaded, native-Windows users
// access-violate inside the game's own Scaleform (GFx) D3D12 CBV_SRV_UAV descriptor-heap ring advance
// (deobf entry `SCALEFORM_DESC_ADVANCE_RVA` = `0x140ec9530`, `f(this /rcx/, count /edx/)`). Verified
// from disassembly: the new-page branch reloads the current-page provider `*(this+0x38)` and
// unconditionally dereferences `[provider+0x20]` at deobf `0x140ec95d1` (`mov 0x20(%rax),%rcx`),
// faulting when the provider is null (crash rva=0xec95d1, fault_addr=0x20). A null provider is a
// fresh/reset HAL (ring capacity `this+0x20 == 0`), so the original ALWAYS takes that branch and
// always faults; we skip the advance until the provider is initialized.
//
// This does NOT branch on OS: the DLL is always the Windows target, and the fix is the same defensive
// null-check the game omits. Vanilla ER never hits the null window (vkd3d masks the reset window on
// Linux/Proton; the native D3D12 driver does not), which is why local runs stay clean and only native
// Windows crashes. UNVALIDATED against native Windows -- confirm with one Windows run before shipping.

/// Detour over the Scaleform CBV_SRV_UAV descriptor-heap ring advance. No-ops while the current-page
/// provider `*(this + SCALEFORM_DESC_PROVIDER_OFFSET)` is null -- the exact state that AVs at deobf
/// `0x140ec95d1`. Otherwise calls the original unchanged (transparent passthrough).
pub(crate) unsafe extern "system" fn scaleform_descriptor_advance_hook(
    this: usize,
    count: u32,
) -> usize {
    if this != 0 && unsafe { *((this + SCALEFORM_DESC_PROVIDER_OFFSET) as *const usize) } == 0 {
        SCALEFORM_DESC_PROVIDER_NULL_HITS.fetch_add(1, Ordering::SeqCst);
        return 0;
    }
    let orig = SCALEFORM_DESC_ADVANCE_ORIG.load(Ordering::SeqCst);
    if orig != TITLE_OWNER_SCAN_START_ADDRESS && orig != HOOK_ORIGINAL_UNSET {
        let f: unsafe extern "system" fn(usize, u32) -> usize = unsafe { std::mem::transmute(orig) };
        return unsafe { f(this, count) };
    }
    0
}

/// Install the always-on Scaleform descriptor-heap null guard. Idempotent; safe to call
/// unconditionally at attach -- not feature-gated, because any user whose native D3D12 driver exposes
/// the null-provider window would otherwise crash. Transparent when the null never occurs.
pub(crate) fn install_scaleform_descriptor_guard() {
    if SCALEFORM_DESC_ADVANCE_INSTALLED.load(Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "scaleform-guard: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(target) = game_rva(SCALEFORM_DESC_ADVANCE_RVA as u32) else {
        append_autoload_debug(format_args!(
            "scaleform-guard: game_rva(0x{SCALEFORM_DESC_ADVANCE_RVA:x}) failed"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            target as *mut c_void,
            scaleform_descriptor_advance_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            SCALEFORM_DESC_ADVANCE_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if unsafe { hook.queue_enable() }.is_err() {
                append_autoload_debug(format_args!(
                    "scaleform-guard: queue_enable failed for 0x{target:x}"
                ));
                return;
            }
            std::mem::forget(hook);
        }
        Err(status) => {
            append_autoload_debug(format_args!(
                "scaleform-guard: MhHook::new failed: {status:?}"
            ));
            return;
        }
    }
    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => {
            SCALEFORM_DESC_ADVANCE_INSTALLED.store(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "scaleform-guard: installed descriptor-heap null guard at 0x{target:x}"
            ));
        }
        status => append_autoload_debug(format_args!(
            "scaleform-guard: MH_ApplyQueued failed: {status:?}"
        )),
    }
}
