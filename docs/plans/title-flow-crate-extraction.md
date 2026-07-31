# Title/Switch Flow Crate Extraction (`er-title-flow`)

User directive 2026-07-31: extracting the `experiments/title/` cluster out of the root DLL is a
**prerequisite** to further portrait bug fixes and semaphore work. Rationale from the field: every
product regression of 2026-07-30/31 (the b78/fd4io warp-target race, the rt5d finalize driver's
three calibrations, the stable-world proof) lives in `title_tick_cover.rs` -- a 3200-line
`include!` module wired to hundreds of cross-cutting globals. The seam itself is the bug factory.

## Scope (the moved cluster, 5,268 lines at branch point `11591419`)

- `title_tick_cover.rs` (2385) -- product autoload tick, b78 guard, switch-oracle emit, stable
  proof, mms18 recovery drives
- `title_load_step_hooks.rs` (918) -- FD4 filecap/dlstring helpers, movemap gate/defer hooks,
  loadlist capture, setstate trace
- `product_autoload_gates.rs` (909), `profile_select_flow.rs` (815),
  `switch_slot_control.rs` (175), `native_title_job.rs` (66)

## Boundary plan (er-loading-portrait / er-save-picker precedent)

1. **Already crate-shared, no work**: er-telemetry counters (~110 symbols), typed game access
   (`eldenring`/`fromsoftware-shared`), fault-safe reads + base/rva (`er-game-base`), MinHook
   (`er-hook`).
2. **Constants (~350 symbols from `er-effects-rs/src/constants/`)**: move the title/switch-owned
   ones into this crate's `constants.rs`; symbols shared with remaining root code get re-export
   shims in the root so nothing else changes.
3. **Host boundary (`host::install_host` fn-pointer pattern, exactly like er-loading-portrait)**:
   callbacks INTO the DLL that cannot move yet -- `append_autoload_debug`, gating predicates,
   `boot_view_epoch_ms`, and the system-quit/startup_hooks calls (the tangle is bidirectional:
   those hooks also call into this cluster, which becomes plain `pub` exports).
4. **Path rewrite**: `crate::constants::` / `crate::experiments::` / `crate::constants::...` inside
   the moved files become `crate::compat::` -- one flat prelude module mapping every symbol to its
   new source (own constants, host fn, er-game-base, er-telemetry). Mechanical, compiler-verified.
5. **Root side**: `experiments/mod.rs` re-exports `er_title_flow::*` under the old names so every
   external call site compiles unchanged.

## Sequencing / proof

- Stage A: scaffold + move + compat rewrite -> compiler error inventory is the boundary ground truth.
- Stage B: burn down errors (constants move, host fns, exports) until `cargo xwin check` is green.
- Stage C: `bash scripts/check.sh` green + release build.
- Stage D: **runtime smoke before any non-breaking claim** (repo doctrine): boot + same-save switch
  + cross-save switch chain via `scripts/run-slot-portrait-proof.py`; oracle parity with the
  pre-refactor run (portrait, handoff-complete, switch oracles).

Known parked items while this lands (user ruling: extraction first): the cross-save stale-gaitem
nude fix (RE report pending), per-slot equip semaphores, the loadwin tracker merge from
`portrait-render-semaphores-20260731`.
