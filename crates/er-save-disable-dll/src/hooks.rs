//! Win32 file-API detours feeding the save-write census.
//!
//! These hook the OS file APIs rather than the game's own file-device layer on
//! purpose. The census exists to catch save writes we did *not* predict, so it must
//! sit below every FromSoft abstraction -- a hook placed inside the game's save code
//! could only ever see the paths we already knew to look for.
//!
//! Under Proton these resolve to Wine's `kernel32`, which is the module the game
//! actually calls, so the detours observe the real write path.
//!
//! Those blind spots do NOT exist in this binary. An exhaustive parse of
//! `eldenring-deobf.bin`'s import directory (RVA 0x4c09000, 23 DLLs) plus its delay-import
//! directory (0x3b03bec, EOSSDK only) shows the game imports none of `NtWriteFile`,
//! `NtCreateFile`, `ZwWriteFile`, `CreateFile2`, `WriteFileEx`, `WriteFileGather`,
//! `ReplaceFileW/A`, `CreateFileMapping*` or `MapViewOfFile*`. `WriteFile` has exactly five
//! xref sites image-wide and `SetEndOfFile` exactly one.
//!
//! What DID escape, found by measurement rather than reasoning: `CopyFileW`. A census run
//! logged one `WriteFile` of 2359328 bytes while the offline BND4 witness found three changed
//! slots totalling 5374016 bytes in `ER0000.sl2` and a mirrored `ER0000.sl2.bak` -- 3014688
//! bytes reached disk through APIs this module did not hook. The game builds the backup with
//! `CopyFileW` and renames through `MoveFileW`, neither of which is a "write" in the obvious
//! sense. They are hooked now.
//!
//! The residual hole, stated rather than hidden: `GetProcAddress`/`LoadLibraryW` are imported
//! and every call site has not been audited, so a dynamically-resolved file API would still be
//! invisible. `escaped_write_sites`, cross-checked against the offline byte diff by
//! `scripts/check-save-suppression.py`, is the standing detector for that remainder.

use std::{
    ffi::c_void,
    sync::atomic::{AtomicUsize, Ordering},
};

use er_hook::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook};

use crate::{log_message, redirect, witness};

type CreateFileWFn =
    unsafe extern "system" fn(*const u16, u32, u32, *mut c_void, u32, u32, *mut c_void) -> usize;
type WriteFileFn =
    unsafe extern "system" fn(usize, *const c_void, u32, *mut u32, *mut c_void) -> i32;
type SetEndOfFileFn = unsafe extern "system" fn(usize) -> i32;
type CloseHandleFn = unsafe extern "system" fn(usize) -> i32;
type MoveFileExWFn = unsafe extern "system" fn(*const u16, *const u16, u32) -> i32;
type MoveFileWFn = unsafe extern "system" fn(*const u16, *const u16) -> i32;
type CopyFileWFn = unsafe extern "system" fn(*const u16, *const u16, i32) -> i32;
type DeleteFileWFn = unsafe extern "system" fn(*const u16) -> i32;

static ORIG_CREATE_FILE_W: AtomicUsize = AtomicUsize::new(0);
static ORIG_WRITE_FILE: AtomicUsize = AtomicUsize::new(0);
static ORIG_SET_END_OF_FILE: AtomicUsize = AtomicUsize::new(0);
static ORIG_CLOSE_HANDLE: AtomicUsize = AtomicUsize::new(0);
static ORIG_MOVE_FILE_EX_W: AtomicUsize = AtomicUsize::new(0);
static ORIG_MOVE_FILE_W: AtomicUsize = AtomicUsize::new(0);
static ORIG_COPY_FILE_W: AtomicUsize = AtomicUsize::new(0);
static ORIG_DELETE_FILE_W: AtomicUsize = AtomicUsize::new(0);

static INSTALLED_HOOKS: AtomicUsize = AtomicUsize::new(0);

/// How many file APIs this module tries to hook. Derived from the target table rather
/// than written as a literal, because a stale literal reported "8/6 hooked" and would
/// have let a partial-coverage run read as complete.
pub(crate) const EXPECTED_HOOKS: usize = 8;

pub(crate) fn installed_hooks() -> usize {
    INSTALLED_HOOKS.load(Ordering::SeqCst)
}

/// Install the census detours. Returns the number of APIs successfully hooked.
///
/// A failure to hook one API is logged and skipped rather than aborting the set:
/// a partial census is still evidence, and it is honestly reported through
/// `installed_hooks` so a run with incomplete coverage cannot be mistaken for a
/// clean one.
pub(crate) fn install() -> usize {
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            log_message(format_args!("census: MH_Initialize failed: {status:?}"));
            return 0;
        }
    }

    let kernel32 = match kernel32_module() {
        Some(module) => module,
        None => {
            log_message(format_args!("census: kernel32 module not found"));
            return 0;
        }
    };

    let targets: [(&str, *mut c_void, &AtomicUsize); EXPECTED_HOOKS] = [
        (
            "CreateFileW",
            create_file_w_hook as *mut c_void,
            &ORIG_CREATE_FILE_W,
        ),
        (
            "WriteFile",
            write_file_hook as *mut c_void,
            &ORIG_WRITE_FILE,
        ),
        (
            "SetEndOfFile",
            set_end_of_file_hook as *mut c_void,
            &ORIG_SET_END_OF_FILE,
        ),
        (
            "CloseHandle",
            close_handle_hook as *mut c_void,
            &ORIG_CLOSE_HANDLE,
        ),
        (
            "MoveFileExW",
            move_file_ex_w_hook as *mut c_void,
            &ORIG_MOVE_FILE_EX_W,
        ),
        (
            "DeleteFileW",
            delete_file_w_hook as *mut c_void,
            &ORIG_DELETE_FILE_W,
        ),
        // The proven escapes: the backup is built by copy, not by write.
        (
            "CopyFileW",
            copy_file_w_hook as *mut c_void,
            &ORIG_COPY_FILE_W,
        ),
        (
            "MoveFileW",
            move_file_w_hook as *mut c_void,
            &ORIG_MOVE_FILE_W,
        ),
    ];

    let mut queued = 0;
    for (name, detour, orig_slot) in targets {
        let Some(address) = proc_address(kernel32, name) else {
            log_message(format_args!("census: {name} not exported by kernel32"));
            continue;
        };
        let hook = match unsafe { MhHook::new(address as *mut c_void, detour) } {
            Ok(hook) => hook,
            Err(status) => {
                log_message(format_args!(
                    "census: MhHook::new({name} @0x{address:x}) failed: {status:?}"
                ));
                continue;
            }
        };
        orig_slot.store(hook.trampoline() as usize, Ordering::SeqCst);
        if let Err(status) = unsafe { hook.queue_enable() } {
            log_message(format_args!(
                "census: queue_enable({name}) failed: {status:?}"
            ));
            continue;
        }
        queued += 1;
    }

    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => {
            // Deliberately silent on success: `lib.rs` logs one install summary line
            // that already carries `census hooks=N/M`. Only the failures above, which
            // that summary cannot explain, get their own line.
            INSTALLED_HOOKS.store(queued, Ordering::SeqCst);
            queued
        }
        status => {
            log_message(format_args!("census: MH_ApplyQueued failed: {status:?}"));
            0
        }
    }
}

/// Access bits that mean the caller intends to modify the file.
const GENERIC_WRITE: u32 = 0x4000_0000;
const FILE_WRITE_DATA: u32 = 0x0000_0002;
const FILE_APPEND_DATA: u32 = 0x0000_0004;

fn is_write_open(desired_access: u32) -> bool {
    desired_access & (GENERIC_WRITE | FILE_WRITE_DATA | FILE_APPEND_DATA) != 0
}

unsafe extern "system" fn create_file_w_hook(
    file_name: *const u16,
    desired_access: u32,
    share_mode: u32,
    security_attributes: *mut c_void,
    creation_disposition: u32,
    flags_and_attributes: u32,
    template_file: *mut c_void,
) -> usize {
    let orig = ORIG_CREATE_FILE_W.load(Ordering::SeqCst);
    if orig == 0 {
        return usize::MAX;
    }
    let original: CreateFileWFn = unsafe { core::mem::transmute(orig) };

    // Divert ONLY write opens. The load path shares this function, so redirecting reads
    // would make the game load the diverted file instead of the player's real save --
    // and the game's own `OpenFile` uses OPEN_EXISTING to read and OPEN_ALWAYS to write,
    // so the access bits are a reliable discriminator.
    let diverted = if is_write_open(desired_access) {
        unsafe { save_path_string(file_name) }.and_then(|path| redirect::diverted_path(&path))
    } else {
        None
    };

    let handle = match &diverted {
        Some(path) => {
            let wide = redirect::to_wide(path);
            let handle = unsafe {
                original(
                    wide.as_ptr(),
                    desired_access,
                    share_mode,
                    security_attributes,
                    creation_disposition,
                    flags_and_attributes,
                    template_file,
                )
            };
            witness::note_diverted_open(path, handle);
            handle
        }
        None => unsafe {
            original(
                file_name,
                desired_access,
                share_mode,
                security_attributes,
                creation_disposition,
                flags_and_attributes,
                template_file,
            )
        },
    };

    // Census the ORIGINAL path either way: what matters is that the game asked to write
    // a save, and whether any byte reached the real file.
    if diverted.is_none() {
        unsafe { witness::note_create_file(file_name, desired_access, handle) };
    }
    handle
}

/// Read a wide path and return it only when it names save data.
///
/// # Safety
/// `ptr` must be a valid wide string pointer or null.
unsafe fn save_path_string(ptr: *const u16) -> Option<String> {
    let path = unsafe { witness::read_wide_path(ptr) }?;
    witness::is_save_path(&path).then_some(path)
}

unsafe extern "system" fn write_file_hook(
    file: usize,
    buffer: *const c_void,
    bytes_to_write: u32,
    bytes_written: *mut u32,
    overlapped: *mut c_void,
) -> i32 {
    // Observe BEFORE the call: the census records intent, so an interception that
    // later fails the write still shows up as a save attempt.
    witness::note_write_file(file, u64::from(bytes_to_write));
    let orig = ORIG_WRITE_FILE.load(Ordering::SeqCst);
    if orig == 0 {
        return 0;
    }
    let original: WriteFileFn = unsafe { core::mem::transmute(orig) };
    unsafe { original(file, buffer, bytes_to_write, bytes_written, overlapped) }
}

unsafe extern "system" fn set_end_of_file_hook(file: usize) -> i32 {
    witness::note_set_end_of_file(file);
    let orig = ORIG_SET_END_OF_FILE.load(Ordering::SeqCst);
    if orig == 0 {
        return 0;
    }
    let original: SetEndOfFileFn = unsafe { core::mem::transmute(orig) };
    unsafe { original(file) }
}

unsafe extern "system" fn close_handle_hook(object: usize) -> i32 {
    witness::note_close_handle(object);
    let orig = ORIG_CLOSE_HANDLE.load(Ordering::SeqCst);
    if orig == 0 {
        return 0;
    }
    let original: CloseHandleFn = unsafe { core::mem::transmute(orig) };
    unsafe { original(object) }
}

unsafe extern "system" fn move_file_ex_w_hook(
    existing: *const u16,
    new: *const u16,
    flags: u32,
) -> i32 {
    unsafe { witness::note_move_file(existing, new) };
    let orig = ORIG_MOVE_FILE_EX_W.load(Ordering::SeqCst);
    if orig == 0 {
        return 0;
    }
    let original: MoveFileExWFn = unsafe { core::mem::transmute(orig) };
    unsafe { original(existing, new, flags) }
}

unsafe extern "system" fn delete_file_w_hook(file_name: *const u16) -> i32 {
    let orig = ORIG_DELETE_FILE_W.load(Ordering::SeqCst);
    if orig == 0 {
        return 0;
    }
    let original: DeleteFileWFn = unsafe { core::mem::transmute(orig) };

    // The game deletes the stale backup before recreating it. Point that at the diverted
    // backup so the player's real .bak is never removed. Report success even if the
    // diverted file was not there -- the game only needs the name to be free.
    match unsafe { save_path_string(file_name) }.and_then(|p| redirect::diverted_path(&p)) {
        Some(path) => {
            witness::note_diverted_op("DeleteFileW");
            let wide = redirect::to_wide(&path);
            unsafe { original(wide.as_ptr()) };
            1
        }
        None => {
            unsafe { witness::note_delete_file(file_name) };
            unsafe { original(file_name) }
        }
    }
}

unsafe extern "system" fn copy_file_w_hook(
    existing: *const u16,
    new: *const u16,
    fail_if_exists: i32,
) -> i32 {
    let orig = ORIG_COPY_FILE_W.load(Ordering::SeqCst);
    if orig == 0 {
        return 0;
    }
    let original: CopyFileWFn = unsafe { core::mem::transmute(orig) };

    // Divert only the DESTINATION. The source stays real so the copy always has something
    // to read (a diverted source would not exist on the first save and the copy would
    // fail); the player's real .bak is never the target.
    let diverted = unsafe { save_path_string(new) }.and_then(|path| redirect::diverted_path(&path));
    match diverted {
        Some(path) => {
            // Counted as diverted, NOT as an escape. Censusing the original path here
            // would list every successful redirect as a save that got away, which is
            // exactly backwards.
            witness::note_diverted_op("CopyFileW");
            let wide = redirect::to_wide(&path);
            // fail_if_exists must be cleared: the diverted backup survives between saves,
            // and the game passes 1, which would fail every save after the first.
            unsafe { original(existing, wide.as_ptr(), 0) }
        }
        None => {
            unsafe { witness::note_copy_file(existing, new) };
            unsafe { original(existing, new, fail_if_exists) }
        }
    }
}

unsafe extern "system" fn move_file_w_hook(existing: *const u16, new: *const u16) -> i32 {
    unsafe { witness::note_move_file_plain(existing, new) };
    let orig = ORIG_MOVE_FILE_W.load(Ordering::SeqCst);
    if orig == 0 {
        return 0;
    }
    let original: MoveFileWFn = unsafe { core::mem::transmute(orig) };

    // A rename touching save data moves BOTH ends into the diverted namespace, so a
    // rename-swap can never clobber the real file with, or replace it by, anything.
    let from = unsafe { save_path_string(existing) }.and_then(|p| redirect::diverted_path(&p));
    let to = unsafe { save_path_string(new) }.and_then(|p| redirect::diverted_path(&p));
    if from.is_none() && to.is_none() {
        return unsafe { original(existing, new) };
    }
    let from_wide = from.as_deref().map(redirect::to_wide);
    let to_wide = to.as_deref().map(redirect::to_wide);
    unsafe {
        original(
            from_wide.as_ref().map_or(existing, |w| w.as_ptr()),
            to_wide.as_ref().map_or(new, |w| w.as_ptr()),
        )
    }
}

fn kernel32_module() -> Option<usize> {
    let name: Vec<u16> = "kernel32.dll\0".encode_utf16().collect();
    let module = unsafe { GetModuleHandleW(name.as_ptr()) };
    (module != 0).then_some(module)
}

fn proc_address(module: usize, name: &str) -> Option<usize> {
    let mut symbol: Vec<u8> = name.as_bytes().to_vec();
    symbol.push(0);
    let address = unsafe { GetProcAddress(module, symbol.as_ptr()) };
    (address != 0).then_some(address)
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetModuleHandleW(module_name: *const u16) -> usize;
    fn GetProcAddress(module: usize, proc_name: *const u8) -> usize;
}
