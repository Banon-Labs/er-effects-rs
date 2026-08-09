#[cfg(windows)]
use std::ffi::{CString, c_void};

#[cfg(windows)]
unsafe extern "system" {
    fn LoadLibraryA(path: *const u8) -> *mut c_void;
    fn RaiseException(code: u32, flags: u32, arg_count: u32, args: *const usize) -> !;
}

#[cfg(windows)]
fn main() {
    let dll_path = std::env::args()
        .nth(1)
        .expect("usage: load_and_crash.exe <path-to-er_crash_logging_dll.dll>");
    let dll_path = CString::new(dll_path).expect("DLL path contains NUL");
    let module = unsafe { LoadLibraryA(dll_path.as_ptr().cast()) };
    assert!(!module.is_null(), "LoadLibraryA failed");
    unsafe { RaiseException(0xc000_0005, 0, 0, std::ptr::null()) }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("load_and_crash is a Windows/Wine smoke helper");
}
