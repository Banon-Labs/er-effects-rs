//! Attach-time installation for profile, System->Quit, and diagnostic hooks.

use super::*;

pub(crate) fn install_profile_and_system_quit_hooks() {
    // Portrait-renderer teardown spare hook: keep the loaded character's portrait renderer alive past the
    // Continue teardown so we can drive realtime look-at + render it post-Continue (the persistent-model
    // path -- the cycling menu can't show a stable portrait). The hook self-gates on product_autoload and
    // only spares a renderer whose model is built (the blank-renderer misfire is guarded in the hook).
    START_PROFILE_RENDERER_TEARDOWN_SPARE.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("er-quickload-portrait-spare".to_owned())
            .spawn(install_profile_renderer_teardown_spare_hook);
    });

    // Profile-renderer table guard (er-effects-rs-j3r): before the native per-slot thumbnail
    // builder runs, log a degraded 10-slot table, rebuild a fully-empty one via the engine's own
    // table setup (only the TitleTopDialog ctor ever calls it natively, so nothing repopulates it
    // across our in-world ProfileSelect reopens -- the 3rd open crashed on the empty table), and
    // fail-soft skip the builder if a slot would still null-deref at [entry+0x754].
    START_PROFILE_SELECT_TABLE_DIAG.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("er-quickload-profileselect-table-diag".to_owned())
            .spawn(install_profile_select_table_diag_hook);
    });

    // System -> Quit Game buttons: always-on multi-slot layout patch plus cloned rows for native
    // 05_010_ProfileSelect and opening the env-provided save folder. Slot activation from that
    // injected in-world route is separately guarded by the System-Quit load flow.
    START_SYSTEM_QUIT_DUPLICATE_BUTTON_HOOK.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("er-quickload-system-quit-load".to_owned())
            .spawn(install_system_quit_duplicate_button_hook);
    });

    // Title Continue confirm guard (0x140b0e180): while a System->Quit->Load-Profile switch is
    // active, drive one fresh feed-deserialize of the picked slot before the confirm streams, so
    // the clean-title reload loads the picked character instead of re-streaming the stale
    // pre-switch state (bd system-quit-cleantitle-load-is-stale-restream-not-slot-source-2026-07-02).
    // Installed unconditionally (single MinHook per address -- this detour also carries the
    // continue-trace cap logging); pure passthrough outside an active switch.
    START_SYSTEM_QUIT_CONTINUE_CONFIRM_HOOK.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("er-quickload-system-quit-continue-confirm".to_owned())
            .spawn(install_system_quit_continue_confirm_hook);
    });

    // Read-only teardown-requester trace: EzChildStepBase::RequestFinish. Identifies who requests
    // the in-world MoveMapStep child's finish -- the post-switch reload bounce is a stale finish
    // request hitting the freshly-created map session (er-effects-rs-qwj investigation).
    START_SYSTEM_QUIT_CHILD_FINISH_TRACE_HOOK.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("er-quickload-system-quit-child-finish-trace".to_owned())
            .spawn(install_system_quit_child_finish_trace_hook);
    });
}

pub(crate) fn install_boot_diagnostics_and_trace_hooks() {
    // MenuWindow latch: install the SceneObjProxy ctor hook (0x14074a700) as early as the
    // splash-skip / online-disable patches, from a thread, so it lands before the title state
    // machine builds the title dialog during boot. On each valid call it latches rdx (the engine-
    // verified host MenuWindow*) for the live-dialog Load-Game path; pure latch + passthrough.
    if product_autoload_enabled() {
        START_MENU_WINDOW_LATCH.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-quickload-menu-window-latch".to_owned())
                .spawn(install_menu_window_latch_hook);
        });
    }

    // Native/asset-backed policy-window oracle: hook the TosTitle constructor early in product
    // autoload runs. Any hit means the Privacy/ToS surface was constructed and the runtime proof is
    // invalid; this is detection only, never auto-accept.
    if product_autoload_enabled() {
        START_POLICY_TOS_TITLE_HOOK.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-quickload-policy-oracle".to_owned())
                .spawn(install_policy_tos_title_hook);
        });
        START_SERVER_STATUS_HOOK.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-quickload-server-status-oracle".to_owned())
                .spawn(install_server_status_hook);
        });
    }

    if safe_input_path().exists() {
        START_SAFE_INPUT_HOOKS.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-quickload-safe-input-hooks".to_owned())
                .spawn(install_safe_input_hooks);
        });
    }
    // Observe-only user32 window-reconfiguration timeline (bd er-effects-rs-rzow): installed at
    // attach so CreateWindowExW is covered before the game builds its startup window. Pure
    // passthrough logging/counting; the RAM semaphore for the mid-boot fullscreen transition
    // whose XWayland servicing blacks the presented surface for a few frames.
    START_WINDOW_RECONFIG_OBSERVER.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("er-quickload-winreconfig-observer".to_owned())
            .spawn(install_window_reconfig_observer_hooks);
    });
    if trace_continue_enabled() {
        write_bootstrap_event(
            BOOTSTRAP_EVENT_CONTINUE_TRACE_REQUESTED,
            BOOTSTRAP_DETAIL_START,
        );
        START_CONTINUE_TRACE.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-quickload-continue-trace".to_owned())
                .spawn(install_continue_trace_hooks);
        });
    }
}
