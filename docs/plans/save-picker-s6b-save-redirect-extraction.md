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

Gate: Windows-target check plus a no-runtime hook-install smoke if available. Runtime proof comes after this, not before.

### S6b.3: boot-hold/title-flow seam

Decide whether standalone save-picker owns the save-check hold itself or calls into `er-title-flow` through a host seam. This touches `startup_modals_menu_cover.rs` and `er-title-flow::title_load_step_hooks`, so it should be reviewed as runtime-affecting and not slipped into S6.

Gate: approved direct/offline boot-with-no-save runtime proof.

## Parent action

Keep S6 as surface/staging proof and S7 as decision-core extraction. Do not merge save redirect into either. Start a reviewed S6b/S8-prep branch only after accepting this boundary, with `er-save-redirect` as the likely new shared owner crate.
