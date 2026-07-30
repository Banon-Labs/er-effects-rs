//! The portrait frame bridge: the worker publishes the captured, alpha-keyed character
//! head here; display hosts (product Wine composite, product native overlay, standalone
//! DLL compositor) read it. Moved from er-effects-rs constants/anti_debug.rs.
//! Host-buildable on purpose (std + er-telemetry only) so host tests can exercise the
//! publish/composite path.

pub use er_telemetry::counters::LOADING_BG_PORTRAIT_GX_CAPTURE_HITS;
/// The kept-alive portrait `CSGxTexture` captured during ProfileSelect (0 until captured). When set,
/// the forge swaps it into its TpfResCap container's TexResCap so the loading screen shows the real
/// rendered character portrait instead of the placeholder checker.
pub use er_telemetry::counters::LOADING_BG_PORTRAIT_GX_KEPT;
/// The live profile-portrait offscreen render target, read back via D3D12 into CPU RGBA8 once the
/// character head has rendered (`portrait_real_pixels_enabled()` gate). Tuple = (width, height,
/// tightly-packed `width*height*4` RGBA8 pixels). `None` until a successful readback. When `Some`,
/// the now-loading forge builds its TPF from these REAL pixels instead of the magenta/yellow checker.
pub static LOADING_BG_PORTRAIT_RGBA: std::sync::Mutex<Option<(u32, u32, Vec<u8>)>> =
    std::sync::Mutex::new(None);
/// 1 if the read-back portrait has any non-black texel (max(R,G,B) > 24) inside a center 64x64
/// region, else 0 (a black/blank capture). Exposed as `oracle_loading_bg_portrait_gx_nonblack`.
pub use er_telemetry::counters::LOADING_BG_PORTRAIT_NONBLACK;
/// Bumped every time LOADING_BG_PORTRAIT_RGBA is REPLACED with a fresh capture. The present-overlay
/// composite watches this: when it changes, the overlay re-uploads its source texture from the new RGBA,
/// so a LIVE per-frame (throttled) readback of the built renderer's offscreen makes the displayed head
/// UPDATE (portrait refreshes) instead of freezing on the first captured frame.
pub use er_telemetry::counters::LOADING_BG_PORTRAIT_RGBA_VERSION;
/// One-shot log latch for the live-display-feed (built RT content -> overlay).
pub use er_telemetry::counters::PROFILE_LIVE_FEED_LOGGED;
