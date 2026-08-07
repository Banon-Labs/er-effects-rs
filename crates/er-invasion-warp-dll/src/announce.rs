//! Puts a line of text on the game's own auto-closing announcement surface.
//!
//! This is the surface that says "Grace discovered" — it appears, scrolls, expires, and never
//! waits for input. It replaces a deleted `system_message` module, which used `showPopupMenu` and
//! therefore produced a BLOCKING MODAL WITH AN OK BUTTON: the user got a dialog they had to dismiss
//! for every rejection, showing squares and then nothing, while the unattended dialog held the
//! Seamless session open long enough to trip the stall watchdog. `showPopupMenu` is named a popup
//! *menu* and behaves like one; that it was chosen at all was a failure to read.
//!
//! # Why this surface can do what a modal cannot
//!
//! `CS::AnnounceMessage` carries a `DLString<wchar_t>` **directly** rather than a message id, so
//! arbitrary text needs no FMG entry — which was the open question that made this feature look
//! impossible. And `FeSystemAnnounceView` owns `systemAnnounceScrollBufferTimer` and
//! `systemAnnounceScrollCount`: it times itself out. Nothing here waits for a button.
//!
//! # How the message gets in without the queue
//!
//! `FeSystemAnnounceView::Update` drains the view-model's queue ONLY when its own embedded message
//! is inactive, disassembled at `0x1408c481a`:
//!
//! ```text
//!   cmpb $0x0, 0xb10(%rbx)     ; if (!view->msg.is_active)
//!   call 0x140841b00           ;     popped = queue.pop()
//!   call 0x1408c4710           ;     view->msg = *popped
//!   movb $0x1, 0xb50(%rbx)     ;     announcePlayState = Load
//!   cmpb $0x0, 0xb10(%rbx)     ; if (view->msg.is_active)
//!   call 0x1408c48c0           ;     display step
//! ```
//!
//! So filling `view->msg` ourselves is byte-for-byte the state a successful pop would have left,
//! and Update walks straight to the display step. The queue is bypassed rather than fought — which
//! matters, because the queue's push function is not symbolised and was never found.
//!
//! The string is copied by the game's own `DLString::assign`, so the game allocates, owns and frees
//! it. That is deliberate: the previous attempt wrote into a donor string it did not own and got
//! silently clamped to its capacity, turning "Rejected m60_42_36_00 (elsewhere)" into "Reject".
//! Handing the allocation to the game removes that failure mode entirely rather than working
//! around it.

#[cfg(windows)]
use std::sync::atomic::{AtomicUsize, Ordering};

/// `CS::FeSystemAnnounceView::Update`. Hooked to learn the live view, which is otherwise reachable
/// only by walking `CSMenuMan`'s window list.
pub const UPDATE_RVA: usize = 0x8c_47c0;
/// `DLTX::DLString::assign(DLString<wchar_t>*, wchar_t*, size_t)` — grows the string itself.
pub const DLSTRING_ASSIGN_RVA: usize = 0x11_e360;

/// Opening bytes, so a game update that moves these fails closed instead of jumping mid-instruction.
pub const UPDATE_PROLOGUE: &[u8] = &[0x40, 0x53, 0x48, 0x83, 0xec, 0x30, 0x0f, 0x29];
pub const DLSTRING_ASSIGN_PROLOGUE: &[u8] =
    &[0x48, 0x89, 0x5c, 0x24, 0x10, 0x48, 0x89, 0x6c, 0x24, 0x18];

/// Offsets into `FeSystemAnnounceView`, from the 1.16.2 dump's own struct definition.
pub const MSG_OFFSET: usize = 0x0b10;
/// `AnnounceMessage::text`, a `DLString<wchar_t>` inside that message.
pub const MSG_TEXT_OFFSET: usize = 0x10;
/// `announcePlayState`, written as a BYTE by the game (`movb $0x1`), not as the enum's full width.
pub const PLAY_STATE_OFFSET: usize = 0x0b50;
/// The value Update writes when it has just loaded a message: `SystemAnnounceViewModelState::Load`.
pub const PLAY_STATE_LOAD: u8 = 1;

/// The longest message we will send. Not a buffer limit — `assign` grows — but a display one: the
/// surface scrolls, and an essay would scroll for the rest of the session.
pub const MAX_CHARS: usize = 96;

#[cfg(windows)]
type DlStringAssignFn = unsafe extern "system" fn(usize, *const u16, usize) -> usize;

#[cfg(windows)]
static LIVE_VIEW: AtomicUsize = AtomicUsize::new(0);
#[cfg(windows)]
static ORIG_UPDATE: AtomicUsize = AtomicUsize::new(0);
#[cfg(windows)]
static HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
#[cfg(windows)]
static SHOWN: AtomicUsize = AtomicUsize::new(0);
#[cfg(windows)]
static REFUSALS: AtomicUsize = AtomicUsize::new(0);

/// Announcements displayed, and attempts refused before display.
#[cfg(windows)]
#[must_use]
pub fn tally() -> (usize, usize) {
    (
        SHOWN.load(Ordering::SeqCst),
        REFUSALS.load(Ordering::SeqCst),
    )
}

#[cfg(not(windows))]
#[must_use]
pub fn tally() -> (usize, usize) {
    (0, 0)
}

/// Resolve a game function and confirm its opening bytes before handing back a pointer.
#[cfg(windows)]
fn verified_fn(rva: usize, prologue: &[u8]) -> Option<usize> {
    let base = er_game_base::mem::game_module_base().ok()?;
    let address = base + rva;
    for (index, expected) in prologue.iter().enumerate() {
        if unsafe { er_game_base::mem::safe_read_u8(address + index) }? != *expected {
            return None;
        }
    }
    Some(address)
}

/// Learn the live view and get out of the way.
///
/// Deliberately does NOT inject from here. Update runs every frame; doing work inside it would put
/// our cost on the frame budget and, worse, would mean writing the message from inside the very
/// function that reads it. Capturing the pointer and writing later from the rejection path keeps
/// the two apart, and both run on the game thread so there is no race to synchronise.
#[cfg(windows)]
unsafe extern "system" fn update_hook(view: usize, a: usize, b: usize, c: usize) -> usize {
    if view != 0 {
        LIVE_VIEW.store(view, Ordering::SeqCst);
    }
    let orig = ORIG_UPDATE.load(Ordering::SeqCst);
    if orig == 0 {
        return 0;
    }
    unsafe { core::mem::transmute::<usize, er_hook::UnionFn>(orig)(view, a, b, c) }
}

/// Install the view-capture hook. Idempotent; retries until the menu system exists.
#[cfg(windows)]
pub fn install() -> bool {
    if HOOK_INSTALLED.swap(1, Ordering::SeqCst) != 0 {
        return true;
    }
    let Some(address) = verified_fn(UPDATE_RVA, UPDATE_PROLOGUE) else {
        HOOK_INSTALLED.store(0, Ordering::SeqCst);
        return false;
    };
    match unsafe {
        er_hook::register_union_hook(address, update_hook as er_hook::UnionFn, &ORIG_UPDATE)
    } {
        Ok(()) => {
            crate::standalone_log(format_args!(
                "announce: watching CS::FeSystemAnnounceView::Update at {address:#x} to learn the \
                 live view -- this is the game's own auto-closing notice, not a dialog"
            ));
            true
        }
        Err(status) => {
            crate::standalone_log(format_args!(
                "announce: could not hook the announce view: {status:?} -- rejections will still \
                 work, only the on-screen notice is missing"
            ));
            false
        }
    }
}

/// Put `text` on the announcement surface. Returns false when it could not be shown.
///
/// # Safety
///
/// Game thread, with the menu system up. Writes into the live view's embedded message, which is
/// exactly what `Update` does when it pops one.
#[cfg(windows)]
pub unsafe fn show(text: &str) -> bool {
    let view = LIVE_VIEW.load(Ordering::SeqCst);
    if view == 0 {
        // The view has not ticked yet -- before the HUD exists there is nothing to write to. Not
        // an error; the next rejection will find it.
        REFUSALS.fetch_add(1, Ordering::SeqCst);
        return false;
    }
    let Some(assign) = verified_fn(DLSTRING_ASSIGN_RVA, DLSTRING_ASSIGN_PROLOGUE) else {
        REFUSALS.fetch_add(1, Ordering::SeqCst);
        return false;
    };
    let assign: DlStringAssignFn = unsafe { core::mem::transmute(assign) };

    let mut units: Vec<u16> = text.encode_utf16().take(MAX_CHARS).collect();
    if units.is_empty() {
        REFUSALS.fetch_add(1, Ordering::SeqCst);
        return false;
    }
    let length = units.len();
    // NUL-terminated because the UI reads it as a C string as well as by length; `assign` copies
    // `length` units, and the terminator keeps the two views of the string agreeing.
    units.push(0);

    let message = view + MSG_OFFSET;
    unsafe {
        assign(message + MSG_TEXT_OFFSET, units.as_ptr(), length);
        // is_active LAST of the two message fields: Update tests exactly this byte to decide
        // whether to pop, so setting it before the text is in place would race a frame boundary
        // and display whatever the string happened to hold.
        (message as *mut u8).write(1);
        ((view + PLAY_STATE_OFFSET) as *mut u8).write(PLAY_STATE_LOAD);
    }

    let count = SHOWN.fetch_add(1, Ordering::SeqCst) + 1;
    if count == 1 {
        crate::standalone_log(format_args!(
            "announce: first notice placed on the game's auto-closing surface (\"{text}\") -- view \
             {view:#x}. No dialog, no OK button; it expires on its own."
        ));
    }
    true
}

#[cfg(not(windows))]
pub unsafe fn show(_text: &str) -> bool {
    false
}

#[cfg(not(windows))]
pub fn install() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The offsets come from the dump's own `FeSystemAnnounceView` definition; pinning them means a
    /// future edit that "tidies" one has to confront that it is a memory layout, not a constant.
    #[test]
    fn the_offsets_match_the_dumps_struct() {
        assert_eq!(MSG_OFFSET, 0x0b10, "AnnounceMessage inside the view");
        assert_eq!(MSG_TEXT_OFFSET, 0x10, "DLString inside AnnounceMessage");
        assert_eq!(PLAY_STATE_OFFSET, 0x0b50, "announcePlayState");
        // is_active sits at the message's own offset 0, which is why the write target is the
        // message base rather than the message plus something.
        //
        // EXACTLY adjacent, not merely non-overlapping: AnnounceMessage is 64 bytes, so the play
        // state begins the instant the message ends. Asserting the exact relationship catches a
        // drifted offset that a `>` would wave through -- and a wrong play-state offset writes a 1
        // into whatever field actually lives there.
        assert_eq!(
            PLAY_STATE_OFFSET,
            MSG_OFFSET + 0x40,
            "announcePlayState immediately follows the 64-byte AnnounceMessage"
        );
    }

    /// A prologue too short to discriminate would let a moved function pass the check and be called
    /// anyway -- the failure mode this guard exists to prevent.
    #[test]
    fn prologues_are_long_enough_to_discriminate() {
        assert!(UPDATE_PROLOGUE.len() >= 8);
        assert!(DLSTRING_ASSIGN_PROLOGUE.len() >= 8);
    }

    /// THE POINT OF THE REWRITE. Nothing here may reach for the modal path: `showPopupMenu` is a
    /// popup MENU, it blocks on an OK button, and shipping it once already cost the user a dialog
    /// per rejection plus a stalled Seamless session.
    #[test]
    fn this_module_never_touches_the_modal_path() {
        // ONLY THE PRODUCT CODE, and only its non-comment lines. Both exclusions are load-bearing
        // and both were learned the hard way. The doc comments name `showPopupMenu` to explain what
        // this module replaces, and the banned list below is itself source text containing every
        // banned token -- so a naive scan of the whole file matches its own checker and fails
        // eternally. The mirror of that mistake (prose SATISFYING a required-token check) shipped a
        // gate that could not fail earlier the same day.
        let source = include_str!("announce.rs");
        let product = source
            .split("#[cfg(test)]")
            .next()
            .expect("the product half of the file");
        let code: String = product
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        for banned in ["SHOW_POPUP_MENU", "showPopupMenu", "0x5f_5e80", "0x5f5e80"] {
            assert!(
                !code.contains(banned),
                "{banned}: the modal is the bug this module replaces"
            );
        }
    }

    /// The cap is a DISPLAY bound, not a buffer one -- `assign` grows the string itself. It exists
    /// so a long message cannot scroll for the rest of the session.
    #[test]
    fn the_length_cap_is_sane_for_a_scrolling_line() {
        assert!(
            MAX_CHARS >= 40,
            "must fit 'Rejected m60_42_36_00 (elsewhere)'"
        );
        assert!(MAX_CHARS <= 200, "a message this long would scroll forever");
    }
}
