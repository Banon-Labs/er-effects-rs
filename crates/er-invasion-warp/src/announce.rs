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
//! `FeSystemAnnounceView::Update` drains the view-model's queue only when its own embedded message
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
//! # Where the text comes from, and why not the embedded `DLString`
//!
//! The display step's `Load` case picks the string like this (`0x1408c48c0`):
//!
//! ```text
//!   text = msg->field8_0x8;                       // +0x08, a raw wchar_t*
//!   if (text == NULL) {                           // only then, the embedded DLString
//!       text = &msg->text.string;                 // SSO: inline buffer...
//!       if (msg->text.capacity > 7) text = *text; // ...or the heap pointer
//!   }
//!   FUN_14074a000(&view->textWidget, text);
//! ```
//!
//! [`MSG_TEXT_POINTER_OFFSET`] is checked first and used verbatim when non-null, so pointing it at
//! a buffer we own is the engine's own primary path — no allocator, no string type, no growth.
//!
//! The obvious-looking alternative, calling the game's `DLString::assign` on `msg->text`, is what
//! shipped first and it produced a banner with no text on it (live, 2026-08-12). The cause is
//! visible in a read of the live message: the view's embedded message is never populated by the
//! game unless the game itself announces something, so its `DLString` is all zeroes — including
//! `allocator`. `assign` memcpys in place only while `capacity >= length`; for anything longer it
//! delegates to a grow that needs that null allocator. Nothing crashed and nothing was copied, so
//! `capacity` stayed `0`, the null-check above fell through to the SSO branch, and the inline
//! buffer it read was the zeroed one — an empty string, drawn as an empty banner, while our own
//! `SHOWN` counter said the notice had been placed. A counter that proves the write happened is
//! not evidence the pixels changed.
//!
//! # Buffer lifetime
//!
//! The pointer is raw and the game never takes ownership of it: `AnnounceMessage`'s copy
//! (`0x1408c4710`) propagates `field8_0x8` by value, and the `Dequeue` state only clears
//! `is_active`. Nothing frees it, so the buffer must outlive the scroll and fade — and it is
//! therefore leaked, the same choice and the same reason as the synthetic param rows. The leak is
//! bounded in practice by the notice policy, which announces only a change of destination: a long
//! session is tens of messages of under 200 bytes each. Leaking is also what makes this race-free
//! without a lock — a buffer that is never freed cannot dangle under a view still reading it.

#[cfg(windows)]
use std::sync::atomic::{AtomicUsize, Ordering};

/// `CS::FeSystemAnnounceView::Update`. Hooked to learn the live view, which is otherwise reachable
/// only by walking `CSMenuMan`'s window list.
///
/// # 1.17: `0x8c5960`, and the prologue does not need regenerating
///
/// It is easy to assume this needs a fresh `UPDATE_PROLOGUE` generated from the 1.17 image, since
/// the existing one is assembled and pin-checked against `eldenring-deobf.bin` (1.16.2). It does
/// not. The function's opening is byte-identical in both builds --
/// `40 53 48 83 ec 30 0f 29 74 24 20` (`push rbx; sub rsp,0x30; movaps [rsp+0x20],xmm6`) -- and
/// the pin the generator asserts is the first eight of those. What is missing is the address, and
/// only the address.
///
/// The 1.17 counterpart is `0x8c5960` (`+0x11a0`), derived four ways and each one independent:
///
/// * the masked signature `40 53 48 83 ec 30 0f 29 74 24 20 48 8b d9 0f 28 f1 e8 ?? ?? ?? ?? 48
///   8b 05 ?? ?? ?? ?? 48 85 c0` occurs EXACTLY ONCE in each image -- at `0x8c47c0` in 1.16.2 and
///   at `0x8c5960` in 1.17;
/// * the `.pdata` run around it has the same five consecutive record sizes in both images
///   (`0x6a, 0x34, 0x6, 0x5b, 0x3e`), and the whole-image map already pairs the four neighbours:
///   `0x8c4710 -> 0x8c58b0`, `0x8c4780 -> 0x8c5920`, `0x8c47c6 -> 0x8c5966`,
///   `0x8c4821 -> 0x8c59c1`. This one record sits between two mapped ones and ends exactly where
///   the next begins, in both images, so its own pair is fixed arithmetically;
/// * the call at `+0x11` targets `0x7459d0` in 1.16.2 and `0x746820` in 1.17, which the map
///   already carries as a pair;
/// * the `CSMenuMan` global it reads moves `0x3d6b7b0 -> 0x3d6f820`, the documented `+0x4070`
///   `.data` shift.
///
/// The mapper missed it because the record is six bytes -- an MSVC chained-unwind prologue record,
/// which has no body to build a masked signature from. That is a property of this function's
/// unwind encoding, not a gap in the evidence.
///
/// So the one thing still needed is a row `0x8c47c0 -> 0x8c5960` in the 1.16.2 -> 1.17 map, which
/// `docs/recon/` owns. Until it lands, `verified_fn` refuses AUDIBLY (it used to refuse in
/// silence) and the announcement banner is simply absent on 1.17.
pub const UPDATE_RVA: usize = 0x8c_47c0;

// `UPDATE_PROLOGUE`: the opening bytes, so a game update that moves this fails closed instead of
// jumping mid-instruction. Assembled from named `iced-x86` instructions by this crate's
// `build.rs`, which also checks them against `eldenring-deobf.bin` when a copy is present.
include!(concat!(env!("OUT_DIR"), "/generated_announce_prologues.rs"));

/// Offsets into `FeSystemAnnounceView`, from the 1.16.2 dump's own struct definition.
pub const MSG_OFFSET: usize = 0x0b10;
/// `AnnounceMessage +0x08` — a raw `wchar_t*` the display step prefers over the embedded string.
///
/// This is the text field for our purposes. The dump types it as a bare `ulonglong`, but the
/// `Load` case dereferences it as the string and only falls back to `+0x10` when it is null.
pub const MSG_TEXT_POINTER_OFFSET: usize = 0x08;
/// `AnnounceMessage::text`, a `DLString<wchar_t>` inside that message.
///
/// Retained as documentation of the layout and as the offset the FALLBACK path reads. Nothing here
/// writes it: on the view's embedded message it is a zeroed string with a null allocator, which is
/// exactly why writing it produced an empty banner.
pub const MSG_TEXT_OFFSET: usize = 0x10;
/// `announcePlayState`, written as a byte by the game (`movb $0x1`), not as the enum's full width.
pub const PLAY_STATE_OFFSET: usize = 0x0b50;

/// `FeSystemAnnounceView +0xb64` — how far the loaded text OVERFLOWS its field, in pixels.
///
/// # What this actually is, corrected after reading it live
///
/// The `Load` case stores `FUN_14074a140`'s result here and the `Scrolling` case advances a scroll
/// offset toward it, which is what makes it a real measurement rather than a status code. It was
/// first taken for the text's width. It is not: `FUN_140d82660` returns
/// `textWidth - (fieldRight - fieldLeft)`, i.e. the overflow, and a live read proved it —
/// "Rejected Castle Front (elsewhere)" measured **-1418** on a 1728 px field, so the text itself is
/// 310 px wide and the value is negative whenever the line comfortably fits.
///
/// That correction matters, because the first version of this oracle tested `!= 0` and would not
/// have caught the bug it was written for: an empty string does not measure zero, it measures
/// `-fieldWidth`. Zero is returned only when the GFx object is not a live text field at all. So the
/// text's own width has to be reconstructed by adding the field width back, and that is what must
/// be positive.
pub const TEXT_OVERFLOW_OFFSET: usize = 0x0b64;

/// The notice field's width, the baseline added back to recover the text's own width.
pub const NOTICE_FIELD_WIDTH_PX: i32 = er_gfx::announce_notice::NOTICE_FIELD_WIDTH_PX;
/// `+0xb5c` — whether the measured text is long enough to need scrolling. Diagnostic company for
/// the width: a wide string that does not scroll is a different bug from one that measures zero.
pub const TEXT_NEEDS_SCROLL_OFFSET: usize = 0x0b5c;

/// Frames to wait before measuring. The `Load` case runs on the next `Update` after the play state
/// is armed, so a same-frame read would measure the previous notice and call a broken one healthy.
/// Three is slack for that one frame, and the value persists until the next `Load`, so reading late
/// costs nothing while reading early would lie.
pub const MEASURE_DELAY_FRAMES: usize = 3;
/// The value Update writes when it has just loaded a message: `SystemAnnounceViewModelState::Load`.
pub const PLAY_STATE_LOAD: u8 = 1;

/// The longest message we will send. Not a buffer limit — `assign` grows — but a display one: the
/// surface scrolls, and an essay would scroll for the rest of the session.
pub const MAX_CHARS: usize = 96;

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
/// Frames left before the pending notice is measured; 0 means nothing is waiting.
#[cfg(windows)]
static MEASURE_COUNTDOWN: AtomicUsize = AtomicUsize::new(0);
/// Notices whose text measured a width of zero -- i.e. drew an empty banner.
#[cfg(windows)]
static MEASURED_EMPTY: AtomicUsize = AtomicUsize::new(0);
/// Notices whose text measured a non-zero width.
#[cfg(windows)]
static MEASURED_DRAWN: AtomicUsize = AtomicUsize::new(0);

/// How the last notices actually measured: `(drawn, empty)`.
///
/// `empty > 0` is the blank-banner bug, reported from the game's own measurement rather than from
/// a user noticing. Both zero with `SHOWN > 0` means the measurement never ran.
#[cfg(windows)]
#[must_use]
pub fn measurement_tally() -> (usize, usize) {
    (
        MEASURED_DRAWN.load(Ordering::SeqCst),
        MEASURED_EMPTY.load(Ordering::SeqCst),
    )
}

#[cfg(not(windows))]
#[must_use]
pub fn measurement_tally() -> (usize, usize) {
    (0, 0)
}

/// Read back what the game measured for the notice we placed, a few frames after placing it.
///
/// Called every frame from the filter tick. Does nothing until a notice is pending, so the cost
/// while idle is one relaxed load.
///
/// # Safety
///
/// Game thread. Reads two scalars out of the live view; both are fault-closed.
#[cfg(windows)]
pub fn poll_measurement() {
    let remaining = MEASURE_COUNTDOWN.load(Ordering::SeqCst);
    if remaining == 0 {
        return;
    }
    if MEASURE_COUNTDOWN
        .compare_exchange(remaining, remaining - 1, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
        || remaining > 1
    {
        return;
    }
    let view = LIVE_VIEW.load(Ordering::SeqCst);
    if view == 0 {
        return;
    }
    let overflow = unsafe { er_game_base::mem::safe_read_i32(view + TEXT_OVERFLOW_OFFSET) };
    let scrolls = unsafe { er_game_base::mem::safe_read_u8(view + TEXT_NEEDS_SCROLL_OFFSET) };
    // The stored value is `textWidth - fieldWidth`, so the text's own width is that plus the field.
    // An empty string lands at exactly `-fieldWidth` and therefore reconstructs to 0 -- which is
    // the whole point of doing the arithmetic rather than testing the raw value against zero.
    let text_width = overflow.map(|overflow| overflow + NOTICE_FIELD_WIDTH_PX);
    match text_width {
        Some(width) if width > 0 => {
            let drawn = MEASURED_DRAWN.fetch_add(1, Ordering::SeqCst) + 1;
            if drawn == 1 {
                crate::standalone_log(format_args!(
                    "announce: the game measured the notice text at {width}px \
                     (overflow={overflow:?} against a {NOTICE_FIELD_WIDTH_PX}px field, \
                     needs_scroll={scrolls:?}) -- it read glyphs out of our own buffer, which is \
                     the proof a SHOWN counter could never give"
                ));
            }
        }
        other => {
            MEASURED_EMPTY.fetch_add(1, Ordering::SeqCst);
            crate::standalone_log(format_args!(
                "announce: BLANK BANNER -- the notice was placed but the game measured its text at \
                 {other:?}px (overflow={overflow:?} against a {NOTICE_FIELD_WIDTH_PX}px field, \
                 needs_scroll={scrolls:?}). The surface is drawing an empty line. The text pointer \
                 at AnnounceMessage+{MSG_TEXT_POINTER_OFFSET:#x} is not reaching the text widget; \
                 do NOT treat the SHOWN tally as success"
            ));
        }
    }
}

#[cfg(not(windows))]
pub fn poll_measurement() {}

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

/// Resolve a game function for the running build and confirm its opening bytes before handing
/// back a pointer.
///
/// # The order matters, and it used to be wrong
///
/// This computed `base + rva` and byte-checked there. On 1.17 that address is not the function --
/// `0x1408c47c0` holds `and $0x38,%al`, the tail of an unrelated instruction -- so the check
/// failed and `install` returned `false` having logged nothing at all. The refusal was correct
/// and completely invisible: the byte check is what kept it from being a corruption, but nothing
/// downstream ever ran, so `er-hook`'s own `HOOK REFUSED` line never got the chance to appear.
/// Worse, checking before resolving meant that even a correct map row could not rescue it -- the
/// stale bytes decide first.
///
/// Resolving first and checking at the resolved address makes the two claims independent: the
/// resolver says where the function is on this build, and the prologue says the code there really
/// is that function. The byte check is not redundant with the resolver -- it is the belt to its
/// braces, and the pattern `apply_splash_skip` established (a signature check turns a stale
/// address into a refusal rather than a corruption).
///
/// `resolve_detour_address`, not the call resolver: the address is about to carry a MinHook
/// detour. `er_hook::register_union_hook` resolves again internally, which is harmless -- the
/// resolver's `already_translated` shortcut hands a 1.17 destination straight back.
/// How many times a refusal here may be logged. `install` runs every tick and clears its own
/// latch on failure, so an unbounded line would be one per frame forever -- the 339,764-refusal
/// failure `scripts/check-no-rva-zero.py` exists to stop. Twice is enough to be found.
#[cfg(windows)]
const REFUSAL_LOG_LIMIT: usize = 2;
#[cfg(windows)]
static REFUSAL_LOGS: AtomicUsize = AtomicUsize::new(0);

#[cfg(windows)]
fn verified_fn(rva: usize, prologue: &[u8], what: &str) -> Option<usize> {
    let say = || REFUSAL_LOGS.fetch_add(1, Ordering::SeqCst) < REFUSAL_LOG_LIMIT;
    let base = er_game_base::mem::game_module_base().ok()?;
    let Some(address) = er_game_base::game_build::resolve_detour_address(base + rva, what) else {
        if say() {
            crate::standalone_log(format_args!(
                "announce: {what} (1.16.2 rva {rva:#x}) has no detour-safe address for {} -- the \
                 view-capture hook is NOT installed on this build",
                er_game_base::game_build::describe_build()
            ));
        }
        return None;
    };
    for (index, expected) in prologue.iter().enumerate() {
        if unsafe { er_game_base::mem::safe_read_u8(address + index) }? != *expected {
            if say() {
                crate::standalone_log(format_args!(
                    "announce: {what} resolved to {address:#x} but byte +{index:#x} is not \
                     {expected:#04x} -- refusing to hook code that is not the function we measured"
                ));
            }
            return None;
        }
    }
    // The unresolved address, deliberately. The prologue was checked at `address`, but
    // `register_union_hook` resolves what it is given, and it must be the one resolve that places
    // the detour. Handing it `address` would resolve a second time -- normally a no-op, but on an
    // address that is both a 1.17 destination and some other row's 1.16.2 source it silently
    // returns a third, unrelated function (measured on three live detours, 2026-08-30). Resolving
    // the same 1.16.2 input twice is fine; resolving the output is the bug.
    // `scripts/check-double-resolved-hook-targets.py` gates the shape.
    Some(base + rva)
}

/// Learn the live view and get out of the way.
///
/// Deliberately does not inject from here. Update runs every frame; doing work inside it would put
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
    let Some(address) = verified_fn(
        UPDATE_RVA,
        UPDATE_PROLOGUE,
        "CS::FeSystemAnnounceView::Update (UPDATE_RVA)",
    ) else {
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
    let mut units: Vec<u16> = text.encode_utf16().take(MAX_CHARS).collect();
    if units.is_empty() {
        REFUSALS.fetch_add(1, Ordering::SeqCst);
        return false;
    }
    // NUL-terminated: the display step is handed a bare pointer with no length beside it, so the
    // terminator is the only thing that ends the string. Without it the widget reads whatever
    // follows in the heap.
    units.push(0);
    // Leaked deliberately -- see the module docs. The game stores this pointer and reads it for the
    // life of the banner while owning none of it, so it has to outlive us; a permanent buffer also
    // means a notice replaced mid-scroll can never leave the view reading freed memory.
    let buffer: &'static [u16] = Box::leak(units.into_boxed_slice());

    let message = view + MSG_OFFSET;
    unsafe {
        // The pointer first, then is_active. Update tests is_active to decide whether to pop a
        // queued message over the top of ours, and the display step reads the pointer only once
        // is_active is set -- so publishing the text before arming it is what keeps a frame
        // boundary from finding an armed message with no string.
        ((message + MSG_TEXT_POINTER_OFFSET) as *mut usize).write(buffer.as_ptr() as usize);
        (message as *mut u8).write(1);
        ((view + PLAY_STATE_OFFSET) as *mut u8).write(PLAY_STATE_LOAD);
    }

    // Arm the read-back. Placing a notice is not the same claim as displaying one, and this is what
    // makes the difference observable without a person looking at the screen.
    MEASURE_COUNTDOWN.store(MEASURE_DELAY_FRAMES, Ordering::SeqCst);

    let count = SHOWN.fetch_add(1, Ordering::SeqCst) + 1;
    if count == 1 {
        crate::standalone_log(format_args!(
            "announce: first notice placed on the game's auto-closing surface (\"{text}\") -- view \
             {view:#x}. No dialog, no OK button; it expires on its own."
        ));
    }
    true
}

/// Host-build stub: there is no announce view to write to, so nothing is ever placed.
///
/// # Safety
/// Nothing to uphold -- the body only returns `false`. `unsafe` only so the signature matches
/// the `cfg(windows)` twin above, which writes into the game's announce view.
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
        assert_eq!(MSG_TEXT_POINTER_OFFSET, 0x08, "the raw wchar_t* text field");
        assert_eq!(MSG_TEXT_OFFSET, 0x10, "DLString inside AnnounceMessage");
        assert_eq!(PLAY_STATE_OFFSET, 0x0b50, "announcePlayState");
        // The two text fields must not overlap, and the pointer must come first -- that order is
        // the whole reason the pointer works: the display step reads `+0x08` and only consults the
        // DLString at `+0x10` when it is null.
        const {
            assert!(
                MSG_TEXT_POINTER_OFFSET + 8 <= MSG_TEXT_OFFSET,
                "the raw pointer must fit before the embedded string"
            )
        };
        // is_active sits at the message's own offset 0, which is why the write target is the
        // message base rather than the message plus something.
        //
        // Exactly adjacent, not merely non-overlapping: AnnounceMessage is 64 bytes, so the play
        // state begins the instant the message ends. Asserting the exact relationship catches a
        // drifted offset that a `>` would wave through -- and a wrong play-state offset writes a 1
        // into whatever field actually lives there.
        assert_eq!(
            PLAY_STATE_OFFSET,
            MSG_OFFSET + 0x40,
            "announcePlayState immediately follows the 64-byte AnnounceMessage"
        );
    }

    /// The measurement offsets, and the order that makes them a state rather than two constants.
    ///
    /// Both are written by the same `Load` case, immediately after the text is handed to the
    /// widget, and both live past `announcePlayState` in the view -- so a drifted `PLAY_STATE`
    /// offset that overlapped either would corrupt the very evidence meant to catch it.
    #[test]
    fn the_measurement_offsets_sit_after_the_play_state_and_do_not_overlap() {
        assert_eq!(TEXT_NEEDS_SCROLL_OFFSET, 0x0b5c);
        assert_eq!(TEXT_OVERFLOW_OFFSET, 0x0b64);
        const {
            assert!(
                PLAY_STATE_OFFSET < TEXT_NEEDS_SCROLL_OFFSET,
                "the play state is written before these are read"
            )
        };
        const {
            assert!(
                TEXT_NEEDS_SCROLL_OFFSET < TEXT_OVERFLOW_OFFSET,
                "the scroll flag must not overlap the width"
            )
        };
        // Measuring on the same frame reads the previous notice's width, which would report a
        // broken banner as healthy -- the exact false-negative this oracle exists to prevent.
        const {
            assert!(
                MEASURE_DELAY_FRAMES >= 2,
                "the Load case runs on a LATER frame than the one that arms it"
            )
        };
    }

    /// A prologue too short to discriminate would let a moved function pass the check and be called
    /// anyway -- the failure mode this guard exists to prevent.
    #[test]
    fn prologues_are_long_enough_to_discriminate() {
        assert!(UPDATE_PROLOGUE.len() >= 8);
    }

    /// The bug this module shipped ONCE: a banner with no text on it.
    ///
    /// `DLString::assign` cannot populate the view's embedded message, because that message is only
    /// ever initialised when the game announces something -- until then its allocator is null and
    /// its capacity is zero, so `assign` copies nothing, the display step's null-check falls
    /// through to the zeroed inline buffer, and the surface draws an empty line while every counter
    /// reports success. Writing the raw pointer needs none of that machinery.
    #[test]
    fn the_text_never_goes_through_the_string_assign_path_again() {
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
        for banned in ["DLSTRING_ASSIGN", "DlStringAssignFn", "assign("] {
            assert!(
                !code.contains(banned),
                "{banned}: assigning the embedded DLString is what produced an empty banner"
            );
        }
        assert!(
            code.contains("MSG_TEXT_POINTER_OFFSET"),
            "the text must be published through the raw pointer field"
        );
    }

    /// The point of the rewrite. Nothing here may reach for the modal path: `showPopupMenu` is a
    /// popup menu, it blocks on an OK button, and shipping it once already cost the user a dialog
    /// per rejection plus a stalled Seamless session.
    #[test]
    fn this_module_never_touches_the_modal_path() {
        // Only the product code, and only its non-comment lines. Both exclusions are load-bearing
        // and both were learned the hard way. The doc comments name `showPopupMenu` to explain what
        // this module replaces, and the banned list below is itself source text containing every
        // banned token -- so a naive scan of the whole file matches its own checker and fails
        // eternally. The mirror of that mistake (prose satisfying a required-token check) shipped a
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

    /// The cap is a display bound, not a buffer one -- `assign` grows the string itself. It exists
    /// so a long message cannot scroll for the rest of the session.
    #[test]
    fn the_length_cap_is_sane_for_a_scrolling_line() {
        const {
            assert!(
                MAX_CHARS >= 40,
                "must fit 'Rejected m60_42_36_00 (elsewhere)'"
            )
        };
        const { assert!(MAX_CHARS <= 200, "a message this long would scroll forever") };
    }
}
