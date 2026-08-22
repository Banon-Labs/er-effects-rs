# Crate-extraction execution roadmap

**Current baseline:** `466c2896` (`origin/main`, 2026-08-14)
**Parent planning PR:** [#193](https://github.com/Banon-Labs/er-effects-rs/pull/193)
**R1 scope:** publish the current ownership, function-partition, and caller ledger for every Rust file below `crates/er-effects-rs/src/experiments/`. This is a documentation/tooling checkpoint. It changes no runtime code and authorizes no extraction.

The earlier planning analyses remain historical evidence in PR #193. This document is the execution record for the current baseline; source functions and caller boundaries below supersede stale line-range plans.

## 1. Current measured state

| scope | files | lines |
|---|---:|---:|
| all `experiments/**` | 76 | 49,613 |
| excluding `startup_hooks/**` | 44 | 25,245 |
| `startup_hooks/**` plus `startup_hooks.rs` | 35 | 25,446 |
| lifecycle S10 split | 5 | 2,304 |
| own-load S11 split | 5 | 2,932 |
| save redirect | 3 | 2,389 |

The `scripts/check-crate-extraction-roadmap.py` gate checks the 79 paths, line counts, and required caller edges below. A source file add/remove/line-count change must refresh this ledger in the same change.

## 2. R1 ownership rules

1. **Current owner means present implementation owner.** Every row states the actual owner at this baseline, not a hoped-for crate destination.
2. **A partition is a named function family or an entire file.** Future work must move the named family, not a stale line interval.
3. **`STAY` is an explicit disposition.** It means the product owns the current implementation until a named decision or later roadmap node changes that fact.
4. **`D1`, `D2`, `D4`, and `D5` are decisions, not destinations.** A decision-gated row remains product-owned until its decision is accepted.
5. **Caller boundaries are source paths, not inferred ownership.** The required edges are checked from current Rust source so a refactor cannot leave this ledger silently stale.

## 3. Completed in-place correctness splits

| completed slice | current files | exact current partition | direct caller boundary |
|---|---|---|---|
| S10 lifecycle | `lifecycle/save_flow.rs` | `save_flow_tick` and its private save-flow state-machine helpers; future R20 implementation candidate | `lifecycle/task_tick.rs` calls `save_flow_tick` |
| S10 lifecycle | `lifecycle/task_tick.rs` | `tick_before_player_lookup`; product recurring-task scheduling stays | `lib_parts/dll_entry_parts/task_registration.rs` calls `tick_before_player_lookup` |
| S10 lifecycle | `lifecycle/title_visual_startup.rs` | `install_title_visual_startup_hooks`; product startup arming/order stays | `lib_parts/dll_entry_parts/bootstrap.rs` calls it |
| S10 lifecycle | `lifecycle/hook_installers.rs` | `install_profile_and_system_quit_hooks` and `install_boot_diagnostics_and_trace_hooks`; product install ordering stays | `lib_parts/dll_entry_parts/bootstrap.rs` calls both |
| S11 own-load | `own_load/loaders/load_drive.rs` | `own_load_drive`, `own_load_continue_fire`, and the job-slot/pump helpers; D5 candidate | `fallback_drives.rs`, `switch_reload.rs`, and `lifecycle/task_tick.rs` call the public functions |
| S11 own-load | `own_load/loaders/switch_reload.rs` | switch-reload reset/feed/FD4IO helpers; D5 candidate | `continue_load/slot_resolution.rs`, `system_quit_repro_guards.rs`, and `lib_parts/runtime_helpers.rs` call the public functions |
| S11 own-load | `own_load/drive.rs` | native-load hooks, world-resource hold, save-byte reader, and stream observer; D5 candidate | S10 task tick, S11 loaders, title resources, and System>Quit guards call the public functions |

S10 and S11 are complete in-place module splits. R1 does not reopen their correctness work or fold their function families back into their former flat files.

## 4. Critical current caller map

### 4.1 Save redirect

| function family | current implementation | direct external callers | R1 disposition |
|---|---|---|---|
| bootstrap source decision | `enforce_save_override_or_abort`, `missing_save_selection_pending`, `complete_missing_save_selection_from_picker` in `save_redirect/path_hooks.rs` | `lib_parts/dll_entry_parts/bootstrap.rs`; boot picker, boot progress, task tick, and picker menu consumers | product owns source selection and arming; R32 re-baselines the existing `er-save-redirect` interface |
| source identity and normalization | `active_steam_id64`, `normalize_save_bytes_to_active_steam_id`, `configured_or_default_save_file`, `active_save_file_for_system_quit` | continue-load, own-load, save-swap, and System>Quit handlers | implementation is a R32/R33 candidate; callers remain product adapters until an interface passes the deletion/depth test |
| rejection terminal state | `own_load_save_rejection_terminal`, fingerprint, repeated-rejection, record, signature, and probe helpers | own-load drive/loaders and title resource reader | current product-owned cross-feature guard; R32 records whether it belongs with redirect implementation or remains product policy |
| file-hook installation | `install_save_file_core_hooks`, `save_file_core_hooks_live`, and `install_save_redirect_hooks` in `save_redirect/file_ops.rs` | `path_hooks.rs` installs redirect hooks; boot picker reads core-hook liveness | existing `er-save-redirect` installation/queue owner remains the only future destination; R1 adds no host layer |

### 4.2 ProfileSelect editor runtime (`profile_05_010_editor_runtime.rs`)

| child node | exact function family | direct caller boundary | current owner |
|---|---|---|---|
| R12B1 transport | `editor_dir`, `write_status`, `write_status_text`, `heartbeat_status`, `read_command`, `defer_path_editor_command`, `status_for`, and `profile_editor_necromancy_tick` | task registration calls `profile_editor_necromancy_tick` | product scheduling; child node selects a transport owner without moving arming |
| R12B2 field application | `profile_editor_field_font_height`, `remember_profile_editor_field_target`, `forget_profile_editor_field_targets`, `cached_profile_editor_field_utf16`, `live_player_name_utf16`, `utf16_status_preview`, `profile_editor_runtime_tick`, `apply_profile_editor_command`, `apply_profile_editor_field_probe`, and `apply_profile_editor_one_field` | save-picker menu reads font height; title resource hooks cache and apply fields; quit-menu teardown forgets fields | product implementation pending R12B2 interface proof |
| R12B3 drive/chrome geometry | `command_targets_drive_row`, chrome/drive/path probe application, current-path and drive cursor transforms, and `apply_drive_row_native_cursor` | title resource hook calls `apply_drive_row_native_cursor` | product implementation pending R12B3 dependency proof |
| R12B4 path-window/caret | `apply_path_editor_window_position`, `reset_path_editor_caret_latch`, `apply_path_editor_caret_to_end`, `place_path_editor_caret_at_end`, and `set_text_field_caret_to_end` | profile-row hook places the window; path editor resets the caret latch | product implementation pending R12B4 lifecycle proof |
| R12B5 Scaleform primitives | proxy transform/value resolution, child resolution/destruction, setter guard, and position/scale setters | used only through the R12B2-R12B4 families | owner selected by D2; product keeps arming |

### 4.3 Native path editor (`save_picker_path_editor.rs`)

| child node | exact function family | direct caller boundary | current owner |
|---|---|---|---|
| R13B1 path model | `PathEditorOutcome`, `path_editor_outcome`, `path_editor_owns_terminal_job`, and `normalize_native_path_editor_text` | terminal hook and menu-pump adapter use the model | product implementation pending R13B1 save-identity proof |
| R13B2 terminal hooks | `software_keyboard_recipe`, `install_software_keyboard_result_hooks`, `software_keyboard_result_state`, `software_keyboard_text`, and the result/terminal callback hooks | used by native submission and terminal capture only | product implementation pending R13B2 runtime-hook proof |
| R13B3 native job submission | `SoftwareKeyboardConfig`, `SoftwareKeyboardRecipe`, `submit_path_editor`, and `apply_path_editor_outcome` | R13B4 menu-pump adapter submits and consumes the result | product implementation pending R13B3 native-queue proof |
| R13B4 lifecycle adapter | `save_picker_path_editor_active`, `path_editor_window_is_live`, `save_picker_note_path_editor_window_state`, `save_picker_reset_path_editor_state`, `save_picker_request_path_editor`, and `save_picker_menu_pump_path_editor` | save-picker menu requests/resets/checks activity; profile rows report window state and pump it | product scheduling stays; child node selects the feature implementation owner |

## 5. Execution sequence after R1

| ID | deliverable | gate | depends on |
|---|---|---|---|
| R2 | delete verified startup remainder and duplicate tests | source caller proof, equivalence proof, fingerprint | R1 |
| R4-R5 | finish loading-bar ownership then split boot progress by owner | loading-bar tests, static gate, file-size gate | R0a |
| R6A-R6D | give each trace family explicit imports | static gate and fingerprint | R0 |
| R7-R11 | move whole startup families only after their caller map remains current | per-family static and runtime gates | R1 |
| R12A-R13A | approve interfaces for the editor families above | reviewed partition and dependency proof | R1, R14, R0a |
| R14-R20 | repair save identity, move parsing/picker/quit families, then move lifecycle save-flow implementation | save corpus equality and feature runtime proof | R1, R0a |
| R32-R37 | move save redirect path/detour implementation only through the existing owner | interface depth review, host tests, redirected-save proof | R1 |
| D1/D2/D4/D5 | accept or reject optional crate extractions from evidence | interface and dependency review | affected current ledger rows |

### R24A decision -- descriptor guard collapsed into R8

Reject a duplicate descriptor-guard extraction. Merged PR #272 already moved the descriptor-advance detour, byte-verified RVA/offset identities, and trampoline state into `er-scaleform-hooks`; its fresh-title proof recorded `oracle_scaleform_desc_guard_installed = 1`. The remaining `scaleform_descriptor_guard.rs` root wrapper is product policy: it retains attach-time ordering and turns the hook crate's installation result into product diagnostic logging. Moving that wrapper would be a different startup-policy change, not R24A's native mechanism move. The remaining R24 resource/message families stay independently executable.

## Appendix A -- R1 current 76-file partition and caller ledger

Every row below is a current source file. `Current partition` is the exact present owner/disposition; `Next node` is a future decision or implementation node and does not change present ownership.

| Current file | Lines | Current partition | Next node |
|---|---:|---|---|
| `can_move_probe.rs` | 467 | product `STAY`: real-module conversion template | `STAY` |
| `continue_load.rs` | 9 | product re-export facade | D5 |
| `continue_load/product_continue.rs` | 653 | product continue/load policy | D5 |
| `continue_load/slot_resolution.rs` | 737 | product slot-resolution policy | D5 and R14 |
| `gating.rs` | 9 | product re-export facade | D1 |
| `gating/env_flags.rs` | 651 | product gate policy | D1 |
| `gating/runtime_modes.rs` | 137 | product runtime-mode policy | D1 |
| `gpu_frame_timing.rs` | 425 | product diagnostic | `STAY` |
| `gpu_readback.rs` | 62 | product GPU-readback facade | R4-R5 |
| `gpu_readback/boot_progress.rs` | 2,627 | loading-bar, boot-cover, and product adapter families | R4-R5 |
| `gpu_readback/gpu_draw_shared.rs` | 469 | boot-cover draw helper family | R5 and R27-R30 |
| `gpu_readback/save_picker_overlay.rs` | 21 | product compatibility shim | R17 |
| `input_block.rs` | 1,404 | product input ownership | `STAY` |
| `input_trace.rs` | 924 | product diagnostic | D4 |
| `lifecycle.rs` | 18 | S10 lifecycle facade | R20 |
| `lifecycle/hook_installers.rs` | 133 | product install ordering | `STAY` |
| `lifecycle/save_flow.rs` | 1,523 | System>Quit save-flow implementation | R20 |
| `lifecycle/task_tick.rs` | 444 | product recurring-task scheduling | `STAY` |
| `lifecycle/title_visual_startup.rs` | 186 | product startup arming/order | R22 |
| `mem.rs` | 24 | product compatibility helpers | R3 and R5 |
| `menu_diag.rs` | 4 | product diagnostic facade | D4 |
| `menu_diag/menu_observation.rs` | 615 | product menu observation | D4 |
| `mod.rs` | 101 | experiments module root and compatibility exports | `STAY` |
| `mod/own_stepper_idx6_memory.rs` | 109 | own-stepper memory family | D5 and R14 |
| `mod/product_core_own_stepper.rs` | 567 | product core own-stepper | D5 |
| `mod/product_core_own_stepper/fallback_drives.rs` | 639 | product fallback-drive diagnostic | D5 |
| `own_load.rs` | 9 | S11 own-load facade | D5 |
| `own_load/drive.rs` | 1,719 | native-load, world-resource, and save-byte families | D5 |
| `own_load/loaders.rs` | 7 | S11 loaders facade | D5 |
| `own_load/loaders/load_drive.rs` | 668 | load-drive implementation family | D5 |
| `own_load/loaders/switch_reload.rs` | 497 | switch-reload adapter family | D5 |
| `own_stepper.rs` | 9 | own-stepper facade | D5 |
| `own_stepper/bootstrap_drive.rs` | 874 | product bootstrap-drive policy | D5 |
| `own_stepper/load_steps.rs` | 741 | product load-step policy | D5 |
| `present_overlay.rs` | 946 | product present mechanism | R3 |
| `save_picker.rs` | 3 | product save-picker compatibility shim | R17 |
| `save_redirect.rs` | 9 | save-redirect facade | R32 |
| `save_redirect/file_ops.rs` | 346 | save-file hook implementation | R32-R37 |
| `save_redirect/path_hooks.rs` | 2,000 | save source/path policy and redirect adapters | R32-R37 |
| `startup_hooks.rs` | 104 | product startup root and arming facade | `STAY` |
| `startup_hooks/diagnostics/dlc_roots_trace.rs` | 169 | product diagnostic | `STAY` |
| `startup_hooks/diagnostics/layout_global_hooks.rs` | 383 | mixed title, quit, and product diagnostics | R11 and R22 |
| `startup_hooks/diagnostics/loadlist_wait_trace.rs` | 139 | product diagnostic | D4 |
| `startup_hooks/diagnostics/mod.rs` | 22 | diagnostics module facade | `STAY` |
| `startup_hooks/diagnostics/msb_parse_trace.rs` | 139 | product diagnostic | `STAY` |
| `startup_hooks/loading_cover/loading_cover_save_slot.rs` | 1,545 | save parsing, portrait, quit, telemetry, and product adapter families | R14-R18 |
| `startup_hooks/loading_cover/mod.rs` | 43 | loading-cover module facade | R15-R16 |
| `startup_hooks/loading_cover/portrait_equip_oracle.rs` | 287 | portrait oracle family | R16 |
| `startup_hooks/loading_cover/profile_table_gfx_files.rs` | 902 | Scaleform resource and profile-table families | D2 and R24 |
| `startup_hooks/loading_cover/scaleform_descriptor_guard.rs` | 39 | Scaleform descriptor guard | R8 |
| `startup_hooks/loading_cover/startup_modals_menu_cover.rs` | 1,083 | title-flow and product modal families | R22 |
| `startup_hooks/loading_cover/title_resources_stats_text.rs` | 2,400 | Scaleform resource, title, and product families | R22 and R24 |
| `startup_hooks/loading_cover/title_scaleform_msgbox.rs` | 828 | title message-box and Scaleform families | R22 and R24 |
| `startup_hooks/loading_cover/window_reconfig_observer.rs` | 473 | window-observation/final-geometry family | R9 |
| `startup_hooks/quit_menu/mod.rs` | 68 | quit-menu module facade | R10-R20 |
| `startup_hooks/quit_menu/profile_05_010_editor_runtime.rs` | 1,768 | R12B1-R12B5 families listed in section 4.2 | R12A-R12B5 |
| `startup_hooks/quit_menu/profile_rows_system_quit_menu.rs` | 1,954 | mixed profile-row title, quit, and sampler families | R11 |
| `startup_hooks/quit_menu/save_dest_commit.rs` | 1,227 | System>Quit destination commit family | R18 |
| `startup_hooks/quit_menu/save_flow_boxes.rs` | 656 | System>Quit confirmation-box family | R18-R20 |
| `startup_hooks/quit_menu/save_picker_menu.rs` | 2,894 | native picker, destination, and row-builder families | R17-R19 |
| `startup_hooks/quit_menu/save_picker_path_editor.rs` | 760 | R13B1-R13B4 families listed in section 4.3 | R13A-R13B4 |
| `startup_hooks/quit_menu/save_swap_profile_table.rs` | 1,226 | product profile renderer and quit swap families | R18-R19 |
| `startup_hooks/quit_menu/system_quit_dialog_handlers.rs` | 1,459 | System>Quit dialog implementation and picker adapter | R10 and R18 |
| `startup_hooks/quit_menu/system_quit_hooks.rs` | 681 | product hooks, deletion candidates, and quit/title hook families | R2, R19, R22 |
| `startup_hooks/quit_menu/system_quit_ownership_repro.rs` | 1,399 | ownership, telemetry, quit, and portrait families | R19 |
| `startup_hooks/quit_menu/system_quit_repro_guards.rs` | 1,157 | product repro guard and quit/title families | R2 and R19 |
| `startup_hooks/quit_menu/system_quit_row_identity.rs` | 289 | System>Quit row identity family | R18 |
| `startup_hooks/save_picker/mod.rs` | 22 | save-picker module facade | R17 |
| `startup_hooks/save_picker/save_picker_boot.rs` | 421 | boot picker surface | R17 |
| `startup_hooks/save_picker/save_picker_os_dialog.rs` | 19 | compatibility shim | R17-R18 |
| `startup_hooks/save_picker/save_picker_surface.rs` | 122 | picker surface routing adapter | R17-R18 |
| `title.rs` | 5 | title facade | R22 |
| `trace.rs` | 10 | trace facade | R6A-R6D and D4 |
| `trace/menu_constructor_capture.rs` | 1,336 | menu constructor capture family | R6B and D4 |
| `trace/menu_trace_hooks.rs` | 2,059 | title reload and menu trace families | R6C, R21, and D4 |
| `trace/native_result_map_hooks.rs` | 739 | native result-map hook family | R6A and D4 |

## Appendix B -- R32 save-redirect ownership rebaseline

R32 audits every remaining root save-redirect function after the pure planner, source validation, Win32 detour installation, original-function slots, and thread-local reentry guards moved to `er-save-redirect`. Every root function is **STAY**: it either applies product configuration, owns a game/UI/Steam boundary, mutates the product's private stage tree, publishes product telemetry, or is a root DLL detour callback. The crate's existing `SaveDetourDepth` and `SaveNtCreateDetourGuard` tests remain the depth/deletion invariant: native disk I/O is refused while either detour is active, the ntdll leg does not inflate healthy Win32 depth, and nested entries are pass-throughs rather than recursive work.

| Root region | Disposition | Exact functions |
|---|---|---|
| `save_redirect/path_hooks.rs:106-1281` | **STAY** -- product config, game/Steam identity, picker state, and product telemetry | `save_override_telemetry_only`, `save_trace_enabled`, `observe_steam_id64_from_save_path`, `active_steam_id64`, `log_save_steam_id_locations`, `normalize_save_bytes_to_active_steam_id`, `save_file_writeback_allowed`, `normalize_env_save_file_to_known_steam_id`, `normalize_env_save_file_to_active_steam_id_once`, `own_load_save_rejection_terminal`, `own_load_save_rejection_fingerprint`, `own_load_save_repeated_identical_rejections`, `record_own_load_save_rejection`, `own_load_save_rejection_signature`, `own_load_save_rejection_probe`, `write_save_redirect_telemetry`, `save_path_kind_label`, `record_save_like_createfile_path_kind`, `record_save_like_query_path_kind`, `direct_stage_no_steamid_kind_label`, `direct_stage_file_status`, `set_missing_save_dialog_state`, `missing_save_selection_pending`, `direct_save_file_source_active`, `env_save_file_path`, `validated_save_file_path`, `picker_status_for_save_source_rejection`, `validated_default_save_file`, `validated_configured_save_file`, `configured_active_steam_id64_env`, `steam_api_active_steam_id64`, `configured_active_steam_id64`, `save_redirect_native_source_dir`, `default_save_root`, `seamless_save_container_name`, `resident_module_path`, `ersc_settings_path`, `active_default_save_file_name`, `staged_save_container_names`, `active_default_save_file_names`, `default_save_with_character`, `default_save_file_for_steam_id64`, `default_save_file_candidates`, `active_default_save_file`, `configured_or_default_save_file`, `direct_mode_native_active_save_file`, `active_save_file_for_system_quit`, `save_redirect_source_for_validated_file`, `save_override_redirect_source`, `activate_save_redirect_source`, `enforce_save_override_or_abort`, `save_picker_seamless_mode_after_settle`, `complete_missing_save_selection_from_picker`, `wait_for_missing_save_dialog_if_pending` |
| `save_redirect/path_hooks.rs:1296-1596` | **STAY** -- private-stage filesystem lifecycle | `wide_len`, `ensure_direct_stage_for_requested_path`, `make_file_writable`, `remove_file_for_overwrite`, `read_normalized_save_for_stage`, `write_staged_save`, `ensure_direct_stage_for_steam_id`, `remove_stale_staged_saves`, `save_redirect_path` |
| `save_redirect/path_hooks.rs:1614-1894` | **STAY** -- root Win32 detour callbacks | `save_redirect_createfilew_hook`, `save_redirect_copyfilew_hook`, `save_path_api_redirect`, `save_redirect_getattrw_hook`, `save_redirect_getattrexw_hook`, `save_redirect_findfirstw_hook` |
| `save_redirect/path_hooks.rs:1929-1991` | **STAY** -- product integration tests | `vanilla_loads_only_sl2_and_never_a_seamless_co2`, `the_write_target_is_the_preferred_load_candidate`, `staging_covers_every_container_the_active_mode_can_ask_for`, `normalize_one_shot_bails_before_resolving_a_path_inside_a_detour` |
| `save_redirect/file_ops.rs:22-232` | **STAY** -- root Win32/ntdll detour callbacks and logging | `save_redirect_shgetfolderpathw_hook`, `save_ntcreatefile_diag_hook`, `save_redirect_getdiskfreew_hook`, `save_redirect_ntqueryvolinfo_hook` |
| `save_redirect/file_ops.rs:271-324` | **STAY** -- product loader resolution and hook lifecycle | `running_under_wine`, `module_proc`, `kernel32_proc`, `install_save_file_core_hooks`, `save_file_core_hooks_live`, `install_save_redirect_hooks` |

## R1 proof

- The ledger has exactly one row for every current Rust source below `experiments/`.
- Every row names its present product partition and next work node; no row derives ownership from a stale line range.
- Sections 3 and 4 pin the S10/S11, save-redirect, ProfileSelect editor, and native path-editor function/caller boundaries that later work must preserve or deliberately update.
- `scripts/check-crate-extraction-roadmap.py --selftest` and the live checker enforce the mechanical inventory and the critical caller map.
