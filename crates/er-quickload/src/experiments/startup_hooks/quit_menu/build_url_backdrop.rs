//! Proof that the dim behind the **Load Build from URL** link field reached the running movie.
//!
//! The dim itself is authored offline, in [`er_gfx::build_url_backdrop`]: a second placement of the
//! movie's own black plate, on the root, under the field's sprite. Nothing in this file draws it.
//! What this file does is answer the two questions a build success cannot.
//!
//! # Did the bytes we handed Scaleform carry it
//!
//! [`attest_derived_build_url_backdrop`] re-parses the derived payload at the moment the MemoryFile
//! swap installs it and looks for the placement by name and depth. A derivation that silently lost
//! it -- a future vanilla payload whose root is shaped differently, a tag the writer refused --
//! shows up here as a count and a log line rather than as a field that went up undimmed.
//!
//! # Did the running movie keep it
//!
//! [`probe_live_build_url_backdrop`] resolves the dim's instance name on the live `MenuWindow`
//! root through `assignComponentWithName`, the same native binder that reaches the editable field.
//! This is the part that was unmeasured: no edit in this repo had added a root-level child to
//! `02_990` before, and the native `CS::SoftwareKeyboard` controller is documented to reconstruct
//! its own child after parsing. If it rebuilds the whole root instead, the dim is in the bytes and
//! not on the screen, and `_derived` rising while `_resolved` stays at zero is what says so.
//!
//! # What neither of them proves
//!
//! Pixels. A resolved display object is a display object, not a darker screen. The rectangle it
//! covers is emitted alongside the counters (`oracle_build_url_backdrop_stage_rect`) so a luma
//! probe knows where to sample and which part -- the field's own plate -- to exclude.

use super::*;
use er_telemetry_core::counters::{
    SYSTEM_QUIT_LOAD_BUILD_URL_BACKDROP_DERIVED, SYSTEM_QUIT_LOAD_BUILD_URL_BACKDROP_MISSING,
    SYSTEM_QUIT_LOAD_BUILD_URL_BACKDROP_RESOLVED, SYSTEM_QUIT_LOAD_BUILD_URL_BACKDROP_UNRESOLVED,
};

/// Frames of one open field over which the live resolve runs.
///
/// Each resolve constructs and destroys a `CSScaleformValue` proxy, and the field can be up for
/// minutes, so this is bounded for the same reason the caret pass is. A handful of frames is
/// enough: the question is whether the child exists in the movie at all, and that does not change
/// while the movie runs.
const BACKDROP_PROBE_FRAMES: usize = 4;

/// Log lines this module may emit per open, so a probe that fails every frame does not bury the
/// rest of the trace.
const BACKDROP_LOG_LIMIT: usize = 2;
static BACKDROP_LOGS: AtomicUsize = AtomicUsize::new(0);

/// Forget the per-open log budget. Called wherever the editor's own state is cleared.
pub(crate) fn reset_build_url_backdrop_probe() {
    BACKDROP_LOGS.store(0, Ordering::SeqCst);
}

/// Check the derived movie for its dim and count the answer, then hand the bytes back unchanged.
///
/// Shaped as a pass-through so the derivation call site stays one expression. A missing dim is
/// reported, not fatal: a field with no dim behind it is the condition this feature fixes, and
/// refusing to serve the movie over it would replace a cosmetic defect with an unusable row.
pub(crate) fn attest_derived_build_url_backdrop(derived: Vec<u8>) -> Result<Vec<u8>, String> {
    match er_gfx::build_url_backdrop::backdrop_in_derived_bytes(&derived) {
        Some(rect) => {
            SYSTEM_QUIT_LOAD_BUILD_URL_BACKDROP_DERIVED.fetch_add(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "system-quit-build-url: derived movie carries the dim alpha={}/256 root-local rect=({:.0},{:.0})..({:.0},{:.0})",
                rect.alpha_mult, rect.left, rect.top, rect.right, rect.bottom
            ));
        }
        None => {
            SYSTEM_QUIT_LOAD_BUILD_URL_BACKDROP_MISSING.fetch_add(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "system-quit-build-url: derived movie carries NO dim; the link field will go up over an undimmed Quit dialog"
            ));
        }
    }
    Ok(derived)
}

/// Resolve the dim on the live movie root and count whether it is there.
///
/// # Safety
///
/// 02_990 `MenuWindowJob::Run` context, `menu_window` live -- the same context the caret and the
/// clipboard mirror resolve their proxies in, and the only one in which these are valid.
pub(crate) unsafe fn probe_live_build_url_backdrop(base: usize, menu_window: usize, frame: usize) {
    if frame >= BACKDROP_PROBE_FRAMES
        || menu_window == 0
        || menu_window == TITLE_OWNER_SCAN_START_ADDRESS
    {
        return;
    }
    let root_proxy = menu_window + OPTION_SETTING_ROOT_PROXY_OFFSET;
    let name = er_gfx::build_url_backdrop::BACKDROP_INSTANCE_NAME;
    let resolved = unsafe { resolve_row_child_proxy(base, root_proxy, name) };
    match resolved {
        Some((proxy, _slot)) => {
            SYSTEM_QUIT_LOAD_BUILD_URL_BACKDROP_RESOLVED.fetch_add(1, Ordering::SeqCst);
            unsafe { destroy_resolved_row_child_proxy(base, proxy) };
            if BACKDROP_LOGS.fetch_add(1, Ordering::SeqCst) < BACKDROP_LOG_LIMIT {
                let rect = er_gfx::build_url_backdrop::backdrop_stage_rect_px();
                append_autoload_debug(format_args!(
                    "system-quit-build-url: the dim resolved on the live 02_990 root window=0x{menu_window:x} \
                     stage rect=({:.0},{:.0})..({:.0},{:.0}) alpha={}/256",
                    rect.left, rect.top, rect.right, rect.bottom, rect.alpha_mult
                ));
            }
        }
        None => {
            SYSTEM_QUIT_LOAD_BUILD_URL_BACKDROP_UNRESOLVED.fetch_add(1, Ordering::SeqCst);
            if BACKDROP_LOGS.fetch_add(1, Ordering::SeqCst) < BACKDROP_LOG_LIMIT {
                append_autoload_debug(format_args!(
                    "system-quit-build-url: child {name} did NOT resolve on the live 02_990 root \
                     window=0x{menu_window:x} frame={frame}; the dim is in the bytes but not in the movie"
                ));
            }
        }
    }
}

/// The one-line summary the telemetry writer emits.
///
/// Kept here rather than inlined at the writer so the reading of the four counters lives beside the
/// code that raises them.
pub(crate) fn build_url_backdrop_telemetry() -> String {
    let rect = er_gfx::build_url_backdrop::backdrop_stage_rect_px();
    format!(
        "  \"oracle_build_url_backdrop_derived\": {},\n  \
           \"oracle_build_url_backdrop_missing\": {},\n  \
           \"oracle_build_url_backdrop_resolved\": {},\n  \
           \"oracle_build_url_backdrop_unresolved\": {},\n  \
           \"oracle_build_url_backdrop_alpha_256\": {},\n  \
           \"oracle_build_url_backdrop_stage_rect\": [{:.1}, {:.1}, {:.1}, {:.1}],\n",
        SYSTEM_QUIT_LOAD_BUILD_URL_BACKDROP_DERIVED.load(Ordering::SeqCst),
        SYSTEM_QUIT_LOAD_BUILD_URL_BACKDROP_MISSING.load(Ordering::SeqCst),
        SYSTEM_QUIT_LOAD_BUILD_URL_BACKDROP_RESOLVED.load(Ordering::SeqCst),
        SYSTEM_QUIT_LOAD_BUILD_URL_BACKDROP_UNRESOLVED.load(Ordering::SeqCst),
        rect.alpha_mult,
        rect.left,
        rect.top,
        rect.right,
        rect.bottom,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The probe window has to be short enough that a field left open for minutes costs nothing,
    /// and non-zero or the oracle never fires.
    #[test]
    fn the_probe_window_is_bounded() {
        const {
            assert!(BACKDROP_PROBE_FRAMES > 0 && BACKDROP_PROBE_FRAMES <= 16);
            assert!(BACKDROP_LOG_LIMIT > 0 && BACKDROP_LOG_LIMIT <= 8);
        }
    }

    /// The telemetry line is hand-built JSON, so a missing comma or quote makes the whole file
    /// unparseable for every watcher that reads it -- not just for this oracle.
    #[test]
    fn the_telemetry_line_is_well_formed() {
        let line = build_url_backdrop_telemetry();
        assert!(line.ends_with(",\n"), "{line}");
        assert_eq!(line.matches('"').count() % 2, 0, "{line}");
        for key in [
            "oracle_build_url_backdrop_derived",
            "oracle_build_url_backdrop_missing",
            "oracle_build_url_backdrop_resolved",
            "oracle_build_url_backdrop_unresolved",
            "oracle_build_url_backdrop_alpha_256",
            "oracle_build_url_backdrop_stage_rect",
        ] {
            assert!(line.contains(&format!("\"{key}\":")), "{key} missing");
        }
        // The rect is emitted for a luma probe to sample, so it has to name a real region of the
        // screen rather than an empty or inverted one.
        let rect = er_gfx::build_url_backdrop::backdrop_stage_rect_px();
        assert!(rect.right > rect.left && rect.bottom > rect.top, "{rect:?}");
        assert!(rect.covers_stage(), "{rect:?}");
    }

    /// A derived payload with no dim must be counted and still served: an undimmed field is the
    /// defect this feature fixes, and refusing the movie over it would break the row entirely.
    #[test]
    fn a_payload_without_a_dim_is_reported_and_still_served() {
        let before = SYSTEM_QUIT_LOAD_BUILD_URL_BACKDROP_MISSING.load(Ordering::SeqCst);
        let bytes = vec![0u8; 8];
        let served = attest_derived_build_url_backdrop(bytes.clone()).expect("still served");
        assert_eq!(served, bytes);
        assert_eq!(
            SYSTEM_QUIT_LOAD_BUILD_URL_BACKDROP_MISSING.load(Ordering::SeqCst),
            before + 1
        );
    }
}
