# Save-picker / quit-menu crate extraction

Plan of record for bd `er-effects-rs-mz7q`. Baseline: `main` @ `1175610b` (PRs #99-#106
merged). Phase 1 (this document + the crate scaffolding) changes no product behavior.

**Goal.** Decouple two features completely out of `er-effects-rs` into crates that can be
bundled into other products AND shipped as independent ME3 DLLs, following the
`er-loading-portrait` / `er-loading-portrait-dll` precedent: the feature crate holds the
logic, a thin `cdylib` shell makes it loadable alone, and a fn-pointer host struct
(`crates/er-loading-portrait/src/host.rs`, `install_host()` at DllMain, inert defaults when
no host is installed) is the ONLY coupling back to the product.

The two products, as the user named them:

* **(A) our own drawn picker** -- the picker we fully draw and manage, shown when the game
  starts without a save provided. Crate `er-save-picker` + shell `er-save-picker-dll`.
* **(B) the customized post-autoload vanilla menu**, as ONE product -- the Save Game button
  and its functionality, BOTH load buttons (Load Character + Load Character from File --
  renamed 2026-07-31 from Load Profile / Load Save Profiles), and
  the Quit to Desktop button. Crate `er-quit-menu` + shell `er-quit-menu-dll`.

---

## 0. Rules this plan is bound by

1. **Nothing is left behind as a shared dependency the new crates reach back into.** If a
   mechanism is used by the extracted crates but NOT by the rest of the product, it moves
   into one of the pairs (or becomes its own crate). A host-seam entry is only legitimate
   when a consumer OUTSIDE the extracted features genuinely still needs the thing.
2. **`er-effects-rs` stays byte-for-byte behaviorally identical at every slice.** This is a
   refactor, not a rewrite -- with **no exceptions**. An earlier draft carved out one, the
   boot dialog's dim, but PR #109 has since landed that behavior on `main`, so every slice
   including S5 is now pure motion. See SS5.3.
3. **The OS-native picker is always available.** We never force a user onto an in-game
   picker we built. Both places that draw one -- (A)'s boot picker and (B)'s in-game browse
   rows -- must offer the OS dialog as a selectable surface.
4. **Any DLL combination must work**: either new DLL, both, neither, freely combined with
   the product / loading-bar / portrait / telemetry / input-harness / reload-trace DLLs, or
   with none of them. SS6 is the design for that.
5. No new env gates; delete-not-gate; no lossy UTF-8; never commit game-derived binaries.
   `bash scripts/check.sh` green before any commit.

---

## 1. Attribution inventory

### 1.1 The seven files named in the epic

**Counts are as of `4ae8c6d1`** (post #107/#109/#110/#111). They drift as main moves --
re-measure before relying on them; three of these grew between the first draft of this plan
and that commit, all in files the picker PRs touched.

| File | Lines | Verdict |
|---|---:|---|
| `experiments/save_picker.rs` | 2002 | -> **A** (whole file; the shared row model) |
| `experiments/gpu_readback/save_picker_overlay.rs` | 912 | -> **A** (whole file) |
| `startup_hooks/save_picker/save_picker_os_dialog.rs` | 792 | **SPLIT**: mechanism + tests (~590: 1-482, 684-792) -> A; the two System>Quit entry points (~200: 483-683) -> B |
| `startup_hooks/quit_menu/save_picker_menu.rs` | 944 | -> **B** (whole file) |
| `startup_hooks/save_picker/save_picker_surface.rs` | 369 | -> **B** (whole file) |
| `startup_hooks/quit_menu/save_picker_dim_overlay.rs` | 876 | -> **B** (whole file) |
| `startup_hooks/quit_menu/save_swap_profile_table.rs` | 1050 | **SPLIT**: save-swap preview/restore/prepare/recommit (1-417) -> B; the profile-renderer drive + table hooks (418-1050) **STAY** |
| **total** | **6945** | |

Structural fact that shapes everything below: five of the seven are **`include!`d** into
`startup_hooks.rs` (lines 25-28) and `gpu_readback.rs` (line 55), not `mod`-declared. They
share one flat namespace with ~20 sibling files and have **no `use` statements for
in-crate items** -- every cross-call is an unqualified free identifier. Extraction is an
API-design job, not a file move; SS2 is the measured cross-call surface that becomes it.

### 1.2 Product (A) -- the drawn boot picker

**Ownership of the OS dialog is a user decision (2026-07-30):** the boot missing-save flow
is tightly coupled to the OS-native file dialog, so the dialog implementation belongs to A.

| Item | Source | Lines |
|---|---|---:|
| Row model: `SavePickerModel`, `PickerIntent`, `PickerRow`, `PickerEntry`, `PickerActivation`, `PickRejection`, `entry_row_base` and every derived index, drive/page cycling, `save_picker_accepts`, `save_picker_extension_accepted`, `civil_from_unix_seconds`, `format_last_saved`, `truncate_utf16`, `ACTIVE_SAVE_PICKER` | `experiments/save_picker.rs` | 2002 (of which ~960 are its own tests) |
| `SaveSlotInfo`, `parse_save_character_slots` | `startup_hooks/loading_cover/loading_cover_save_slot.rs` 754-799 | 47 |
| Boot overlay: arm/disarm, both input paths, file stage + character sub-stage, `overlay_save_picker_onto`, deferred pick completion | `gpu_readback/save_picker_overlay.rs` | 912 |
| comdlg32 mechanism: `os_dialog_run`, `os_pick_validated`, `classify_os_outcome`, `should_reopen`, `os_dialog_filter`, `OsDialogClaim`, `os_dialog_owner`, `os_pick_path_from_buffer`, `OsPickOutcome` + its tests | `startup_hooks/save_picker/save_picker_os_dialog.rs` 1-482, 684-792 | ~590 |
| Config keys `preferred_save_picker_dir`, `autoupdate_preferred_picker_dir`, `os_native_save_picker` (+ `use_os_file_picker` / `save_picker.os_native` aliases): struct fields, accessors, parse + validation, generated boilerplate text, `remember_preferred_save_picker_dir`, their tests | `src/config.rs` (26-31, 116-200, 253-266, 463-495, 600-670) | ~120 |
| **A total** | | ** 3671** |

`SaveSlotInfo` / `parse_save_character_slots` move rather than staying because their only
consumers anywhere in the crate are `save_picker.rs` (6 refs), `save_picker_overlay.rs`
(1 ref) and their own definition site -- rule 0.1.

**A installs zero game-address detours.** Its only OS hook is a per-DLL
`SetWindowsHookExW(WH_KEYBOARD_LL)` at `save_picker_overlay.rs:521`. That single fact is
why A can never collide with anything (SS6). Re-verified at `4ae8c6d1`: none of A's three
files contains a `MhHook::new` / `register_union_hook` / MinHook call site.

### 1.3 Product (B) -- the customized System>Quit menu

The user's verified finding stands: the System>Quit row-cloning machinery is referenced by
these features only, so B owns the row table it clones into. B is therefore substantially
larger than "three buttons".

| Item | Source | P (moves) |
|---|---|---:|
| Quit-row resolver `QuitRow` / `QuitRowFacts` / `QuitRowVerdict`, `resolve_quit_row`, the row table, `system_quit_row_gate_instant_quit` (the single gate on the irreversible `ExitProcess(0)`) | `system_quit_row_identity.rs` (921 total, 242 tests) | 921 |
| Row CLONER `system_quit_duplicate_add_cancel_button_hook` (:1295-1529), row ROUTER `system_quit_route_button_action_or_forward` (:170-354, whose `None => 0` arm is what suppresses the native Return-to-Desktop), Save Game label hook (:946-988), save calls (:990-1268), ProfileSelect submit (:2-110), PR-#103 Scaleform ctor/dtor double-free skip (:1535-1601) | `system_quit_dialog_handlers.rs` (1601) | 1577 |
| In-game `05_010` browse surface: row staging, activation, menu-pump rebuild/resubmit, browse stats lines, list-builder re-stage hook | `save_picker_menu.rs` | 944 |
| Surface router: `open_picker_for_intent`, `os_native_picker_active`, `save_dest_start_dir`, `save_dest_route_picked_target` | `save_picker_surface.rs` | 369 |
| Dim overlay (layered GDI window, z-order sampling) | `save_picker_dim_overlay.rs` | 876 |
| OS entry points `os_open_save_picker_load`, `os_open_save_dest_picker` | `save_picker_os_dialog.rs` 483-683 | ~200 |
| Save-flow confirm chain, `menu_job_emit_result_hook` | `save_flow_boxes.rs` (790) | 750 |
| Destination identity (file-id probes, atomic write) | `save_dest_identity.rs` (465, 170 tests) | 465 |
| Destination commit / redirect / verify | `save_dest_commit.rs` (1286, 333 tests) | 1286 |
| Save-flow stage machine `save_flow_tick`, `save_flow_fire_gate_tick` | `experiments/lifecycle.rs` 229-1000 | ~770 |
| Foreign-save ProfileSummary preview, swap/restore, prepare-selected-slot, recommit-after-return-title-save | `save_swap_profile_table.rs` 1-417 | 417 |
| Ownership ledger + `delay_delete_enqueue_renderer`, PR-#103 Scaleform lifecycle guard, `~MenuWindowJob` doomed-window UAF fix, window-list push hook, quit-to-desktop clean kill, the noop-action / Save-Game-text / Save-Game-confirm installers, `system_quit_profile_load_activate_hook` | `system_quit_ownership_repro.rs` 26-102, 386-1340 | ~950 |
| ProfileSelect confirm routing, portrait retarget at switch, `system_quit_arm_quickload_autoload` (the core of Load Character, ex-Load Profile), in-world load guards, continue-confirm hook | `system_quit_repro_guards.rs` 1286-1544, 1610-2022 | ~680 |
| Quit-tab pane restore, return-title chain, ProfileSelect top-menu tick, OptionSetting pane sampling, `system_quit_menu_window_run_post` | `profile_rows_system_quit_menu.rs` 510-1314, 1424-1581, 1663-1834 | ~1315 |
| Continue-confirm installer, stuck-testnet-step force-finish, profile-load-job-run hook, the three profile-load installers | `system_quit_hooks.rs` 2-55, 160-310, 383-482, 982-1034 | ~480 |
| `install_system_quit_duplicate_button_hook` -- the single entry point that installs the whole feature | `layout_global_hooks.rs` 61-160 | ~100 |
| **B total** | | **>= 12 102** |

The B total is a floor, not a measurement. Only the surface-router and OS-entry-point rows
were re-measured at `4ae8c6d1` (+147 and +25 against the original 11 930); several other
rows -- `lifecycle.rs`, `save_dest_commit.rs`, `system_quit_dialog_handlers.rs`,
`profile_rows_system_quit_menu.rs` -- also grew with #107/#110 and their line ranges are
NOT re-derived here. Re-measure before using any of these for slice sizing. The conclusion
they support is unaffected: B is several times A, and far more than "three buttons".

### 1.4 Product code vs agent-harness code

Separating these was explicitly required. Classification evidence: no `ER_EFFECTS_*` env
var is read by any of these files; every remaining gate is a compile-time `false`, a
module-presence check (`harness_dll_present()` = `GetModuleHandleA("er_input_harness_dll.dll")`),
or a control file.

**STAYS in `er-effects-rs` as diagnostics / harness (~1950 lines).** These are agent-only
and must not be dragged into a shipped feature crate:

| Block | Why |
|---|---|
| `system_quit_repro_guards.rs` 1-1285 (`system_quit_repro_tick` + its state helpers) | The self-driving autopilot. First line of the tick is `if !system_quit_repro_enabled() { return; }`, and that gate is `harness_dll_present() && ...` |
| `system_quit_ownership_repro.rs` 104-383 (GX command-queue telemetry) | Self-described "never alters queue behavior"; pure producer accounting |
| `system_quit_hooks.rs` 56-159 (child-finish trace) | "READ-ONLY teardown-requester trace" |
| `profile_rows_system_quit_menu.rs` 867-921, 1315-1423 | Per-frame save-gate diagnostic; OptionSetting row-table sampling that writes only telemetry |
| `layout_global_hooks.rs` 166-266 | MenuWindow latch + c30-writer diagnostic; the latter is gated on a hard `false` |

**PRODUCT despite living in a file named `*_repro*`.** `system_quit_ownership_repro.rs`
carries the merged PR #103 UAF ownership fix, and `system_quit_repro_guards.rs` carries the
core of Load Character (ex-Load Profile). Do not mistake either for scaffolding:

* `ownership_take` / `ownership_release` / `delay_delete_enqueue_renderer` (26-102)
* `install_scaleform_handler_lifecycle_guard` (386-472) -- the PR-#103 double-free guard
* the `~MenuWindowJob` doomed-window UAF fix (473-716, bd `er-effects-rs-j74t`)
* `install_quit_to_desktop_clean_kill_hook` (886-939) -- the Quit to Desktop clean kill
* `system_quit_arm_quickload_autoload` (`repro_guards.rs` 1403-1544) -- Load Character's core (the row was called Load Profile before 2026-07-31)

**DELETE (~640 lines, X).** Verified zero callers; deleting them is behavior-preserving by
construction and is its own slice:

| Block | Lines | Evidence |
|---|---:|---|
| `system_quit_hooks.rs` 483-944 -- four `install_system_quit_gaitem_*` / `..._gameman_load_save_hook` + their `disable_*` | ~460 | All four `install_*` have zero callers; only the `disable_*` are called (`title_tick_cover.rs:2967-2969`), disabling hooks that were never installed. `repro_guards.rs:1420-1427` explains they "only ever CORRUPT the gaitem singleton" |
| `system_quit_hooks.rs` 311-381 | ~70 | Detours for those never-installed hooks |
| `system_quit_ownership_repro.rs` 717-791 + `profile_rows:1582-1662` | ~135 | `install_system_quit_menu_window_job_run_hook`'s only call site is commented out (`layout_global_hooks.rs:91`); its detour is dead with it |
| `layout_global_hooks.rs` 2-56 `apply_system_quit_multislot_layout_patch` | 55 | Zero callers; the caller comment (:62-68) says the GFx component-index patch was deliberately abandoned |
| `profile_rows_system_quit_menu.rs` 149-200 `install_title_custom_cover_run_hook` | 52 | No callers anywhere in `src/` |
| `repro_guards.rs` 1545-1603 gaitem finalize/lookup detours | 59 | Installers never called |

**Belongs to a DIFFERENT feature -- leave alone.** `profile_rows_system_quit_menu.rs`
2-148 and 201-509 are title-cover / stats-panel hooks called from `lifecycle.rs:1826-1864`;
`layout_global_hooks.rs` 272-429 is the boot splash skip and the pre-world Wwise mute.
`save_swap_profile_table.rs` 418-1050 (`force_profile_render_tick`,
`profile_renderer_teardown_spare_hook`, `profile_select_table_diag_hook`) is the
loading-screen profile-model render drive, called from `task_registration.rs`,
`title_tick_cover.rs`, `loading_cover_save_slot.rs` and `env_flags.rs`.

### 1.5 Hook addresses each product installs

This table decides SS6. Every address B touches is contended, so B's every detour must go
through the `er-hook` union.

| Address | Function | Owner | Also hooked by |
|---|---|---|---|
| -- | `SetWindowsHookExW(WH_KEYBOARD_LL)` | **A** | nothing (per-DLL OS hook, cannot collide) |
| `0x920c90` | `AddCancelButton` (the row cloner) | **B** | -- |
| `0x875590` | `ProfileSelect` item-list builder | **B** | -- |
| `0x9a4670` | `ProfileLoadDialog` vtable slot 20 (row activate) | **B** | -- |
| `0x9a4d90` | profile-load confirmed | **B** | -- |
| `0x826d50` | profile-load job Run | **B** | -- |
| `0x9a4ed0` | profile-load list rebuild (called, not hooked) | **B** | -- |
| `0x7ac890` | ProfileSelect native close (called, not hooked) | **B** | -- |
| `0x733ef0` | MenuWindow list push | **B** | -- |
| `0x7ac720` | `~MenuWindowJob` | **B** | -- |
| `0xd3ed90` | `MenuOffscrRendParam` lookup (quit clean kill) | **B** | -- |
| `0x961640` / `0x9610d0` / `0x9749f0` | return-title / return-desktop action, button-controller activate | **B** | -- |
| `0x7633d0` | `MsgRepository::GetAndFormat` (Save Game label) | **B** | -- |
| `0x67a3a0` | return-title request | **B** | -- |
| `0x11a8870` / `0x11a8900` | Scaleform handler ctor / dtor | **B** | -- |
| `0xb0e180` | continue-confirm | **B** | **product** + `er-reload-trace-dll` (already unioned) |
| `0x67b200` / `0x67b290` | request-load-slot / in-world load | **B** | product (bare `MhHook`; `er-reload-trace-dll` skips them for that reason -- `UNION_SKIP_RVAS`) |
| `0x7ad1c0` | `MenuWindowJob::Run` / PAB node update | **product** | B reaches it only via `system_quit_menu_window_run_post`, called from the product's PAB detour |
| `0x1aeae60` / `0x1b3bda0` | GX queue telemetry | product (diagnostic) | -- |

---

## 2. Dependency + seam design

### 2.1 Crate list

```
er-save-picker          (A) model + boot overlay + comdlg32 surface + picker config keys
  +- er-save-picker-dll (A) thin cdylib shell
er-quit-menu            (B) the customized System>Quit menu   -- depends on --> er-save-picker
  +- er-quit-menu-dll   (B) thin cdylib shell
er-hook                 (extended, no new crate) union-ownership election
```

**`er-quit-menu` -> `er-save-picker`, one way.** There is no shared core crate: the user
ruled that out. The direction is acyclic because of one asymmetry the user pinned -- the dim
overlay is for the in-game quit-menu case ONLY, and the boot missing-save dialog is not
dimmed. B needs A (for the model and the dialog); A never needs the dim.

It also satisfies rule 0.3 with no duplication: B gets the OS-native fallback surface
through that dependency.

### 2.2 Crate dependency is not DLL dependency

`er-quit-menu` statically links `er-save-picker`, so a profile listing ONLY
`er_quit_menu_dll.dll` still offers the OS-native surface -- but must NOT thereby acquire
A's boot missing-save behavior. Two mechanisms, and only the second is a guarantee:

1. **cargo feature `boot-flow`** (default on; `er-quit-menu` takes `er-save-picker` with
   `default-features = false, features = ["os-dialog"]`). This isolates the standalone-B
   build. It is **not** sufficient on its own: cargo unifies features across a build graph,
   so in the product DLL -- which wants `boot-flow` -- `er-quit-menu` gets it too.
2. **an explicit arm entry point.** Nothing in A's boot flow installs a hook, spawns a
   thread or arms a model until a host calls `er_save_picker::arm_boot_picker()`.
   `er-quit-menu` never calls it. This holds in every build, feature unification included.

Both are checked in CI: `scripts/check-rust-build.sh` compiles the union build and the
`--no-default-features` build of `er-save-picker` on the shipping target.

### 2.3 `SavePickerHost` (product A)

Each field is one measured outbound cross-call from A's files into the rest of the product.

| Field | Product implementation | Neutral default |
|---|---|---|
| `append_autoload_debug` | `telemetry::append_autoload_debug` | no-op |
| `missing_save_selection_pending` | `save_redirect::path_hooks` | `false` |
| `complete_missing_save_selection_from_picker` | `save_redirect::path_hooks` | `false` (refuse) |
| `save_picker_seamless_mode_after_settle` | `save_redirect::path_hooks` | `false` (vanilla) |
| `picker_start_dir` | `save_picker_title_start_dir` | empty path |
| `remember_picker_dir` | `config::remember_preferred_save_picker_dir` | no-op |
| `game_main_window` | `experiments::input_block::game_main_window` | `0` |
| `save_file_core_hooks_live` | `save_redirect::file_ops` | `false` -- the H4 gate stays CLOSED, so an un-hosted crate never throws a modal over a window it knows nothing about |
| `windows_path_for_log` | `system_quit_windows_path_for_log` | identity |
| `save_dest_commit_window_armed` | `save_dest_commit` | `false` (log-only) |

Plus the caller-supplied cover: `os_pick_validated` takes a `PickerCoverFactory`
(`fn(&str) -> Option<PickerCover>`), the cross-crate form of the pre-extraction caller
argument -- which is how the dim stays out of A's business (SS5.3).

### 2.4 `QuitMenuHost` (product B)

B's seam is larger because it genuinely shares state with the rest of the product. Entries
that cross with std/primitive types are already in the scaffold; the rest land with their
slice.

| Field | Product implementation | Neutral default |
|---|---|---|
| `append_autoload_debug`, `append_crash_log` | `telemetry` | no-op |
| `default_save_root`, `save_picker_seamless_mode_after_settle` | `save_redirect::path_hooks` | `None` / `false` |
| `system_quit_env_save_path`, `system_quit_env_save_dir` | `system_quit_dialog_handlers` (moves with B; the seam entry exists because `save_dest_commit` and the product's own paths still ask) | `Err` |
| `normalize_save_bytes_to_active_steam_id` | `save_redirect::path_hooks` | `false` |
| `system_quit_profile_summary_ptr` | `loading_cover_save_slot` | `0` |
| `portrait_loaded_slot`, `portrait_loaded_slot_confirmed`, `portrait_target_slot` | `loading_cover_save_slot` (same shapes `PortraitHost` already uses) | `0` / `None` / `0` |
| `maybe_build_profile_table_for_loading`, `force_profile_render_tick`, `native_loading_screen_active` | `loading_cover_save_slot` / `save_swap_profile_table` 418+ | inert |
| `game_main_window`, `release_input_block_now` | `experiments::input_block` | `0` / no-op |
| `take_save_write_bypass` | `er-save-suppress` | **`false`** -- an un-hosted crate must never authorise a real save |
| `product_autoload_enabled`, `switch_reload_active` | `gating::env_flags` | `false` |
| *(pending, needs a reshaped type)* the save-swap ledger (`SystemQuitSaveSwapState`, `system_quit_save_swap_lock`, `system_quit_save_swap_arm_original`, `write_profile_summary_record`, `system_quit_hash_bytes`, `system_quit_file_stamp`, `kick_target_profile_slot`), the serialized-slot reader (`SerializedSaveSlot`, `read_utf16_name_units`), `own_load_read_sl2_bytes`, `profile_slot_fingerprint` | `loading_cover_save_slot` / `own_load` / `continue_load` | -- |

### 2.5 The boundary this plan draws around saving

The Save Game button IS in scope for B (user decision 2026-07-30), extracted from `main`.
The adjacent save machinery is NOT:

* **Moves to B** -- the Save Game menu ROW and the FLOW it drives: label substitution
  (`0x7633d0`), the confirm chain (`save_flow_boxes.rs`), the destination browser, the
  commit + verify (`save_dest_commit.rs`, `save_dest_identity.rs`), and the `save_flow_tick`
  stage machine.
* **Stays where it is** -- save SUPPRESSION and save REDIRECT internals: `er-save-suppress`
  (already its own crate, with its own `er-save-disable-dll`) and
  `experiments/save_redirect/*`. B touches them through exactly one seam entry,
  `take_save_write_bypass`.

That line is drawn here so the `feature/save-game-flow` epic (bd `er-effects-rs-llui`, P0
gate `er-effects-rs-k85t`) can rebase against it cleanly.

### 2.6 What cannot be decoupled without more RE

1. **`save_swap_profile_table.rs` cannot move whole.** It straddles the boundary: the
   save-swap preview is B, the profile-renderer drive is product. Both halves read the same
   `SystemQuitSaveSwapState` in `loading_cover_save_slot.rs`. The split needs that ledger
   reshaped into a seam type; until then the file is split by line range, not cleanly.
2. **The `05_010` browse rows are shared state between A and B at runtime.**
   `save_picker_browse_stats_lines`, `save_picker_row_slot_info` and
   `save_picker_profile_list_builder_hook` all gate on
   `SAVE_PICKER_MODE_ACTIVE != 0 || missing_save_selection_pending()`. The second disjunct
   is a leftover from the era when the boot picker WAS the native window (the stale
   "STARTUP (TITLE) MISSING-SAVE PICKER" comment block at `save_picker_menu.rs:911-922`
   documents that design). Today the boot picker is DLL-drawn and stages no ProfileSummary
   rows -- but the branch is still live if a user opens the native Load Game list during the
   boot hold. **Do not delete it on inspection alone**; prove reachability first, then
   either delete it (A and B become fully independent at runtime) or keep it and route it
   through a seam.
3. **`SAVE_PICKER_MODE_ACTIVE`, `SAVE_PICKER_DEST_MODE`, and 57 sibling counters** live in
   `er-telemetry`, which is already a shared crate -- no work needed, both crates just
   depend on it. 59 `SAVE_PICKER_*`, 148 `SYSTEM_QUIT_*`, 41 `SAVE_FLOW_*`, 28
   `SAVE_DEST_*`, 3 `MISSING_SAVE*`.

---

## 3. Answers from the code to the four boundary questions

### Q1 -- who owns the OS dialog + the dim overlay?

**User-decided (2026-07-30): OS dialog -> A, dim overlay -> B, dependency B -> A one way.**

What the code shows, as supporting evidence:

* On `main` the OS dialog has **three** callers, all reached through
  `open_picker_for_intent` (`save_picker_surface.rs:159`): `PickerOpenRequest::LoadSource
  { action_obj }` from the Load Character from File row (ex-Load Save Profiles), `PickerOpenRequest::SaveDestination
  { system_dialog }` from the Save Game flow, and -- since PR #109 --
  `PickerOpenRequest::MissingSaveBoot` from the boot flow (`save_picker_boot.rs:222`,
  dispatched at `save_picker_surface.rs:170`/`:179`). The boot flow no longer only draws
  its own overlay: under `os_native_save_picker` it opens the OS dialog like the other two.
  So A owns the mechanism by decision AND is already its own caller.
* `picker_dim_arm` now has exactly **one** call site -- the product-side
  `picker_dim_cover_factory` in `startup_hooks/save_picker/save_picker_os_dialog.rs`.
  That factory is passed only by the two System>Quit entry points. The boot flow passes
  `er_save_picker::os_dialog::no_picker_cover`. The cover is therefore still the caller's
  decision after S5 -- see SS5.3.
* The dialog file's own doc (rule H3) says it "reads no game pointers, calls no game
  function, and dereferences nothing from `game_module_base()`". That is exactly why the
  mechanism half is portable and the entry-point half is not.

### Q2 -- how entangled is the Save Game button with `feature/save-game-flow`?

**Not entangled. The branch is stale; its work is already on `main`.**

* `origin/feature/save-game-flow` is 32 commits ahead / **95 behind** `origin/main` (80 when
  this was first measured; the gap only widens as main moves). Its
  payload was squash-merged on 2026-07-29 as `6ba6f44a` (+ `b1533624`, `98b839f1`).
  `git cherry` lists the commits as unique only because the squash changed patch-ids;
  **blob hashes prove otherwise** -- `save_flow_boxes.rs`, `save_dest_identity.rs`,
  `profile_rows_system_quit_menu.rs` and `layout_global_hooks.rs` are byte-identical
  between the two.
* Sibling branches `promote/save-flow`, `promote/system-quit-row-identity`,
  `promote/picker-rows`, `feature/os-native-picker`, `feat/picker-dim-overlay` are all
  0 commits ahead of main. `feature/system-quit-save-game` does not exist on the remote.
* Of the seven picker files the branch touches exactly **two** (`save_picker.rs`,
  `save_picker_menu.rs`); it never had `save_picker_os_dialog.rs`, `save_picker_surface.rs`
  or `save_picker_dim_overlay.rs`, which arrived on main afterwards.
* Where the two disagree, **main is ahead**: main replaced the branch's private
  `save_file_character_slots` with the public predicate trio `PickRejection` /
  `save_picker_extension_accepted` / `save_picker_accepts`, and split
  `system_quit_open_save_dest_picker` into OS-dialog and in-game variants.

* **Merging it would revert a different extraction.** The branch predates the
  `er-loading-portrait` split (PR #98) and still carries those sources under
  `crates/er-effects-rs/src/` -- a `git diff origin/main origin/feature/save-game-flow`
  shows them as renames BACK into the product
  (`crates/{er-loading-portrait/src => er-effects-rs/src/constants}/portrait_lookat.rs`,
  `.../stats_loading_text.rs`, `.../resource_readback.rs`, `.../portrait_semaphores.rs`)
  plus a 2346-line `gpu_readback/overlay_composite.rs` main no longer has. This, not the
  already-landed save-flow work, is the real hazard in merging it.

**Consequence:** main is the baseline; do not merge that branch into the refactor. If
anything on it is genuinely unlanded, cherry-pick the specific hunk.

### Q3 -- would the two standalone DLLs double-detour the same addresses?

**A cannot collide with anything. B collides with the product, and today nothing elects an
owner when the product is absent.**

* **A installs no game-address detour at all** (SS1.5). Its `WH_KEYBOARD_LL` hook is
  per-DLL. A is safe in every combination, unconditionally.
* **B contends on ~18 game addresses**, several shared with the product (`0xb0e180`
  continue-confirm; `0x67b200`/`0x67b290`, which the product hooks with a **bare**
  `MhHook::new` -- `er-reload-trace-dll` skips exactly those two for that reason,
  `UNION_SKIP_RVAS`). Two MinHook instances patching one address corrupt each other's
  trampolines.
* The union that solves this **already exists** in `crates/er-hook/src/lib.rs`
  (`register_union_hook`, the 96-slot dispatcher pool, `HOOK_REGISTRY`). The gap is
  ownership: the `#[no_mangle] er_effects_union_register` entry point lives ONLY in
  `crates/er-effects-rs/src/mh.rs:37`, deliberately ("keeping it in this crate ensures ONLY
  `er_effects_rs.dll` exports it"), and companions resolve it by the hard-coded module name
  `er_effects_rs.dll` (`er-reload-trace-dll/src/lib.rs:77`, `resolve_union_register` :898).
  In the user's "neither" case -- the two new DLLs and no product -- nothing exports it and
  no one owns the shared addresses. SS6 closes that.
* There is a **second**, distinct collision the union does not solve: if the product
  bundles `er-quit-menu` AND the user also lists `er_quit_menu_dll.dll`, the feature is
  installed twice -- two cloned row sets, two routers. That needs a feature-ownership
  election, not a hook election. Also SS6.

### Q4 -- what exactly triggers the missing-save picker today?

`enforce_save_override_or_abort()`, `save_redirect/path_hooks.rs:1123-1152`, called EARLY in
`DllMain` before any save IO. It sets `MISSING_SAVE_DIALOG_PENDING` when **all three** hold:

1. not telemetry-only mode (`save_override_telemetry_only()` is false);
2. no plausible active default save -- either a save file is configured, or
   `active_default_save_file()` returns `None` (no readable+writable
   `%APPDATA%/EldenRing/<steamid>/ER0000.{sl2,co2}` of at least
   `SAVE_OVERRIDE_MIN_PLAUSIBLE_BYTES`);
3. no usable configured source -- `save_override_redirect_source()` returns `None`
   (configured save missing, invalid, or read-only).

That latch, read through `missing_save_selection_pending()`, is the single arm/gate/disarm
signal for the whole boot picker: the save-data job is held with CONTINUE every frame
(`startup_modals_menu_cover.rs:541-567`, so the boot bar sticks at `SAVE_CHECK` and Present
keeps firing), `save_picker_overlay_active()` is `SAVE_PICKER_OVERLAY_ARMED && pending`, and
`complete_missing_save_selection_from_picker` (`path_hooks.rs:1180`) clears it after
validating the container (size floor + BND4 parse), activating the redirect and installing
the redirect hooks.

---

## 4. Hook-ownership election (SS6 referenced above)

### 4.1 Requirement

A user must be able to list either new DLL, both, or neither, freely combined with our other
DLLs or with none of them. Every combination works. Load order must not matter, or the
remaining constraint must be documented.

### 4.2 Where it lives

**In `er-hook`. No new crate.** The union implementation is already there; only the
exported entry point is in the wrong place.

### 4.3 Design

1. **Move the export into `er-hook`**, behind a macro each cdylib invokes exactly once
   (`er_hook::export_union_registrar!();`). The symbol name, C ABI and semantics of
   `er_effects_union_register(target, handler, *mut orig_slot) -> i32` are **unchanged**, so
   `er-reload-trace-dll` and `er-input-harness-dll` keep working untouched, product present
   or not. `er-effects-rs/src/mh.rs` keeps re-exporting for source compatibility.
2. **Elect one owner per process, first-loader-wins.** At `DllMain`, each of our cdylibs:
   a. creates/opens a named OS primitive (`CreateMutexW(L"Local\\ErEffectsHookUnionOwner")`);
      the creator (i.e. `GetLastError() != ERROR_ALREADY_EXISTS`) is the owner and calls
      `MH_Initialize()` exactly once;
   b. a non-owner **discovers the live owner by scanning loaded modules for the export**
      (`EnumProcessModules` + `GetProcAddress`), never by filename. This removes the
      hard-coded `er_effects_rs.dll` lookup and with it the "product must be listed FIRST"
      profile constraint documented at `scripts/me3-launch-lib.sh:115`.
3. **Feature-ownership election, one layer up.** Each extractable feature declares a name
   (`"save-picker-boot"`, `"quit-menu"`). Arming takes a named claim through the same
   primitive; a second claimant logs
   `feature 'quit-menu' already provided by <module>` and stays inert. This is what makes
   "product bundles B **and** the user lists `er_quit_menu_dll.dll`" safe rather than
   double-installed.
4. **Every B detour goes through `register_union_hook`.** No bare `MhHook::new` in the new
   crates -- a lint-style check in `scripts/` should enforce that for the two crates.

### 4.4 Failure modes

| Mode | Consequence | Mitigation |
|---|---|---|
| Two DLLs race in `DllMain` | Two MinHook instances | Named mutex creation is atomic; loser never calls `MH_Initialize` |
| Owner DLL is unloaded | Dangling trampolines | We never unload; assert it and log |
| A non-owner registers before the owner has mapped | Registration lost | Bounded retry, already the shape of `resolve_union_register` (60 x 50 ms) |
| Union slot pool exhausted | `MH_ERROR_MEMORY_ALLOC` | `MAX_UNION_SLOTS = 96`; B adds ~18, so the ceiling is fine, but the count must be asserted in a test |
| Product hooks `0x67b200`/`0x67b290` with a bare `MhHook` | B's union dispatcher on the same address makes the product's later create return `ALREADY_CREATED` and silently drops the product's reload hook | Convert those two product installs to `register_union_hook` **before** B installs them |

### 4.5 How it is tested

Host-side (in `er-hook`): chain ordering, idempotent re-registration of the same handler,
slot exhaustion, and the election's first-wins/second-loses decision, with the OS primitive
behind a trait so the decision logic is testable without Windows.

Runtime matrix -- every row must smoke, and the riskiest is called out:

| # | Profile contents | Risk |
|---|---|---|
| 1 | product only | baseline; must be unchanged |
| 2 | `er_save_picker_dll` only | low (A installs no game detour) |
| 3 | `er_quit_menu_dll` only | **elects the union owner with no product present -- the case that does not work today** |
| 4 | product + `er_save_picker_dll` | low |
| 5 | product + `er_quit_menu_dll` | **HIGHEST: the feature is bundled AND listed; proves the feature-ownership election, not just the hook election** |
| 6 | both new DLLs, no product | medium |
| 7 | product + both new + reload-trace + input-harness + telemetry | regression net for the whole election |
| 8 | row 7 with the product listed LAST | proves the load-order constraint is gone |

### 4.6 Related pre-existing defect -- bd `er-effects-rs-fe08`

The inventory does explain it. `install_title_scene_obj_proxy_named_child_bind_hook`
(`title_resources_stats_text.rs:512`) sets its `..._INSTALLED` latch only at line **549**,
after `MH_ApplyQueued()` succeeds -- but `MH_CreateHook` already succeeded at line 531. When
the enable/apply step returns anything but `MH_OK`, the hook IS created and the latch is
NOT set, so the next three calls re-enter and hit `MH_ERROR_ALREADY_CREATED`. That is
exactly the observed shape: the SAME detour address (`dll+0x370d0`) reported as both the
prior and the new registrant for `0x14074a2f0`, 3x per boot.

Durable fix: route it through `register_union_hook`, which is idempotent per
`(target, handler)` (`er-hook/src/lib.rs:123`). Not done in this phase -- it is
runtime-affecting and this phase runs no game. Recorded on the bd issue.

---

## 5. Landable slices

Each slice keeps `er-effects-rs` behaviorally identical when bundled, ends green on
`bash scripts/check.sh`, and is its own PR.

| # | Slice | Behavior risk | Gate |
|---|---|---|---|
| **S0** | **This PR.** Crate skeletons, workspace members, host-seam stubs, gate coverage (host tests + windows-target check + the feature matrix). Nothing depends on the new crates | **none** -- no product file is touched | `check.sh` |
| S1 | Hook-ownership election in `er-hook`: move the export behind a macro, module-scan discovery, named-mutex election, feature claims. Product keeps exporting the same symbol | none (same symbol, same ABI) | host tests in `er-hook`; runtime matrix rows 1, 7 |
| S2 | Delete the ~640 dead lines from SS1.4 | none by construction (zero callers) -- but audit each before deleting | `check.sh` + one product smoke |
| S3 | Move the row model + `SaveSlotInfo`/`parse_save_character_slots` + the three config keys -> `er-save-picker`. `er-effects-rs` re-exports under the old paths so no call site changes | none | ~960 lines of tests become host-runnable -- the biggest single win |
| S4 | Move the boot overlay -> `er-save-picker`, behind `arm_boot_picker()` | low | boot-with-no-save smoke |
| S5 | **This branch.** Split `save_picker_os_dialog.rs`: mechanism + tests -> `er-save-picker::os_dialog`; entry points stay in the product shim for now; the caller-supplied cover is the `PickerCoverFactory` seam (SS5.3) | **none** -- the caller-decides shape was already on main and is preserved across the crate seam | all three surfaces: boot, load, save-as |
| S6 | `er-save-picker-dll` becomes real + its standalone smoke script | none to the product | matrix rows 2, 4 |
| S7 | Move B's decision core -> `er-quit-menu`: `system_quit_row_identity.rs`, `save_dest_identity.rs`, `save_dest_commit.rs`, `save_flow_boxes.rs` (~3400 lines, ~745 of them tests) | low (near-pure, heavily tested) | `check.sh` |
| S8 | Move B's hooked surfaces -> `er-quit-menu`: `system_quit_dialog_handlers.rs`, `save_picker_menu.rs`, `save_picker_surface.rs`, `save_picker_dim_overlay.rs`, the OS entry points, `install_system_quit_duplicate_button_hook`. Every detour switches to `register_union_hook` | **high** -- this is the feature | full System>Quit smoke: all four rows |
| S9 | Split the partial files -> `er-quit-menu`: `system_quit_ownership_repro.rs`, `system_quit_repro_guards.rs`, `profile_rows_system_quit_menu.rs`, `system_quit_hooks.rs`, `save_swap_profile_table.rs` 1-417, `lifecycle.rs`'s `save_flow_tick`. Reshape the save-swap ledger seam (SS2.6) | high | same as S8 + a switch-load smoke |
| S10 | `er-quit-menu-dll` becomes real; feature-ownership election wired; the full coexistence matrix | high | matrix rows 3, 5, 6, 8 |

**Ordering constraints.** S1 before S8 (B needs the union to exist). S3 before S4/S5/S7
(everything needs the model). S5 strictly after `rsxi`. Convert the product's bare
`MhHook::new` on `0x67b200`/`0x67b290` before S8 (SS4.4).

### 5.3 The cover is already the caller's decision -- S5 preserves it, it does not change it

An earlier draft of this plan said `os_pick_validated` arms the dim unconditionally, so the
boot missing-save dialog was dimmed too, and listed un-dimming it as S5's one deliberate
behavior change. **That is no longer true and S5 has no behavior change in it.** PR #109
landed the caller-decides shape when it routed the boot flow through the OS dialog:

* the extracted `er-save-picker::os_dialog::os_pick_validated` takes a `PickerCoverFactory`;
* the two System>Quit entry points pass a product-side factory that arms the dim cover;
* the boot flow passes `no_picker_cover`, preserving the existing rule: no game thread is
  blocked at a missing-save boot, so Present keeps running and there is nothing frozen for a
  cover to explain.

Measured, not only read: a live boot-picker run on `4ae8c6d1`
(`product-continue-direct-20260730-124605`) recorded `oracle_save_picker_dim_arm_count = 0`
while the overlay's bring-up self-test passed (`dim_selftest = 1`) -- the mechanism was
healthy and simply was not asked to arm.

S5 carries that shape across the crate boundary unchanged: `PickerCoverFactory` is the
seam, the System>Quit product shim passes a factory that arms its dim, and A's boot flow
passes none. The cover must keep its two current properties: it drops BEFORE the dialog
claim, and it spans the WHOLE reopen loop rather than each individual dialog (so an
invalid pick does not flash the game back at full brightness between two dialogs).

**Reviewer note:** do not "restore" dimming to the boot dialog while executing S5. The boot
dialog being undimmed is current, intended, user-directed behavior.

---

## 6. Decisions that remain user-gated

**Later resolution (roadmap D3):** the product keeps bundling B as the `er-quit-menu` library
inside the single shipped `er_effects_rs.dll`. `er-quit-menu-dll` is an optional harness only and
is never a required native in the product ME3 profile. This supersedes open item 2 below.

Everything the user has already settled is folded in above and is **not** re-opened here:
OS dialog -> A; dim -> B; B -> A one way; the boot dialog is not dimmed; Save Game is in scope
and `main` is the baseline; every DLL combination must work; the OS-native surface is always
available.

Still open, and each blocks the slice named:

1. **Is the `missing_save_selection_pending()` branch in the `05_010` browse path still
   reachable?** (SS2.6 item 2.) If not, delete it and A/B become fully independent at
   runtime; if it is, it needs a seam. Blocks S8. Answerable offline by proving whether the
   native Load Game list can be opened during the boot hold -- but it changes user-visible
   behavior either way, so the user should confirm the intent.
2. **RESOLVED by roadmap D3:** the product keeps bundling B through its `er-quit-menu`
   library dependency. The standalone DLL is harness-only; a plain product profile lists only
   `er_effects_rs.dll`.
3. **Should the ~1950 lines of System>Quit diagnostics (SS1.4) stay, or be deleted?** They
   are agent-only and gated on the input-harness DLL's presence. Keeping them is free but
   leaves ~1300 lines of autopilot in the product; deleting them removes the ability to
   self-drive a System>Quit repro. Blocks S9.
