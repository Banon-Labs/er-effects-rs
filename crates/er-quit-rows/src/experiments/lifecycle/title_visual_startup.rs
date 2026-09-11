//! Attach-time installation for title, portrait, and loading-surface hooks.

use super::*;
use er_quit_menu_core::install_picker_dim_overlay;

pub(crate) fn install_title_visual_startup_hooks() {
    // Passive title-resource observer is deliberately independent of the cover/hide bundle: recent
    // branches have kept the stock logo invisible, so resource-path proof must not depend on any
    // visual/logo-hide state.
    if title_menu_resource_observer_enabled() {
        START_TITLE_MENU_RESOURCE_ACQUIRE_OBSERVER.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-quickload-title-resource-observer".to_owned())
                .spawn(install_title_menu_resource_acquire_observer_hook);
        });
    }

    // Stats-panel native text: arm the 05_010 GFX runtime edit (face box removed + `ErStats` field
    // added; served in-place by the Scaleform file-open observer) and install the row-populate hook
    // + the named-child binder hook (idempotent) so the character's attribute line renders in the
    // game's own MenuFont_01 in its own row field. Independent of the title-cover conditions below
    // -- it must run on every stats-panel product path, so it is gated on `stats_panel_enabled()`
    // directly (product lever; no per-feature env gate).
    if stats_panel_enabled() {
        START_PROFILE_STATS_TEXT.call_once(|| {
            PROFILE_05_010_RUNTIME_EDIT_ARMED.store(1, Ordering::SeqCst);
            // Install the shared PlayerGameData name getter synchronously. The title-load current row
            // can be built before a spawned helper thread gets scheduled; when that happens the first
            // native `PlayerName` write has already cached the shortened `pgd+0x8e8` display string.
            // This hook must be live before the first 05_010/System summary populate call.
            install_profile_row_populate_hook();
            let _ = std::thread::Builder::new()
                .name("er-quickload-profile-stats-text".to_owned())
                .spawn(|| {
                    // The row-populate hook drives the per-slot attribute push and stays: it is
                    // what fills the Load Character submenu's rows. The named-child binder that
                    // used to follow it is deleted -- its only duty here was the title cover,
                    // hiding `PressStart`/`StaticSystemText` at bind time.
                    install_profile_row_populate_hook();
                });
        });
    }
    // Deleted here: the title-cover "masquerade", nine hook installers whose whole job was to stop
    // native title visuals being drawn. None was a GFx edit, which is why trimming the movie swaps
    // left them running and the logo still gone. Measured from a live log before they went, they
    // forced `TitleBackViewParts` and `05_001_Title_Logo` hidden three separate ways
    // (`SetVisible(false)`, again at construction, and again after the native start-login call),
    // hid `PressStart`/`StaticSystemText` at SceneObjProxy bind, blanked the `05_000_Title` FadeIn
    // flash, and covered `05_020_TitleInformation`. The title screen is not Save Game, Load
    // Character or Load Character from File, so this crate has no business repainting it.
    //
    // Two installers shared that gate by accident and are kept, deliberately outside any
    // title-visual condition. `install_title_flow_context_record_regulation_fix_hook` is a
    // TitleFlowContext record fix, not a visual hide. The resource-acquire observer matters far
    // more: it installs `title_scaleform_file_open_observer_hook`, the Scaleform file-open detour
    // through which every movie edit this crate serves is delivered -- the `02_040` six-cell Quit
    // grid, the `05_010_ProfileSelect` stats panel, the `02_990` path-editor field. Losing it with
    // the cover took the Load Character and Load Character from File rows off the tab entirely (the
    // grid stayed vanilla, so there were no cells for them) and reverted the Save Game menu's
    // layout, measured live: `02_040 quit6` served 0 times in that run against 1 before it. Serving
    // a movie is not hiding a visual, so do not fold these two back into a visual gate.
    START_TITLE_MENU_RESOURCE_ACQUIRE_OBSERVER.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("er-quickload-title-resource-observer".to_owned())
            .spawn(install_title_menu_resource_acquire_observer_hook);
    });
    START_TITLE_FLOW_CONTEXT_RECORD_REGULATION.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("er-quickload-tfc-record-fix".to_owned())
            .spawn(install_title_flow_context_record_regulation_fix_hook);
    });
    // er-effects-rs-jsm PIVOT: suppress the native loading tips (our overlay renders player-stats text
    // instead). Install at attach -- Before the KnowledgeLoadingScreen ctor's one-shot initial tip (~15s),
    // else the first tip is already set and only later cycles are suppressed. Live portrait overlay path only.
    if portrait_overlay_enabled() {
        START_TIP_SUPPRESSION.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-quickload-tip-suppress".to_owned())
                .spawn(install_tip_suppression_hook);
        });
    }
    // er-effects-rs-y22i: Always-on Scaleform descriptor-heap null guard (native-Windows crash
    // 0xec95d1). Not feature-gated -- it is a crash guard, a transparent passthrough when the null
    // never occurs. Installed at attach so it is live before the first loading-screen composite.
    START_SCALEFORM_GUARD.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("er-quickload-scaleform-guard".to_owned())
            .spawn(install_scaleform_descriptor_guard);
    });
    // D3D12 present OVERLAY: the deterministic display path -- draw the captured portrait directly onto the
    // swapchain backbuffer when the now-loading screen is up (the in-pipeline forge/Scaleform routes cannot
    // drive the displayed image). Install only on the portrait path (diagnostic), via the dummy-swapchain
    // vtable technique. Phase 1 is log-only (proves the hook fires) before any backbuffer write.
    // Also install under telemetry-only for cadence MEASUREMENT: the present detour records the present-
    // cadence + GX semaphores read-only (the flow-modifying composite is separately gated off when the
    // overlay is not a product feature this run). Lets a flow-faithful vanilla baseline capture the
    // render-bound fingerprint (bd present-cadence-gx-instrumentation-coupled-to-overlay-install-gate;
    // Vanilla-run2-forcedrive-works-...cadence-decouple-insufficient).
    if portrait_overlay_enabled()
        || save_override_telemetry_only()
        || crate::experiments::measure_no_composite()
    {
        START_PRESENT_OVERLAY.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-quickload-present-overlay".to_owned())
                .spawn(install_present_overlay_hook);
        });
    }
    // Native-Windows loading overlay (bd er-effects-rs-8jz): a separate topmost window with our own D3D12
    // device/swapchain that owns the screen during boot + every loading screen. On native Windows we
    // cannot composite on the game's shared device (it crashes the strict driver), so this is the only
    // safe display path there. Wine/vkd3d keeps the in-swapchain composite above. Install is idempotent.
    if is_native_windows() {
        install_native_overlay();
    }
    // OS-PICKER DIM: stand the cover's window up now, while nothing is waiting on it. The dialog it
    // covers blocks the menu thread, so the moment it opens is the moment we can no longer afford to
    // be creating a window and a full-screen DIB. Self-gated to sessions that actually run the OS
    // picker (`os_native_save_picker = true`); the in-game browser needs no cover.
    install_picker_dim_overlay();
}
