use std::ffi::c_void;

/// Windows loader entrypoint for the quickload/autoload-only DLL.
///
/// This crate deliberately exports only `DllMain`: it is intended for a loader
/// such as LazyLoader/chainload, while `er_effects_rs.dll` remains the DINPUT8
/// proxy + HUD/effects DLL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn DllMain(
    hmodule: er_runtime::HINSTANCE,
    reason: u32,
    reserved: *mut c_void,
) -> i32 {
    unsafe { er_runtime::quickload_dll_main(hmodule, reason, reserved) }
}
