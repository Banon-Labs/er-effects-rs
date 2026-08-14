# Crate-extraction execution roadmap

**Current baseline:** `ddac122d` (`main`, 2026-08-13)  
**Parent planning PR:** [#193](https://github.com/Banon-Labs/er-effects-rs/pull/193)  
**Scope:** finish the ownership cleanup begun in `crates/er-effects-rs/src/experiments/**`, including `startup_hooks/**`, without changing product behavior merely to make extraction easier.

This is the current execution map. The two older analyses remain the evidence books:

- [`experiments-crate-targets.md`](experiments-crate-targets.md)
- [`startup-hooks-crate-targets.md`](startup-hooks-crate-targets.md)

Their source-level findings still matter, but their old line coordinates and status columns are not execution state. This file owns sequencing and completion.

## 1. Current measured state

| scope | files | lines |
|---|---:|---:|
| all `experiments/**` | 79 | 50,576 |
| excluding `startup_hooks/**` | 44 | 25,054 |
| `startup_hooks/**` plus `startup_hooks.rs` | 35 | 25,522 |

Largest remaining non-startup clusters:

| cluster | files | lines | current disposition |
|---|---:|---:|---|
| trace/menu diagnostics | 4 | 4,572 | split title-owned world reload first; defer separate trace crate until re-costed |
| own-load/product stepper | 6 | 4,124 | in-place split complete; separate crate still fails the interface-depth test |
| GPU readback/boot cover | 3 | 3,102 | loading-bar lane first, then boot-cover extraction |
| save redirect | 2 | 2,296 | re-baseline remaining detour bodies/path policy against the already-extracted install/queue owner; then shrink the existing callback interface |
| continue load | 2 | 1,446 | remains product-owned until load-drive decision |
| gates | 2 | 825 | 72 boolean gates, 35 hard-false; decision required before creating a crate |

The startup plan was originally measured before 4,682 lines of ProfileSelect/save-picker work landed. Its destination assignments are useful, but every split must be re-derived from current functions. Two previously unclassified files now account for 2,523 lines:

- `quit_menu/profile_05_010_editor_runtime.rs` -- 1,765
- `quit_menu/save_picker_path_editor.rs` -- 758

## 2. Completed foundation -- S1 through S12

| slice | result |
|---|---|
| S1 | zero-caller deletion, #229 |
| S2 | effect-selector HUD deletion, #230 |
| S3 | DXGI factory-export deletion, #231 |
| S4a-S4f | hard-false/dead-route deletion stack, #232/#234/#235/#237/#238/#239 |
| S5 | code-patch primitives to `er-hook`, #241 |
| S6 | `er-boot-profiler`, #247 |
| S7-S9 | PlayerGameData identity chain to `er-game-base`/`er-loading-portrait`, #251 |
| S10 | lifecycle in-place split, #252 |
| S11 | own-load/stepper in-place split, #253 |
| S12 | shared MessageBoxDialog RVAs to `er-game-base`, #254 |

These slices established the proof method used below: same-directory fingerprints, CGU1 for module graph changes, FP-DELTA for residual movement, and runtime proof only when emitted behavior changes.

## 3. Definition of complete

This roadmap is complete when all of the following are true:

1. Every current `experiments/**` source region has one explicit disposition: existing crate, approved new crate, product-owned `STAY`, or deleted.
2. No extracted crate calls back into the product through a broad host structure merely to recreate the old monolith. New interfaces must pass the deletion test and be meaningfully deep.
3. All duplicated runtime identities and pure algorithms have one owner.
4. Product arming/order stays in `er-effects-rs`; feature implementations live with their feature crate.
5. Agent-only diagnostics and harnesses stay product-owned or move to explicit companion DLLs; they do not silently ship inside reusable feature crates.
6. Every move passes its listed static gates. Every emitted runtime change passes feature-specific runtime proof with a freshly built DLL.
7. The final inventory and dependency graph are regenerated, and all roadmap Beads tasks are closed or explicitly rejected with evidence.
8. Known duplicate implementations are resolved explicitly: ProfileSummary layout and FNV-1a each have one owner, equivalence tests, and a scanner preventing recurrence.
9. The shipped autoload product remains one `er_effects_rs.dll` in one ME3 `[[natives]]` entry. Feature libraries may also have optional standalone harness DLLs, but required product behavior never depends on adding one.
10. Approved extracted library implementations leave the root product; provisional/deferred regions remain explicitly product-owned until their decision gate resolves. No line is moved merely to reduce this directory.

Completion does **not** require an empty `experiments/` directory. Product orchestration, attach-time arming, cross-feature policy, and genuinely product-only diagnostics belong there.

## 4. Execution sequence

The sequence below replaces the old S13+ ordering where current evidence has changed it. `R` numbers are stable work packages, not a promise of exactly 58 PRs.

R0 expanded the umbrella rows into **101 single-PR/decision nodes** in Beads. The machine DAG is authoritative there: every node has labels `pr193-roadmap` and `roadmap-<normalized-id>`, is parented under `er-effects-rs-q4oh`, and has explicit blocking edges. Existing exact-scope issues were reused for R0, R0A3, R2, R11, R15, and R31 instead of duplicated. The expansions are:

| roadmap umbrella | executable child IDs |
|---|---|
| R0a | R0A1 load identity gate; R0A2 loading/render oracle gate; R0A3 picker correctness |
| R0b | R0B1 FNV-1a owner; R0B2 ProfileSummary owner |
| R6a-R6d | R6A native-result map; R6B constructor capture; R6C trace hooks; R6D observation |
| R12b+ | R12B1-R12B5 editor transport, field application, geometry, path-window/caret, Scaleform primitives |
| R13b+ | R13B1-R13B4 path model, terminal hooks, job submission, lifecycle adapter |
| R17a+ | R17A-R17G picker model, boot surface, editor adapter, keyboard, cursor, mouse/scroll, list builder |
| R19a+ | R19A-R19F quit routing, finish/confirm, activation, ownership/pump, latches, repro/input guards |
| R22 | R22A-R22C title arming, resources, named-child/value hooks |
| R24a+ | R24A-R24H descriptor, resource, named-child, ProfileSelect, text-input, quit-menu, policy/message-box, status/spec hooks |
| R27-R30 | R27-R30 model, CPU raster, D3D12 adapter, runtime owner |
| R38-R54 | one node per numbered optional extraction, blocked by D1/D4/D5 |

The R0 proof is reproducible with `$HOME/.local/bin/bd list --all -n 0 --label pr193-roadmap --json` (101 unique roadmap labels) and `$HOME/.local/bin/bd dep cycles` (no cycles). A ticket or branch without a real qualifying PR does not count as plan translation.

### Phase 0 -- make the roadmap executable

| ID | Deliverable | Gate | Depends on |
|---|---|---|---|
| R0 | Regenerate the exact function/owner/caller/disposition ledger for **all 79 current files**, expand every umbrella into one issue per intended PR, create the Beads dependency graph, and link governing feature epics plus known runtime/oracle defects | zero unclassified regions; machine-readable DAG has no cycles; every approved R/D node has a Beads issue | -- |
| D2 | Decide Scaleform ownership: deepen `er-gfx` or create `er-scaleform-hooks` | compare interfaces/dependency graph/standalone consumers; pick the option with less exported coupling | R0 |
| D3 | **Resolved product constraint:** `er-quit-menu` remains a library linked into the single shipped `er_effects_rs.dll`; any `er-quit-menu-dll` is an optional harness only | product contract `autoload-dll-product-requirements` | -- |
| R0a | Repair or explicitly gate on open correctness/oracle defects in each feature family before moving it | relevant issue proof is green or the move is blocked | R0 |
| R0b | Centralize ProfileSummary layout and FNV-1a implementations with equivalence tests and duplicate scanners | host tests + scanner red/green selftest | R0 |

### Phase A -- refresh the map and take low-risk wins

| ID | PR-sized deliverable | Main files/owners | Gate | Depends on |
|---|---|---|---|---|
| R1 | Re-classify current function partitions after R0, with startup emphasis and S10/S11 split modules included | all 79 files; special attention to mixed startup files and current save-redirect ownership | inventory checker + caller map | R0 |
| R2 | Delete the verified startup remainder and dedupe duplicated tests | Beads `er-effects-rs-dc9k`; small sites plus test parity | CHK + test-equivalence proof + FP-CGU1 | R1 |
| R3 | Dedupe present-address resolution into `er-d3d12-compositor` | `present_overlay.rs` and compositor | CHK + FP | R0 |
| R4 | Finish pure loading-bar geometry/raster ownership | `gpu_readback/boot_progress.rs`, `er-loading-bar`; coordinate Beads `er-effects-rs-em21` | loading-bar tests + CHK + output equivalence | R0, R0a |
| R5 | Split `boot_progress.rs` by loading-bar, boot-cover, and product adapter ownership | GPU readback modules | CHK + FP-CGU1 + file-size gate | R4 |
| R6a-R6d | Convert each trace module from glob coupling to explicit imports in a separate PR | native result map, constructor capture, trace hooks, observation | CHK + FP | R0 |

**Exit:** all later cuts have current coordinates; easy duplicates and coupling noise are gone.

### Phase B -- startup whole-file and clean single-cut moves

| ID | PR-sized deliverable | Destination | Gate | Depends on |
|---|---|---|---|---|
| R7 | Move telemetry-owned startup counters/readers | `er-telemetry` | CHK + telemetry tests + FP | R1 |
| R8 | Move `scaleform_descriptor_guard.rs` whole | final Scaleform owner selected in D2 | CHK + FP + runtime hook/title proof | D2, R1 |
| R9a | Split observe-only window instrumentation from the final-geometry product fix | diagnostics `STAY` or explicit diagnostic harness; product fix isolated | CHK + FP-DELTA + runtime boot/window proof | R1 |
| R9b | Move the final-geometry fix only if its proposed interface passes the deletion/depth test | new `er-boot-window` or existing owner; otherwise explicit `STAY` | architecture review + runtime window/boot proof | R9a |
| R10 | Move dialog-handler implementation with only the picker adapter split out | `er-quit-menu` + `er-save-picker` | CHK + runtime System-Quit dialog proof | R1 |
| R11 | Split current `profile_rows_system_quit_menu.rs` at the function seam, not stale line 511 | title hooks - `er-title-flow`; quit implementation - `er-quit-menu`; product sampler stays | CHK + FP-DELTA + runtime title/quit proof; Beads `er-effects-rs-5obc` | R1, R2, R0a |
| R12a | Specify interfaces and owner partitions for `profile_05_010_editor_runtime.rs` | no movement; produces child PR ledger | reviewed partition + dependency proof | R1, R14, R0a |
| R12b+ | Move ProfileSelect editor families one concern per PR | owners selected by R12a; product arming stays | CHK + editor tests + live editor proof for every runtime family | R12a |
| R13a | Specify interfaces and owner partitions for `save_picker_path_editor.rs` | no movement; produces child PR ledger | reviewed partition + save-identity proof | R1, R14, R0a |
| R13b+ | Move path-editor model, adapter, and native hooks as separate PRs | `er-save-picker`/`er-quit-menu` per R13a | CHK + editor tests + picker runtime proof per runtime family | R13a |

**Exit:** directory names no longer conceal obvious whole-feature ownership.

### Phase C -- save parsing, picker, portrait, and quit-menu ownership

| ID | PR-sized deliverable | Destination | Gate | Depends on |
|---|---|---|---|---|
| R14 | Prove BND4/PGD walk identity on the local save corpus and repair the governing slot/portrait identity oracles | comparison harness + linked correctness issues | corpus equality; zero mismatches; requested/published/loaded identities agree | R1, R0a |
| R15 | Remove the duplicated 557-line slot walk and use `er-save-loader` | `loading_cover_save_slot.rs` - `er-save-loader`; Beads `er-effects-rs-qaba` | CHK + corpus equality + save/load runtime proof | R14 |
| R16 | Move loading-cover portrait ownership | `er-loading-portrait` | portrait tests + rendered-output oracle | R15, R0a |
| R17a+ | Move picker pure path/model, boot surface, editor adapter, software-keyboard hooks, cursor/mouse/scroll hooks, and list-builder hook as separate child PRs | `er-save-picker`; product arming stays | picker tests + runtime picker oracle on every hook/surface PR | R12b+, R13b+, R15, R0a |
| R18 | Move save destination/identity and dialog implementation | `er-quit-menu` | save-source tests + System-Quit-Load Profile runtime proof | R10, R11, R15 |
| R19a+ | Move remaining quit-menu hook families, one stable child ID and one hook family per PR | `er-quit-menu`, linked into the product DLL | CHK + runtime proof per family | R18, D3 |
| R20 | Move lifecycle save-flow implementation and tests last | `er-quit-menu`; product keeps scheduling/arming | CHK + boot load + same-character System-Quit-Load Profile to movable world | R19a+, R0a |

**Exit:** the dominant startup owner is extracted; save flow has one implementation owner.

### Phase D -- title, loading cover, and rendering

| ID | PR-sized deliverable | Destination | Gate | Depends on |
|---|---|---|---|---|
| R21 | Move world/block-resource reload fix | `er-title-flow` | CHK + same-character reload runtime proof | trace-hook explicit-import child R6c, R0a |
| R22 | Move remaining title-owned startup hooks in bounded families | `er-title-flow` | CHK + runtime title/autoload proof | R11, R21 |
| R23 | Establish the Scaleform hook owner selected by D2 | `er-gfx` **or** new `er-scaleform-hooks` | architecture review + dependency-cycle check | D2 |
| R24a+ | Move Scaleform descriptor/resource/message hook families as explicit child PRs | selected owner | CHK + GFX tests + runtime title/ProfileSelect proof on every hook/callback slice | R23 |
| R27-R30 | Extract boot-cover model, raster, adapter, then runtime owner | new `er-boot-cover`; product keeps arming | CHK + per-stage tests + rendered-output runtime proof for every hook/log/allocator/callback-crossing slice | R5, R16, R24a+ |
| R31 | Finish standalone loading-bar adapter and retire product duplicate | `er-loading-bar`/`er-loading-bar-dll` | bundled + standalone rendered-output proof | R4, R5, R27; Beads `er-effects-rs-em21` |

**Exit:** loading bar, portrait, and boot cover are separate modules with separate runtime adapters.

### Phase E -- save redirect

| ID | PR-sized deliverable | Destination | Gate | Depends on |
|---|---|---|---|---|
| R32 | Re-baseline the **existing** `er-save-redirect` interface and run the deletion/depth test | inventory remaining detour bodies/path policy in `path_hooks.rs`/`file_ops.rs`; do not add another host layer | interface review + tests | R1 |
| R33 | Move path planning and Win32 path helpers | `er-save-redirect` | host tests + FP | R32 |
| R34 | Move CreateFile/GetAttributes detour bodies | `er-save-redirect` | redirected-save runtime proof | R33 |
| R35 | Move CopyFile/FindFirst/SHGetFolderPath detour bodies | `er-save-redirect` | redirected-save runtime proof | R34 |
| R36 | Move NtCreateFile/free-space diagnostics or record product `STAY` | owner selected by R32/R1 | CHK + diagnostic/runtime proof | R35 |
| R37 | Shrink/delete existing detour/resolver callbacks and product wrappers after implementation ownership moves; installation/queueing already lives in `er-save-redirect` | product keeps source selection/arming | real vanilla + actual Seamless runtime matrix | R36 |

**Exit:** save interception has one implementation owner and supports both product save modes.

### Phase F -- decisions that must be evidence-driven

| ID | Decision/re-cost | Evidence required | Resulting work |
|---|---|---|---|
| D1 | Create `er-gates`, or keep gates product-owned? | recount after R2/R20/R22/R37; classify all 35 current hard-false gates and delete those whose guarded bodies are gone | if approved: R38 moves live gates, R39-R42 delete title/portrait/quit/picker host seams; otherwise close S13-S17 as rejected |
| D2 | **Decided in Phase 0 before execution** | dependency graph, interface comparison, standalone consumer needs | controls R8/R23/R24a+ |
| D3 | **Resolved:** required quit-menu behavior remains bundled as a library in the single product DLL | product contract | controls final arming interface for R19a+/R20 |
| D4 | Separate `er-menu-trace` crate? | re-count after R6/R21; number of required host fields and live consumers | if interface is deep enough: R43-R48 move latch families; otherwise leave tracing product-owned |
| D5 | Separate `er-load-drive` crate? | re-count after R20/R22; proposed interface must fall below the previous 25-35 callback estimate and hide rather than export state-machine internals | if viable: R49-R54; otherwise explicitly retain product ownership |

A decision may end in **reject extraction**. That is completion when evidence shows a new crate would only relocate coupling.

### Phase G -- optional approved extractions

Only execute these if Phase F approves them:

- **R38-R42:** central gate module and four consumer seam deletions.
- **R43-R48:** menu-trace hook/latch families, with runtime proof on every hook/log/allocator/callback-crossing slice.
- **R49-R54:** load-drive phases, native queue ownership, and product adapter; final proof is boot plus same-character repeat load to rendered, movable world.

### Phase H -- final convergence

| ID | Deliverable | Gate |
|---|---|---|
| R55a | Delete provably dead re-exports/adapters | caller proof + CHK + FP-DELTA |
| R55b | Remove live host callbacks/wiring exposed by completed moves | CHK + feature-specific runtime proof |
| R56 | Regenerate full ownership inventory, dependency graph, duplicate scan, gate count, and file-size report | zero unclassified regions; no dependency cycles or duplicate identities/algorithms |
| R57 | Run final product matrix | exact oracle matrix below |
| R58 | Close/reject every roadmap issue and update release/profile documentation | Beads and docs agree with the tree |

## 5. Cross-cutting proof rules

1. Build before/after DLLs in the same checkout.
2. Use CGU1 whenever a module is added or removed.
3. `MATERIAL` starts FP-DELTA; it does not prove behavior changed.
4. Byte-identical emitted code discharges runtime proof only when logging/callback initialization did not cross a new seam.
5. Hook, logging, allocator, or callback-seam changes require runtime proof even when fingerprints are localized.
6. Visual behavior needs direct rendered-output proof; hook counters are not product proof.
7. Save/load moves require the real product path and active default save, not the deprecated staged-save probe.
8. Every runtime candidate uses a freshly built DLL whose hash is recorded and verified after staging.
9. Every startup split begins from the current function map. Old line coordinates are evidence only.
10. All discovered remaining work is recorded in Beads, not appended as an untracked checklist.
11. Cross-cutting proof rules override weaker gates written in any row.

### Final R57 product oracle matrix

- Fresh explicit release builds with recorded before/after/staged hashes; no hidden `RUNTIME_*` or `ER_EFFECTS_*` product-enabling variables.
- The real `/home/banon/Elden/launch.sh` / ME3 product profile and active default save.
- Separate offline-vanilla `.sl2` and actual Seamless/ERSC `.co2` modes; `ersc.dll` is referenced from the game install and never bundled.
- One shipped `er_effects_rs.dll` product native entry; optional companion DLL profiles are tested separately and cannot be required by the product path.
- Agent-driven boot autoload and System-Quit-Load Profile repeat load of the same character to rendered **and movable** world.
- Requested/published/loaded character identity agreement, save-source/write integrity, zero `CS::MessageBoxDialog` builds, and no AV/assert/panic.
- Direct rendered-output oracles for portrait/cover/bar behavior; hook and draw counters alone do not pass.
- Coexistence proof for bundled features and applicable companion/other-mod combinations.

## 6. Critical paths

The machine-readable DAG created by R0 is authoritative. The current dominant chain is:

`R0 - R0a/R0b - R1 - R2 - R7 - R10 - R11 - R14 - R15 - R18 - R19a+ - R20 - R21 - R22 - R32-R37 - Phase F re-cost - R55a/R55b - R56-R58`

Loading/render work proceeds beside it through the existing `er-effects-rs-em21` lane:

`R0 - R0a/R0b - R1 - R14 - R15 - R4 - R5 - R16 - D2 - R23 - R24a+ - R27-R31`

This avoids the two worst failure modes in the old plan: creating shallow callback-heavy crates too early, and moving stale line ranges after eleven days of product development.

## Appendix A -- current 79-file disposition ledger

Generated against `ddac122d` by tracked-file line count. R0 must replace every **UNCLASSIFIED** entry with exact function-level partitions before extraction begins.

| Current file | Lines | Disposition at this baseline |
|---|---:|---|
| `can_move_probe.rs` | 418 | **STAY** (rule 4) -- **the conversion template** |
| `continue_load.rs` | 11 | **STAY** as re-export shim |
| `continue_load/product_continue.rs` | 698 | er-load-drive (~435) / **STAY** (~449) / **DELETE** (62) |
| `continue_load/slot_resolution.rs` | 748 | er-load-drive (~408) / er-loading-portrait (~40, rescoped) / **STAY** (~320) |
| `gating.rs` | 11 | **STAY** unless D1 approves `er-gates`; then rewrite as its re-export shim |
| `gating/env_flags.rs` | 684 | D1 decision: approved live subset - `er-gates`, rejected/dead subset - DELETE, otherwise **STAY** |
| `gating/runtime_modes.rs` | 141 | D1 decision: approved live subset - `er-gates`, rejected/dead subset - DELETE, otherwise **STAY** |
| `gpu_frame_timing.rs` | 424 | **STAY** (rule 4: control-file gated, device-removed the game) |
| `gpu_readback.rs` | 70 | **STAY** until subtree moves, then delete |
| `gpu_readback/boot_progress.rs` | 2,603 | er-boot-cover (~2,440) / er-loading-bar (~160) / **DELETE** (~454) |
| `gpu_readback/gpu_draw_shared.rs` | 476 | er-boot-cover (whole) |
| `gpu_readback/save_picker_overlay.rs` | 23 | **STAY** -- re-home as a sibling of `experiments/` |
| `input_block.rs` | 1,421 | **STAY** (996) / **DELETE** (17) |
| `input_trace.rs` | 924 | **STAY** (rule 4 + blocked on startup_hooks) |
| `lifecycle.rs` | 18 | er-quit-menu (1,485) / **STAY** (748) / **DELETE** (92) |
| `lifecycle/hook_installers.rs` | 133 | STAY product hook-install ordering (S10 split of lifecycle.rs) |
| `lifecycle/save_flow.rs` | 1,523 | er-quit-menu after startup ownership lands; tests move with implementation |
| `lifecycle/task_tick.rs` | 445 | STAY product recurring-task scheduling/orchestration |
| `lifecycle/title_visual_startup.rs` | 185 | STAY product startup arming/order; title implementation moves by family |
| `mem.rs` | 67 | er-game-base (36) / **er-hook** (109) / **STAY** (61, the er-game-base re-export shim) |
| `menu_diag.rs` | 8 | **DELETE** when children move |
| `menu_diag/menu_observation.rs` | 641 | er-menu-trace (629) / **DELETE** (226) |
| `mod.rs` | 113 | **STAY** -- 20 `mod` + 2 `#[path]`, 21 globs, 1,414 items |
| `mod/own_stepper_idx6_memory.rs` | 112 | er-load-drive (~102) / er-loading-portrait (10) |
| `mod/product_core_own_stepper.rs` | 627 | er-load-drive (634) / **STAY** (694, unreachable tail) |
| `mod/product_core_own_stepper/fallback_drives.rs` | 641 | STAY diagnostic/fallback tail; reassess only after load-drive decision |
| `own_load.rs` | 11 | **STAY** as re-export shim |
| `own_load/drive.rs` | 1,703 | er-load-drive (~1,040) / **STAY** (~662, rule-4 gated) |
| `own_load/loaders.rs` | 7 | er-load-drive (590) / **STAY** (550) |
| `own_load/loaders/load_drive.rs` | 656 | er-load-drive only if D5 interface-depth re-cost passes; otherwise STAY |
| `own_load/loaders/switch_reload.rs` | 490 | mixed load-drive/product reload adapter; classify in R0 before D5 |
| `own_stepper.rs` | 11 | **STAY** as re-export shim |
| `own_stepper/bootstrap_drive.rs` | 909 | er-load-drive (51) / **STAY** (851) / **DELETE** (48) |
| `own_stepper/load_steps.rs` | 741 | er-load-drive (420) / **STAY** (388) / **DELETE** (36) |
| `present_overlay.rs` | 1,099 | STAY (mechanism) / er-d3d12-compositor (128) / er-hook (34) / **DELETE** (66) |
| `save_picker.rs` | 3 | **STAY** |
| `save_redirect.rs` | 11 | **DELETE** when children move |
| `save_redirect/file_ops.rs` | 352 | er-save-redirect (whole) -- **cannot move without path_hooks.rs** |
| `save_redirect/path_hooks.rs` | 1,944 | er-save-redirect (~1,660) / **STAY** (~75) / **DELETE** (9) |
| `startup_hooks.rs` | 197 | STAY product startup module root/arming facade |
| `startup_hooks/diagnostics/dlc_roots_trace.rs` | 169 | STAY 162 |
| `startup_hooks/diagnostics/layout_global_hooks.rs` | 383 | er-title-flow 160 / er-quit-menu 112 / STAY 105 / DELETE 55 |
| `startup_hooks/diagnostics/loadlist_wait_trace.rs` | 139 | STAY 135 |
| `startup_hooks/diagnostics/mod.rs` | 174 | STAY 172 |
| `startup_hooks/diagnostics/msb_parse_trace.rs` | 139 | STAY 136 |
| `startup_hooks/loading_cover/dlc_roots_self_heal.rs` | 2 | DELETE 2 |
| `startup_hooks/loading_cover/loading_cover_save_slot.rs` | 1,587 | er-save-loader 557 / er-loading-portrait 458 / er-quit-menu 208 / STAY 173 / er-telemetry 10 / DELETE 1 |
| `startup_hooks/loading_cover/mod.rs` | 189 | STAY 188 |
| `startup_hooks/loading_cover/portrait_equip_oracle.rs` | 277 | er-loading-portrait 220 / DELETE 51 |
| `startup_hooks/loading_cover/profile_table_gfx_files.rs` | 898 | NEW:er-scaleform-hooks 653 / er-quit-menu 51 / er-loading-portrait 51 / DELETE 43 |
| `startup_hooks/loading_cover/scaleform_descriptor_guard.rs` | 95 | NEW:er-scaleform-hooks 94 |
| `startup_hooks/loading_cover/startup_modals_menu_cover.rs` | 1,075 | er-title-flow 879 / STAY 185 / DELETE 52 / er-telemetry 9 |
| `startup_hooks/loading_cover/title_resources_stats_text.rs` | 2,402 | NEW:er-scaleform-hooks 648 / er-title-flow 320 / STAY 100 / DELETE 1 |
| `startup_hooks/loading_cover/title_scaleform_msgbox.rs` | 868 | er-title-flow 769 / DELETE 106 / NEW:er-scaleform-hooks 41 |
| `startup_hooks/loading_cover/window_reconfig_observer.rs` | 471 | NEW:er-boot-window 461 |
| `startup_hooks/quit_menu/mod.rs` | 204 | STAY 196 |
| `startup_hooks/quit_menu/profile_05_010_editor_runtime.rs` | 1,765 | R12B1 editor control/status transport (56-183, 277-508); R12B2 ProfileSelect field text/size application (184-276, 1271-1463, 1555-1653); R12B3 drive-row/chrome/cursor geometry (509-970); R12B4 path-window/caret (972-1164); R12B5 Scaleform proxy/value/binder primitives (1165-1270, 1464-1554, 1654-1677); product arming **STAY** |
| `startup_hooks/quit_menu/profile_rows_system_quit_menu.rs` | 1,954 | STAY 902 / er-quit-menu 875 / DELETE 51 |
| `startup_hooks/quit_menu/save_dest_commit.rs` | 1,243 | er-quit-menu 1026 / DELETE 206 |
| `startup_hooks/quit_menu/save_dest_identity.rs` | 7 | DELETE 5 |
| `startup_hooks/quit_menu/save_flow_boxes.rs` | 655 | er-quit-menu 628 / DELETE 7 |
| `startup_hooks/quit_menu/save_picker_dim_overlay.rs` | 6 | DELETE 5 |
| `startup_hooks/quit_menu/save_picker_menu.rs` | 2,895 | er-quit-menu 1017 / STAY 21 / DELETE 13 / NEW:er-save-picker::path_form 13 |
| `startup_hooks/quit_menu/save_picker_path_editor.rs` | 758 | R13B1 pure outcome/text model (120-133, 312-340, 374-377, 579-621); R13B2 SoftwareKeyboard recipe/result hooks (1-119, 209-432); R13B3 native job construction/submission (433-578); R13B4 lifecycle/menu-pump adapter (134-208, 622-704); residual product scheduling **STAY** |
| `startup_hooks/quit_menu/save_swap_profile_table.rs` | 1,163 | STAY 643 / er-quit-menu 367 / er-loading-portrait 73 |
| `startup_hooks/quit_menu/system_quit_dialog_handlers.rs` | 1,459 | er-quit-menu 1395 / er-save-picker 66 |
| `startup_hooks/quit_menu/system_quit_hooks.rs` | 682 | DELETE 439 / STAY 339 / er-quit-menu 150 / er-title-flow 50 / er-hook 50 -- **part of DELETE row landed** |
| `startup_hooks/quit_menu/system_quit_ownership_repro.rs` | 1,407 | er-quit-menu 988 / er-telemetry 347 / DELETE 83 / er-loading-portrait 32 / STAY 7 |
| `startup_hooks/quit_menu/system_quit_repro_guards.rs` | 1,181 | DELETE 903 / STAY 452 / er-quit-menu 396 / er-title-flow 221 / er-loading-portrait 67 -- **DELETE row largely already landed** |
| `startup_hooks/quit_menu/system_quit_row_identity.rs` | 289 | er-quit-menu 263 / DELETE 19 |
| `startup_hooks/save_picker/mod.rs` | 171 | STAY 169 |
| `startup_hooks/save_picker/save_picker_boot.rs` | 469 | er-save-picker 387 / DELETE 64 / STAY 1 |
| `startup_hooks/save_picker/save_picker_os_dialog.rs` | 27 | DELETE 25 |
| `startup_hooks/save_picker/save_picker_surface.rs` | 122 | er-quit-menu 53 / STAY 32 / er-save-picker 23 / DELETE 10 |
| `title.rs` | 7 | **STAY** -- removing it is a constants-cluster job |
| `trace.rs` | 14 | **DELETE** when children move |
| `trace/menu_constructor_capture.rs` | 1,227 | er-menu-trace (whole) |
| `trace/menu_trace_hooks.rs` | 2,028 | er-menu-trace (~1,046) / er-title-flow (~1,000) / **DELETE** (31) |
| `trace/native_result_map_hooks.rs` | 676 | er-menu-trace (677) / **DELETE** (25) |
