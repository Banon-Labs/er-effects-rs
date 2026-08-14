# `startup_hooks/` crate targets -- supporting analysis

> **Execution status moved:** use [`crate-extraction-execution-roadmap.md`](crate-extraction-execution-roadmap.md) for the current baseline, sequence, dependencies, gates, decisions, and completion criteria. This file preserves the original partition evidence. Its frozen totals and line coordinates must not drive edits without the roadmap's R1 reclassification.

**Analysis baseline: `f15cce1a` (main, 2026-08-02).** Every line number was measured against the
source at that commit, not inherited from an earlier plan. Produced by two parallel sweeps
(5 clusters + a 2-group gap-fill), each cluster adversarially re-verified against the source:
**76 claims refuted, 456 confirmed.** Where a verifier refuted a mapper, the verifier's correction
is what appears below.

**Re-measured against `877f1261` (main, 2026-08-13) -- see SS0.1. The directory grew 22% in eleven
days and the analysis was NOT re-run.** Destination assignments below are still the plan of record;
intra-file line offsets are only trustworthy for files SS0.1 marks unchanged.

**Re-checked at `f54e4041` (main, 2026-08-13, after #236): this directory is BYTE-IDENTICAL to
`877f1261`.** `git diff 877f1261..f54e4041 -- startup_hooks/` is empty and no commit in that range
touches it -- the six merged `experiments/` deletion slices (#229-#235) reached zero files here,
including the `profile_rows_system_quit_menu.rs` edit SS6 of the sibling plan warned about (it is
deferred to S4d, still open). **SS0.1 stands in full.**

**One SS0.1 number was wrong and is corrected here: the directory is 35 files / 25,525 lines, not
25,516.** `quit_menu/system_quit_hooks.rs` is **682**, not 673, at both commits -- so its DELETE row
had already shed **364** lines, not 373, and the two files together shed **1,250** of the planned
2,141, not 1,259. This is a re-measurement error in SS0.1, not drift; nothing moved.

**Scope:** at baseline, 32 files / 20,834 lines under
`crates/er-effects-rs/src/experiments/startup_hooks/`. At `877f1261`: **34 files / 25,319 lines**
in-directory, plus `startup_hooks.rs` (197) = **25,525** (SS0.1 said 25,516; corrected above). The rest of `experiments/` is covered by
`docs/plans/experiments-crate-targets.md`.

---

## 0.1 Drift since the analysis baseline (measured `877f1261`, 2026-08-13)

**+4,682 lines, +2 files, and none of it is classified.** Eleven days of save-picker / ProfileSelect
product work (PRs #197-series through #228) landed inside the directory this plan partitions. The
destination *assignments* survive -- that work is more quit-menu and more title code, not a new
concern -- but the **line accounting in SS2 and SS3 is baselined at `f15cce1a` and 4,682 lines are
unassigned**. Do not quote the SS2 totals as current.

**Two files exist that this plan never saw:**

| file | lines | note |
|---|---:|---|
| `quit_menu/profile_05_010_editor_runtime.rs` | 1,765 | ProfileSelect 05_010 live editor runtime. Unclassified |
| `quit_menu/save_picker_path_editor.rs` | 758 | Load Game path editor (#226). Unclassified |

**Files that MOVED -- every intra-file line number in SS3/SS5 for these is stale:**

| file | plan | now | delta | consequence |
|---|---:|---:|---:|---|
| `quit_menu/save_picker_menu.rs` | 1,096 | 2,895 | **+1,799** | 4-way split offsets void; file now 2.6x plan size |
| `loading_cover/title_resources_stats_text.rs` | 1,088 | 2,402 | **+1,314** | the `er-scaleform-hooks` 648-line row is void |
| `quit_menu/system_quit_repro_guards.rs` | 2,067 | 1,181 | **-886** | **the 903-line DELETE row largely LANDED** -- re-verify before re-deleting |
| `quit_menu/system_quit_hooks.rs` | 1,046 | **682** | **-364** | part of its 439-line DELETE row landed |
| `loading_cover/loading_cover_save_slot.rs` | 1,444 | 1,587 | +143 | SS7 dedupe line refs stale; the finding itself is unaffected |
| `quit_menu/profile_rows_system_quit_menu.rs` | 1,858 | 1,957 | +99 | **the line-511 cut in SS6.4 must be re-derived** |
| `quit_menu/save_swap_profile_table.rs` | 1,097 | 1,163 | +66 | 3-way split offsets stale |
| `loading_cover/profile_table_gfx_files.rs` | 810 | 898 | +88 | 4-way split offsets stale |
| `loading_cover/startup_modals_menu_cover.rs` | 1,150 | 1,075 | -75 | 5-way split offsets stale |
| `quit_menu/system_quit_ownership_repro.rs` | 1,478 | 1,407 | -71 | 5-way split offsets stale |
| `loading_cover/title_scaleform_msgbox.rs` | 935 | 868 | -67 | 3-way split offsets stale |
| `diagnostics/layout_global_hooks.rs` | 439 | 383 | -56 | 4-way split offsets stale |
| `quit_menu/system_quit_dialog_handlers.rs` | 1,452 | 1,459 | +7 | 96%-one-destination claim unaffected |
| `quit_menu/mod.rs` | 198 | 204 | +6 | -- |

**The other 18 files are byte-stable in line count**, so their rows -- including every whole-file
move in SS6.3 (`scaleform_descriptor_guard.rs`, `window_reconfig_observer.rs`,
`portrait_equip_oracle.rs`, `save_picker_boot.rs`, `save_dest_commit.rs`, `save_flow_boxes.rs`,
`system_quit_row_identity.rs`) -- carry forward unchanged.

**Consequence for sequencing: SS6's "deletions first" is partly done.** `system_quit_repro_guards.rs`
and `system_quit_hooks.rs` already shed 1,250 of the planned 2,141 dead lines. Re-run the caller
proofs before opening a deletion slice here; do not assume the SS5 table is still live.

---

## 0. Read this before using any number below

**A wrong dependency list was fed to the gap-fill sweep.** Its brief asserted
`er-title-flow` does NOT depend on `er-loading-portrait`, `er-save-loader` or `er-tpf`. That is
false; only "no `er-gfx`" was true. The gap-fill's verifier caught it, but its *mapper* had already
assigned blocks under the wrong constraint. **Consequence: `er-title-flow` is a CHEAPER destination
than the `loading_cover` rows assume, so those rows are conservative.** Re-check any
`loading_cover/*` row that was pushed to `STAY` or to a new crate "to avoid a dependency" before
acting on it.

Re-measured from the manifests at `f15cce1a`, authoritative:

```
er-hook              -> (none)
er-loading-bar       -> (none)
er-gfx               -> (none)
er-tpf               -> (none)
er-game-base         -> eldenring, fromsoftware-shared
er-save-loader       -> eldenring, er-game-base, er-safe-input
er-telemetry         -> eldenring, er-game-base, er-save-loader, fromsoftware-shared
er-save-picker       -> er-loading-bar, er-save-loader, er-telemetry
er-save-suppress     -> er-game-base, er-hook
er-save-redirect     -> er-hook, er-save-loader, er-telemetry
er-d3d12-compositor  -> er-hook, er-loading-bar
er-loading-portrait  -> eldenring, er-game-base, er-gfx, er-hook, er-telemetry, fromsoftware-shared
er-quit-menu         -> eldenring, er-game-base, er-hook, er-save-loader, er-save-picker, er-telemetry, fromsoftware-shared
er-title-flow        -> eldenring, er-game-base, er-hook, er-loading-portrait, er-save-loader,
                        er-telemetry, er-tpf, fromsoftware-shared
```

The two edges that decide most assignments:

* **`er-title-flow -> er-loading-portrait` exists.** So anything moved INTO `er-loading-portrait`
  must not need an `er-title-flow` symbol, or the cycle closes. This is the single most common
  blocker in the tables below.
* **`er-quit-menu` does NOT depend on `er-loading-portrait`.** That is what forces the portrait
  fields in `QuitMenuHost` to stay seams rather than becoming moves.

---

## 1. Bottom line

The cheap extractions already happened. What is left is **94% game-coupled**: 1,481 `unsafe` lines
and 128 hook-install sites against only 1,312 lines of test code (6%). The tested pure logic left in
PRs #180-#188; the detours stayed.

Four facts shape every slice:

1. **`er-quit-menu` is the dominant owner** -- 7,529 lines, more than a third of the directory.
2. **2,141 lines are dead.** Not "probably dead": each has a whole-repo caller search returning only
   its own definition.
3. **The directory names lie about ownership.** `quit_menu/profile_rows_system_quit_menu.rs` is
   mostly *title* code; `diagnostics/layout_global_hooks.rs` arms the entire quit-menu feature.
4. **Extraction needs no new infrastructure.** `er-hook` already exports `MhHook` and every
   destination crate already depends on it. `er-loading-portrait` ships 13 bare `MhHook::new` sites
   today, so the union-hook conversion is NOT a precondition -- it is only needed for the
   standalone-DLL coexistence matrix.

---

## 2. Totals by destination

**These totals are frozen at `f15cce1a` and do not sum to the directory's current size.** At
`877f1261` there are 4,682 more lines here than this table accounts for (SS0.1). Use it for
*relative* weight -- which destination dominates -- not as a current line budget.

| destination | lines | new crate? |
|---|---:|---|
| `er-quit-menu` | 7,529 | no |
| **STAY** (product arming + genuine diagnostics) | 4,118 | -- |
| `er-title-flow` | 2,399 | no |
| **DELETE** (proven dead) | 2,141 | -- |
| `NEW:er-scaleform-hooks` | 1,436 | **yes** |
| `er-loading-portrait` | 901 | no |
| `er-save-loader` (dedupe, not move) | 557 | no |
| `er-save-picker` | 476 | no |
| `NEW:er-boot-window` | 461 | **yes** |
| `er-telemetry` | 366 | no |
| `er-hook` | 50 | no |
| `NEW:er-save-picker::path_form` | 13 | module |

---

## 3. Per-file assignment -- all 32 analysed files (+ 2 unanalysed)

Paths relative to `crates/er-effects-rs/src/experiments/startup_hooks/`. **`plan` is the line count
the destination split was measured against (`f15cce1a`); `now` is `877f1261`.** Where they differ,
the destinations still hold but the line *quantities* -- and any offset derived from them -- do not.

| File | plan | now | Destination(s) | Splits |
|---|---:|---:|---|---:|
| `quit_menu/save_picker_menu.rs` | 1096 | **2895** | er-quit-menu 1017 / STAY 21 / DELETE 13 / NEW:er-save-picker::path_form 13 | 4 |
| `loading_cover/title_resources_stats_text.rs` | 1088 | **2402** | NEW:er-scaleform-hooks 648 / er-title-flow 320 / STAY 100 / DELETE 1 | 3 |
| `quit_menu/profile_rows_system_quit_menu.rs` | 1858 | **1957** | STAY 902 / er-quit-menu 875 / DELETE 51 | 3 |
| `quit_menu/profile_05_010_editor_runtime.rs` | -- | **1765** | **UNANALYSED** -- new since baseline | ? |
| `loading_cover/loading_cover_save_slot.rs` | 1444 | **1587** | er-save-loader 557 / er-loading-portrait 458 / er-quit-menu 208 / STAY 173 / er-telemetry 10 / DELETE 1 | 6 |
| `quit_menu/system_quit_dialog_handlers.rs` | 1452 | **1459** | er-quit-menu 1395 / er-save-picker 66 | 2 |
| `quit_menu/system_quit_ownership_repro.rs` | 1478 | **1407** | er-quit-menu 988 / er-telemetry 347 / DELETE 83 / er-loading-portrait 32 / STAY 7 | 5 |
| `quit_menu/save_dest_commit.rs` | 1243 | 1243 | er-quit-menu 1026 / DELETE 206 | 2 |
| `quit_menu/system_quit_repro_guards.rs` | 2067 | **1181** | DELETE 903 / STAY 452 / er-quit-menu 396 / er-title-flow 221 / er-loading-portrait 67 -- **DELETE row largely already landed** | 5 |
| `quit_menu/save_swap_profile_table.rs` | 1097 | **1163** | STAY 643 / er-quit-menu 367 / er-loading-portrait 73 | 3 |
| `loading_cover/startup_modals_menu_cover.rs` | 1150 | **1075** | er-title-flow 879 / STAY 185 / DELETE 52 / er-telemetry 9 | 5 |
| `loading_cover/profile_table_gfx_files.rs` | 810 | **898** | NEW:er-scaleform-hooks 653 / er-quit-menu 51 / er-loading-portrait 51 / DELETE 43 | 4 |
| `loading_cover/title_scaleform_msgbox.rs` | 935 | **868** | er-title-flow 769 / DELETE 106 / NEW:er-scaleform-hooks 41 | 3 |
| `quit_menu/save_picker_path_editor.rs` | -- | **758** | **UNANALYSED** -- new since baseline (#226) | ? |
| `quit_menu/system_quit_hooks.rs` | 1046 | **682** | DELETE 439 / STAY 339 / er-quit-menu 150 / er-title-flow 50 / er-hook 50 -- **part of DELETE row landed** | 5 |
| `quit_menu/save_flow_boxes.rs` | 655 | 655 | er-quit-menu 628 / DELETE 7 | 2 |
| `loading_cover/window_reconfig_observer.rs` | 471 | 471 | NEW:er-boot-window 461 | 1 |
| `save_picker/save_picker_boot.rs` | 469 | 469 | er-save-picker 387 / DELETE 64 / STAY 1 | 3 |
| `diagnostics/layout_global_hooks.rs` | 439 | **383** | er-title-flow 160 / er-quit-menu 112 / STAY 105 / DELETE 55 | 4 |
| `quit_menu/system_quit_row_identity.rs` | 289 | 289 | er-quit-menu 263 / DELETE 19 | 2 |
| `loading_cover/portrait_equip_oracle.rs` | 277 | 277 | er-loading-portrait 220 / DELETE 51 | 2 |
| `quit_menu/mod.rs` | 198 | **204** | STAY 196 | 1 |
| `loading_cover/mod.rs` | 189 | 189 | STAY 188 | 1 |
| `diagnostics/mod.rs` | 174 | 174 | STAY 172 | 1 |
| `save_picker/mod.rs` | 171 | 171 | STAY 169 | 1 |
| `diagnostics/dlc_roots_trace.rs` | 169 | 169 | STAY 162 | 1 |
| `diagnostics/msb_parse_trace.rs` | 139 | 139 | STAY 136 | 1 |
| `diagnostics/loadlist_wait_trace.rs` | 139 | 139 | STAY 135 | 1 |
| `save_picker/save_picker_surface.rs` | 122 | 122 | er-quit-menu 53 / STAY 32 / er-save-picker 23 / DELETE 10 | 4 |
| `loading_cover/scaleform_descriptor_guard.rs` | 95 | 95 | NEW:er-scaleform-hooks 94 | 1 |
| `save_picker/save_picker_os_dialog.rs` | 27 | 27 | DELETE 25 | 1 |
| `quit_menu/save_dest_identity.rs` | 7 | 7 | DELETE 5 | 1 |
| `quit_menu/save_picker_dim_overlay.rs` | 6 | 6 | DELETE 5 | 1 |
| `loading_cover/dlc_roots_self_heal.rs` | 2 | 2 | DELETE 2 | 1 |

---

## 4. The two new crates

### `er-scaleform-hooks` (1,436 lines)

The game's Scaleform/GFx interception surface, kept together because every member shares ONE
mechanism -- writing data/len/cursor on the native `MemoryFile` -- and nothing else:
`AcquireMenuResource` + file-open + resource-ctor observers, the three in-place movie swaps
(05_000 title strip, 05_010 stats panel, 02_040 quit4), the bind observer, and the descriptor-heap
null guard.

Sources: `title_resources_stats_text.rs` (648), `profile_table_gfx_files.rs` (653),
`title_scaleform_msgbox.rs` (41), `scaleform_descriptor_guard.rs` (94).

Deps: `er-hook`, `er-game-base`, `er-telemetry`, `er-gfx`, and **`er-title-flow`** (for
`OWNER_CTX_MIN/MAX_PLAUSIBLE_PTR`, `SCENE_OBJ_*`). That last edge is acyclic
(`er-scaleform-hooks -> er-title-flow -> er-loading-portrait`) but only if
`TITLE_SCENE_OBJ_PROXY_NAMED_CHILD_BIND_ORIG`/`_RVA` move to `er-title-flow` WITH their detour and
installer and become its public API.

**Open question the verifier raised and I have not settled: should `er-gfx` own the movie swaps
instead?** `er-gfx` (7,691 lines, zero deps) already owns `.gfx` parsing and `TagEdit`. Decide from
what it exports, not from the name.

### `er-boot-window` (461 lines)

`window_reconfig_observer.rs` whole. Zero semantic overlap with the title flow. Its only root
couplings are `crate::hooks::{own_window, safe_input_proc, create_absolute_hook}` and
`trace_first_game_caller_rva`.

**This is NOT diagnostics** -- an earlier plan said it was. `lifecycle.rs:2106` installs it
unconditionally, and lines 280-471 are a product fix (early final-geometry apply that removes
mid-boot black flashes). Only the observe-only detours at 60-209 are telemetry-only, and they ship.

---

## 5. The deletions (2,141 lines)

Every entry below has a whole-repo `python3` caller search returning only its own definition line.
The largest:

| lines | site | note |
|---:|---|---|
| 715 | `system_quit_repro_guards.rs:442-1156` | unreachable autopilot arms |
| 130 | `save_dest_commit.rs:897-1026` | tests duplicated into `er-quit-menu` |
| 109 | `system_quit_hooks.rs:694-802` | `install_system_quit_gaitem_deserialize_hook` |
| 83 | `system_quit_ownership_repro.rs:830-912` | `install_system_quit_menu_window_job_run_hook` |
| 80 | `system_quit_hooks.rs:833-912` | `install_system_quit_gameman_load_save_hook` |
| 76 + 76 | `system_quit_hooks.rs:482-557`, `:588-663` | gaitem finalize / lookup installers |
| 57 | `system_quit_repro_guards.rs:91-147` | tab-row fingerprints |
| 55 | `layout_global_hooks.rs:3-57` | `apply_system_quit_multislot_layout_patch` |
| 55 | `title_scaleform_msgbox.rs:70-124` | `load_memory_gfx_from_env` (unreachable: `let path = String::new();`) |
| 51 | `profile_rows_system_quit_menu.rs:151-201` | `install_title_custom_cover_run_hook` |
| 42 | `profile_table_gfx_files.rs:305-346` | `construct_title_scaleform_memory_file` |

Deleting `load_memory_gfx_from_env`'s body cascades: `TITLE_SCALEFORM_MEMORY_GFX` and
`TITLE_SCALEFORM_05_000_MEMORY_GFX` can never be `Some`, which kills
`construct_title_scaleform_memory_file`, which makes `TITLE_MINIMAL_MAGENTA_GFX` /
`TITLE_MINIMAL_MAGENTA_COUNTER_GFX` (`constants/stats_panel_text.rs:346,363`) unreachable -- and
those are exactly the large embedded byte arrays `AGENTS.md` calls out as a problem.

**The deletion slice is NOT free.** A verifier refuted the "pure deletion, zero risk" framing: it
must also re-home the `SQ_REPRO_PAUSED_AT_PROFILE_SELECT` read at `write_telemetry.rs:732`, delete
three now-orphaned gates, and handle `sq_repro_drive_wm_key`, which stays live via the `set_pad`
closure at `system_quit_repro_guards.rs:374`.

**Prove it, do not argue it -- but the proof rule below is CORRECTED.** An earlier version of
this section said "`.text` byte-identical proves deadness; if `.text` moves the code was reachable."
The second half is **wrong**, falsified while executing this slice (PR #192, bd
`er-effects-rs-4k75`): a deletion of never-codegen'd code reported `.text` MATERIAL while every
section size was unchanged and `.pdata` was **byte-identical** -- 11,468 differing bytes, essentially
all single-byte deltas in RIP-relative displacement operands from statics re-ordering inside `.data`.
Inserting **one comment line** on top of that deletion reproduces the *same* hash and the same
MATERIAL verdict, deterministically. So the hash alone cannot distinguish codegen from re-ordering.

Corrected rule:

| observation | verdict |
|---|---|
| `.text` byte-identical | deadness PROVEN; pure refactor |
| `.text` differs, but `.pdata` byte-identical AND every section size unchanged | static/CGU re-ordering, NOT codegen; still a pure refactor |
| `.pdata` changed OR any section size moved | real codegen change; needs a runtime run |

Cheaper independent corroboration: search the BASELINE DLL for each deleted function's distinctive
log string. `count=0` proves the code was never shipped; 1-before/0-after proves it was.

Build both sides in the SAME directory -- a sibling worktree differs in ~9% of `.text` at identical
section sizes, and the build path is absent from the binary so grepping for it cannot detect the
problem.

---

## 6. Ordering

1. **Deletions first.** Every later line-count is wrong until 2,141 dead lines are gone.
   **Status at `877f1261`: partly done.** `system_quit_repro_guards.rs` (-886) and
   `system_quit_hooks.rs` (-364) already shed 1,250 of those lines. Re-run the caller proofs in SS5
   before opening a deletion slice; several rows are already satisfied.
2. **`er-telemetry` counter moves** (366 lines) -- mechanical, unblocks the accounting.
3. **Whole-file moves** with no split: `scaleform_descriptor_guard.rs`, `window_reconfig_observer.rs`,
   `system_quit_dialog_handlers.rs` (2-way, 96% one destination).
4. **Single-cut splits** -- the cleanest is `profile_rows_system_quit_menu.rs` at **line 511**:
   1-511 are title hooks -> `er-title-flow`, 512+ are quit-menu. Independently confirmed by arming
   call sites: its title installers are all armed from `lifecycle.rs`.
   **The 511 is stale** -- the file grew 1,858 -> 1,957 at `877f1261`. The *cut* is still real (the
   title/quit-menu boundary is a structural fact, not a line number); re-derive the offset from the
   last title installer before slicing. Tracked as bd `er-effects-rs-5obc`.
5. **`loading_cover_save_slot.rs` 6-way split** -- unblocks three things at once, and its
   `er-save-loader` half is a **dedupe, not a move** (see SS7).
6. **The hook-heavy quit-menu surfaces** -- highest risk, do last.

The arming surface is the choke point every slice edits: **30 `install_*`/`apply_*` functions**
defined here are called from outside, 26 from `experiments/lifecycle.rs` (~:1740-2110) and 4 from
`lib_parts/dll_entry_parts/bootstrap.rs`.

Slice size, calibrated on PRs #180-#188: **2-8 files, ~100-350 net lines, one concern, titled
"Move X into Y crate"**. Split large `git mv` batches across several Bash calls -- the Cupcake OPA
guard crashes on long ones (bd `er-effects-rs-56zd`).

---

## 7. `loading_cover_save_slot.rs` -> `er-save-loader` is a DEDUPE

557 lines duplicate `er-save-loader/src/bnd4.rs`, and the duplication is exact, not approximate:

* 38 constants byte-identical in name AND value to `bnd4.rs:285-323`; a 39th
  (`SAVE_PGD_CHARACTER_NAME_BYTES` = `0x10*2`) equals `bnd4.rs:297` (`0x20`) after expansion.
* `bnd4.rs:531`'s opaque `slot_add_offset(&mut offset, 0x1d)` is exactly our named block at 707-715
  (`0x03+0x04+0x04+0x01+0x04+0x04+0x01+0x04 = 0x19`) plus the 4-byte in-game timer.
* Section walk verified line by line: ours 897-924 == `bnd4.rs:496-513` identically; ours 1012-1033
  == `bnd4.rs:514-533` with one benign re-decomposition (`0x01+0x44+4+4` = `1+0x40+3*4` = `0x4d`).

**The earlier claim that this duplicates `stats.rs` is wrong** -- `er-save-loader/src/stats.rs` is a
different locator (Rune-Level invariant scan).

Before deleting our copy, prove the two are behaviorally IDENTICAL, not merely similar. A dedupe
that silently changes slot scoring is a product regression wearing a refactor's clothes.

---

## 8. Verifier corrections worth carrying forward

| Claim | Correction |
|---|---|
| `er-title-flow` has no `er-loading-portrait`/`er-save-loader`/`er-tpf` dep | All three exist. Only "no `er-gfx`" is true. **This wrong list was in the gap-fill brief** -- see SS0 |
| `er-quit-menu` has no `er-hook`/`er-game-base` | Both exist. Only "no `er-loading-portrait`" is true |
| `system_quit_menu_window_job_run_hook` is a live detour | **Dead.** -> DELETE |
| `system_quit_menu_window_run_post` has 3 outside callers | Exactly **one**: `product_core_own_stepper.rs:506` |
| Five `QuitMenuHost` portrait fields are seams to preserve | Dead scaffolding in `er-quit-menu/src/host.rs`; DELETE them |
| `er_title_flow::fnv1a64` re-exports `er-gfx`'s | Two independent FNV-1a64 implementations -- a separate duplication finding |
| ProfileSummary record layout duplicated twice | **Five and six** independent copies found |
| `save_picker_boot.rs:400-447` tests are byte-identical dupes | DELETE only AFTER porting two doc comments and a longer assertion message into `er-save-picker/src/boot.rs` |

---

## 9. Open decisions

1. **Keep or delete the System>Quit repro autopilot?** 715 lines at
   `system_quit_repro_guards.rs:442-1156` are unreachable today; deleting removes the ability to
   self-drive a System>Quit repro. `docs/plans/save-picker-crate-extraction.md` SS6.3 also defers
   this to a human. **Recommend: delete.** It is unreachable, and the input-harness DLL is the
   supported self-drive path.
2. **`er-scaleform-hooks` or fold into `er-gfx`?** See SS4. Blocks the 1,436-line Scaleform slice.
3. **Does the product keep BUNDLING `er-quit-menu`, or does it become listed-only?** Unchanged from
   the older plan; it decides whether the feature-ownership election is load-bearing.

---

## 10. Not covered here

* The rest of `experiments/` -- see `docs/plans/experiments-crate-targets.md`.
* Runtime behavior. Nothing in this plan was validated against a running game; it is a pure static
  measurement. Any slice that changes a detour needs its own runtime proof.
