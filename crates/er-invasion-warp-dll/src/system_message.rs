//! Show a transient message on the game's own system-message banner.
//!
//! This is the widget that says "Cannot connect to network" — non-modal, self-dismissing, and
//! rendered by the game rather than by any overlay of ours. It is deliberately NOT a
//! `CS::MessageBoxDialog`: this repo treats those as a hard investigation trigger and never
//! displays one as product behaviour. A banner is a different object on a different path.
//!
//! # The recipe, taken from the game's own caller
//!
//! `CS::CSLuaEventManImp::OnUnstableFrameRate` does exactly this and nothing else:
//!
//! ```text
//!   MenuString s;
//!   GetGR_System_Message(&s, 0x1041);   // resolve an FMG id into a string
//!   showPopupMenu(&s);                  // hand it to CSPopupMenu
//!   if (s.dLString.capacity > 7) allocator->Deallocate(allocator, ptr);
//!   s.capacity = 7; s.length = 0; s.inline[0] = 0;
//! ```
//!
//! So the display path is `showPopupMenu(MenuString*)`, which reads `GLOBAL_CSMenuMan`, takes
//! `+0x80` as the popup menu, and calls `CS::CSPopupMenu::ShowMenu`. Both pointers are null-checked
//! there, so calling this before the menu manager exists is a no-op rather than a fault.
//!
//! # Why we do not build a `DLString` by hand
//!
//! The obvious approach — point a `DLString` at our own UTF-16 buffer — hinges on whether the game
//! takes ownership of that pointer. It does not: `CSPopupMenu::ShowMenu` pushes onto a
//! `deque<MenuString>` whose `Push` ends in `DLTX::DLString<wchar_t>::Copy(dest, src)`, a deep copy
//! into the queue's own storage. That was checked before writing any of this, because the failure
//! mode is the game freeing a static buffer.
//!
//! Even so, this asks the GAME to build the string and then overwrites the characters in place,
//! clamped to the capacity it allocated. That way the allocator, the vtable and every field except
//! the text come from the game's own constructor, and we allocate and free nothing. The cost is a
//! length ceiling set by the donor message; see [`DONOR_MESSAGE_ID`].
//!
//! # Verified addresses (1.16.2, shift zero — dump VA == deobf VA == runtime VA)
//!
//! Both prologues were read out of `eldenring-deobf.bin` before being used, not copied from a
//! decompiler listing.

use std::sync::atomic::{AtomicUsize, Ordering};

/// `CS::GetGR_System_Message(MenuString* rcx, int edx)` — resolves an FMG id into a `MenuString`.
///
/// Re-exported from the shared table rather than re-declared: `er-effects-rs` hooks the same
/// function read-only, and two literals for one address is the drift `er_game_base::rva` exists to
/// prevent.
pub use er_game_base::rva::GR_SYSTEM_MESSAGE_RVA as GET_GR_SYSTEM_MESSAGE_RVA;
/// Opening bytes at that RVA: `mov [rsp+8],rcx; push rbx; sub rsp,0x40`.
pub const GET_GR_SYSTEM_MESSAGE_PROLOGUE: &[u8] =
    &[0x48, 0x89, 0x4c, 0x24, 0x08, 0x53, 0x48, 0x83, 0xec, 0x40];

/// `showPopupMenu(MenuString* rcx)` — pushes the string onto `CSPopupMenu`'s queue.
pub const SHOW_POPUP_MENU_RVA: usize = 0x5f_5e80;
/// Opening bytes: `sub rsp,0x38; xor eax,eax; mov rdx,rcx`.
pub const SHOW_POPUP_MENU_PROLOGUE: &[u8] = &[0x48, 0x83, 0xec, 0x38, 0x33, 0xc0, 0x48, 0x8b, 0xd1];

/// The FMG id borrowed to construct a valid `MenuString`.
///
/// `0x1041` (4161) is what `OnUnstableFrameRate` shows. It is used ONLY as a donor: the string it
/// resolves is overwritten before anything is displayed, so its wording never reaches the player.
/// What it does contribute is its ALLOCATION — our text is clamped to the capacity this message's
/// text required, so a longer donor buys more room. If messages start arriving truncated, that is
/// the knob.
pub const DONOR_MESSAGE_ID: i32 = 0x1041;

/// `MenuString` is 56 bytes: `wchar_t* rawString` then a 48-byte `DLString<wchar_t>`.
pub const MENU_STRING_SIZE: usize = 56;
const RAW_STRING_OFFSET: usize = 0x00;
const DLSTRING_OFFSET: usize = 0x08;
/// Within `DLString<wchar_t>`: allocator, a 16-byte union (8 inline wchars OR a heap pointer),
/// then `length` and `capacity`, both `size_t`, both counted in CHARACTERS.
const ALLOCATOR_OFFSET: usize = DLSTRING_OFFSET;
const UNION_OFFSET: usize = DLSTRING_OFFSET + 0x08;
const LENGTH_OFFSET: usize = DLSTRING_OFFSET + 0x18;
const CAPACITY_OFFSET: usize = DLSTRING_OFFSET + 0x20;
/// A capacity at or below this means the text lives inline in the union rather than on the heap.
/// The game's own reset path writes exactly this value, so it is the game's threshold, not ours.
const INLINE_CAPACITY: usize = 7;
/// `DLAllocator`'s vtable slot for `Deallocate`, as used by the game's own free path.
const DEALLOCATE_VTABLE_SLOT: usize = 0x08;

type GetSystemMessageFn = unsafe extern "system" fn(*mut u8, i32) -> *mut u8;
type ShowPopupMenuFn = unsafe extern "system" fn(*mut u8);
type DeallocateFn = unsafe extern "system" fn(usize, usize);

static MESSAGES_SHOWN: AtomicUsize = AtomicUsize::new(0);
static REFUSALS: AtomicUsize = AtomicUsize::new(0);

/// How many banners were displayed, and how many attempts were refused before display.
#[must_use]
pub fn tally() -> (usize, usize) {
    (
        MESSAGES_SHOWN.load(Ordering::SeqCst),
        REFUSALS.load(Ordering::SeqCst),
    )
}

/// Resolve a game function and confirm its opening bytes before handing back a pointer.
///
/// A byte check rather than a bare RVA because a game update that moves this lands us in the middle
/// of an instruction. Failing to find it disables the banner; jumping into one crashes the game.
#[cfg(windows)]
fn verified_fn(rva: usize, prologue: &[u8]) -> Option<usize> {
    let base = er_game_base::mem::game_module_base().ok()?;
    let address = base + rva;
    for (index, expected) in prologue.iter().enumerate() {
        let actual = unsafe { er_game_base::mem::safe_read_u8(address + index) }?;
        if actual != *expected {
            return None;
        }
    }
    Some(address)
}

/// Write `text` into a `MenuString` the game already built, clamped to its capacity.
///
/// Returns the number of characters written. Never grows the allocation, never reallocates, and
/// always leaves a NUL terminator inside the buffer the game owns.
fn overwrite_text(menu_string: *mut u8, text: &str) -> usize {
    let capacity = unsafe { menu_string.add(CAPACITY_OFFSET).cast::<usize>().read() };
    if capacity == 0 {
        return 0;
    }
    // The union holds the characters directly when the string is short enough, and a pointer to
    // them otherwise. Which of the two is decided by the SAME threshold the game's own reset path
    // uses, so this cannot disagree with the code that will later free it.
    let buffer: *mut u16 = if capacity <= INLINE_CAPACITY {
        unsafe { menu_string.add(UNION_OFFSET).cast::<u16>() }
    } else {
        let heap = unsafe { menu_string.add(UNION_OFFSET).cast::<usize>().read() };
        if heap == 0 {
            return 0;
        }
        heap as *mut u16
    };

    // Leave room for the terminator: `capacity` counts characters the buffer can hold, and the
    // game's strings are NUL-terminated.
    let room = capacity.saturating_sub(1);
    let mut written = 0usize;
    for unit in text.encode_utf16() {
        if written >= room {
            break;
        }
        unsafe { buffer.add(written).write(unit) };
        written += 1;
    }
    unsafe { buffer.add(written).write(0) };
    unsafe {
        menu_string
            .add(LENGTH_OFFSET)
            .cast::<usize>()
            .write(written);
        // `rawString` is the plain pointer the UI reads; point it at the same characters so the two
        // views of this string cannot disagree.
        menu_string
            .add(RAW_STRING_OFFSET)
            .cast::<usize>()
            .write(buffer as usize);
    }
    written
}

/// Free the donor string exactly the way the game's own caller does.
///
/// Mirrors `OnUnstableFrameRate`: deallocate through the allocator's vtable only when the string
/// went to the heap, then reset the struct to the empty inline state. Getting this wrong leaks on
/// every message, and getting it wrong in the other direction frees a pointer we do not own.
fn release(menu_string: *mut u8) {
    let capacity = unsafe { menu_string.add(CAPACITY_OFFSET).cast::<usize>().read() };
    if capacity > INLINE_CAPACITY {
        let allocator = unsafe { menu_string.add(ALLOCATOR_OFFSET).cast::<usize>().read() };
        let heap = unsafe { menu_string.add(UNION_OFFSET).cast::<usize>().read() };
        if allocator != 0 && heap != 0 {
            if let Some(vtable) = unsafe { er_game_base::mem::safe_read_usize(allocator) } {
                if let Some(slot) =
                    unsafe { er_game_base::mem::safe_read_usize(vtable + DEALLOCATE_VTABLE_SLOT) }
                {
                    if slot != 0 {
                        let deallocate: DeallocateFn =
                            unsafe { core::mem::transmute::<usize, DeallocateFn>(slot) };
                        unsafe { deallocate(allocator, heap) };
                    }
                }
            }
        }
    }
    unsafe {
        menu_string
            .add(CAPACITY_OFFSET)
            .cast::<usize>()
            .write(INLINE_CAPACITY);
        menu_string.add(LENGTH_OFFSET).cast::<usize>().write(0);
        menu_string.add(UNION_OFFSET).cast::<u16>().write(0);
    }
}

/// Show `text` on the game's system-message banner. No-op if anything is not ready.
///
/// # Safety
/// Must run on the game thread, in the same context the game's own callers use — it calls two game
/// functions and touches a struct the game allocated.
#[cfg(windows)]
pub unsafe fn show(text: &str) -> bool {
    let Some(getter) = verified_fn(GET_GR_SYSTEM_MESSAGE_RVA, GET_GR_SYSTEM_MESSAGE_PROLOGUE)
    else {
        REFUSALS.fetch_add(1, Ordering::SeqCst);
        return false;
    };
    let Some(shower) = verified_fn(SHOW_POPUP_MENU_RVA, SHOW_POPUP_MENU_PROLOGUE) else {
        REFUSALS.fetch_add(1, Ordering::SeqCst);
        return false;
    };
    let get: GetSystemMessageFn =
        unsafe { core::mem::transmute::<usize, GetSystemMessageFn>(getter) };
    let show_popup: ShowPopupMenuFn =
        unsafe { core::mem::transmute::<usize, ShowPopupMenuFn>(shower) };

    // Zeroed, because the getter constructs into this and a stale stack pattern would be read as a
    // live allocation if construction bailed early.
    let mut storage = [0u8; MENU_STRING_SIZE];
    let menu_string = storage.as_mut_ptr();
    unsafe { get(menu_string, DONOR_MESSAGE_ID) };

    let written = overwrite_text(menu_string, text);
    if written == 0 {
        // Nothing was written, so nothing is worth showing -- but the donor still allocated and
        // must be released regardless.
        release(menu_string);
        REFUSALS.fetch_add(1, Ordering::SeqCst);
        return false;
    }
    unsafe { show_popup(menu_string) };
    release(menu_string);
    MESSAGES_SHOWN.fetch_add(1, Ordering::SeqCst);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The struct layout, pinned. Everything else in this module is arithmetic on these, and they
    /// came from the 1.16.2 dump: MenuString is 56 bytes = `wchar_t*` + a 48-byte DLString whose
    /// fields are allocator / 16-byte union / length / capacity.
    #[test]
    fn the_layout_matches_the_dump() {
        assert_eq!(MENU_STRING_SIZE, 56);
        assert_eq!(RAW_STRING_OFFSET, 0x00);
        assert_eq!(DLSTRING_OFFSET, 0x08);
        assert_eq!(ALLOCATOR_OFFSET, 0x08);
        assert_eq!(UNION_OFFSET, 0x10);
        assert_eq!(LENGTH_OFFSET, 0x20);
        assert_eq!(CAPACITY_OFFSET, 0x28);
        // The union is 16 bytes and sits between the allocator and length.
        assert_eq!(LENGTH_OFFSET - UNION_OFFSET, 16);
        // Every field stays inside the struct.
        assert!(CAPACITY_OFFSET + 8 <= MENU_STRING_SIZE);
    }

    /// The inline/heap threshold must be the value the GAME writes when it resets a string, or our
    /// free path and its free path would disagree about whether a pointer needs deallocating.
    #[test]
    fn the_inline_threshold_is_the_games_own() {
        assert_eq!(INLINE_CAPACITY, 7);
    }

    /// Prologues are what makes a moved function fail closed instead of crashing, so an empty or
    /// truncated one would silently disable the check.
    #[test]
    fn prologues_are_long_enough_to_discriminate() {
        assert!(GET_GR_SYSTEM_MESSAGE_PROLOGUE.len() >= 8);
        assert!(SHOW_POPUP_MENU_PROLOGUE.len() >= 8);
        assert_ne!(GET_GR_SYSTEM_MESSAGE_PROLOGUE, SHOW_POPUP_MENU_PROLOGUE);
    }
}
