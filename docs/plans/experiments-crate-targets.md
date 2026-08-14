# experiments/ crate targets -- supporting analysis

> **Execution status moved:** use [`crate-extraction-execution-roadmap.md`](crate-extraction-execution-roadmap.md) for the current baseline, sequence, dependencies, gates, decisions, and completion criteria. This file preserves the measured analysis and proof history behind that roadmap. Its old coordinates and `OPEN` labels are not current task state.

**Analysis baseline: `b49dd5e2` (2026-08-02).** Every line number below was measured against that
commit, not inherited. The working tree was clean when measured.

**Re-measured and re-proved at `877f1261` (main, 2026-08-13) -- see SS0.1.** Unlike the
`startup_hooks/` half, this subtree barely moved: **30 of 40 files are unchanged to the line**, and
**all 26 caller-count proofs behind S1-S4 still hold**. The corrected line numbers are folded into
SS5 directly; the rest of the plan stands as written.

**Re-baselined again at `f54e4041` (main, 2026-08-13, after #236) -- see SS0.2.** Six deletion
slices have now MERGED and the directory is **1,796 lines smaller than SS0.1 measured**. Every line
number in SS0.1 and SS5 for a file those slices touched is stale by construction; SS0.2 re-pins the
ones that still matter and states, item by item, exactly what is left of the S4 block.

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

### Execution status -- the whole deletion stack MERGED (2026-08-13)

Six slices, each proven by fingerprint or by a static gate reading, **none requiring a runtime run**:

| slice | PR | merge | net | fingerprint verdict |
|---|---|---|---:|---|
| **S1** delete zero-caller code | #229 | `516c9f1c` | -118 | `.text` + `.pdata` byte-identical; 24 `.rdata` bytes, all `panic::Location` line numbers shifted by exactly the lines deleted above them |
| **S2** delete the effect-selector HUD | #230 | `616326e7` | -454 | **whole DLL byte-identical** |
| **S3** delete the dxgi factory-export hook | #231 | `c3721b7f` | -67 | **whole DLL byte-identical** |
| **S4a** delete `submit.rs` + its 4 hard-false levers | #232 | `acb31bd1` | -608 | **whole DLL byte-identical at `codegen-units=1`** -- see FP-CGU1 in SS4 |
| **S4b** delete the live-dialog / native-profile-capture path | #234 | `9253a126` | -529 | **byte-identity impossible** -- this code WAS emitted; proof is static (three literal-`false` gates) |
| **S4c** delete the retired menu-task-update trace lever | #235 | `5814bf8a` | -47 | **regime B** -- address-taken detour, emitted but never installed |

Measured directory effect: **27,847 -> 26,051 lines, 40 -> 38 files** (SS0.2). The PR nets sum to
1,823 against a measured 1,796 because four of the six also deleted lines outside this directory
(`er-telemetry/src/counters.rs`, `lib_parts/`, one startup_hooks file).

### The three proof regimes -- which one a slice is in decides the gate

S4b is the boundary case that makes this worth stating, because the first four slices all landed in
regime A and it is easy to assume every deletion does:

| regime | what was deleted | fingerprint result | what proves it |
|---|---|---|---|
| **A -- never emitted** | zero-caller items; items behind a `false` gate that rustc folded away | **byte-identical** (`.text` at minimum; use FP-CGU1 if a module disappears) | the fingerprint itself. No runtime run |
| **B -- emitted but unreachable** | code the compiler DID emit, reachable only through a literal-`false` gate. **Includes any address-taken fn** (`x as *mut c_void` for a hook installer) -- taking the address defeats DCE even when the branch taking it never runs, which is exactly why S4c is regime B | **cannot be identical** -- `.gfids` moves and every later function shifts | a STATIC gate-body reading. A runtime run is a regression smoke, not an equivalence proof |
| **C -- reachable** | anything a product path can actually execute | irrelevant | a runtime run, per the repo's standing rules |

Telling A from B is not a judgement call -- build it and look. The trap is treating a regime-B
`MATERIAL` as regime C and reaching for the game; the gate cannot distinguish "this code changed"
from "this code left the image", and only the gate-body reading answers it.

Corrections these produced, folded into the sections below: the S1 headline `-218` never matched its
own itemisation (116); S2's counter range is only safe because three live `EFFECT_SELECTOR_*`
counters are declared outside it; S3's import narrows for one more reason than the plan gave
(`MH_ApplyQueued` was already unused in that file); and **S4a falsified the plan's prediction that
S2-S4 would all come back `.text`-identical at the default profile** -- it does not, and the fix is
FP-CGU1, not a runtime run.

Note S4b took only the `live_loadgame_node` half of the plan's S4b sketch, and S4c took only the
`menu_task_update_wrapper_hook` pair. **Six S4 items are still in the tree**; SS0.2 re-proves each of
them at `f54e4041` and re-cuts them into three slices (S4d/S4e/S4f) that are sized like #229-#235.

---

## 0.2 Re-baseline at `f54e4041` (2026-08-13, after #236)

**38 files / 26,051 lines** -- down **1,796** from SS0.1, with **two files gone**: `submit.rs` (577,
S4a) and `menu_diag/live_loadgame_node.rs` (200, S4b). Every other file in the subtree is unchanged
to the line except the fifteen below. (With S4d/S4e/S4f applied on top: **25,541**.)

| file | SS0.1 | now | delta | slice |
|---|---:|---:|---:|---|
| `gpu_readback/boot_progress.rs` | 2603 | **2603** | -452 | S1, S2 |
| `continue_load/product_continue.rs` | 698 | **698** | -295 | S1, S4b |
| `input_block.rs` | 1421 | **1421** | -22 | S1 |
| `save_redirect/path_hooks.rs` | 1944 | **1944** | -10 | S1 |
| `gating/runtime_modes.rs` | 141 | **141** | -16 | S1, S4a |
| `gating/env_flags.rs` | 706 | **706** | -21 | S1, S4a, S4b, S4c |
| `present_overlay.rs` | 1099 | **1099** | -67 | S3 |
| `own_stepper/load_steps.rs` | 780 | **780** | -64 | S4b |
| `mod/product_core_own_stepper.rs` | 1301 | **1301** | -27 | S4b |
| `trace/menu_trace_hooks.rs` | 2064 | **2064** | -13 | S4c |
| `trace/native_result_map_hooks.rs` | 676 | **676** | -26 | S4c |
| `mod.rs` | 116 | **116** | -3 | S4a, S4b |
| `menu_diag.rs` | 8 | **8** | -3 | S4b |
| `submit.rs` | 577 | **gone** | -577 | S4a |
| `menu_diag/live_loadgame_node.rs` | 200 | **gone** | -200 | S4b |

`boot_progress.rs` is now **2,603**, i.e. **597 lines of headroom** under the
`check-rust-file-sizes.py` 3,200 hard fail (was 145 before S2). Ordering constraint 1's stated
purpose is discharged.

### The six S4 items still in the tree -- proofs re-run at `f54e4041`

Corpus: **621 files** (`**/*.rs` minus `target/`, `.worktrees/`, `.claude/`), `//` comments
stripped. Every gate body below was read at its current line, not carried over.

| item | def | sole caller | gate | gate line now | body |
|---|---|---|---|---|---|
| `switch_harness_discovery_tick` (+ `_enabled`, `_note_menu_filename`) | `lifecycle.rs:47`, `:26`, `:32` | `lifecycle.rs:1388`; `profile_rows_system_quit_menu.rs:1755-1756` | `lifecycle.rs:26` | -- | `false` |
| `fire_titletop_load_entry` | `menu_observation.rs:340` | `product_core_own_stepper.rs:1159` | `legacy_menu_drive_enabled` | `env_flags.rs:597` | `false` |
| `functor_ptr_hits_factory` | `menu_observation.rs:239` | `menu_observation.rs:378` (inside the above) | transitive | -- | -- |
| `cursor_offset_probe` | `menu_observation.rs:430` | `product_core_own_stepper.rs:1062,1064` | `inject_nav_enabled` | `env_flags.rs:512` | `false` |
| `worldres_coldbuild_probe` | `bootstrap_drive.rs:98` | `product_core_own_stepper.rs:653` | `worldres_coldbuild_probe_enabled` | `env_flags.rs:606` | `false` |
| `step3_init_rebuild_call_enabled` + branch | `menu_trace_hooks.rs:1464` | `:1614` | self | -- | `false` |
| `invoke_menu_item_functor` | `load_steps.rs:15` | `product_core_own_stepper.rs:815` | not a call -- `as usize` inside a discarded `let _ = (...)` | -- | -- |

**Three gate line numbers moved and the old ones are now wrong**: `legacy_menu_drive_enabled`
618 -> **597**, `cursor_offset_probe`'s gate 533 -> **512** (and it is `inject_nav_enabled`, which
SS6 did not name), `worldres_coldbuild_probe_enabled` 627 -> **606**. `switch_reload_autopilot_enabled`
-- the name SS6 used for the switch-harness gate -- **does not exist anywhere in the corpus**; the real
gate is `lifecycle.rs:switch_harness_discovery_enabled`.

### Re-cut into three slices

| # | PR title | Files | Net | Predicted regime | **Measured** |
|---|---|---|---:|---|---|
| **S4d** #237 | Delete the switch-harness discovery probe | 3 | **-99** | B | **A** -- every section byte-identical at CGU1 |
| **S4e** #238 | Delete the disproven legacy menu-drive route | 5 | **-263** | B | **A-ish** -- ONE dead instruction left the image |
| **S4f** #239 | Delete the last three hard-false S4 levers | 7 | **-148** | B | **A-ish** -- one `format_args` argument left the image |

Stacked #237 <- #238 <- #239 off `f54e4041`. Directory after all three: **38 files / 25,541 lines**.

### What executing them taught the regime model

**The A/B prediction was wrong on all three, in the same direction: the plan over-predicted B.**
That matters because regime B is the one whose proof is a prose argument; regime A's proof is a hash.
Two rules come out of it:

1. **Where the gate sits decides the regime, not whether a gate exists.** S4d's gate is the FIRST
   statement of the entry point (`if !enabled() { return; }`) and its other call site is wrapped in
   `if enabled()`, so rustc folded the whole probe and the code was never emitted -- pure regime A.
   The SS0.1 row for B should be read narrowly: **address-taken** functions (S4f's
   `invoke_menu_item_functor`) and code reachable through a runtime value the compiler cannot fold.
2. **`MATERIAL` is not the end of the measurement, it is the start of one.** All three came back
   `MATERIAL`, and in all three the actual machine-code delta was recoverable exactly:

   | slice | how it was pinned down | delta |
   |---|---|---|
   | S4d | section hashes at CGU1 | **0 bytes of code.** 37 `.rdata` bytes = 25 `panic::Location` line numbers, each `-94`, matching the 94 lines deleted |
   | S4e | `.pdata` entry-by-entry: 6,266 entries both sides, exactly one length change (own_stepper `0x70a90`, `-3`), then a disassembly diff of that one function | **one instruction**: a dead `mov rcx,[rax]` whose value the next instruction reloads into `rax` |
   | S4f | same method: one length change (`0x75100`, `-48`) | **one `format_args` argument**: the `mov byte ptr [rbp+0xdf],0` materializing a constant `false`, its `lea`, its formatter slot; frame `0x168`->`0x148` |

   **The method, now the standard for any regime-B claim:** compare `.pdata` entry counts (proves no
   function was added or removed), find the entries whose `End-Begin` changed (that is the complete
   set of functions whose bodies moved), then disassemble just those with `rip`/`rbp`/`rsp`
   displacements and branch targets masked. Everything else in a `MATERIAL` verdict is relocation.
   Cross-check it with a string count in the BEFORE DLL -- `count=0` proves the deleted code was
   never shipped, which is a stronger statement than any hash comparison.

Corrections to SS0.2's own table, found while executing:

* `STEP3_INIT_REBUILD_FIRED`/`_COUNT` are **not** an "oracle cascade". Three references each: the
  re-export, the deleted body, the `counters.rs` declaration. No oracle reads them.
* `cursor_offset_probe`'s gate is `inject_nav_enabled`, which SS6 never named, and deleting the probe
  orphans five `CURSOR_PROBE_*` constants in `constants/system_quit.rs` that SS6 did not account for.
* Two deletions uncovered **orphaned doc blocks sitting above the wrong function** -- the same class
  of finding as S1's stray `render_liveness_probe` doc. Both were re-homed onto the live, previously
  undocumented function they actually describe (`functor_chain_hits_factory`,
  `dump_titletop_menu_entries`) rather than deleted along with the code they sat above. **Check for
  this on every deletion: a doc block above a deleted item may not belong to it.**

---

## 1. Bottom line

There are **38 files / 26,051 lines** here at `f54e4041` (27,847 at `877f1261`, 27,106 at the
analysis baseline; not the
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

| File (under `experiments/`) | Lines (`f54e4041`) | Mechanism | Destination | Splits? |
|---|---|---|---|---|
| `gpu_readback/boot_progress.rs` | 2603 | real `mod` (`gpu_readback.rs:66`), `use super::*` | er-boot-cover (~2,440) / er-loading-bar (~160) / **DELETE** (~454) | **yes, 4 ways** |
| `lifecycle.rs` | 2374 | real `mod` (`mod.rs:110`), `use super::*:6` | er-quit-menu (1,485) / **STAY** (748) / **DELETE** (92) | **yes, 4 ways** |
| `trace/menu_trace_hooks.rs` | 2064 | real `mod` (`trace.rs:7`), pasted 45-line header + `use super::*:45` | er-menu-trace (~1,046) / er-title-flow (~1,000) / **DELETE** (31) | **yes, 3 ways** |
| `save_redirect/path_hooks.rs` | 1944 | real `mod` (`save_redirect.rs:7`), near-copy header + `use super::*:62` | er-save-redirect (~1,660) / **STAY** (~75) / **DELETE** (9) | **yes, 3 ways** |
| `own_load/drive.rs` | 1703 | real `mod` (`own_load.rs:7`), explicit preamble + `use super::*` | er-load-drive (~1,040) / **STAY** (~662, rule-4 gated) | **yes** |
| `mod/product_core_own_stepper.rs` | 1301 | `#[path]` `mod` (`mod.rs:113-115`), `use super::*:1` | er-load-drive (634) / **STAY** (694, unreachable tail) | **yes, cuts one 776-line fn** |
| `trace/menu_constructor_capture.rs` | 1227 | real `mod` (`trace.rs:10`), `use super::*:1` | er-menu-trace (whole) | no |
| `present_overlay.rs` | 1099 | real `mod` (`mod.rs:61`), own imports + `use super::*:43` | STAY (mechanism) / er-d3d12-compositor (128) / er-hook (34) / **DELETE** (66) | **yes, 4 ways** |
| `own_load/loaders.rs` | 1145 | real `mod` (`own_load.rs:10`), `use super::*:1` **only** | er-load-drive (590) / **STAY** (550) | **yes -- live/dead alternate 5x** |
| `input_block.rs` | 1421 | real `mod` (`mod.rs:74`), own 55-line preamble + `use super::*:55` | **STAY** (996) / **DELETE** (17) | minimal |
| `continue_load/product_continue.rs` | 698 | real `mod` (`continue_load.rs:7`), explicit preamble + `use super::*:45` | er-load-drive (~435) / **STAY** (~449) / **DELETE** (62) | **yes, 3 ways** |
| `own_stepper/bootstrap_drive.rs` | 950 | real `mod` (`own_stepper.rs:7`), preamble + `use super::*:45` | er-load-drive (51) / **STAY** (851) / **DELETE** (48) | **yes -- 5% live** |
| `input_trace.rs` | 924 | real `mod` (`mod.rs:77`), `use super::*:21` | **STAY** (rule 4 + blocked on startup_hooks) | no |
| `menu_diag/menu_observation.rs` | 855 | real `mod` (`menu_diag.rs:7`), pasted 45-line header + `use super::*:45` | er-menu-trace (629) / **DELETE** (226) | **yes** |
| `own_stepper/load_steps.rs` | 780 | real `mod` (`own_stepper.rs:10`), `use super::*:1` **only** | er-load-drive (420) / **STAY** (388) / **DELETE** (36) | **yes, 3 ways** |
| `continue_load/slot_resolution.rs` | 773 | real `mod` (`continue_load.rs:10`), `use super::*:1` **only** | er-load-drive (~408) / er-loading-portrait (~40, rescoped) / **STAY** (~320) | **yes -- see override** |
| `gating/env_flags.rs` | 706 | real `mod` (`gating.rs:7`), `use super::*:45` | **er-gates** (720) / **DELETE** (7) | minimal |
| `trace/native_result_map_hooks.rs` | 676 | real `mod` (`trace.rs:13`), `use super::*:1` **only** | er-menu-trace (677) / **DELETE** (25) | minimal |
| `gpu_readback/gpu_draw_shared.rs` | 476 | real `mod` (`gpu_readback.rs:63`), `use super::*:1` | er-boot-cover (whole) | no |
| `gpu_frame_timing.rs` | 424 | real `mod` (`mod.rs:64`), own imports + `use super::*:54` | **STAY** (rule 4: control-file gated, device-removed the game) | no |
| `can_move_probe.rs` | 418 | real `mod` (`mod.rs:73`), **no `use super::*`** -- explicit imports only | **STAY** (rule 4) -- **the conversion template** | no |
| `profiler.rs` | 383 | real `mod` (`mod.rs:104`), real minimal imports + `use super::*:56` | **er-boot-profiler** (whole) | no |
| `save_redirect/file_ops.rs` | 352 | real `mod` (`save_redirect.rs:10`), `use super::*:1` | er-save-redirect (whole) -- **cannot move without path_hooks.rs** | no |
| `mem.rs` | 206 | real `mod` (`mod.rs:86`), preamble + `use super::*:49` | er-game-base (36) / **er-hook** (109) / **STAY** (61, the er-game-base re-export shim) | **yes, 3 ways** |
| `gating/runtime_modes.rs` | 141 | real `mod` (`gating.rs:10`), `use super::*:1` | **er-gates** (149) / **DELETE** (8) | minimal |
| `mod.rs` | 116 | root of the tree (`lib.rs:60` `mod experiments;`) | **STAY** -- 20 `mod` + 2 `#[path]`, 21 globs, 1,414 items | no |
| `mod/own_stepper_idx6_memory.rs` | 112 | `#[path]` `mod` (`mod.rs:117-119`), `use super::*:1` | er-load-drive (~102) / er-loading-portrait (10) | **yes** |
| `gpu_readback.rs` | 70 | real `mod` (`mod.rs:58`) | **STAY** until subtree moves, then delete | no |
| `gpu_readback/save_picker_overlay.rs` | 23 | real `mod` (`gpu_readback.rs:69`), fully qualified | **STAY** -- re-home as a sibling of `experiments/` | no |
| `trace.rs` | 14 | real `mod` (`mod.rs:52`) | **DELETE** when children move | no |
| `save_redirect.rs` | 11 | real `mod` (`mod.rs:49`) | **DELETE** when children move | no |
| `own_stepper.rs` | 11 | real `mod` (`mod.rs:92`) | **STAY** as re-export shim | no |
| `own_load.rs` | 11 | real `mod` (`mod.rs:80`) | **STAY** as re-export shim | no |
| `menu_diag.rs` | 8 | real `mod` (`mod.rs:83`) | **DELETE** when children move | no |
| `gating.rs` | 11 | real `mod` (`mod.rs:89`) | **STAY**, rewritten to `pub(crate) use er_gates::*;` | no |
| `continue_load.rs` | 11 | real `mod` (`mod.rs:98`) | **STAY** as re-export shim | no |
| `title.rs` | 7 | real `mod` (`mod.rs:95`) -- `pub(crate) use er_title_flow::*;` | **STAY** -- removing it is a constants-cluster job | no |
| `save_picker.rs` | 3 | real `mod` (`mod.rs:107`) -- `pub(crate) use er_save_picker::model::*;` | **STAY** | no |

**Totals, restated at `f54e4041`:** the ~1,700-line DELETE column is **1,796 lines already gone**
(SS0.2) with ~580 left in S4d/S4e/S4f. Of the 26,051 remaining, ~13,900 are targeted at crates and
~11,500 STAY (harness + orchestrator + shims + rule-4 gated code). The two DELETE-entire-file rows
(`submit.rs`, `menu_diag/live_loadgame_node.rs`) are struck from the table above because the files
no longer exist.

---

## 4. Ordered slice list

Every slice is one PR sized like #180-#188 (2-8 files, ~100-350 net lines, one concern).

### Gate vocabulary

- **FP** = `python3 scripts/dll-code-fingerprint.py <before.dll> <after.dll>`. Its rule
  (`scripts/dll-code-fingerprint.py:5-9`): if `.text` is **byte-identical**, the change cannot have
  altered behavior and **needs no runtime run**. If `.text` moves, the slice needs a runtime proof.
  **Build both sides in the SAME directory** -- a sibling worktree perturbs ~9% of `.text` on its
  own, which makes the gate meaningless.
- **FP-CGU1** = the same command, with **both** sides built under
  `CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1`. **Mandatory for any slice that adds or removes a
  module**; without it the gate returns a false `MATERIAL` and sends you to a runtime run you do not
  need. Measured on S4a (2026-08-13): deleting `submit.rs` at the default profile
  (`codegen-units=16`) reported **every** section DIFF -- `.text`, `.pdata`, `.data`, `.rdata`,
  `.reloc` -- with a common prefix of only `0x20` bytes, i.e. essentially all 2.1 MB of `.text`
  moved. Rebuilt at `codegen-units=1`, the two DLLs are **byte-identical** (exit 0, `NOT MATERIAL`);
  the only residual bytes are 36 in `.rdata` at the PE debug directory, an `RSDS` PDB GUID plus
  build timestamp, which the script already masks.

  **Mechanism:** removing a module re-partitions rustc's codegen units, relocating nearly every
  function in the crate. That is layout churn, not semantics. It also breaks the "section size
  moved" half of the SS5 rule table in a non-obvious direction -- a *pure deletion* was observed to
  GROW `.pdata` by 24, `.data` by 16 and `.reloc` by 76 bytes. Intra-file deletions that leave the
  module graph intact (S1-S3) do **not** need this; all three came back byte-identical at the
  default profile. This is a measurement-only setting -- do not change the shipped profile.
- **FP-DELTA** = what to do when FP-CGU1 still says `MATERIAL`. Do NOT conclude "runtime run" from the
  hash; localise the change instead. (1) Compare `.pdata` **entry counts** -- equal counts prove no
  function was added or removed. (2) Find the entries whose `End - Begin` changed; that is the
  complete set of functions whose bodies moved. (3) Disassemble only those, masking `rip`/`rbp`/`rsp`
  displacements and branch targets, and diff. Everything a `MATERIAL` verdict shows beyond that is
  relocation. Cross-check with a string count in the **BEFORE** DLL: `count=0` for a deleted
  function's distinctive log literal proves the code was never shipped, which no hash can say.
  Measured on S4e (one dead instruction) and S4f (one `format_args` argument) -- see SS0.2.
- **CHK** = `bash scripts/check.sh` green (includes `check-rust-build.sh`, so the DLL is linked).
- **SZ** = `scripts/check-rust-file-sizes.py` -- warn > 900, **fail > 3200** (measured at
  `scripts/check-rust-file-sizes.py:13-14`).
- **RVA** = `scripts/check-rva-alias-drift.py` -- one game address, one hex literal.

### Ordering constraints, stated

1. **Deletions come first (S1-S4).** **Discharged for S1-S4c as of `f54e4041`.** They were the only
   slices whose `.text` is provably unchanged, and they shrank `boot_progress.rs` out of the 3,200
   hard-fail danger zone (3,055 -> **2,603 measured**, headroom 145 -> **597**). 1,796 lines are gone;
   S4d/S4e/S4f carry the last ~580 (SS0.2). Every later motion PR now has that much less to reason
   about. Note the predicted 2,601 was 2 lines off the measured 2,603 -- the S2 range was itemised
   before S1 deleted 3 lines above it.
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

| # | PR title | Files | Net | Gate | Depends on | Status |
|---|---|---|---|---|---|---|
| **S1** | **Delete zero-caller code from experiments** | 6 | **-118** *(not -218)* | CHK + **FP `.text`** | -- | **LANDED #229** |
| **S2** | Delete the unreachable effect-selector HUD from boot_progress | 2 | -454 | CHK + FP + SZ | S1 | **LANDED #230** |
| **S3** | Delete the dxgi factory-export hook from present_overlay | 2 | -67 | CHK + FP | S1 | **LANDED #231** |
| **S4a** | Delete submit.rs and its four hard-false levers | 6 | -608 | CHK + **FP-CGU1** | S1 | **LANDED #232** |
| **S4b** | Delete the live-dialog / native-profile-capture path | 8 | -529 | CHK + **static gate proof** (regime B) | S4a | **LANDED #234** |
| **S4c** | Delete the retired menu-task-update trace lever | 3 | -47 | CHK + static gate proof (regime B) | S4b | **LANDED #235** |
| **S4d** | Delete the switch-harness discovery probe | 3 | -99 | CHK + **FP-CGU1** (came back regime A) | S4c | **OPEN #237** |
| **S4e** | Delete the disproven legacy menu-drive route | 5 | -263 | CHK + FP-CGU1 + `.pdata`/disasm delta | S4d | **OPEN #238** |
| **S4f** | Delete the last three hard-false S4 levers | 7 | -148 | CHK + FP-CGU1 + `.pdata`/disasm delta | S4e | **OPEN #239** |
| **S5** | **Move the code-patch primitives into er-hook** | 7 | +28 | CHK + FP-CGU1 + **runtime smoke** | -- | **OPEN #241** |
| **S6** | Move the boot profiler into er-boot-profiler | 5 | ~+30 | CHK | S1 |
| **S7** | Move the PGD name offsets into er-game-base | 3 | ~+15 | CHK | -- |
| **S8** | Move the UTF-16 save-name readers into er-game-base | 4 | ~-20 | CHK | S7 |
| **S9** | Move char_fingerprint into the loading portrait crate | 3 | ~+10 | CHK | S8 |
| **S10** | Split lifecycle.rs into four modules | 5 | ~0 | CHK + **FP** + SZ | S4f |
| **S11** | Split loaders.rs live/dead and cut own_stepper_idx10 | 4 | ~0 | CHK + FP + SZ | S4f |
| **S12** | Move the shared dialog RVAs into er-game-base | 3 | ~+40 | CHK + RVA | -- |
| **S13** | Move the gate layer into er-gates *(new crate)* | 6 | ~+950 | CHK | S1, S12 |
| **S14-S17** | Delete the duplicated gate seams from er-title-flow / er-loading-portrait / er-quit-menu / er-save-picker (one crate per PR) | 2-3 ea | ~-60 ea | CHK | S13 |
| **S18** | Dedupe the present-address resolver into er-d3d12-compositor | 3 | -128 | CHK + FP | S3 |
| **S19** | Move the bar geometry and raster helpers into er-loading-bar | 3 | ~-160 | CHK + `cargo test -p er-loading-bar` | S2 |
| **S20** | Split boot_progress.rs into three modules | 4 | ~0 | CHK + FP + SZ | S2, S19 |
| **S21-S24** | Move the boot cover into er-boot-cover *(new crate, 4 slices)* | 3-5 ea | ~+600 ea | CHK + **runtime** | S20 |
| **S25** | Convert native_result_map_hooks to explicit imports | 1 | ~+30 | CHK + FP | S4f |
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

**Why S5 is so early:** er-hook has **zero `[dependencies]`** (only `build-dependencies = cc`,
`Cargo.toml:13-14`) and already solved the logging problem -- `pub type HookLogFn` at
`er-hook/src/lib.rs:27` and `pub fn set_hook_logger` at `:32` -- so the 5 `append_autoload_debug`
calls become hook-logger calls with **no new seam**, and the move **deletes two** er-title-flow seam
fields.

**Executed as #241. Three things the plan had wrong, all in S5's favour except the last:**

* It is **not** net-negative. Measured **+28 lines**, not ~-40: er-hook gains 156, the product sheds
  109 plus 2 bootstrap lines, er-title-flow sheds 26 host lines and gains 5 constant lines. The
  *seam* count is what falls (-2 fields, -2 defaults, -2 wrappers, -2 assignments), and that was
  always the real point.
* **The product had ZERO call sites of its own.** Both functions existed only to be handed across
  the seam, so the move empties them out of `experiments/` entirely rather than leaving a re-export.
* **It needs a runtime smoke and the earlier "CHK + FP" gate was too weak.** This is the first slice
  that touches reachable product code, and the log sink genuinely changes -- direct
  `append_autoload_debug` call becomes `hook_log`'s atomic-load-and-indirect-call. That is visible in
  the measurement: the two moved functions **GREW** (310->331, 355->399), which is the per-log-site
  cost of the indirection. FP-DELTA localises the whole change to 5 of 6,266 functions and shows every
  log string surviving with an identical count, but a string table is not a running game.
  The smoke ran (`run-product-continue-direct-probe.sh`, staged DLL hash verified equal to the build):
  both primitives fired at +347ms with byte-identical text, and `patch_3byte_stub`'s **return value**
  came back `ok=true`, so validation + VirtualProtect + write all succeeded from the new crate. Boot
  reached a loaded character with zero MessageBoxDialog builds.

**Generalise the gate, not just this row: any slice that moves code whose logging crosses a seam
needs a runtime smoke, because the fingerprint can prove the strings survived and still not prove the
sink is installed when they fire.**

**Why S7 before S8 before S9:** `read_utf16_name_units` returns
`([u16; PGD_NAME_LEN_U16], usize)`, and `PGD_NAME_LEN_U16` is derived in
`er-loading-portrait/src/pgd_layout.rs:40` -- moving the function without the constant gives
`er-game-base -> er-loading-portrait -> er-game-base`. S7 moves the constant under er-game-base's
**existing** optional `game-types` feature (`er-game-base/Cargo.toml:19-25`), which er-telemetry and
the product already enable, so the cycle never forms.

---

## 5. Slice 1, fully specified -- **LANDED as #229 (`516c9f1c`)**

Kept as the worked example of the method: a zero-caller proof, a bottom-up itemisation, and a
fingerprint gate that discharges the runtime requirement. **The line numbers below are at `877f1261`
and are spent** -- the twelve items no longer exist. Reuse the shape, not the coordinates.

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

**Measured outcome: 1,796 lines removed across six merged slices (S1-S4c), ~580 left in S4d/S4e/S4f.**
The estimate here was 1,700 across four slices.

**Every line number in this section is at `b49dd5e2` and is now stale for any file S1-S4c touched.**
For the six items still in the tree, use the re-proved table in **SS0.2**, not the table below --
three of its gate line numbers moved and one gate name (`switch_reload_autopilot_enabled`) does not
exist. The proofs below were re-run at `b49dd5e2` over a 565-file corpus (`**/*.rs` minus `target/`,
`.worktrees/`, `.claude/`) with `//` comments stripped; SS0.2 re-runs the surviving ones over 621.

### S1 -- zero-caller items (218 lines) -- **LANDED #229 at -118**
Twelve items, each returning **exactly 1 comment-stripped code hit = its own definition**. Full table
in SS5. The 218 headline never matched its own itemisation (116); the merged net was -118.

### S2 -- the in-world effect-selector HUD (454 lines) -- **LANDED #230**
`gpu_readback/boot_progress.rs:2614-3055` + `er-telemetry/src/counters.rs:1131-1141`.
**Two independent proofs.** (a) `composite_effect_selector_on_swapchain` has exactly 1 code hit --
its definition at `boot_progress.rs:2648`. (b) Even if called, the body is inert by construction:
`composite_effect_selector_inner:2702` is `let text = String::new();` and `:2703` returns on
`text.trim().is_empty()`. The live effect-selector HUD is a different implementation in a different
crate (`er-net-effects-dll/src/present_overlay.rs:71`). No `oracle_effect_selector*` field exists, so
`check-oracle-writers.py` stays green. **This is the slice that removes the 3,200 hard-fail exposure**
-- `boot_progress.rs` 3,055 -> 2,601.

### S3 -- the dxgi factory-export hook (67 lines) -- **LANDED #231**
`present_overlay.rs:383-448` + `er-telemetry/src/counters.rs:114`.
`install_dxgi_factory_export_hook` has 1 code hit (its definition at `:415`; the block including
`FACTORY2_ORIG` and `Factory2Fn` spans 383-448). `factory2_hook` appears only at `:389` (def) and
`:435` (inside the dead installer). Superseded by the GxDrawContext chain finder -- see the comment at
`present_overlay.rs:936-942`.
**Correction to the source analysis:** do **not** delete `use crate::mh::{...}` at `:41`.
`MH_Initialize` is used at `:467, :471` and `MH_STATUS` at `:468`, both outside the dead block.
The import **narrows to** `use crate::mh::{MH_Initialize, MH_STATUS};`.

### S4 -- hard-false levers (800 lines) -- **S4a #232, S4b #234, S4c #235 LANDED; S4d/S4e/S4f open**
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

**Split S4 into 3-4 PRs** -- executed as **six**: S4a `submit.rs`; S4b `live_loadgame_node`;
S4c the trace pair; and then S4d the switch harness, S4e the legacy menu-drive route (which is where
the `menu_observation` trio actually lives -- it is tangled into `product_core_own_stepper.rs`'s
`legacy_menu_drive_enabled` branch, not into `live_loadgame_node`), S4f the last three levers. Three of these blocks make **native
state-changing calls** -- `submit_play_game_once`'s SetState/deserialize/streaming-enable,
`ingameinit_drive_tick`'s `IngameInit` on a leaked synthetic `this`, `fire_live_loadgame_node`'s
dialog-factory call and profile-slot pre-activate -- so this removes latent save-adjacent risk, not
just lines. The switch-harness block blocks the user's keyboard and injects a synthetic `DIK_ESCAPE`
(`lifecycle.rs:66-77`); it is rule-4 harness code with a dead gate.

**Cross-cluster warning:** the switch-harness deletion touches
`startup_hooks/quit_menu/profile_rows_system_quit_menu.rs:1680-1682` (3 lines). Coordinate with the
startup_hooks owner. `filename`, used at `:1684`, must survive.

**Gate for all four slices:** `dll-code-fingerprint.py`. **This prediction was half wrong and the
correction is in SS0.1.** S1-S3 came back `.text`-identical as predicted; S4a needed FP-CGU1 to do so;
and S4b/S4c/S4d/S4e/S4f cannot be identical at all -- they delete code the compiler actually emitted
(regime B), so `MATERIAL` is the expected verdict and the proof is the literal-`false` gate body plus
the caller count, not the hash.

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

**Recommendation: do it, but delete the dead gates first.** Run S4 to completion, then re-count.
If the 45 hard-false gates fall to ~20, er-gates is an ~500-line crate of live product levers and is
clearly right. Building it at 892 lines with 45 dead entries just relocates the debt.

**Re-count at `f54e4041` (S4a-S4c landed, S4d-S4f still open): `gating/` holds 74 `fn() -> bool`, of
which 37 have a body that is exactly the literal `false`** -- measured by matching a `fn NAME() ->
bool` line followed by a bare `false` and a closing brace, across `env_flags.rs` (706) and
`runtime_modes.rs` (141), 847 lines total. That is **50%**, down from 55%, and the directory is 45
lines lighter than the 892 this decision was costed against. The trend is right but the target was
~20; **the decision stays open until S4d-S4f land and it is counted a third time.** S4d/S4e/S4f
remove three more gates between them (`switch_harness_discovery_enabled`, `legacy_menu_drive_enabled`,
`worldres_coldbuild_probe_enabled`), which lands the count near 34 -- still well above ~20, so the
honest reading is that the remaining hard-false gates are NOT concentrated in the S4 block and
er-gates would ship roughly half-dead whenever it is built.

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
