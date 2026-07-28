//! Path rewriting: send save WRITES somewhere harmless while leaving READS alone.
//!
//! The game is never lied to. It opens a file, writes it, truncates it, backs it up and
//! runs its whole save state machine exactly as it would unmodified, and every one of
//! those operations genuinely succeeds. The only thing that changes is which path the
//! bytes land on. That is why this needs no knowledge of *why* a save happened: the five
//! request sites, the inline periodic autosave, the rune-counter save that fires on every
//! rune change, and the boot system-slot save all converge on the same file open.
//!
//! Reads are deliberately untouched, so the game still loads the player's real save and
//! the load menu still shows the playtime from their last genuine save.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::config::runtime_config;

static REDIRECTS: AtomicU64 = AtomicU64::new(0);
static REDIRECT_REFUSALS: AtomicU64 = AtomicU64::new(0);

pub(crate) fn redirect_count() -> u64 {
    REDIRECTS.load(Ordering::SeqCst)
}

pub(crate) fn refusal_count() -> u64 {
    REDIRECT_REFUSALS.load(Ordering::SeqCst)
}

/// Split a Windows path into (directory-with-separator, filename).
///
/// Deliberately handles both separators: the game builds `C:\users\...` but a
/// user-supplied `save_directory` is far more likely to be written with `/`.
fn split_parent(path: &str) -> (&str, &str) {
    match path.rfind(['\\', '/']) {
        Some(index) => path.split_at(index + 1),
        None => ("", path),
    }
}

/// Compute the diverted path for `original`, or `None` if it must not be diverted.
///
/// Returning `None` is the safe direction: the caller then leaves the path alone, and the
/// census still records whatever happens, so a missed redirect shows up as an escaped
/// write rather than as silence.
pub(crate) fn diverted_path(original: &str) -> Option<String> {
    if original.is_empty() {
        return None;
    }
    let config = runtime_config();
    if config.suffix.is_empty() {
        REDIRECT_REFUSALS.fetch_add(1, Ordering::SeqCst);
        return None;
    }
    // Already diverted: the game re-opening its own diverted file (or a stale one from a
    // previous session) must not grow a second suffix.
    if original.ends_with(&config.suffix) {
        return None;
    }

    let (parent, filename) = split_parent(original);
    let diverted = match &config.save_directory {
        Some(dir) => {
            let dir = dir.display().to_string();
            let separator = if dir.ends_with(['\\', '/']) { "" } else { "\\" };
            format!("{dir}{separator}{filename}{}", config.suffix)
        }
        None => format!("{parent}{filename}{}", config.suffix),
    };

    // The property this whole crate exists to guarantee. If the computed destination is
    // somehow the original, refuse rather than overwrite the player's save.
    if diverted.eq_ignore_ascii_case(original) {
        REDIRECT_REFUSALS.fetch_add(1, Ordering::SeqCst);
        return None;
    }
    REDIRECTS.fetch_add(1, Ordering::SeqCst);
    Some(diverted)
}

/// Ensure a configured `save_directory` exists, once, so the first diverted write does
/// not fail on a missing directory.
pub(crate) fn ensure_destination_directory() {
    if let Some(dir) = &runtime_config().save_directory {
        let _ = std::fs::create_dir_all(dir);
    }
}

/// Encode a Rust string as a NUL-terminated wide string for passing back to Win32.
pub(crate) fn to_wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAVE: &str = r"C:\users\steamuser\AppData\Roaming\EldenRing\7656\ER0000.sl2";

    #[test]
    fn default_diverts_beside_the_original_keeping_the_original_extension_visible() {
        let out = diverted_path(SAVE).expect("save paths divert");
        assert_eq!(out, format!("{SAVE}.save-disabled"));
        assert!(
            out.contains(r"\EldenRing\7656\"),
            "stays in the same directory"
        );
    }

    #[test]
    fn the_backup_keeps_its_bak_extension_before_the_suffix() {
        // ER0000.sl2.bak must become ER0000.sl2.bak.save-disabled, not
        // ER0000.save-disabled -- the whole filename is preserved.
        let out = diverted_path(&format!("{SAVE}.bak")).expect("diverts");
        assert_eq!(out, format!("{SAVE}.bak.save-disabled"));
    }

    #[test]
    fn an_already_diverted_path_is_not_diverted_again() {
        assert_eq!(diverted_path(&format!("{SAVE}.save-disabled")), None);
    }

    #[test]
    fn a_diverted_path_never_equals_the_original() {
        // The one outcome that would destroy a save file.
        for path in [SAVE, &format!("{SAVE}.bak"), "ER0000.co2"] {
            if let Some(out) = diverted_path(path) {
                assert_ne!(out.to_ascii_lowercase(), path.to_ascii_lowercase());
            }
        }
    }

    #[test]
    fn splits_both_separator_styles() {
        assert_eq!(split_parent(r"C:\a\b\c.sl2"), (r"C:\a\b\", "c.sl2"));
        assert_eq!(split_parent("/a/b/c.sl2"), ("/a/b/", "c.sl2"));
        assert_eq!(split_parent("c.sl2"), ("", "c.sl2"));
    }
}
