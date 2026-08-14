//! Product-facing policy for the native Scaleform descriptor guard.
//!
//! `er-scaleform-hooks` owns the detour, its game RVA, and the provider-null
//! mechanism. This wrapper owns product logging and remains the attach-time
//! call target, so startup ordering does not move with the hook implementation.

use crate::telemetry::append_autoload_debug;
use er_scaleform_hooks::{
    DescriptorGuardInstall, DescriptorGuardInstallError,
    install_scaleform_descriptor_guard as install_native_scaleform_descriptor_guard,
};

/// Install the always-on Scaleform descriptor-heap null guard.
///
/// Native mechanism lives in `er-scaleform-hooks`; product-only reporting stays
/// here with the existing startup orchestration.
pub(crate) fn install_scaleform_descriptor_guard() {
    match install_native_scaleform_descriptor_guard() {
        Ok(DescriptorGuardInstall::AlreadyInstalled) => {}
        Ok(DescriptorGuardInstall::Installed) => append_autoload_debug(format_args!(
            "scaleform-guard: installed descriptor-heap null guard"
        )),
        Err(DescriptorGuardInstallError::Initialize(status)) => append_autoload_debug(
            format_args!("scaleform-guard: MH_Initialize failed: {status:?}"),
        ),
        Err(DescriptorGuardInstallError::ResolveGameRva) => append_autoload_debug(format_args!(
            "scaleform-guard: descriptor advance game RVA resolution failed"
        )),
        Err(DescriptorGuardInstallError::CreateHook(status)) => append_autoload_debug(
            format_args!("scaleform-guard: MhHook::new failed: {status:?}"),
        ),
        Err(DescriptorGuardInstallError::QueueEnable) => {
            append_autoload_debug(format_args!("scaleform-guard: queue_enable failed"))
        }
        Err(DescriptorGuardInstallError::ApplyQueued(status)) => append_autoload_debug(
            format_args!("scaleform-guard: MH_ApplyQueued failed: {status:?}"),
        ),
    }
}
