//! Reading the Windows clipboard, so the build-url editor can be filled with a copied link.
//!
//! # Why this exists at all
//!
//! The obvious design is "open a text field and let the player paste". That does not work: the
//! field the row opens has no paste. Ctrl+V inside it is inert, and the reason is structural
//! rather than a missing binding -- see [`build_url_editor`] for the full path, but in short the
//! visible field is a Scaleform `DefineEditText` inside `02_990`, driven by the game's own
//! keyboard-event handling, and nothing in that handling consults a clipboard.
//!
//! The image does contain a working `CF_UNICODETEXT` reader -- `FUN_1426760c0`, which opens the
//! clipboard on an HWND held at `this+0x8b0` and copies the wide string into a `DLString`. It is
//! not reachable from here: it occupies slot `+0x100` of exactly one vtable (`0x143296bb8`, the
//! only vtable in the whole image that holds it), whose siblings measure text through
//! `GLOBAL_FD4FontManager`, and no call site for that slot exists. The software keyboard's own
//! stack is a different object graph entirely -- `SoftwareKeyboard::detail::
//! SoftwareKeyboardManagerImpl` (Steam gamepad text input) with a Scaleform `02_990` MenuWindow
//! fallback -- and neither half touches that vtable.
//!
//! So the paste happens on our side of the boundary: this DLL reads the clipboard and puts the
//! text into the field. Once when the field opens, and again whenever the clipboard changes while
//! it is open -- the second half is what makes a player's Ctrl+V appear to work, because by the
//! time they press it they have already copied the link.
//!
//! Under Wine the Windows clipboard is bridged to the host X11/Wayland selection, so a link copied
//! in a Linux browser arrives here. That bridge is asynchronous, which is the other reason the
//! open-time read alone was not enough: a link copied moments before the row press can arrive
//! after it. A failure to read simply leaves the field as it was.

use crate::host::append_autoload_debug;

use windows::Win32::Foundation::{HANDLE, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardSequenceNumber,
    IsClipboardFormatAvailable, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};

// `GlobalFree` is not in `windows` 0.62's `Win32_System_Memory` binding, though its three
// siblings are. Declared by hand rather than worked around, in the same style as the WinHTTP
// imports in `er-build-import-runtime::http`: the exact ABI is then visible at the call site and
// cannot drift with a crate upgrade. It is needed on exactly one path -- the allocation that
// `SetClipboardData` refused, which this process still owns and must not leak.
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GlobalFree(hmem: *mut core::ffi::c_void) -> *mut core::ffi::c_void;
}

/// `CF_UNICODETEXT`. The only format asked for: the field is UTF-16 and a link is text.
const CF_UNICODETEXT: u32 = 13;

/// Longest clipboard text considered. A share link is ~60 characters; anything past this is not a
/// link the player meant to paste, and the bound keeps a huge clipboard (a copied document) from
/// being walked a code unit at a time.
const MAX_CLIPBOARD_UNITS: usize = 512;

/// The clipboard's text, or `None` when there is none, it is not text, or the clipboard is locked
/// by another process.
///
/// Every failure is `None` rather than an error: the caller's response to all of them is the same
/// (open with the bare prefix instead), and a player whose clipboard holds an image does not need
/// to be told about it.
pub fn clipboard_text() -> Option<String> {
    // Safety: the clipboard API is process-wide and this is the only place this DLL touches it.
    // Every handle is released on every path below, including the early returns.
    unsafe {
        if IsClipboardFormatAvailable(CF_UNICODETEXT).is_err() {
            return None;
        }
        // A null owner is legal and means "this task". The clipboard is briefly locked to every
        // other process while open, so it is closed again as soon as the bytes are copied out.
        if OpenClipboard(Some(HWND::default())).is_err() {
            return None;
        }
        let text = read_open_clipboard();
        let _ = CloseClipboard();
        text
    }
}

/// Copy the open clipboard's UTF-16 text out. Split from [`clipboard_text`] so there is exactly one
/// `CloseClipboard`, on every path, no matter which read step fails.
///
/// # Safety
///
/// The clipboard must be open and `CF_UNICODETEXT` available.
unsafe fn read_open_clipboard() -> Option<String> {
    // Safety: guaranteed by the caller's contract.
    let handle: HANDLE = unsafe { GetClipboardData(CF_UNICODETEXT) }.ok()?;
    if handle.is_invalid() {
        return None;
    }
    // Safety: a CF_UNICODETEXT handle is a movable global; locking yields the wide string.
    let locked = unsafe { GlobalLock(windows::Win32::Foundation::HGLOBAL(handle.0)) };
    if locked.is_null() {
        return None;
    }
    let mut units: Vec<u16> = Vec::new();
    for index in 0..MAX_CLIPBOARD_UNITS {
        // Safety: the block is NUL-terminated by the CF_UNICODETEXT contract, and the loop is
        // bounded independently so a clipboard that lies cannot walk off the allocation.
        let unit = unsafe { *(locked as *const u16).add(index) };
        if unit == 0 {
            break;
        }
        units.push(unit);
    }
    // Safety: paired with the lock above.
    let _ = unsafe { GlobalUnlock(windows::Win32::Foundation::HGLOBAL(handle.0)) };
    String::from_utf16(&units).ok()
}

/// A monotonically increasing count of clipboard writes, process-wide.
///
/// This is the cheap half of the live mirror: it needs no clipboard lock, cannot fail, and cannot
/// block another process, so the open field can ask it every frame. Only a change justifies the
/// real read below. Zero is returned when the caller has no clipboard access at all, which the
/// mirror treats as "no news" rather than as a change.
pub fn clipboard_sequence() -> u32 {
    // Safety: a pure read of a process-wide counter; it takes no handle and locks nothing.
    unsafe { GetClipboardSequenceNumber() }
}

/// The clipboard's contents when they are an importable build link, trimmed. `None` for anything
/// else, including an unreadable clipboard.
///
/// The clipboard is only used when it VALIDATES. Filling the field with arbitrary clipboard
/// contents would mean a player who last copied a paragraph gets a paragraph -- and has to clear it
/// before they can type -- strictly worse than leaving the field alone. It is also what makes the
/// live mirror safe to run while the field is open: an unrelated copy cannot overwrite what the
/// player is typing, because it cannot pass this gate.
pub fn clipboard_build_url() -> Option<String> {
    let text = clipboard_text()?;
    er_build_import_core::validate_build_url(&text).ok()?;
    Some(text.trim().to_owned())
}

/// The text the editor should open with: the clipboard when it holds an importable link, else the
/// bare prefix for the player to complete.
pub fn build_url_initial_text() -> String {
    match clipboard_build_url() {
        Some(link) => {
            append_autoload_debug(format_args!(
                "system-quit-build-url: prefilling the editor from the clipboard ({} chars)",
                link.chars().count()
            ));
            link
        }
        None => er_build_import_core::BUILD_URL_PREFIX.to_owned(),
    }
}

// ---------------------------------------------------------------------------------------------
// Writing. The read above exists because the game's own field has no paste; this exists because the
// Generate Build Link row produces something the player will want to send to someone, and a link
// that only ever appeared inside a browser tab they then closed is a link they cannot get back.
//
// Under Wine the Windows clipboard is bridged to the host X11/Wayland selection in both directions,
// so a link copied here can be pasted into a Linux application. That bridge is asynchronous going
// out as well as coming in -- ownership is handed over when the host asks for it -- which is why
// this reports only that the clipboard accepted the data, never that a paste elsewhere will work.

/// Put `text` on the clipboard as `CF_UNICODETEXT`.
///
/// Returns whether the clipboard took it. Every failure is a `false` rather than an error for the
/// same reason the read returns `None`: the caller's response to all of them is identical, and the
/// row reports the outcome in one word either way.
///
/// The clipboard takes ownership of the global on success, so it is freed only on the paths where
/// `SetClipboardData` did not take it -- freeing it afterwards would hand every later paste a
/// dangling block.
pub fn set_clipboard_text(text: &str) -> bool {
    let mut units: Vec<u16> = text.encode_utf16().collect();
    units.push(0);
    let bytes = core::mem::size_of_val(units.as_slice());

    // Safety: the clipboard API is process-wide; the handle is opened, filled and closed on every
    // path below, and ownership of the allocation is transferred exactly once.
    unsafe {
        if OpenClipboard(Some(HWND::default())).is_err() {
            return false;
        }
        // `EmptyClipboard` is not optional: it is what makes this process the clipboard owner, and
        // `SetClipboardData` fails without that ownership.
        if EmptyClipboard().is_err() {
            let _ = CloseClipboard();
            return false;
        }
        let Ok(block) = GlobalAlloc(GMEM_MOVEABLE, bytes) else {
            let _ = CloseClipboard();
            return false;
        };
        let locked = GlobalLock(block);
        if locked.is_null() {
            GlobalFree(block.0);
            let _ = CloseClipboard();
            return false;
        }
        core::ptr::copy_nonoverlapping(units.as_ptr().cast::<u8>(), locked.cast::<u8>(), bytes);
        let _ = GlobalUnlock(block);

        let taken = SetClipboardData(CF_UNICODETEXT, Some(HANDLE(block.0))).is_ok();
        if !taken {
            // Only here: on success the clipboard owns the block and freeing it would leave every
            // later paste reading freed memory.
            GlobalFree(block.0);
        }
        let _ = CloseClipboard();
        taken
    }
}
