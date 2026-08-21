# S6b save-redirect ownership extraction

Branch: `refactor/s6b-save-redirect-extraction-20260802`
Base: S6 `refactor/s6-save-picker-dll-20260802` at `4758cb13`
Issue: `er-effects-rs-orao`

## Result

Do **not** fold full save-redirect ownership into S6.

Static inspection shows the missing-save picker completion path is only the front door. A true standalone `er-save-picker-dll` save load also needs the product save-redirect owner and the boot-hold/title-flow owner. Moving just `complete_missing_save_selection_from_picker` would create another surface proof: it could validate a picked path, but it could not make Elden Ring read that save or resume the held boot job.

This branch implements the first safe slice anyway: `crates/er-save-redirect` now owns the host-runnable missing-save state machine and save-source planning/validation. It enforces the exact fixed PC save size (`0x1ba03d0`) for `.sl2`/`.co2`, not a loose minimum, and it exposes the staged-root/direct-file plan without installing runtime hooks.

## Current ownership chain

The S6 standalone DLL currently installs a `SavePickerHost`, arms `er_save_picker::overlay::arm_boot_picker()`, records a selected path, and releases its local latch. That is intentionally not autoload proof.

The product path that makes a picked save become the active game save is:

1. Product bootstrap installs the picker host seam in `crates/er-effects-rs/src/lib_parts/dll_entry_parts/bootstrap.rs`:
   - `missing_save_selection_pending -> experiments::missing_save_selection_pending`
   - `complete_missing_save_selection_from_picker -> experiments::complete_missing_save_selection_from_picker`
   - `save_file_core_hooks_live -> experiments::save_file_core_hooks_live`
2. The save-source decision and missing-save latch live in `crates/er-effects-rs/src/experiments/save_redirect/path_hooks.rs`:
   - `enforce_save_override_or_abort()` decides telemetry-only/default-user-save/redirect/missing-save-pending.
   - `complete_missing_save_selection_from_picker()` validates the picked file, activates the redirect source, calls `install_save_redirect_hooks()`, and changes the latch to ready.
   - `active_default_save_file()`, `save_redirect_source_for_validated_file()`, `activate_save_redirect_source()`, and direct-stage helpers own source selection and staging state.
3. The actual redirect hook owner lives in `crates/er-effects-rs/src/experiments/save_redirect/file_ops.rs`:
   - `install_save_file_core_hooks()` installs the always-live `CreateFileW` core hook used by save-destination commit.
   - `install_save_redirect_hooks()` installs or queues `CreateFileW`, `CopyFileW`, `GetFileAttributesW`, `GetFileAttributesExW`, `FindFirstFileW`, `SHGetFolderPathW`, Wine free-space overrides, and `NtCreateFile` diagnostics.
4. The boot-hold/title-flow side is outside `save_redirect/*`:
   - `bootstrap.rs` installs `install_title_setstate_trace_hook()` and spawns `install_show_progress_shortcircuit_hook()` while `missing_save_selection_pending()` is true.
   - `crates/er-effects-rs/src/experiments/startup_hooks/loading_cover/startup_modals_menu_cover.rs` holds `CS::ShowProgressJob::Run` at save-check and suppresses early `TitleTopDialog::open_menu` until the save is picked.
   - `crates/er-title-flow/src/title_load_step_hooks.rs` owns `install_title_setstate_trace_hook()`.

## Why the safe extraction is not small

A standalone autoload/save-load slice needs all of these properties at once:

- one owner for process-wide Win32/NT save hooks, otherwise co-loading `er_effects_rs.dll` and `er_save_picker_dll.dll` can double-detour the same `kernel32`/`shell32`/`ntdll` prologues;
- the missing-save latch shared by the picker overlay, save-redirect activation, boot-progress hold, title menu suppression, and title state gate;
- source validation/staging state shared by `CreateFileW`/`SHGetFolderPathW` detours and later System>Quit/save-destination code;
- boot title-flow hooks that are not owned by Product A today and are not listed in S6; they overlap later S8/S9 territory.

So the smallest honest implementation is not "add a callback to S6". It is a new shared owner boundary, probably `er-save-redirect`, plus a host seam for the remaining product-only surfaces.

## Recommended split

### S6b.1: shared save-redirect core crate, no runtime hook move yet

Implemented on this branch as the first code slice: `crates/er-save-redirect` moves host-runnable pieces first:

- missing-save state machine (`idle` / `pending` / `ready`),
- save source validation and source plan (`staged root` vs `direct file`),
- exact fixed-size `.sl2`/`.co2` plus BND4 validation for picked/configured saves,
- Wine path-root formatting helpers,
- direct-stage path planning.

Gate: host tests only. This gives `er-save-picker-dll` a real shared completion planner without installing hooks yet; the standalone shell validates/plans the selected save through this crate, then still stops at surface/staging proof.

### S6b.2: move the save file hook owner

Move `experiments/save_redirect/file_ops.rs` and the remaining state it needs into `er-save-redirect`, or wrap it behind a single exported owner chosen by the same feature-ownership scheme as the other DLLs. The moved owner must be idempotent and co-load-safe for both product and standalone profiles.

First slice on `refactor/s6b2-save-hook-owner-20260802`: move the save-detour reentry/depth guard into `er-save-redirect`. That guard is hook-owner state, is host-testable, and must travel with the eventual Win32/NT hook owner rather than staying buried in product `experiments`. The detour bodies still live in product after this slice.

Second slice on `refactor/s6b2b-save-hook-install-owner-20260802`: move the core/redirect one-shot install gate and core-CreateFileW-live flag shape into `er-save-redirect::SaveHookInstallState`. Product still owns the actual MinHook install calls, but the idempotency/live-state contract now belongs to the shared hook-owner boundary.

Third slice on `refactor/s6b2c-save-path-classifier-20260802`: move host-runnable UTF-16 save-path classification helpers into `er-save-redirect` (`SavePathKind`, `DirectStageNoSteamIdKind`, save-file suffix detection, SteamID extraction, ASCII case-insensitive wide matching). Product telemetry and detour bodies still own counters/logging, but their save-like path categories now come from the shared redirect core.

Fourth slice on `refactor/s6b2d-save-redirect-path-map-20260802`: move the pure `%APPDATA%\\Roaming\\EldenRing` wide-path rewrite into `er-save-redirect::redirect_wide_roaming_eldenring_path`. Product still owns observation, direct-file staging side effects, counters, logging, and detour bodies.

Fifth slice on `refactor/s6b2e-save-path-side-effects-20260802`: move the save-path redirect flow ordering into `er-save-redirect::redirect_wide_save_path_with_side_effects`, with product callbacks for SteamID observation and direct-file staging. Product still owns the callback bodies plus hook detours/install.

Sixth slice on `refactor/s6b2f-save-hook-install-primitives-20260802`: move the save hook original/trampoline slots into `er-save-redirect`. Product still resolves addresses, creates MinHook objects, installs detours, and owns the callback bodies, but the shared core now owns the process-global trampoline cells those bodies call through.

Seventh slice on `refactor/s6b2g-save-hook-queue-helper-20260802`: move the resolved-target MinHook queue/store primitive into `er-save-redirect::queue_resolved_save_hook`. Product still resolves module exports, gates install sequencing, applies queued hooks, and owns callback bodies.

Eighth slice on `refactor/s6b2h-core-createfile-install-20260802`: move the always-on core `CreateFileW` install sequence into `er-save-redirect::install_core_createfilew_hook`. Product still supplies export resolution, the detour callback, and logging, but the shared core now owns the idempotent install sequence for the hook required by save-destination commits.

Ninth slice on `refactor/s6b2i-save-redirect-batch-install-20260802`: move the redirect-mode hook batch sequencing into `er-save-redirect::install_redirect_save_hooks`. Product still owns the redirect/trace gate, module/export resolution callbacks, detour bodies, and telemetry sink, but the shared core now owns the idempotent MinHook initialize/queue/apply/forget sequence for the redirect batch.

Tenth slice on `refactor/s6b2j-save-redirect-install-gate-20260802`: move the redirect/trace readiness gate into `er-save-redirect::install_redirect_save_hooks_when_ready`. Product still supplies the raw readiness booleans and all runtime-specific callbacks, but the shared core now owns the deferred-install decision and log line.

Eleventh slice on `refactor/s6b2k-free-space-detour-core-20260802`: move Wine free-space detour output patching into host-testable `er-save-redirect` helpers. Product detour bodies still own the Win32/NT ABI boundary, original-call trampoline, and telemetry logging, but the shared core now owns the exact ample-space constants and output-buffer mutation logic.

Twelfth slice on `refactor/s6b3a-ntcreatefile-diag-core-20260802`: move NtCreateFile save-path/access diagnostic classification into host-testable `er-save-redirect` helpers. Product still owns OBJECT_ATTRIBUTES decoding, original-call trampoline, missing-save waiting, SteamID observation, normalization side effects, and telemetry logging.

Thirteenth slice on `refactor/s6b3b-shgetfolderpath-core-20260802`: move SHGetFolderPathW APPDATA redirect classification and output-buffer writing into host-testable `er-save-redirect` helpers. Product still owns first-load/root counters, original-call trampoline, and telemetry logging.

Fourteenth slice on `refactor/s6b3c-createfile-diag-core-20260802`: move CreateFileW save-like/save-file/backup diagnostic classification and log-hit policy into host-testable `er-save-redirect` helpers. Product still owns the save-destination redirect, SteamID observation, direct staging, normalization side effects, original-call trampoline, counters, and telemetry logging.

Fifteenth slice on `refactor/s6b3d-query-path-diag-core-20260802`: move save existence/query API path diagnostic classification into host-testable `er-save-redirect` helpers. Product still owns redirect path construction, path-kind counters, rate counters, original-call trampolines, and telemetry logging.

Sixteenth slice on `refactor/s6b3e-copyfile-endpoint-core-20260802`: move CopyFileW endpoint wait/redirect planning into `er-save-redirect::classify_copyfile_endpoint`. Product still owns pointer decoding, missing-save wait side effect, redirect path construction callback, original-call trampoline, and telemetry logging.

Seventeenth slice on `refactor/s6b3f-createfile-open-plan-20260802`: move CreateFileW post-save-destination open planning into `er-save-redirect::plan_create_file_open`. Product still owns the save-destination fast path, SteamID observation, missing-save wait side effect, redirect path construction callback, normalization side effect, original-call trampoline, counters, and telemetry logging.

Eighteenth slice on `refactor/s6b3g-query-path-plan-20260802`: move save existence/query API redirect planning into `er-save-redirect::plan_save_query_path`. Product still owns API-specific log budgets, path-kind counters, redirect path construction callback, original-call trampolines, and telemetry logging.

Nineteenth slice on `refactor/s6b4a-save-path-kind-core-20260802`: move the pure mapping from `SavePathKind` to telemetry-counted bucket into `er-save-redirect::SavePathTelemetryBucket`. Product still owns the actual counters, telemetry serialization, and hook-side recording.

Twentieth slice on `refactor/s6b5a-direct-stage-status-core-20260802`: move direct-stage file existence/byte-size probing into `er-save-redirect::probe_direct_stage_file_status`. Product still owns global stage/source state and telemetry serialization.

Twenty-first slice on `refactor/s6b5b-direct-stage-dirs-core-20260802`: move the direct-stage case-directory convention (`eldenring` and `EldenRing`) into `er-save-redirect::direct_stage_case_dirs`. Product still owns when to create those directories and how to log stage state.

Twenty-second slice on `refactor/s6b5c-direct-stage-request-plan-20260802`: move direct-stage requested-path planning into `er-save-redirect::plan_direct_stage_request`, returning either a SteamID64 or a no-SteamID diagnostic kind. Product still owns counters, capped logging, directory creation timing, and staging side effects.

Twenty-third slice on `refactor/s6b5d-save-path-telemetry-plan-20260802`: move save-like path telemetry planning into `er-save-redirect::plan_save_path_telemetry`, returning both the shared kind and optional counted telemetry bucket. Product still owns the actual counters and serialization.

Twenty-fourth slice on `refactor/s6b6a-writeback-path-core-20260802`: move case-insensitive path equality and default-root writeback eligibility into `er-save-redirect::save_file_writeback_allowed`. Product still owns default-root discovery, BND4 normalization side effects, file writes, and telemetry logging.

Twenty-fifth slice on `refactor/s6b6b-readonly-status-core-20260802`: move save-file readonly status probing into `er-save-redirect::save_file_is_readonly`. Product still owns any permission mutation and user-facing diagnostic logging.

Twenty-sixth slice on `refactor/s6b7a-save-normalize-hash-core-20260802`: move save normalization byte hashing into `er-save-redirect::save_normalize_hash_bytes`. Product still owns active SteamID discovery, BND4 normalization call sites, file writes, and telemetry logging.

Twenty-seventh slice on `refactor/s6b8a-steamid-validation-core-20260802`: move plausible SteamID64 range validation into `er-save-redirect::plausible_steam_id64`. Product still owns environment/config reads, Steam API access, active-user selection policy, and telemetry logging.

Twenty-eighth slice on `refactor/s6b8b-steamid-dir-name-core-20260802`: move SteamID64 directory-name parsing into `er-save-redirect::steam_id64_from_dir_name`. Product still owns default-save root enumeration, candidate validation, active-user selection policy, and telemetry logging.

Twenty-ninth slice on `refactor/s6b8c-default-save-path-core-20260802`: move default save path construction into `er-save-redirect::default_save_file_path`. Product still owns default-save root discovery/enumeration, candidate validation, active-user selection policy, and telemetry logging.

Gate: Windows-target check plus a no-runtime hook-install smoke if available. Runtime proof comes after this, not before.

### S6b.3: boot-hold/title-flow seam

Decide whether standalone save-picker owns the save-check hold itself or calls into `er-title-flow` through a host seam. This touches `startup_modals_menu_cover.rs` and `er-title-flow::title_load_step_hooks`, so it should be reviewed as runtime-affecting and not slipped into S6.

Gate: approved direct/offline boot-with-no-save runtime proof.

## Parent action

Keep S6 as surface/staging proof and S7 as decision-core extraction. Do not merge save redirect into either. Start a reviewed S6b/S8-prep branch only after accepting this boundary, with `er-save-redirect` as the likely new shared owner crate.
