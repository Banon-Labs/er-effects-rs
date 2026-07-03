//! Shared corpus location for er-gfx integration tests.
//!
//! Game-derived `.gfx` bytes are never versioned in this repo; tests that need
//! real movies read them from the local extraction corpus and SKIP when it is
//! absent. The root is overridable via `ER_GFX_CORPUS_ROOT` so a moved or
//! re-extracted corpus (the default path embeds an extraction timestamp) needs
//! no source edit.

use std::path::PathBuf;

/// Default local extraction root (nuxe menu dump). Overridden by
/// `ER_GFX_CORPUS_ROOT`.
const DEFAULT_CORPUS_ROOT: &str = "/home/banon/er-extract/nuxe-menu-20260619-170932/menu";

/// Resolve the corpus root: `$ER_GFX_CORPUS_ROOT` if set and non-empty, else
/// the default local extraction path.
pub fn corpus_root() -> PathBuf {
    match std::env::var("ER_GFX_CORPUS_ROOT") {
        Ok(v) if !v.trim().is_empty() => PathBuf::from(v),
        _ => PathBuf::from(DEFAULT_CORPUS_ROOT),
    }
}
