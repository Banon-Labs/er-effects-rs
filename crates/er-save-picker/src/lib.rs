//! Product (A): the DLL-drawn boot save picker, its shared row model, and the OS-native
//! common-file-dialog mechanism.
//!
//! The save-picker crate extraction has moved the host-testable row model, config keys,
//! slot parser, reusable OS common-file-dialog mechanism, and DLL-drawn boot overlay here.
//! Product entrypoints still live under `er-effects-rs/src/experiments/startup_hooks/save_picker/`
//! until the remaining quit-menu seams are extracted.
//!
//! Contents and remaining planned moves:
//! * `model` -- moved from `experiments/save_picker.rs`: `SavePickerModel`,
//!   `PickerIntent`, `PickerRow`, `PickerEntry`, `PickerActivation`, `PickRejection`, the
//!   dense row layout (`entry_row_base` and everything derived from it), drive/page
//!   cycling, `save_picker_accepts` / `save_picker_extension_accepted`, the civil-time
//!   helpers, `truncate_utf16`, and the process-wide `ACTIVE_SAVE_PICKER` slot. Pure
//!   filesystem logic with ~960 lines of its own tests -- host-runnable, which is the
//!   point: the cancel/reopen state machine is only exercisable by launching the whole
//!   game today.
//! * `slots` -- `SaveSlotInfo` + `parse_save_character_slots`, moved from
//!   `startup_hooks/loading_cover/loading_cover_save_slot.rs` because they are picker-owned
//!   offline save-container parsing.
//! * `overlay` -- moved from `experiments/gpu_readback/save_picker_overlay.rs`: the
//!   arm/disarm lifecycle keyed off the missing-save hold, both input paths (the
//!   render-thread `GetAsyncKeyState`/XInput poll and the dedicated `WH_KEYBOARD_LL`
//!   thread), the file stage and the character sub-stage, the CPU compositor
//!   (`overlay_save_picker_onto`), the explicit [`overlay::arm_boot_picker`] entrypoint,
//!   and the deferred pick completion that runs the redirect install on the game-task
//!   thread.
//! * `os_dialog` -- moved from the mechanism half of
//!   `startup_hooks/save_picker/save_picker_os_dialog.rs`: `os_dialog_run`,
//!   `os_pick_validated`, `classify_os_outcome`, `should_reopen`, `os_dialog_filter`,
//!   `OsDialogClaim`, `os_dialog_owner`, `os_pick_path_from_buffer`, `OsPickOutcome`. It
//!   converts strings and calls comdlg32, with product state supplied through host callbacks
//!   and a caller-supplied cover factory. Its two System>Quit entrypoints
//!   (`os_open_save_picker_load`, `os_open_save_dest_picker`) are not here -- they are
//!   quit-menu callers and still live in the product shim for now.
//! * `config` -- the three picker keys and their plumbing, moved out of
//!   `er-effects-rs/src/config.rs`: `preferred_save_picker_dir`,
//!   `autoupdate_preferred_picker_dir` and `os_native_save_picker` (with its
//!   `use_os_file_picker` / `save_picker.os_native` aliases), their parse + validation,
//!   the generated boilerplate doc text, and `remember_preferred_save_picker_dir`. Only
//!   picker code reads them, so they move with the picker; the product's `er-effects.toml`
//!   parser keeps one file and delegates those keys here.
//! * `surface` -- the pure surface/outcome routing types (`PickerOpenRequest`,
//!   `PickerSurface`, `PickerOpenOutcome`) and destination target routing (`DestRoute`).
//!   The host product still opens native menu windows and stages save-flow state.
//! * `boot` -- boot missing-save telemetry state values and the pure OS-dialog abort
//!   decision table; the host product still owns the dialog thread, telemetry flush, and
//!   process exit.
//!
//! # The screen cover is the CALLER's decision, not this crate's
//!
//! Under the 2026-07-30 user decision the dim belongs to product (B) and the boot dialog
//! must NOT be dimmed. The extracted `os_pick_validated` preserves that caller-decides
//! shape through the [`host::PickerCoverFactory`] seam: the System>Quit product shim passes
//! a factory that arms its dim, and the boot flow passes [`os_dialog::no_picker_cover`].
//! Same behavior, expressed across a crate boundary instead of an in-crate enum.
//!
//! # The OS-native surface is a REQUIREMENT, not an option
//!
//! We never force a user onto an in-game picker we built (user principle 2026-07-30).
//! Both places that draw one -- this crate's boot picker and `er-quit-menu`'s in-game
//! browse rows -- must offer the OS-native dialog as a selectable surface. That is why the
//! comdlg32 mechanism and the `os_native_save_picker` config key live HERE, in the crate
//! both products depend on, rather than being duplicated: `er-quit-menu` gets the fallback
//! surface through its one-way dependency on this crate.
//!
//! # Linking this crate is not arming it
//!
//! `er-quit-menu` statically links this crate, so a profile containing ONLY the standalone
//! product-(B) DLL still offers the OS-native surface. It must NOT thereby acquire (A)'s
//! boot missing-save behavior. Two mechanisms, and only the second is a guarantee:
//!
//! 1. the `boot-flow` cargo feature, which `er-quit-menu` turns off. This isolates the
//!    standalone-(B) build, but cargo UNIFIES features across a build graph, so in the
//!    product DLL -- which wants `boot-flow` -- `er-quit-menu` gets it too. A feature alone
//!    therefore cannot carry the requirement.
//! 2. an explicit arm entry point. Nothing in the boot flow installs a hook, spawns a
//!    thread or arms a model until a host calls it; `er-quit-menu` never does. This holds
//!    in every build, feature unification included, and is the real guarantee.
//!
//! # Product state crosses the seam as injected function pointers
//!
//! See [`host::install_host`]. This crate must not depend on the root crate.

pub mod boot;
pub mod config;
pub mod drive_strip_router;
pub mod host;
pub mod model;
#[cfg(feature = "os-dialog")]
pub mod os_dialog;
#[cfg(feature = "boot-flow")]
pub mod overlay;
pub mod path_editor_lifecycle;
pub mod slots;
pub mod surface;

pub use boot::*;
pub use config::*;
pub use drive_strip_router::*;
pub use host::*;
pub use model::*;
#[cfg(feature = "os-dialog")]
pub use os_dialog::*;
pub use path_editor_lifecycle::*;
pub use slots::*;
pub use surface::*;

#[cfg(test)]
mod picker_activation_source_contract_tests {
    const DIALOG_HANDLERS: &str = include_str!(concat!(
        "../../er-effects-rs/src/experiments/startup_hooks/quit_menu/",
        "system_quit_dialog_handlers.rs"
    ));
    const SAVE_PICKER_MENU: &str = include_str!(concat!(
        "../../er-effects-rs/src/experiments/startup_hooks/quit_menu/",
        "save_picker_menu.rs"
    ));
    const OWNERSHIP_REPRO: &str = include_str!(concat!(
        "../../er-effects-rs/src/experiments/startup_hooks/quit_menu/",
        "system_quit_ownership_repro.rs"
    ));
    const PROFILE_CONSTANTS: &str = include_str!(concat!(
        "../../er-effects-rs/src/constants/",
        "profile_render.rs"
    ));
    const HOOK_INSTALLS: &str = include_str!(concat!(
        "../../er-effects-rs/src/experiments/startup_hooks/quit_menu/",
        "system_quit_hooks.rs"
    ));

    fn between<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
        source
            .split_once(start)
            .unwrap_or_else(|| panic!("missing start boundary {start}"))
            .1
            .split_once(end)
            .unwrap_or_else(|| panic!("missing end boundary {end}"))
            .0
    }

    #[test]
    fn foreign_property_button_hook_has_no_picker_provenance_symbols() {
        let hook = between(
            DIALOG_HANDLERS,
            "pub(crate) unsafe extern \"system\" fn property_new_button_controller_activate_hook(",
            "\nconst _: PropertyNewButtonControllerActivateFn",
        );
        for forbidden in [
            "save_picker_compose_activation_provenance_with",
            "save_picker_arm_drive_strip_activation_provenance",
            "save_picker_clear_pending_drive_strip_target",
            "forward_drive_strip_native_activation_once",
            "activation_provenance",
        ] {
            assert!(
                !hook.contains(forbidden),
                "foreign hook still owns {forbidden}"
            );
        }
        assert!(hook.contains("system_quit_forward_button_controller_activation"));
    }

    #[test]
    fn scoped_update_hook_owns_context_and_profile_callback_has_exact_abi() {
        let update = between(
            SAVE_PICKER_MENU,
            "pub(crate) unsafe extern \"system\" fn profile_load_menu_window_update_hook(",
            "\nconst _: ProfileLoadMenuWindowUpdateFn",
        );
        for required in [
            "save_picker_dialog_identity(dialog)",
            "PickerNativeLifecycleAdapter",
            "run_update_with",
            "save_picker_compose_activation_provenance_with",
        ] {
            assert!(
                update.contains(required),
                "scoped update omitted {required}"
            );
        }
        assert!(SAVE_PICKER_MENU.contains("SAVE_PICKER_ACTIVATION_RING_CAPACITY: usize = 64"));
        assert!(SAVE_PICKER_MENU.contains("native-matcher-no-callback"));
        assert!(SAVE_PICKER_MENU.contains("source: \"profile-load-late\""));
        let lifecycle_update = between(
            SAVE_PICKER_MENU,
            "unsafe fn run_update_with(",
            "\n    pub(crate) unsafe fn dispatch_profile_load(",
        );
        let enter = lifecycle_update
            .find("PickerActivationScope::enter")
            .expect("lifecycle enters context");
        let original = enter
            + lifecycle_update[enter..]
                .find("original(identity.dialog, update_scalar, row_input_gate)")
                .expect("owned lifecycle calls typed update original");
        let finish = lifecycle_update
            .find("scope.finish()")
            .expect("lifecycle finalizes context");
        assert!(
            enter < original && original < finish,
            "context must span the typed native update original"
        );
        assert_eq!(
            SAVE_PICKER_MENU
                .matches("type ProfileLoadMenuWindowUpdateFn =")
                .count(),
            1,
            "the verified update ABI must have one shared alias",
        );
        assert!(!SAVE_PICKER_MENU.contains("static SAVE_PICKER_DRIVE_STRIP_PENDING_CELL"));

        let profile = between(
            OWNERSHIP_REPRO,
            "pub(crate) unsafe extern \"system\" fn system_quit_profile_load_activate_hook(dialog: usize) {",
            "\nconst _: unsafe extern \"system\" fn(usize)",
        );
        for required in [
            "save_picker_dialog_identity(dialog)",
            "identity.picker_mode_active",
            "identity.is_exact_active_picker()",
            "PickerNativeLifecycleAdapter",
            "dispatch_profile_load(identity, cursor)",
        ] {
            assert!(
                profile.contains(required),
                "profile callback omitted {required}"
            );
        }
        assert!(!profile.contains("unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)"));
        assert!(!profile.contains("original(dialog, b, c, d)"));
    }

    #[test]
    fn physical_classifier_converts_event_client_pixels_before_stage_hit_testing() {
        let physical = between(
            SAVE_PICKER_MENU,
            "unsafe fn save_picker_physical_activation_provenance(",
            "\nfn save_picker_decision_labels(",
        );
        for required in [
            "save_picker_event_client_point(row_input_gate)",
            "save_picker_validated_game_window_geometry()",
            ".client_point_to_movie_stage(event_client_x, event_client_y)",
            "save_picker_validated_game_pointer_from(geometry)",
            "event_stage_x",
            "event_stage_y",
            "event_target",
            "live_target",
        ] {
            assert!(
                physical.contains(required),
                "physical classifier omitted {required}"
            );
        }
        let event_route = between(
            physical,
            "let event_target = er_save_picker::route_drive_strip_native_click(",
            "\n    let live_target =",
        );
        assert!(event_route.contains("event_stage_x"));
        assert!(event_route.contains("event_stage_y"));
        assert!(!event_route.contains("event_client_x"));
        assert!(!event_route.contains("event_client_y"));
        assert!(
            physical
                .find("classify_picker_physical_row")
                .expect("ordinary-row gate")
                < physical
                    .find("save_picker_event_client_point")
                    .expect("drive-row event point"),
            "ordinary rows must return before drive-strip coordinate validation"
        );
        for diagnostic in [
            "raw_event_client=",
            "event_stage=",
            "live_stage=",
            "event_target=",
            "live_target=",
        ] {
            assert!(
                physical.contains(diagnostic),
                "physical diagnostic line omitted {diagnostic}"
            );
        }
        let ring = between(
            SAVE_PICKER_MENU,
            "pub(crate) fn save_picker_activation_ring_json()",
            "\nunsafe fn save_picker_event_client_point(",
        );
        for diagnostic in [
            "\\\"raw_event_client\\\":{}",
            "\\\"event_stage\\\":{}",
            "\\\"live_stage\\\":{}",
            "\\\"event_target\\\":{}",
            "\\\"live_target\\\":{}",
        ] {
            assert!(
                ring.contains(diagnostic),
                "activation ring omitted {diagnostic}"
            );
        }
    }

    #[test]
    fn installed_targets_are_the_byte_verified_1162_functions() {
        assert!(
            PROFILE_CONSTANTS
                .contains("pub(crate) const PROFILE_LOAD_MENU_WINDOW_UPDATE_RVA: u32 = 0x745570;")
        );
        assert!(
            PROFILE_CONSTANTS.contains(
                "pub(crate) const SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_RVA: u32 = 0x9a4670;"
            )
        );
        assert!(
            !PROFILE_CONSTANTS.contains("SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_RVA: u32 = 0x9a4170")
        );
        let typed_installer = between(
            HOOK_INSTALLS,
            "fn mh_install_profile_update_hook_once_with(",
            "\npub(crate) fn install_profile_load_menu_window_update_hook()",
        );
        for required in [
            "handler: ProfileLoadMenuWindowUpdateFn",
            "create(addr, handler)",
            "enable(addr)",
            "PROFILE_LOAD_MENU_WINDOW_UPDATE_INSTALLING",
        ] {
            assert!(
                typed_installer.contains(required),
                "typed installer omitted {required}"
            );
        }
        for forbidden in ["UnionFn", "register_union_hook", "mh_install_hook_once"] {
            assert!(
                !typed_installer.contains(forbidden),
                "float-ABI installer regressed through {forbidden}"
            );
        }
        let installed = between(
            HOOK_INSTALLS,
            "pub(crate) fn install_profile_load_menu_window_update_hook()",
            "\npub(crate) fn install_system_quit_profile_load_activate_hook()",
        );
        assert!(installed.contains("profile_load_menu_window_update_hook,"));
        assert!(installed.contains("MH_CreateHook("));
        assert!(installed.contains("MH_EnableHook("));
        assert!(!installed.contains("UnionFn"));
        assert!(!installed.contains("register_union_hook"));
    }
}
