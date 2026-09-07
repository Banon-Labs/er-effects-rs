//! The System>Quit row text layer: the fixed-capacity wide label/help buffers the game's
//! `CS::MenuString` reads by raw pointer, the compile-time widener that fills them, the two live
//! help lines the build-url rows rewrite as their own status readout, and the Wine path spelling
//! helpers the same rows and the save-destination browser share.
//!
//! Moved verbatim out of `experiments/startup_hooks/quit_menu/system_quit_dialog_handlers.rs`:
//! nothing here touches the game -- it is wide-string data plus pure path-spelling transforms --
//! so it carries no seam entry at all.
//!
//! The button names. `SYSTEM_QUIT_LOAD_PROFILE_LABEL_W` reads **Load Character** and
//! `SYSTEM_QUIT_LOAD_SAVE_PROFILES_LABEL_W` reads **Load Character from File** (renamed
//! 2026-07-31; the symbols kept the old words). `scripts/check-retired-button-labels.py` is the
//! gate that keeps the retired words out of the bytes.

use std::sync::atomic::{AtomicU16, Ordering};

pub fn wide_z(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(core::iter::once(0)).collect()
}

pub fn system_quit_path_for_windows(path: &str) -> Vec<u16> {
    let mut win = if path.starts_with('/') {
        format!("Z:{}", path.replace('/', "\\"))
    } else {
        path.replace('/', "\\")
    };
    while win.ends_with('\\') && win.len() > 3 {
        win.pop();
    }
    wide_z(&win)
}

#[allow(dead_code)] // Retained: Wide-picker path decode, beside the live wide/Windows path helpers it belongs with.
pub fn system_quit_path_from_windows_picker(path: &[u16]) -> Option<String> {
    let end = path.iter().position(|c| *c == 0).unwrap_or(path.len());
    if end == 0 {
        return None;
    }
    String::from_utf16(&path[..end]).ok()
}

pub fn system_quit_windows_path_for_log(path: &str) -> String {
    if let Some(rest) = path
        .strip_prefix("Z:\\")
        .or_else(|| path.strip_prefix("z:\\"))
    {
        format!("/{}", rest.replace('\\', "/"))
    } else {
        path.to_owned()
    }
}

/// Fixed capacity (UTF-16 units) of a System>Quit row label / line-help / dialog-prompt buffer.
///
/// `CS::MenuString` stores the raw pointer it is given and reads to the first NUL, so a zero-padded
/// fixed buffer is exactly as valid as an exact-length one -- and an over-long string fails at
/// compile time here instead of losing its tail at runtime. It replaced eight hand-expanded
/// `[b'L' as u16, b'o' as u16, ...]` arrays whose lengths had to be counted by hand.
pub const SYSTEM_QUIT_ROW_TEXT_CAPACITY: usize = 96;

/// Widen an ASCII row string into a NUL-terminated UTF-16 buffer at compile time. Every result must
/// live in a `const`/`static` with process lifetime: `CS::MenuString` keeps the pointer, not a copy.
pub const fn system_quit_row_text(text: &[u8]) -> [u16; SYSTEM_QUIT_ROW_TEXT_CAPACITY] {
    let mut out = [0_u16; SYSTEM_QUIT_ROW_TEXT_CAPACITY];
    let mut idx = 0;
    while idx < text.len() {
        out[idx] = text[idx] as u16;
        idx += 1;
    }
    out
}

// The two cloned row labels, and why these words (2026-07-31).
//
// They used to read "Load Profile" and "Load Save Profiles", which a reviewer could not tell apart:
// "I'm not clear on the difference between Load Profile and Load Save Profiles. It looks like Load
// Save Profiles will load a character profile and then you have access to that character's saves.
// Does it make more sense to call this 'Load Character' or 'Load Character Profile'?"
//
// The guess was inverted -- it is the other row that ends in a character list -- and that inversion
// is the evidence the words were wrong, not just similar. So each row is now named after what it
// takes as INPUT: a character out of the container already loaded, or a file off the disk.
//
// Both fit, measured rather than assumed. The Quit-tab cell renders its label through
// `02_040_optionsetting.gfx` sprite 129 -> `Text_0` -> sprite 96 -> `DefineEditText` char 95, whose
// bounds are -40..7960 twips = 400px at a 24px `MenuFont_01`, single-line with wordwrap off -- an
// overflow would clip, not wrap, so it would silently eat the tail.
// `scripts/gfx_text_width.py --height-px 24 --box-px 400` sums that font's own advance table:
// "Load Character" 144.5px, "Load Character from File" 234.6px, against the native "Return to
// Desktop" at 172.8px which is known to render on this row. Nothing is near the edge, so no
// placement matrix and no row width was touched. The line-help field on the same tab (char 87,
// -40..22760 twips = 1140px, also 24px) has room to spare for the help strings below.
pub const SYSTEM_QUIT_LOAD_PROFILE_LABEL_W: [u16; SYSTEM_QUIT_ROW_TEXT_CAPACITY] =
    system_quit_row_text(b"Load Character");

pub const SYSTEM_QUIT_LOAD_PROFILE_HELP_W: [u16; SYSTEM_QUIT_ROW_TEXT_CAPACITY] =
    system_quit_row_text(b"Load another character from the save you are playing");

pub const SYSTEM_QUIT_LOAD_SAVE_PROFILES_LABEL_W: [u16; SYSTEM_QUIT_ROW_TEXT_CAPACITY] =
    system_quit_row_text(b"Load Character from File");

pub const SYSTEM_QUIT_LOAD_SAVE_PROFILES_HELP_W: [u16; SYSTEM_QUIT_ROW_TEXT_CAPACITY] =
    system_quit_row_text(b"Browse for another save file and load a character from it (ER0000.sl2)");

// Seamless Co-op variant of the row help above: same text but naming `ER0000.co2`, the container
// ERSC actually reads/writes. Selected at row-build time so the row never advertises the save
// flavor the active mode ignores (matches the picker's mode-locked filter; user directive
// 2026-07-06).
pub const SYSTEM_QUIT_LOAD_SAVE_PROFILES_HELP_CO2_W: [u16; SYSTEM_QUIT_ROW_TEXT_CAPACITY] =
    system_quit_row_text(b"Browse for another save file and load a character from it (ER0000.co2)");

// The third cloned row. Named after what it takes, like the two above it: a planner share link.
//
// "Build" rather than "Character" is the whole distinction the label has to carry -- the two rows
// above it swap which character you are playing, and this one changes the character you are already
// playing. It is also the only row on the tab that neither returns to the title nor touches a save
// container, which is why its help says the import happens where you stand.
//
// 203.7px at 24px MenuFont_01 against the cell's 400px non-wrapping field
// (`scripts/gfx_text_width.py --height-px 24 --box-px 400`), so it clips no tail.
pub const SYSTEM_QUIT_LOAD_BUILD_URL_LABEL_W: [u16; SYSTEM_QUIT_ROW_TEXT_CAPACITY] =
    system_quit_row_text(b"Load Build from URL");

// The fourth cloned row, and the only one that reads rather than writes.
//
// "Generate Build Link" is the exact inverse of the row above it: that one takes a planner link and
// rewrites this character, this one takes this character and writes a planner link. Naming it after
// what it produces rather than what it takes is the one place this tab's convention has to bend --
// every other row is named for its input because every other row consumes something the player
// supplies, and this row consumes nothing at all.
//
// "Generate" rather than "Share" or "Copy": the link does not exist until the row is pressed, and
// both other verbs imply it already does. The help line is where the two side effects are stated,
// because a row that silently reaches for the clipboard and silently opens a browser is a row that
// looks broken when either one is blocked.
//
// 189.9px at 24px MenuFont_01 against the cell's 400px non-wrapping field
// (`scripts/gfx_text_width.py --height-px 24 --box-px 400`), the narrowest of the four cloned rows.
pub const SYSTEM_QUIT_GENERATE_BUILD_LINK_LABEL_W: [u16; SYSTEM_QUIT_ROW_TEXT_CAPACITY] =
    system_quit_row_text(b"Generate Build Link");

// The row'S help line is the editor'S indicator, which is why it is WRITABLE.
//
// `CS::MenuString` stores the raw pointer it is handed and reads to the first NUL every time the
// row is drawn, so a buffer this DLL can rewrite becomes a live readout: when the link field
// refuses an accept, the reason is on the row sitting behind it before the field re-opens.
//
// It is `AtomicU16` rather than a `static mut` because the game's render thread reads these units
// while the menu pump writes them. `AtomicU16` has the layout of `u16`, so the pointer handed to
// `MenuString` is still an ordinary wide string; what the atomics buy is that the race is defined
// rather than undefined. A torn read shows one stale character for one frame, which is a cosmetic
// outcome the alternative (UB) does not offer.
pub static SYSTEM_QUIT_LOAD_BUILD_URL_HELP_BUF: [AtomicU16; SYSTEM_QUIT_ROW_TEXT_CAPACITY] =
    [const { AtomicU16::new(0) }; SYSTEM_QUIT_ROW_TEXT_CAPACITY];

/// Overwrite the Load Build from URL row's help line.
///
/// Truncated to the buffer, and always NUL-terminated: the native reader stops at the first NUL and
/// has no length to bound it, so leaving one off would walk into whatever follows. Non-ASCII is
/// dropped rather than encoded -- every string this is called with is ASCII, and a lone surrogate
/// here would be a rendering bug in the menu rather than an error anyone sees.
pub fn set_build_url_row_help(text: &str) {
    let capacity = SYSTEM_QUIT_ROW_TEXT_CAPACITY - 1;
    let mut written = 0;
    for unit in text.chars().filter(char::is_ascii).map(|c| c as u16) {
        if written >= capacity {
            break;
        }
        SYSTEM_QUIT_LOAD_BUILD_URL_HELP_BUF[written].store(unit, Ordering::SeqCst);
        written += 1;
    }
    for unit in SYSTEM_QUIT_LOAD_BUILD_URL_HELP_BUF.iter().skip(written) {
        unit.store(0, Ordering::SeqCst);
    }
}

/// The Generate Build Link row's help line, live for the same reason the row above it has one: the
/// export happens with no field and no dialog in front of it, so this row's own help text is the
/// only surface that can report what happened. It reads "Create a shareable link..." at rest and
/// becomes the outcome -- URL length, clipboard, browser -- once a press completes.
pub static SYSTEM_QUIT_GENERATE_BUILD_LINK_HELP_BUF: [AtomicU16; SYSTEM_QUIT_ROW_TEXT_CAPACITY] =
    [const { AtomicU16::new(0) }; SYSTEM_QUIT_ROW_TEXT_CAPACITY];

/// What the row's help says when nothing has been pressed yet.
pub const GENERATE_BUILD_LINK_ROW_HELP: &str =
    "Create a shareable link to this character and open it in your browser";

/// Overwrite the Generate Build Link row's help line. Same contract as
/// [`set_build_url_row_help`]: truncated to the buffer, always NUL-terminated, ASCII only.
pub fn set_generate_build_link_row_help(text: &str) {
    let capacity = SYSTEM_QUIT_ROW_TEXT_CAPACITY - 1;
    let mut written = 0;
    for unit in text.chars().filter(char::is_ascii).map(|c| c as u16) {
        if written >= capacity {
            break;
        }
        SYSTEM_QUIT_GENERATE_BUILD_LINK_HELP_BUF[written].store(unit, Ordering::SeqCst);
        written += 1;
    }
    for unit in SYSTEM_QUIT_GENERATE_BUILD_LINK_HELP_BUF
        .iter()
        .skip(written)
    {
        unit.store(0, Ordering::SeqCst);
    }
}

/// The Generate Build Link help buffer as the wide string `MenuString` will read.
///
/// # Safety
///
/// `AtomicU16` and `u16` share a layout, and the buffer has process lifetime, so the pointer stays
/// valid for as long as the row does.
pub fn generate_build_link_row_help_wide() -> &'static [u16] {
    // Safety: layout-compatible reinterpretation of a process-lifetime buffer.
    unsafe {
        core::slice::from_raw_parts(
            SYSTEM_QUIT_GENERATE_BUILD_LINK_HELP_BUF
                .as_ptr()
                .cast::<u16>(),
            SYSTEM_QUIT_ROW_TEXT_CAPACITY,
        )
    }
}

/// The help buffer as the wide string `MenuString` will read.
///
/// # Safety
///
/// `AtomicU16` and `u16` share a layout, and the buffer has process lifetime, so the pointer stays
/// valid for as long as the row does.
pub fn build_url_row_help_wide() -> &'static [u16] {
    // Safety: layout-compatible reinterpretation of a process-lifetime buffer.
    unsafe {
        core::slice::from_raw_parts(
            SYSTEM_QUIT_LOAD_BUILD_URL_HELP_BUF.as_ptr().cast::<u16>(),
            SYSTEM_QUIT_ROW_TEXT_CAPACITY,
        )
    }
}

pub const SYSTEM_QUIT_SAVE_GAME_LABEL_W: [u16; SYSTEM_QUIT_ROW_TEXT_CAPACITY] =
    system_quit_row_text(b"Save Game");

// The help says choose, because that is what the row now does: pressing it opens the destination
// list rather than asking a question. Promising "Save and return to playing the game" would have
// described the pre-2026-07-31 flow, where the row's first act was a yes/no box.
pub const SYSTEM_QUIT_SAVE_GAME_HELP_W: [u16; SYSTEM_QUIT_ROW_TEXT_CAPACITY] =
    system_quit_row_text(b"Choose where to save, then return to playing");

// Native dialog id 110000's text. The product row press suppresses the native activation and opens
// the destination list, so this substitution should never be reached; it stays because a build
// where the suppression did not take would otherwise show the vanilla quit prompt on a row labelled
// Save Game, which is worse than a stale-but-harmless sentence.
pub const SYSTEM_QUIT_SAVE_GAME_DIALOG_W: [u16; SYSTEM_QUIT_ROW_TEXT_CAPACITY] =
    system_quit_row_text(b"Save and return to playing the game?");
