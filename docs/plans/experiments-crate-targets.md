# experiments/ crate targets -- plan of record

**Analysis baseline: `b49dd5e2` (2026-08-02).** Every line number below was measured against that
commit, not inherited. The working tree was clean when measured.

**Re-measured and re-proved at `877f1261` (main, 2026-08-13) -- see SS0.1.** Unlike the
`startup_hooks/` half, this subtree barely moved: **30 of 40 files are unchanged to the line**, and
**all 26 caller-count proofs behind S1-S4 still hold**. The corrected line numbers are folded into
SS5 directly; the rest of the plan stands as written.

**Scope: `crates/er-effects-rs/src/experiments/**` EXCLUDING `startup_hooks/`.** startup_hooks is
owned by a separate concurrent analysis -- see SS8.

---

## 0.1 Re-verification at `877f1261` (2026-08-13)

**40 files / 27,847 lines** (was 27,106 -- **+741**, no files added or removed).

Ten files moved; only four materially:

| file | plan | now | delta | consequence |
|---|---:|---:|---:|---|
| `input_block.rs` | 1013 | 1443 | **+430** | `render_liveness_probe` moved 997 -> **1287-1303**. The orphaned doc block is still at **57-60** and `#[allow(dead_code)]` still at **61** -- unmoved |
| `save_redirect/path_hooks.rs` | 1741 | 1954 | **+213** | `wide_with_nul` 1319 -> **1510-1514**; `SAVE_CREATEFILEW_DIAG_ALL_BELOW` 564 -> **576-579**. SS8.5's "+81 past line 743" advice is superseded |
| `trace/menu_constructor_capture.rs` | 1176 | 1227 | +51 | whole-file move to er-menu-trace; no offsets to correct |
| `lifecycle.rs` | 2336 | 2374 | +38 | S10's 4-way split offsets need re-deriving |
| `own_load/loaders.rs` | 1140 | 1145 | +5 | S11 offsets need re-deriving |
| `continue_load/slot_resolution.rs` | 769 | 773 | +4 | -- |
| `profiler.rs` | 382 | 383 | +1 | whole-file move; unaffected |
| `input_trace.rs` | 925 | 924 | -1 | STAY; unaffected |

**Every S1-S4 target file is either unchanged or has been re-pinned above.** `boot_progress.rs`
(3,055), `submit.rs` (577), `live_loadgame_node.rs` (200), `product_continue.rs` (993),
`present_overlay.rs` (1,166), `menu_observation.rs` (855), `menu_trace_hooks.rs` (2,077),
`native_result_map_hooks.rs` (702), `bootstrap_drive.rs` (950), `load_steps.rs` (844),
`env_flags.rs` (727), `runtime_modes.rs` (157) are all **unchanged to the line**.

**Proofs re-run, not assumed:**

* **S1: all 12 symbols still return exactly 1 comment-stripped code hit** (their own definition)
  over a **605-file** corpus (was 565).
* **S2/S3: still zero-caller.** `composite_effect_selector_on_swapchain` 1 hit
  (`boot_progress.rs:2648`); `install_dxgi_factory_export_hook` 1 hit (`present_overlay.rs:415`);
  `factory2_hook` 2 hits, both inside the dead block (`:389` def, `:435` use).
* **S4: all 14 items still have exactly one caller, and every gate is still a literal `false`.**
  A fresh whole-file scan of `gating/` finds **45 hard-`false` gates** -- exactly the count SS7
  Decision 2 was costed against, so that decision's arithmetic is unchanged.

**Nothing in this plan has been executed.** No S-slice has landed; `submit.rs` and
`live_loadgame_node.rs` are still present at full size.

---

## 1. Bottom line

There are **40 files / 27,847 lines** here at `877f1261` (27,106 at the analysis baseline; not the
41 / 27,216 the task brief stated -- measured at
both `e930b7fc` = 27,019 and HEAD = 27,106; the delta is commit `b49dd5e2`, which added 87 lines to
`save_redirect/path_hooks.rs` and `own_load/drive.rs`). **Zero of these files use `include!`** -- I
scanned all of `experiments/**` and found 0 sites, so the brief's central worry is void and every
move here is a real file move, not an untangling job. The material content is six coherent features
-- the save-load drive (~3.5k), the boot/loading cover (~2.4k), the menu/save-dispatch trace (~3.5k),
the save-redirect Win32 hook layer (~1.9k), the save-flow commit machine (~1.5k), and the gate layer
(~892) -- sitting on top of ~4.4k lines of agent-only harness that ground rule 4 bars from any
shipped crate and ~1.7k lines that are provably dead. **The single biggest structural obstacle is
not size, it is that `experiments/` is already the workspace's de-facto shared bottom layer**: 21
gating functions plus `read_utf16_name_units`, `patch_3byte_stub`, `apply_xor_ret_stub`,
`game_main_window` and `create_continue_trace_hook` are *already* re-implemented as fn-pointer host
seams inside four extracted crates, so most of this directory cannot move *up* into a feature crate
without a cycle -- it has to move *down* into new crates below them, or stay.

---

## 2. Crate targets

Ordered by confidence. "Disputed" = an adversarial verifier refuted part of the proposing analysis
and I sided with the verifier.

| # | Target | New? | Lines | Charter | Deps needed | Host seam? | `-dll` shell? | Status |
|---|---|---|---|---|---|---|---|---|
| 1 | **`er-hook`** | no | 109 | Existing zero-dep MinHook crate; gains the raw code-patch primitives (validate byte -> VirtualProtect -> write -> restore -> flush icache) | **none** (raw `kernel32` externs, pattern already at `er-hook/src/lib.rs:181,245`) | **no** -- `set_hook_logger` already exists (`er-hook/src/lib.rs:27,32`); **deletes 2** er-title-flow seams | no | **Confident** |
| 2 | **`er-boot-profiler`** | **yes** | 382 | Boot-phase CPU/RIP sampler on its own OS thread -> NDJSON. Diagnostic-only, never on a product path | `er-game-base`, `windows` | 1 entry (`append_autoload_debug`) or use `er_game_base::log::append_line` and have none | no | **Confident** |
| 3 | **`er-game-base`** | no | 36 | Tier A; gains the UTF-16 save-name readers | **none** (needs its existing optional `game-types` feature) | **deletes 1** portrait seam (`read_utf16_name_units`) | no | **Confident**, rescoped |
| 4 | **`er-loading-portrait`** | no | ~40 | Already owns PlayerGameData/ProfileSummary layout; gains `char_fingerprint` | none | **deletes 0, adds 0** | no | **Confident**, heavily rescoped -- see below |
| 5 | **`er-gates`** | **yes** | 892 | The workspace's single gate/decision layer -- every product lever, diagnostic switch and module-presence probe that the product DLL *and* the feature crates must agree on | `er-telemetry`; `windows` avoidable via raw kernel32 externs | **+2** (`save_override_telemetry_only`, `missing_save_selection_pending`); **deletes 22** across 4 hosts | no | **Confident**, blocked on 4 const moves |
| 6 | **`er-menu-trace`** | **yes** | ~3,500 | Native menu / dialog / save-dispatch observation and pointer latching; publishes the live pointers the autoload machine consumes | `er-hook`, `er-telemetry`, `er-game-base[game-types]`, `er-save-loader`, `eldenring`, `fromsoftware-shared`, `windows` | ~10 new (incl. the 4-fn `crashlog::module_resolution` family); **deletes 3** er-title-flow seams | no | **Plausible** -- sizing corrected from 2,870 |
| 7 | **`er-boot-cover`** | **yes** | ~2,440 | Turn ER's RAM load semaphores into a phase/substep model, rasterize it, composite it onto the backbuffer until the world is ready | `er-loading-bar`, `er-telemetry`, `er-loading-portrait`, `er-save-picker`, `er-d3d12-compositor`, `er-game-base`, `eldenring`, `windows` | ~8 new | no | **Plausible** |
| 8 | **`er-loading-bar`** | no | ~160 | Existing zero-dep, `forbid(unsafe_code)` bar primitives; gains the exact `BarStyle` duplicate, text-scale, FNV hash, substep combinators, 2 CPU raster helpers | **none** | no | no | **Plausible** |
| 9 | **`er-save-redirect`** | no | ~1,900 | Existing crate finally owns the process-wide Win32/NT save hooks it says (`lib.rs:3-5`) it deferred | **+`er-game-base`, +`er-save-picker`** | **+8** new `er_save_redirect::host` | no | **Plausible**, line numbers stale |
| 10 | **`er-load-drive`** | **yes** | ~3,500 | The menu-free save-load drive: title step-fn detours, STAGE2/fullread/continue phase machines, the System->Quit switch-reload commit | `er-title-flow`, `er-telemetry`, `er-game-base`, `er-hook`, `er-save-loader`, `er-save-suppress`, `er-save-redirect`, `er-loading-portrait`, `er-save-picker`, `eldenring`, `windows` | **25-35** new | no | **Disputed** -- see SS7 |
| 11 | **`er-quit-menu`** | no | ~1,520 | Existing crate; gains the `save_flow_tick` stage machine its own `lib.rs:28-30` already claims | **+`er-save-suppress`** (26 fns, not 21) | uses existing | no | **Blocked on startup_hooks** |
| 12 | **`er-d3d12-compositor`** | no | 128 *(not 928)* | Existing crate; gains only the deduped `resolve_present_addrs` + `dummy_wndproc` | none | no | no | **Disputed** -- see SS7 |

### Verifier overrides I applied

| Claim | Proposer said | Verifier found | I sided with |
|---|---|---|---|
| Slice P1 (6 identity fns -> er-loading-portrait) | "cycle_risk: NONE, no new deps" | 4 of 6 read `OWN_STEPPER_SLOT_ZERO/NONE` (`er-title-flow/src/constants_moved.rs:773,770`); er-title-flow already deps er-loading-portrait (`Cargo.toml:25`) => **hard cycle** | **Verifier.** Verified myself: `slot_resolution.rs:428,465,467,482,505` use those consts; `:608,609` use two more er-title-flow-only offsets. **P1 rescoped to `char_fingerprint` + the 3 utf16 helpers only.** |
| `game_main_window` -> er-game-base | "ZERO new crate deps, NO cycle risk at all, only outbound call is one log line" | Block reads/writes 4 er-telemetry statics => `er-game-base -> er-telemetry -> er-game-base` | **Verifier.** Verified myself: `input_block.rs:127,128,129,146,150,151,153,155,163,168,204` touch `SQ_REPRO_BEST_AREA/BEST_HWND/ER_HWND/IS_FOREGROUND`, all `pub(crate) use er_telemetry::counters::` at `:72,100,102,174`; `er-telemetry/Cargo.toml:14,22` deps er-game-base twice. **Move dropped from the plan.** |
| er-d3d12-compositor absorbs ~800 lines of present mechanism | "already does the same job" | The mechanism reads the ER RVA `g_GxDrawContext` and stores 12 `PRESENT_FIND_*` oracles, violating that crate's own charter (`lib.rs:8-10`); deps would drag `eldenring` into `er-loading-bar-dll`, whose `lib.rs:3-8` exists to prove the opposite | **Verifier.** Target cut to the 128-line dedupe only. |
| er-menu-trace blocked by a cycle needing an er-game-base const lift first | "slice #1, unblocks everything" | er-title-flow reaches those symbols via fn-pointer seams (`host.rs:74,83,84`), not Cargo deps -- **no cycle exists** unless you also delete those seams | **Verifier.** The const lift is de-prioritised; it is an optimisation, not a precondition. |
| er-menu-trace = 2,870 lines | -- | Sums to ~3,500 from the proposer's own per-file allocation | **Verifier.** |
| er-save-suppress = 21 fns | -- | 26 distinct | **Verifier.** |
| gating already pays "28 seams" | -- | 21 distinct gating fns / 22 entries; the rest are save_redirect or input_block symbols | **Verifier.** |
| Delete `use crate::mh::{...}` at `present_overlay.rs:41` as a bonus | "no remaining MhHook consumer" | `MH_Initialize` (467, 471) and `MH_STATUS` (468) are outside the dead block | **Verifier.** Verified myself. Import **narrows**, does not vanish. |

---

## 3. Per-file assignment -- all 40 files

Module mechanism verified for every file: **all real `mod` + glob re-export; 0 `include!` sites in
the entire subtree.** `use super::*` is the actual coupling cost.

| File (under `experiments/`) | Lines (`877f1261`) | Mechanism | Destination | Splits? |
|---|---|---|---|---|
| `gpu_readback/boot_progress.rs` | 3055 | real `mod` (`gpu_readback.rs:66`), `use super::*` | er-boot-cover (~2,440) / er-loading-bar (~160) / **DELETE** (~454) | **yes, 4 ways** |
| `lifecycle.rs` | 2374 | real `mod` (`mod.rs:110`), `use super::*:6` | er-quit-menu (1,485) / **STAY** (748) / **DELETE** (92) | **yes, 4 ways** |
| `trace/menu_trace_hooks.rs` | 2077 | real `mod` (`trace.rs:7`), pasted 45-line header + `use super::*:45` | er-menu-trace (~1,046) / er-title-flow (~1,000) / **DELETE** (31) | **yes, 3 ways** |
| `save_redirect/path_hooks.rs` | 1954 | real `mod` (`save_redirect.rs:7`), near-copy header + `use super::*:62` | er-save-redirect (~1,660) / **STAY** (~75) / **DELETE** (9) | **yes, 3 ways** |
| `own_load/drive.rs` | 1703 | real `mod` (`own_load.rs:7`), explicit preamble + `use super::*` | er-load-drive (~1,040) / **STAY** (~662, rule-4 gated) | **yes** |
| `mod/product_core_own_stepper.rs` | 1328 | `#[path]` `mod` (`mod.rs:113-115`), `use super::*:1` | er-load-drive (634) / **STAY** (694, unreachable tail) | **yes, cuts one 776-line fn** |
| `trace/menu_constructor_capture.rs` | 1227 | real `mod` (`trace.rs:10`), `use super::*:1` | er-menu-trace (whole) | no |
| `present_overlay.rs` | 1166 | real `mod` (`mod.rs:61`), own imports + `use super::*:43` | STAY (mechanism) / er-d3d12-compositor (128) / er-hook (34) / **DELETE** (66) | **yes, 4 ways** |
| `own_load/loaders.rs` | 1145 | real `mod` (`own_load.rs:10`), `use super::*:1` **only** | er-load-drive (590) / **STAY** (550) | **yes -- live/dead alternate 5x** |
| `input_block.rs` | 1443 | real `mod` (`mod.rs:74`), own 55-line preamble + `use super::*:55` | **STAY** (996) / **DELETE** (17) | minimal |
| `continue_load/product_continue.rs` | 993 | real `mod` (`continue_load.rs:7`), explicit preamble + `use super::*:45` | er-load-drive (~435) / **STAY** (~449) / **DELETE** (62) | **yes, 3 ways** |
| `own_stepper/bootstrap_drive.rs` | 950 | real `mod` (`own_stepper.rs:7`), preamble + `use super::*:45` | er-load-drive (51) / **STAY** (851) / **DELETE** (48) | **yes -- 5% live** |
| `input_trace.rs` | 924 | real `mod` (`mod.rs:77`), `use super::*:21` | **STAY** (rule 4 + blocked on startup_hooks) | no |
| `menu_diag/menu_observation.rs` | 855 | real `mod` (`menu_diag.rs:7`), pasted 45-line header + `use super::*:45` | er-menu-trace (629) / **DELETE** (226) | **yes** |
| `own_stepper/load_steps.rs` | 844 | real `mod` (`own_stepper.rs:10`), `use super::*:1` **only** | er-load-drive (420) / **STAY** (388) / **DELETE** (36) | **yes, 3 ways** |
| `continue_load/slot_resolution.rs` | 773 | real `mod` (`continue_load.rs:10`), `use super::*:1` **only** | er-load-drive (~408) / er-loading-portrait (~40, rescoped) / **STAY** (~320) | **yes -- see override** |
| `gating/env_flags.rs` | 727 | real `mod` (`gating.rs:7`), `use super::*:45` | **er-gates** (720) / **DELETE** (7) | minimal |
| `trace/native_result_map_hooks.rs` | 702 | real `mod` (`trace.rs:13`), `use super::*:1` **only** | er-menu-trace (677) / **DELETE** (25) | minimal |
| `submit.rs` | 577 | real `mod` (`mod.rs:101`), pasted header + `use super::*:49` | **DELETE -- entire file** | n/a |
| `gpu_readback/gpu_draw_shared.rs` | 476 | real `mod` (`gpu_readback.rs:63`), `use super::*:1` | er-boot-cover (whole) | no |
| `gpu_frame_timing.rs` | 424 | real `mod` (`mod.rs:64`), own imports + `use super::*:54` | **STAY** (rule 4: control-file gated, device-removed the game) | no |
| `can_move_probe.rs` | 418 | real `mod` (`mod.rs:73`), **no `use super::*`** -- explicit imports only | **STAY** (rule 4) -- **the conversion template** | no |
| `profiler.rs` | 383 | real `mod` (`mod.rs:104`), real minimal imports + `use super::*:56` | **er-boot-profiler** (whole) | no |
| `save_redirect/file_ops.rs` | 352 | real `mod` (`save_redirect.rs:10`), `use super::*:1` | er-save-redirect (whole) -- **cannot move without path_hooks.rs** | no |
| `mem.rs` | 206 | real `mod` (`mod.rs:86`), preamble + `use super::*:49` | er-game-base (36) / **er-hook** (109) / **STAY** (61, the er-game-base re-export shim) | **yes, 3 ways** |
| `menu_diag/live_loadgame_node.rs` | 200 | real `mod` (`menu_diag.rs:10`), `use super::*:1` | **DELETE -- entire file** | n/a |
| `gating/runtime_modes.rs` | 157 | real `mod` (`gating.rs:10`), `use super::*:1` | **er-gates** (149) / **DELETE** (8) | minimal |
| `mod.rs` | 119 | root of the tree (`lib.rs:60` `mod experiments;`) | **STAY** -- 20 `mod` + 2 `#[path]`, 21 globs, 1,414 items | no |
| `mod/own_stepper_idx6_memory.rs` | 112 | `#[path]` `mod` (`mod.rs:117-119`), `use super::*:1` | er-load-drive (~102) / er-loading-portrait (10) | **yes** |
| `gpu_readback.rs` | 70 | real `mod` (`mod.rs:58`) | **STAY** until subtree moves, then delete | no |
| `gpu_readback/save_picker_overlay.rs` | 23 | real `mod` (`gpu_readback.rs:69`), fully qualified | **STAY** -- re-home as a sibling of `experiments/` | no |
| `trace.rs` | 14 | real `mod` (`mod.rs:52`) | **DELETE** when children move | no |
| `save_redirect.rs` | 11 | real `mod` (`mod.rs:49`) | **DELETE** when children move | no |
| `own_stepper.rs` | 11 | real `mod` (`mod.rs:92`) | **STAY** as re-export shim | no |
| `own_load.rs` | 11 | real `mod` (`mod.rs:80`) | **STAY** as re-export shim | no |
| `menu_diag.rs` | 11 | real `mod` (`mod.rs:83`) | **DELETE** when children move | no |
| `gating.rs` | 11 | real `mod` (`mod.rs:89`) | **STAY**, rewritten to `pub(crate) use er_gates::*;` | no |
| `continue_load.rs` | 11 | real `mod` (`mod.rs:98`) | **STAY** as re-export shim | no |
| `title.rs` | 7 | real `mod` (`mod.rs:95`) -- `pub(crate) use er_title_flow::*;` | **STAY** -- removing it is a constants-cluster job | no |
| `save_picker.rs` | 3 | real `mod` (`mod.rs:107`) -- `pub(crate) use er_save_picker::model::*;` | **STAY** | no |

**Totals:** ~27,106 lines -> ~13,900 to crates, ~1,700 deleted, ~11,500 STAY (harness + orchestrator
+ shims + rule-4 gated code).

---

## 4. Ordered slice list

Every slice is one PR sized like #180-#188 (2-8 files, ~100-350 net lines, one concern).

### Gate vocabulary

- **FP** = `python3 scripts/dll-code-fingerprint.py <before.dll> <after.dll>`. Its rule
  (`scripts/dll-code-fingerprint.py:5-9`): if `.text` is **byte-identical**, the change cannot have
  altered behavior and **needs no runtime run**. If `.text` moves, the slice needs a runtime proof.
- **CHK** = `bash scripts/check.sh` green (includes `check-rust-build.sh`, so the DLL is linked).
- **SZ** = `scripts/check-rust-file-sizes.py` -- warn > 900, **fail > 3200** (measured at
  `scripts/check-rust-file-sizes.py:13-14`).
- **RVA** = `scripts/check-rva-alias-drift.py` -- one game address, one hex literal.

### Ordering constraints, stated

1. **Deletions come first (S1-S4).** They are the only slices whose `.text` is provably unchanged,
   they shrink `boot_progress.rs` out of the 3,200 hard-fail danger zone (3,055 -> 2,601, headroom
   145 -> 599 lines), and every later motion PR then has ~1,700 fewer lines to reason about.
2. **A file split precedes its crate move.** `loaders.rs` alternates live/dead five times and
   `own_stepper_idx10` must be cut at its early return before either can move without dragging
   agent-only code into a shipped crate (ground rule 4).
3. **`file_ops.rs` and `path_hooks.rs` are one indivisible unit** -- 12 symbols cross between them.
4. **Anything touching `save_dest_commit.rs`, `save_flow_boxes.rs` or
   `system_quit_dialog_handlers.rs` waits for startup_hooks** (SS8).
5. **er-gates needs 4 const moves first** or it cycles back into er-title-flow / er-loading-portrait.
6. **The er-game-base const lift is NOT a precondition for er-menu-trace** -- verifier refuted that;
   it is an optimisation for the seam-deletion step only.

### The slices

| # | PR title | Files | Net | Gate | Depends on |
|---|---|---|---|---|---|
| **S1** | **Delete zero-caller code from experiments** | 6 | **-218** | CHK + **FP `.text`** | -- |
| **S2** | Delete the unreachable effect-selector HUD from boot_progress | 2 | -454 | CHK + FP + SZ | S1 |
| **S3** | Delete the dxgi factory-export hook from present_overlay | 2 | -67 | CHK + FP | S1 |
| **S4** | Delete the four hard-false submit levers and the live-loadgame node | 6 | -800 | CHK + FP | S1 |
| **S5** | **Move the code-patch primitives into er-hook** | 5 | ~-40 | CHK + FP | S1 |
| **S6** | Move the boot profiler into er-boot-profiler | 5 | ~+30 | CHK | S1 |
| **S7** | Move the PGD name offsets into er-game-base | 3 | ~+15 | CHK | -- |
| **S8** | Move the UTF-16 save-name readers into er-game-base | 4 | ~-20 | CHK | S7 |
| **S9** | Move char_fingerprint into the loading portrait crate | 3 | ~+10 | CHK | S8 |
| **S10** | Split lifecycle.rs into four modules | 5 | ~0 | CHK + **FP** + SZ | S4 |
| **S11** | Split loaders.rs live/dead and cut own_stepper_idx10 | 4 | ~0 | CHK + FP + SZ | S4 |
| **S12** | Move the shared dialog RVAs into er-game-base | 3 | ~+40 | CHK + RVA | -- |
| **S13** | Move the gate layer into er-gates *(new crate)* | 6 | ~+950 | CHK | S1, S12 |
| **S14-S17** | Delete the duplicated gate seams from er-title-flow / er-loading-portrait / er-quit-menu / er-save-picker (one crate per PR) | 2-3 ea | ~-60 ea | CHK | S13 |
| **S18** | Dedupe the present-address resolver into er-d3d12-compositor | 3 | -128 | CHK + FP | S3 |
| **S19** | Move the bar geometry and raster helpers into er-loading-bar | 3 | ~-160 | CHK + `cargo test -p er-loading-bar` | S2 |
| **S20** | Split boot_progress.rs into three modules | 4 | ~0 | CHK + FP + SZ | S2, S19 |
| **S21-S24** | Move the boot cover into er-boot-cover *(new crate, 4 slices)* | 3-5 ea | ~+600 ea | CHK + **runtime** | S20 |
| **S25** | Convert native_result_map_hooks to explicit imports | 1 | ~+30 | CHK + FP | S4 |
| **S26-S28** | Same for menu_constructor_capture / menu_trace_hooks / menu_observation | 1 ea | ~+30 ea | CHK + FP | S25 |
| **S29** | Move the world-res reload fix into er-title-flow | 3 | ~+1000 | CHK + **runtime** | S27 |
| **S30-S39** | Move the menu trace into er-menu-trace *(new crate, ~10 slices)* | 2-5 ea | ~350 ea | CHK + runtime on the latch slices | S29 |
| **S40** | Add the er_save_redirect::host seam | 3 | ~+90 | CHK | -- |
| **S41-S47** | Move the save-redirect hooks into er-save-redirect (7 slices) | 2-4 ea | ~250 ea | CHK + **runtime** | S40 |
| **S48+** | er-load-drive -- **gated on the SS7 decision** | -- | -- | -- | S10, S11 |
| **S49+** | Save-flow -> er-quit-menu -- **gated on startup_hooks** | -- | -- | -- | SS8 |

**Why S1 before S2/S3/S4:** S1 is the only deletion slice with zero cross-cluster reach. S2 also
touches `er-telemetry/src/counters.rs:1131-1141`; S3 touches `counters.rs:114`; S4 touches
`lib_parts/dll_entry_parts/task_registration.rs`, `lib_parts/runtime_helpers.rs`,
`mod/product_core_own_stepper.rs` and one startup_hooks file. Landing S1 first proves the
fingerprint workflow on the smallest possible blast radius.

**Why S5 is so early:** it is the cheapest net-negative slice in the whole plan. er-hook has **zero
`[dependencies]`** (only `build-dependencies = cc`, `Cargo.toml:13-14`) and already solved the
logging problem -- `pub type HookLogFn` at `er-hook/src/lib.rs:27` and `pub fn set_hook_logger` at
`:32` -- so the 5 `append_autoload_debug` calls become hook-logger calls with **no new seam**, and
the move **deletes two** er-title-flow seam fields.

**Why S7 before S8 before S9:** `read_utf16_name_units` returns
`([u16; PGD_NAME_LEN_U16], usize)`, and `PGD_NAME_LEN_U16` is derived in
`er-loading-portrait/src/pgd_layout.rs:40` -- moving the function without the constant gives
`er-game-base -> er-loading-portrait -> er-game-base`. S7 moves the constant under er-game-base's
**existing** optional `game-types` feature (`er-game-base/Cargo.toml:19-25`), which er-telemetry and
the product already enable, so the cycle never forms.

---

## 5. Slice 1, fully specified

### PR title
`Delete zero-caller code from experiments`

### Why this one first
Fourteen items with a **verified zero-caller count**, spread across six files, all deletable without
touching any other cluster and without deleting a guarded call-site body. It is the only slice in
the plan whose correctness is *mechanically provable* rather than argued.

### Proof search (re-run this to reproduce)

```bash
python3 -c "
import re,glob
roots=[p for p in glob.glob('**/*.rs',recursive=True) if not p.startswith(('target/','.worktrees/','.claude/'))]
print('corpus files:',len(roots))
for s in ['own_stepper_selffire_enabled','title_registrar_advance_gate_enabled','render_liveness_probe',
          'wide_with_nul','SAVE_CREATEFILEW_DIAG_ALL_BELOW','boot_bg_image_rgba_clone','BOOT_VIEW_GLYPH_W',
          'BOOT_VIEW_EPOCH_KIND_BOOT','BOOT_VIEW_HANDOFF_HOLD_BAIL_MS','product_continue_entry_action',
          'captured_continue_task_node','drive_product_continue_post_click_dispatchers']:
    pat=re.compile(r'\b'+s+r'\b'); hits=[]
    for f in roots:
        for i,l in enumerate(open(f,encoding='utf-8',errors='replace'),1):
            if pat.search(l.split('//')[0]): hits.append(f'{f}:{i}')
    print(f'{s}: {len(hits)} -> {hits}')
"
```

**Measured result at `b49dd5e2`: corpus 565 files; every symbol returns exactly 1 code hit -- its own
definition. Re-run at `877f1261`: corpus 605 files; still exactly 1 code hit each.** Comments are
stripped, so doc-comment mentions do not inflate the count; the scan is by bare name, so the
`pub(crate) use <child>::*` glob chain (`experiments/mod.rs` 21 globs) cannot hide a caller; and
there are **0 `include!` sites under `experiments/`**, so no file can be textually pasted somewhere a
name search would miss.

### Exact edits

**Line numbers below are pinned at `877f1261`.** The five that moved since the analysis baseline are
marked; each was re-derived by walking the item's own brace/attribute extent, not by adding an
offset.

| # | File | Delete lines | Item | Notes |
|---|---|---|---|---|
| 1 | `experiments/gating/runtime_modes.rs` | **103-110** | `own_stepper_selffire_enabled` + doc | 8 lines. Unmoved |
| 2 | `experiments/gating/env_flags.rs` | **256-262** | `title_registrar_advance_gate_enabled` + doc | 7 lines. Unmoved. Do **not** touch its sibling `title_accept_byte_gate_enabled` -- that is live at `er-title-flow/src/product_autoload_gates.rs:223` |
| 3 | `experiments/input_block.rs` | **1287-1303** | `render_liveness_probe` | 17 lines. **Moved from 997-1013** (the file grew +430). Doubly dead: first statement is `if !title_accept_enabled() { return; }` and that gate is a bare `false` at `gating/runtime_modes.rs:132`. Also delete the **orphaned doc block at `input_block.rs:57-60`** (unmoved), which describes this function but sits on the unrelated `BLOCK_INPUT_ACTIVE` re-export at `:66`. **Leave `#[allow(dead_code)]` at `:61` alone** -- it is a live attribute on that re-export, not stray |
| 4 | `experiments/save_redirect/path_hooks.rs` | **1510-1514** | `wide_with_nul` | 5 lines. **Moved 1319 -> 1510.** NUL termination now happens in `er_save_redirect::redirect_wide_roaming_eldenring_path` |
| 5 | `experiments/save_redirect/path_hooks.rs` | **576-579** | `SAVE_CREATEFILEW_DIAG_ALL_BELOW` + 3-line doc | 4 lines. **Moved 564 -> 576.** Superseded by `er_save_redirect::CreateFileSavePathDiag::should_capture_diag_log`. Do **not** confuse with `SAVE_REDIRECT_MODE_UNSET`, which is live |
| 6 | `experiments/gpu_readback/boot_progress.rs` | **1215-1217** | `boot_bg_image_rgba_clone` | 3 lines. Unmoved |
| 7 | `experiments/gpu_readback/boot_progress.rs` | **233** | `BOOT_VIEW_GLYPH_W` | 1 line. Unmoved |
| 8 | `experiments/gpu_readback/boot_progress.rs` | **86-87** | `BOOT_VIEW_EPOCH_KIND_BOOT` + doc | 2 lines. Unmoved. It documents the `0` default; the only explicit call passes `..._RELOAD` |
| 9 | `experiments/gpu_readback/boot_progress.rs` | **195-197** | `BOOT_VIEW_HANDOFF_HOLD_BAIL_MS` + doc | 3 lines. Unmoved. **File an issue** -- its documented 5s backstop is unimplemented; `BOOT_VIEW_EPOCH_COMPOSITE_CAP_MS` now covers it. Do not silently re-add the backstop inside a deletion PR |
| 10 | `experiments/continue_load/product_continue.rs` | **204-236** | `product_continue_entry_action` | 33 lines. Unmoved |
| 11 | `experiments/continue_load/product_continue.rs` | **237-251** | `captured_continue_task_node` | 15 lines. Unmoved |
| 12 | `experiments/continue_load/product_continue.rs` | **252-265** | `drive_product_continue_post_click_dispatchers` | 14 lines. Unmoved. This strands `SYNTH_MMS_OWNER`, `B80_DISPATCHER1_RVA`, `B80_DISPATCHER2_RVA` -- **leave them**, they are a follow-up constants slice |

Delete **bottom-up within each file** so earlier deletions do not shift later line numbers.

### Import changes

**None.** Every deleted item is a leaf. Two things to check after deleting, because the repo builds
with a global `-Awarnings` and will not tell you:

- `input_block.rs`: `render_liveness_probe` was the file's only user of `RENDER_FRAME_COUNT`,
  `RENDER_PROBE_INTERVAL`, `CSFEMAN_SINGLETON_RVA` and `TITLE_ACCEPT_LATCH_RVA`. Leave those
  declarations in place -- they are re-exports with other consumers.
- `title_accept_enabled` (`gating/runtime_modes.rs:132`) drops to exactly **one** remaining caller,
  `lib_parts/dll_entry_parts/task_registration.rs:284`. It is **not** yet deletable.

### New module header doc

None -- this slice creates no module.

### Verification

```bash
# Build BOTH sides in the SAME directory -- a sibling worktree differs in ~9% of .text at
# identical section sizes, which would make the gate meaningless.
SCRATCH=${SCRATCH:-$(mktemp -d)}

# 1. Build the BEFORE DLL and stash it.
cargo xwin build --release --target x86_64-pc-windows-msvc
cp -f target/x86_64-pc-windows-msvc/release/er_effects_rs.dll "$SCRATCH"/before.dll

# 2. Apply the 12 deletions.

# 3. Rebuild.
cargo xwin build --release --target x86_64-pc-windows-msvc

# 4. THE GATE. Exit 0 with .text identical => provably no behavior change, NO RUNTIME RUN REQUIRED.
python3 scripts/dll-code-fingerprint.py \
  "$SCRATCH"/before.dll \
  target/x86_64-pc-windows-msvc/release/er_effects_rs.dll

# 5. Full quality gate.
bash scripts/check.sh
```

**Interpreting step 4.** `.text` identical is the expected result: rustc already dead-code-eliminates
a zero-caller `pub(crate)` item, so removing the source cannot move a byte of machine code. That is
the proof, and per the script's own rule (`dll-code-fingerprint.py:5-9`) it discharges the runtime
requirement. If `.text` **differs**, stop -- one of the twelve had a reachable path the name scan
missed, and you must find it before merging rather than shipping the deletion.

### Commit

Per the repo's commit-timing rule, commit after the fingerprint comes back clean. Branch, do not
push to `main`; open a draft PR.

---

## 6. Deletions

**1,700 lines total across four slices.** Every proof below was re-run at `b49dd5e2` over a 565-file
corpus (`**/*.rs` minus `target/`, `.worktrees/`, `.claude/`) with `//` comments stripped.

### S1 -- zero-caller items (218 lines)
Twelve items, each returning **exactly 1 comment-stripped code hit = its own definition**. Full table
in SS5.

### S2 -- the in-world effect-selector HUD (454 lines)
`gpu_readback/boot_progress.rs:2614-3055` + `er-telemetry/src/counters.rs:1131-1141`.
**Two independent proofs.** (a) `composite_effect_selector_on_swapchain` has exactly 1 code hit --
its definition at `boot_progress.rs:2648`. (b) Even if called, the body is inert by construction:
`composite_effect_selector_inner:2702` is `let text = String::new();` and `:2703` returns on
`text.trim().is_empty()`. The live effect-selector HUD is a different implementation in a different
crate (`er-net-effects-dll/src/present_overlay.rs:71`). No `oracle_effect_selector*` field exists, so
`check-oracle-writers.py` stays green. **This is the slice that removes the 3,200 hard-fail exposure**
-- `boot_progress.rs` 3,055 -> 2,601.

### S3 -- the dxgi factory-export hook (67 lines)
`present_overlay.rs:383-448` + `er-telemetry/src/counters.rs:114`.
`install_dxgi_factory_export_hook` has 1 code hit (its definition at `:415`; the block including
`FACTORY2_ORIG` and `Factory2Fn` spans 383-448). `factory2_hook` appears only at `:389` (def) and
`:435` (inside the dead installer). Superseded by the GxDrawContext chain finder -- see the comment at
`present_overlay.rs:936-942`.
**Correction to the source analysis:** do **not** delete `use crate::mh::{...}` at `:41`.
`MH_Initialize` is used at `:467, :471` and `MH_STATUS` at `:468`, both outside the dead block.
The import **narrows to** `use crate::mh::{MH_Initialize, MH_STATUS};`.

### S4 -- hard-false levers (800 lines)
Each is one caller behind a gate whose entire body is the literal `false` (all gate bodies
re-measured at `b49dd5e2`). Delete the item **and** its `if <gate>()` block **and** the gate, in one PR.

| Item | Def | Sole caller | Gate | Gate body |
|---|---|---|---|---|
| `submit.rs` -- **entire file, 577 lines** | | | | |
| | `ingamestep_pump_tick` | `submit.rs:63` | `task_registration.rs:350` | `env_flags.rs:268` | `false` |
| | `submit_play_game_once` | `submit.rs:126` | `task_registration.rs:304` | `runtime_modes.rs:112` | `false` |
| | `ingameinit_drive_tick` | `submit.rs:394` | `task_registration.rs:337` | `runtime_modes.rs:116` | `false` |
| | `call_force_play_game_once` | `submit.rs:491` | `runtime_helpers.rs:39` | `env_flags.rs:244` | `false` |
| `live_loadgame_node.rs` -- **entire file, 200 lines** | | | | |
| | `locate_live_loadgame_node` | `:23` | `product_continue.rs:770` | `env_flags.rs:72` **and** `env_flags.rs:297` | `false`, `false` |
| | `fire_live_loadgame_node` | `:115` | `load_steps.rs:48` | `runtime_modes.rs:12` | `false` |
| `fire_titletop_load_entry` | `menu_observation.rs:340` | `product_core_own_stepper.rs:1186` | `env_flags.rs:618` | `false` |
| `functor_ptr_hits_factory` | `menu_observation.rs:239` | `menu_observation.rs:378` (inside the above) | transitive | -- |
| `cursor_offset_probe` | `menu_observation.rs:430` | `product_core_own_stepper.rs:1067,1069` | `env_flags.rs:533` | `false` |
| `menu_task_update_wrapper_hook` | `native_result_map_hooks.rs:169` | `menu_trace_hooks.rs:330` | `env_flags.rs:236` | `false` |
| `step3_init_rebuild_call_enabled` + branch | `menu_trace_hooks.rs:1477` | `:1627` | self | `false` |
| `worldres_coldbuild_probe` | `bootstrap_drive.rs:98` | `product_core_own_stepper.rs:658` | `env_flags.rs:627` | `false` |
| `invoke_menu_item_functor` | `load_steps.rs:79` | `product_core_own_stepper.rs:820` | not a call -- an `as usize` element of a discarded `let _ = (...)` tuple | -- |
| switch-harness autopilot, `lifecycle.rs:8-99` + `counters.rs:1468-1469` + `profile_rows_system_quit_menu.rs:1680-1682` | `:26,:32,:47` | `:48`, `:1372`, `:1680` | `lifecycle.rs:26` | `false` |

**Split S4 into 3-4 PRs** (submit.rs; live_loadgame_node + menu_observation; the trace pair; the
switch harness) to stay inside the #180-#188 size calibration. Three of these blocks make **native
state-changing calls** -- `submit_play_game_once`'s SetState/deserialize/streaming-enable,
`ingameinit_drive_tick`'s `IngameInit` on a leaked synthetic `this`, `fire_live_loadgame_node`'s
dialog-factory call and profile-slot pre-activate -- so this removes latent save-adjacent risk, not
just lines. The switch-harness block blocks the user's keyboard and injects a synthetic `DIK_ESCAPE`
(`lifecycle.rs:66-77`); it is rule-4 harness code with a dead gate.

**Cross-cluster warning:** the switch-harness deletion touches
`startup_hooks/quit_menu/profile_rows_system_quit_menu.rs:1680-1682` (3 lines). Coordinate with the
startup_hooks owner. `filename`, used at `:1684`, must survive.

**Gate for all four slices:** `dll-code-fingerprint.py`. For S1 expect `.text` identical. For S2-S4
`.text` will likely be identical too (the bodies are behind compile-visible `false`), and if it is,
that is a stronger safety proof than any runtime run could give.

---

## 7. Open decisions for the user

### Decision 1 -- Is `er-load-drive` worth building at all right now?

**Blocks:** S48+ (~3,500 lines, the largest single target).

**For:** 3,500 lines share one concern -- driving a save load to a rendered, movable world -- and the
inbound seam already exists: 13 `TitleFlowHost` fn-pointer fields at `er-title-flow/src/host.rs:85-98,110-111`,
installed at `bootstrap.rs:228-250`. After the move those 13 lines change from `crate::experiments::X`
to `er_load_drive::X` and nothing else on the title-flow side changes. No cycle: er-title-flow reaches
the drive through fn pointers, not calls.

**Against:** the verifier costed the outbound side and it is the dominant expense -- **25-35 new
`LoadDriveHost` fields**, not the "one genuinely new seam" the analysis claimed, because a new crate
cannot borrow er-title-flow's `pub(crate)` host wrappers. On top of that: two prerequisite in-place
splits (S10, S11) because live and dead code alternate five times inside `loaders.rs` and one
776-line function must be cut at its early return; `pab_node_update_detour` reaches into quit-menu
territory and is blocked on startup_hooks; and `own_stepper_stage2`'s CONFIRM branch fires the
save-writing `SetState5`, so this needs live runtime proof, not `cargo check`.

**Recommendation: defer.** Land S10 and S11 (the in-place splits) because they are net-zero, pure
motion, provable by fingerprint, and they improve `check-rust-file-sizes.py` on two files that
already warn. Then stop and re-cost. A 30-field host struct is the same monolith one layer down; if
the number does not fall below ~15 after S10/S11 clarify the real boundary, the honest answer is that
this code is not ready to leave the product yet.

### Decision 2 -- Does `er-gates` justify a new bottom-layer crate?

**Blocks:** S13-S17 (892 lines moved, 22 seam entries deleted).

**For:** 21 distinct gating functions are already duplicated as fn-pointer seams across four crates
(er-title-flow 15, er-loading-portrait 5, er-quit-menu 2 entries incl. 1 duplicate, er-save-picker's
being a save_redirect symbol). No existing crate can host them: er-title-flow -> er-loading-portrait
already exists (`er-title-flow/Cargo.toml:25`), so gating-in-er-title-flow cycles; er-game-base is
deliberately zero-external-dep and gating needs er-telemetry. The content is 892 lines of pure
`fn() -> bool` with no hooks and no game calls -- trivially reviewable. Net seam delta: **+2, -22**.

**Against:** 45 of the 82 gates are hard `false` and cannot be deleted from this cluster alone
(their guarded bodies live in `lib_parts/`, `lifecycle.rs`, `mod/`, `own_load/`, `startup_hooks/`),
so er-gates ships as a crate that is **55% dead levers on day one**. And it needs 4 prerequisite
const moves (`PROFILE_SELECT_LOAD_FLOW_ENABLED`, `TITLE_ANIM_SPEEDUP_MIN`/`_DEFAULT`,
`SYSTEM_QUIT_QUICKLOAD_PHASE_RETURN_TITLE_REQUESTED`, `OWN_STEPPER_CALL_INC`) plus repointing the
root crate's *separate* `pub(crate) use er_title_flow::X;` re-export layer inside `constants/` --
which the proposing analysis missed entirely (it attributed those bindings to `experiments/title.rs`;
they are actually at `constants/anti_debug.rs:204`, `constants/profile_render.rs:303`,
`constants/own_load_pump.rs:88`).

**Recommendation: do it, but delete the dead gates first.** Run S4 to completion, then re-count. If
the 45 hard-false gates fall to ~20, er-gates is an ~500-line crate of live product levers and is
clearly right. Building it at 892 lines with 45 dead entries just relocates the debt.

### Decision 3 -- `er-menu-trace`: new crate, or fold into er-title-flow?

**Blocks:** S30-S39 (~3,500 lines).

**For a new crate:** consumers span five different root-crate areas, not just title-flow --
`c30_writer_hook` is installed by `startup_hooks/diagnostics/layout_global_hooks.rs:251`;
`b80_mount_trace_summary` is read by `crashlog/veh_exit_hooks.rs:497,502`;
`task_node_update_rva` by `continue_load/product_continue.rs:242`; `MenuTraceSnapshot` by `hooks.rs`;
`functor_chain_hits_factory` by `own_stepper/load_steps.rs:321`. Folding it into er-title-flow would
force all five to depend on a title-flow crate for menu-pointer resolution.

**Against:** ~3,500 lines needing ~10 new seam fields including the whole 4-function
`crashlog::module_resolution` family (`trace_callers_summary` alone has 33 cluster call sites), and
it is genuinely live product code -- `install_continue_trace_hooks` installs ~47 detours on **every**
default product boot (`product_autoload_gates.rs:61-64` arms unconditionally unless a diagnostic
marker file is present), several of which latch state the autoload machine reads. That is a runtime
proof requirement on most slices, not a refactor.

**Recommendation: split the target, do the cheap half.** S29 -- moving the world-res/blockres reload
fix into er-title-flow -- is unambiguously correct under ground rule 1: `blockres_stalecap_fix_enabled`,
`map_mount_guard_flip_tick` and `run_ebl_mount_census` are `TitleFlowHost` fields
(`host.rs:100-102`) whose **only** external callers are `er-title-flow/src/title_tick_cover.rs:1668,
1630, 1670`. Every consumer is inside er-title-flow, so those are moves, not seam entries, and the
move deletes three fields. Do that. Hold the remaining ~2,500-line er-menu-trace crate behind the
same "re-cost after the seam count is real" rule as Decision 1.

---

## 8. What this plan does not cover

**`startup_hooks/` is owned by a separate concurrent analysis and is not planned here.** Measured at
`b49dd5e2`: **33 files / 20,999 lines** (the brief said 20,834). `startup_hooks.rs` uses real `mod`
declarations, not an `include!` shim, since PR #180. Three slices in this plan touch a startup_hooks
file and must be coordinated:

- **S4** deletes `profile_rows_system_quit_menu.rs:1680-1682` (3 lines).
- The **save-flow -> er-quit-menu** move (~1,485 lines, `lifecycle.rs:101-1369` + tests `2121-2336`)
  is hard-blocked: it calls 16 symbols in `startup_hooks/quit_menu/save_dest_commit.rs`,
  `save_flow_boxes.rs` and `system_quit_dialog_handlers.rs`. `er-quit-menu/src/lib.rs:28-30` already
  names `save_flow_tick` as planned contents, so the destination is agreed -- only the sequencing is
  open. It is the **last** slice of the quit-menu extraction, not the first.
- **er-menu-trace** must expose `c30_writer_hook` (installed at
  `startup_hooks/diagnostics/layout_global_hooks.rs:251`) as public API.

### Unresolved

1. **`gpu_frame_timing.rs` (424 lines) -- cannot be classified.** It is 100% control-file gated
   (`er-effects-gpu-frame-oracle.txt`) and its own doc records that the ECL piggyback device-removed
   the game ~28s in on native, so rule 4 says STAY. But its counters **are** read to emit oracles at
   `telemetry/runtime_oracles/write_game_module_oracles.rs:233,235`, so deleting it would trip
   `check-oracle-writers.py` in reverse unless the oracle emission goes too. That is a call for
   whoever owns the framerate-parity goal.

2. **`input_trace.rs` (925 lines) -- blocked, not decided.** Rule-4 gated (`ER_EFFECTS_INPUT_TRACE` or
   a marker file), *and* its 294-line semaphore reader depends on
   `startup_hooks/loading_cover/loading_cover_save_slot.rs`. Revisit after startup_hooks lands.

3. **`experiments/mod.rs`'s 21 glob re-exports.** 1,414 `pub(crate)` items flow through them into 61
   files that do `use super::*`. Removing them is a 1,414-symbol / 61-file explicit-path rewrite. It
   is **not** a prerequisite for any slice here -- each crate extraction deletes exactly one glob line.
   Do not attempt it as part of this work.

4. **`experiments/title.rs` (7 lines) sizing is unverified.** The claim that deleting it costs a
   260-symbol rewrite rests on a count I did not reproduce, and the verifier showed the reasoning was
   partly wrong: the root crate maintains a *second*, independent `pub(crate) use er_title_flow::X;`
   re-export layer inside `constants/` (418 such lines), so an unknown fraction of those 260 symbols
   already resolve without the glob. STAY is right; the cost figure is not load-bearing anywhere in
   this plan.

5. **`save_redirect/path_hooks.rs` line numbers are twice-shifted and the "+81" rule is dead.**
   The file went 1,741 -> 1,954 between `b49dd5e2` and `877f1261`, on top of the earlier
   `e930b7fc` -> `b49dd5e2` shift. Do **not** apply any fixed offset to a line number from either
   older analysis -- re-derive it. The two S1 items in this file are re-pinned in SS5. Its block list also
   has three overlapping ranges with contradictory destinations (470-573 swallows both 538-556 and
   564-567); re-cut that file into a true partition before slicing it.

6. **`.rs` files under `.claude/worktrees/` (3,237 of them) were excluded** from every proof search
   here, along with `target/` and `.worktrees/`. If a caller lives only in a worktree checkout, these
   proofs do not see it -- which is correct, since those are not part of the build.
