//! Attach-time installation for title, portrait, and loading-surface hooks.

use super::*;
use er_quit_menu::install_picker_dim_overlay;

pub(crate) fn install_title_visual_startup_hooks() {
    // Passive title-resource observer is deliberately independent of the cover/hide bundle: recent
    // branches have kept the stock logo invisible, so resource-path proof must not depend on any
    // visual/logo-hide state.
    if title_menu_resource_observer_enabled() {
        START_TITLE_MENU_RESOURCE_ACQUIRE_OBSERVER.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-title-resource-observer".to_owned())
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
                .name("er-effects-profile-stats-text".to_owned())
                .spawn(|| {
                    // The row-populate hook drives the per-slot attribute push; the named-child binder
                    // hook still runs the title-cover duties. Both are idempotent.
                    install_profile_row_populate_hook();
                    install_title_scene_obj_proxy_named_child_bind_hook();
                });
        });
    }
    // Title-cover masquerade Part A: install the BeginTitle `05_000_Title` hook as early as
    // splash/foreground patches, before STEP_BeginTitle can build the native title Scaleform. This
    // does NOT touch STEP_Wait or CSMenuMan+0x21; it preserves the native MenuWindowJob and hides
    // only its draw bit from the MenuWindowJob::Run/FadeIn path.
    if title_native_menu_visual_suppression_enabled() {
        START_TITLE_NATIVE_MENU_VISUAL_SUPPRESS.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-title-cover-part-a".to_owned())
                .spawn(install_title_native_menu_visual_suppression_hook);
        });
        START_TITLE_NATIVE_MENU_VISUAL_RENDER_SUPPRESS.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-title-cover-render".to_owned())
                .spawn(install_title_native_menu_visual_render_suppression_hook);
        });
        START_TITLE_LOGO_FORCE_HIDDEN.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-title-logo-force-hidden".to_owned())
                .spawn(install_title_logo_force_hidden_hooks);
        });
        START_TITLE_LOGO_START_LOGIN_HIDE.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-title-logo-start-login-hide".to_owned())
                .spawn(install_title_logo_start_login_hide_hook);
        });
        START_TITLE_PAB_INFORMATION_COVER.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-title-pab-cover".to_owned())
                .spawn(install_title_pab_information_visual_hook);
        });
        START_TITLE_GFX_VALUE_SET_VISIBLE.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-title-gfx-visible".to_owned())
                .spawn(install_title_gfx_value_set_visible_hook);
        });
        START_TITLE_SCENE_OBJ_PROXY_NAMED_CHILD_BIND.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-title-child-bind".to_owned())
                .spawn(install_title_scene_obj_proxy_named_child_bind_hook);
        });
        START_TITLE_SCALEFORM_BIND_OBSERVER.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-title-bind-observer".to_owned())
                .spawn(install_title_scaleform_bind_observer_hook);
        });
        START_TITLE_MENU_RESOURCE_ACQUIRE_OBSERVER.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-title-resource-observer".to_owned())
                .spawn(install_title_menu_resource_acquire_observer_hook);
        });
        // Do not install the independent custom-cover MenuWindowJob pump here. Runtime artifact
        // product-continue-direct-20260628-121039 proved that pumping a separate 01_900_Black job
        // keeps job+0x130 live and stalls the title flow before player/world. Future cover work must
        // use an epilogue-neutral path (mutate an already-scheduled title surface/resource, or prove
        // explicit completion semantics before adding an independent MenuWindowJob).
        START_TITLE_FLOW_CONTEXT_RECORD_REGULATION.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-tfc-record-fix".to_owned())
                .spawn(install_title_flow_context_record_regulation_fix_hook);
        });
    } else if title_resource_memory_gfx_enabled() {
        // Branch-owned `05_001_Title_Logo` replacement: keep TitleBack visible, but hide the later
        // title text layers (`PRESS ANY BUTTON` / Continue-ish title information) so the custom
        // resource is not overdrawn by native text. Do not install the TitleBack/logo hide hooks here.
        START_TITLE_PAB_INFORMATION_COVER.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-title-text-latch".to_owned())
                .spawn(install_title_pab_information_visual_hook);
        });
        START_TITLE_GFX_VALUE_SET_VISIBLE.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-title-text-gfx-visible".to_owned())
                .spawn(install_title_gfx_value_set_visible_hook);
        });
        START_TITLE_SCENE_OBJ_PROXY_NAMED_CHILD_BIND.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-title-text-child-bind".to_owned())
                .spawn(install_title_scene_obj_proxy_named_child_bind_hook);
        });
        START_TITLE_SCALEFORM_BIND_OBSERVER.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-title-text-bind-observer".to_owned())
                .spawn(install_title_scaleform_bind_observer_hook);
        });
    } else if native_profile_capture_enabled() {
        // Native ProfileSelect diagnostic: install only the passive Scaleform bind observer. Do not
        // install title-cover/custom-cover hooks; this mode is specifically meant to prove native
        // ProfileSelect/profile-renderer provenance without the product cover mutation path.
        START_TITLE_SCALEFORM_BIND_OBSERVER.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-native-profile-bind-observer".to_owned())
                .spawn(install_title_scaleform_bind_observer_hook);
        });
    }

    // er-effects-rs-jsm PIVOT: suppress the native loading tips (our overlay renders player-stats text
    // instead). Install at ATTACH -- BEFORE the KnowledgeLoadingScreen ctor's one-shot initial tip (~15s),
    // else the first tip is already set and only later cycles are suppressed. Live portrait overlay path only.
    if portrait_overlay_enabled() {
        START_TIP_SUPPRESSION.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-tip-suppress".to_owned())
                .spawn(install_tip_suppression_hook);
        });
    }
    // er-effects-rs-y22i: ALWAYS-ON Scaleform descriptor-heap null guard (native-Windows crash
    // 0xec95d1). NOT feature-gated -- it is a crash guard, a transparent passthrough when the null
    // never occurs. Installed at attach so it is live before the first loading-screen composite.
    START_SCALEFORM_GUARD.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("er-effects-scaleform-guard".to_owned())
            .spawn(install_scaleform_descriptor_guard);
    });
    // D3D12 PRESENT OVERLAY: the deterministic display path -- draw the captured portrait directly onto the
    // swapchain backbuffer when the now-loading screen is up (the in-pipeline forge/Scaleform routes cannot
    // drive the displayed image). Install only on the portrait path (diagnostic), via the dummy-swapchain
    // vtable technique. Phase 1 is log-only (proves the hook fires) before any backbuffer write.
    // Also install under telemetry-only for CADENCE MEASUREMENT: the present detour records the present-
    // cadence + GX semaphores read-only (the flow-modifying composite is separately gated off when the
    // overlay is not a product feature this run). Lets a flow-faithful vanilla baseline capture the
    // render-bound fingerprint (bd present-cadence-gx-instrumentation-coupled-to-overlay-install-gate;
    // VANILLA-run2-forcedrive-WORKS-...cadence-decouple-insufficient).
    if portrait_overlay_enabled()
        || save_override_telemetry_only()
        || crate::experiments::measure_no_composite()
    {
        START_PRESENT_OVERLAY.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-present-overlay".to_owned())
                .spawn(install_present_overlay_hook);
        });
    }
    // NATIVE-WINDOWS LOADING OVERLAY (bd er-effects-rs-8jz): a SEPARATE topmost window with our OWN D3D12
    // device/swapchain that OWNS the screen during boot + every loading screen. On native Windows we
    // cannot composite on the game's shared device (it crashes the strict driver), so this is the only
    // safe display path there. Wine/vkd3d keeps the in-swapchain composite above. Install is idempotent.
    if is_native_windows() {
        install_native_overlay();
    }
    // OS-PICKER DIM: stand the cover's window up NOW, while nothing is waiting on it. The dialog it
    // covers blocks the menu thread, so the moment it opens is the moment we can no longer afford to
    // be creating a window and a full-screen DIB. Self-gated to sessions that actually run the OS
    // picker (`os_native_save_picker = true`); the in-game browser needs no cover.
    install_picker_dim_overlay();
}
