//! Always-on native Scaleform descriptor-heap null guard.
//!
//! The game dereferences the current-page provider during descriptor-ring advance
//! without checking whether a reset/new-page transition has initialized it. The
//! new-page branch reloads `*(this + 0x38)` then dereferences `[provider + 0x20]`;
//! a null provider faults at deobf `0x140ec95d1` (native crash rva `0xec95d1`,
//! fault address `0x20`). A fresh/reset HAL has that null provider, so this hook
//! skips advance only until initialization, then becomes a transparent pass-through.
//! The RVA and offset are byte-verified 1.16.2 Scaleform mechanism facts (bd
//! `er-effects-rs-y22i`), not product startup policy.

use std::{
    ffi::c_void,
    sync::atomic::{AtomicUsize, Ordering},
};

use er_game_base::mem::game_rva;
use er_hook::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook};
use er_telemetry::counters::{SCALEFORM_DESC_ADVANCE_INSTALLED, SCALEFORM_DESC_PROVIDER_NULL_HITS};

const SCALEFORM_DESC_ADVANCE_RVA: usize = 0x00ec_9530;
const SCALEFORM_DESC_PROVIDER_OFFSET: usize = 0x38;
const ORIGINAL_UNSET: usize = 0;

static SCALEFORM_DESC_ADVANCE_ORIGINAL: AtomicUsize = AtomicUsize::new(ORIGINAL_UNSET);

/// Outcome of an idempotent descriptor guard installation.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum DescriptorGuardInstall {
    AlreadyInstalled,
    Installed,
}

/// A native-hook setup failure which the product owns for policy/logging.
#[derive(Debug)]
pub enum DescriptorGuardInstallError {
    Initialize(MH_STATUS),
    ResolveGameRva,
    CreateHook(MH_STATUS),
    QueueEnable,
    ApplyQueued(MH_STATUS),
}

/// Detour over the Scaleform CBV_SRV_UAV descriptor-heap ring advance.
///
/// No-ops while the current-page provider is null, the exact state that would
/// otherwise fault at the game's provider dereference. Once initialized, calls
/// the original function unchanged.
unsafe extern "system" fn scaleform_descriptor_advance_hook(this: usize, count: u32) -> usize {
    if this != 0 && unsafe { *((this + SCALEFORM_DESC_PROVIDER_OFFSET) as *const usize) } == 0 {
        SCALEFORM_DESC_PROVIDER_NULL_HITS.fetch_add(1, Ordering::SeqCst);
        return 0;
    }

    let original = SCALEFORM_DESC_ADVANCE_ORIGINAL.load(Ordering::SeqCst);
    if original == ORIGINAL_UNSET {
        return 0;
    }

    let advance: unsafe extern "system" fn(usize, u32) -> usize =
        unsafe { std::mem::transmute(original) };
    unsafe { advance(this, count) }
}

/// Hand the installed detour over to MinHook and stop tracking its handle here.
///
/// This install site used to end in `std::mem::forget(hook)`. That was a no-op: `MhHook` is
/// three raw pointers with no `Drop` impl, so dropping it never uninstalled anything --
/// MinHook has owned the detour since `MH_CreateHook`. `clippy::forget_non_drop` flags
/// exactly that. Same statement (and same wording) as `er_effects_rs::mh::leak_installed_hook`,
/// restated here so this crate needs no dependency on the product DLL.
///
/// Takes `MhHook` by value rather than a generic on purpose: a generic would silently accept a
/// type that *does* implement `Drop` and really run its destructor.
fn leak_installed_hook(_hook: MhHook) {}

/// Install the always-on descriptor-heap null guard.
///
/// This owns only the native mechanism. The product caller decides how an
/// installation failure is reported; successful installation is idempotent.
pub fn install_scaleform_descriptor_guard()
-> Result<DescriptorGuardInstall, DescriptorGuardInstallError> {
    if SCALEFORM_DESC_ADVANCE_INSTALLED.load(Ordering::SeqCst) != 0 {
        return Ok(DescriptorGuardInstall::AlreadyInstalled);
    }

    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => return Err(DescriptorGuardInstallError::Initialize(status)),
    }

    let target = game_rva(SCALEFORM_DESC_ADVANCE_RVA as u32)
        .map_err(|_| DescriptorGuardInstallError::ResolveGameRva)?;
    let hook = unsafe {
        MhHook::new(
            target as *mut c_void,
            scaleform_descriptor_advance_hook as *mut c_void,
        )
    }
    .map_err(DescriptorGuardInstallError::CreateHook)?;

    SCALEFORM_DESC_ADVANCE_ORIGINAL.store(hook.trampoline() as usize, Ordering::SeqCst);
    if unsafe { hook.queue_enable() }.is_err() {
        return Err(DescriptorGuardInstallError::QueueEnable);
    }
    leak_installed_hook(hook);

    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => {
            SCALEFORM_DESC_ADVANCE_INSTALLED.store(1, Ordering::SeqCst);
            Ok(DescriptorGuardInstall::Installed)
        }
        status => Err(DescriptorGuardInstallError::ApplyQueued(status)),
    }
}
