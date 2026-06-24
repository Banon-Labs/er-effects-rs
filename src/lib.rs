use std::ffi::c_void;

/// DINPUT8.dll proxy export for the HUD/effects DLL.
///
/// The implementation lives in `er-runtime`; this wrapper keeps the shipped
/// `er_effects_rs.dll` filename/exports stable while allowing another cdylib
/// crate to reuse the same runtime without exporting this proxy surface.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DirectInput8Create(
    hinst: er_runtime::HINSTANCE,
    version: u32,
    riid: *const c_void,
    out: *mut *mut c_void,
    outer: *mut c_void,
) -> i32 {
    unsafe { er_runtime::direct_input8_create(hinst, version, riid, out, outer) }
}

/// Windows loader entrypoint for `er_effects_rs.dll`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn DllMain(
    hmodule: er_runtime::HINSTANCE,
    reason: u32,
    reserved: *mut c_void,
) -> i32 {
    unsafe { er_runtime::effects_dll_main(hmodule, reason, reserved) }
}
