# Shipping "Load Build from URL" + "Generate Build Link" as a standalone DLL

Status: **Phase 0 done; Phase 1 started, not finished; Phases 2-6 not started.** No shell arms
anything yet, so an `er_quit_menu.dll`-only profile still shows no build rows. Every number below
was measured on `feat/ersc-seam-owner-hunt`, the dated ones on 2026-09-10 and the re-measured ones
on 2026-09-11, each reproducible with the command beside it.

## Read this before the rest of the document

Two things below are now wrong, and one whole blocker is missing. Both corrections were measured on
this branch, not argued.

**The five-argument problem is solved.** A sibling branch
(`refactor/build-rows-standalone-dll`, bd `er-effects-rs-ehvj`) concluded **No** on this plan, and
its first and strongest reason was that `system_quit_duplicate_add_cancel_button_hook` takes five
arguments while `er_hook::UnionFn` takes four, so the row cloner could never leave a bare
`MhHook::new` and could never share its prologue with a second DLL. That was true when it was
written and is no longer: `er_hook::UnionFn5` / `register_union_hook5` / `union_dispatch5` landed
the same evening (`crates/er-hook/src/lib.rs:131`, and the commit says so in its subject -- "a
five-argument union, so the Quit row cloner can share its prologue"), and the cloner is **already
registered on it** (`experiments/startup_hooks/diagnostics/layout_global_hooks.rs`, which now reads
"The union, not a bare `MhHook`"). `scripts/check-union-hook-abi.py` gates the arity. That sibling
branch was never merged, so the repository currently holds two documents that disagree; this
paragraph is the reconciliation, and the sibling's remaining findings are folded in below.

**Its other three findings survive, but two of them are answered by Phase 5 rather than by code.**
The Quit grid is one six-cell derivation that fail-closes on a second deriver
(`er-gfx/src/options_02_040.rs:174`, asserted by `er-gfx/tests/options_02_040.rs:64`), and
`QUIT_ROW_TABLE_ROWS` is one six-row array whose resolver gates the irreversible `ExitProcess(0)`
(`er-quit-menu-core/src/rows.rs:127`, `row_identity.rs:292`). Both only bite when the product and
the shell load together, which Phase 5 exists to refuse. The conflict table already predicts the
answer in prose: the `[compatible]` entry for `er-quit-menu` says that "if the harness ever arms
that row, this entry becomes a `duplicate-owner` conflict with `er-build-import` and with
`er-quickload`". Phase 5 is writing that down, not deciding it.

**The fourth finding is live, and it is the one that stops Phase 2.** Re-measured on this branch on
2026-09-11: `build_url_editor.rs` still calls ten symbols defined in files this plan does not move
-- eight in `save_picker_path_editor.rs` (`submit_build_url_keyboard`, `build_url_menu_pump_tick`,
`BuildUrlKeyboardOutcome`, ...) and two in `profile_05_010_editor_runtime.rs`
(`place_text_input_02_990_caret_at_end` at `:1289`, `set_text_input_02_990_text` at `:1197`), which
the file table below marks "no -- ProfileSelect browse surface". And `save_picker_path_editor.rs`
opens with an `include!(concat!(env!("OUT_DIR"), ...))` of a generated prologue table (`:19`), while
`er-quit-menu-core` has no `build.rs` at all (bd `er-effects-rs-pq31`). Coupling **inside** the
`quit_menu/` tree, not the `use crate::*` glob, is what binds this slice.

**The missing blocker: nothing in the phase list owns the GFx swap.** A shell-only profile shows no
rows at all unless the shell itself derives and swaps two movies -- the `02_040` quit6 grid the rows
are cells of, and the `02_990` link field. Both are served from one URL-keyed chain in
`experiments/startup_hooks/loading_cover/profile_table_gfx_files.rs` (the `02_040` arm at `:930`,
the build-url `02_990` arm at `:936`), reached from the Scaleform file-open prologue
`TITLE_SCALEFORM_FILE_OPEN_RVA` (0x11ced80) -- already a two-way `[[shared]]` with
`er-armament-icons`, and the pair that went silently inert for a day on 2026-08-23. That file is in
`loading_cover/`, not `quit_menu/`, and also serves the loading cover and the profile table, so it
has to be carved rather than moved. Add it as a phase before Phase 4 or the shell arms rows onto a
grid that has no cells for them.

## The verdict in one line

Achievable, and now cheaper than when this was written because the five-argument union exists. What
is left is a code move plus one carve-out the phase list did not account for: the row machinery is
16,741 lines inside `er-quickload`, and the GFx swap that makes the rows visible is in a second
subtree.

## What is already done (do not redo these)

| Fact | Measured by |
|---|---|
| The import/export engine is already a standalone shell: `crates/er-build-import` (cdylib, own `DllMain`) over `er-build-import-runtime` + `er-build-import-core`, with `er-build-export` host-buildable | `cat crates/er-build-import/Cargo.toml` |
| A shell crate already exists and is already registered: `er-quit-menu` (112 lines) is in `me3_shells` in `scripts/check-rust-build.sh` (29 shells), so it builds, links and gets conflict-analysed every run | `grep -A32 me3_shells scripts/check-rust-build.sh` |
| A feature crate already exists: `er-quit-menu-core`, 16 files / 6,491 lines, with a 30-field `QuitMenuHost` function-pointer seam and game deps (`eldenring`, `er-hook`, `er-game-base`) already wired | `wc -l crates/er-quit-menu-core/src/*.rs` |
| Two-DLL hook collisions are gated by value, not by spelling: `check-shared-hook-rvas.py` reports 31 DLLs, 311 hook targets, 36 shared and all declared, and runs in `check.sh` with a `--selftest` | `python3 scripts/check-shared-hook-rvas.py` |
| The union mechanism is proven in production: `er-quickload` + `er-armament-icons` share `TITLE_SCALEFORM_FILE_OPEN_RVA` (0x11ced80) through `register_union_hook` / `register_shared_hook`, declared as `[[shared]]` | `scripts/me3-dll-conflicts.toml` |

## What is actually blocking it

`er-quit-menu-core` arms nothing. Its only public entry point is `install_host`, and it contains
**zero** `register_union_hook` / `register_shared_hook` calls -- so the `er-quit-menu` shell's
"arms nothing yet" comment is accurate, not stale. Its `Cargo.toml` *claims* the AddCancelButton
row cloning as product (B), but that machinery is still in `er-quickload`:

```
crates/er-quickload/src/experiments/startup_hooks/quit_menu/   17 files, 16,478 lines
```

and its `mod.rs` opens with `use crate::*`, i.e. the whole root-crate namespace. That glob is the
cost: every file that moves has to have its root-crate reach converted into `QuitMenuHost` function
pointers, the same way the existing 30 fields were.

### The files the two rows stand on

| File | Lines | Needed by the build rows? |
|---|---:|---|
| `build_url_editor.rs` | 700 | yes -- the whole native-keyboard flow |
| `build_url_row.rs` | 178 | yes |
| `generate_build_link_row.rs` | 8 | yes |
| `build_url_clipboard.rs` | 7 | yes |
| `system_quit_row_identity.rs` | 77 | yes -- positive row identity |
| `system_quit_dialog_handlers.rs` | 1,492 | yes -- row cloning + the label statics |
| `profile_rows_system_quit_menu.rs` | 2,132 | yes -- the row-population path (22 `MhHook::new` sites) |
| `save_picker_path_editor.rs` | 1,543 | partly -- the shared 02_990 editor path |
| `system_quit_hooks.rs` | ~? | yes -- the arm point (3 `MhHook::new` sites) |
| `profile_05_010_editor_runtime.rs` | 1,991 | no -- ProfileSelect browse surface |
| `save_swap_profile_table.rs` | ~? | no -- save-swap preview |
| `save_dest_commit.rs`, `save_picker_menu.rs`, `save_flow_boxes.rs` | -- | no -- Save Game flow |
| `system_quit_ownership_repro.rs`, `system_quit_repro_guards.rs` | -- | no -- diagnostics (26 `MhHook::new` sites) |

Plus `crates/er-gfx/src/build_url_02_990.rs` (468 lines), which is already in a shared crate and
does not need to move.

## The shape to build

Arm the **existing** `er-quit-menu` shell rather than create a new crate, and select the row set at
the arm call:

```rust
// crates/er-quit-menu-core/src/arm.rs  (new)
pub struct RowSet { pub load_character: bool, pub load_from_file: bool,
                    pub save_game: bool, pub build_url: bool, pub generate_link: bool }
pub const BUILD_ROWS_ONLY: RowSet = RowSet { build_url: true, generate_link: true, ..RowSet::NONE };
pub unsafe fn arm(rows: RowSet) -> Result<(), ArmError>;
```

Reasons this beats a fresh `er-build-rows` crate: the shell is already in `me3_shells`, already
conflict-analysed, already has the host seam and the panic reporter, and a fresh crate would need
the same row-cloning move anyway -- so a new crate adds registration work and subtracts nothing.
If the two rows must ship without the other three, that is a `RowSet`, not a second crate.

## Phases

Each phase ends green on the **scoped gate list** below and leaves the product DLL's behaviour
unchanged until Phase 5. Phases 1-3 are pure refactors of `er-quickload` and are individually
revertible.

`bash scripts/check.sh` is deliberately NOT the per-phase gate. It fails closed inside an agent
worktree (`repo_root == */.claude/worktrees/agent-*`, exit 2) and refuses a second concurrent run
anywhere, so a worktree agent cannot run it and must not reach for `ER_CHECK_FORCE=1`. The
whole-workspace verdict is the orchestrator's, run once in the main tree at integration.

The per-phase gate, in full:

```bash
cargo test -p er-quit-menu-core                  # and -p er-gfx if that crate is touched
cargo fmt -p <each crate touched> -- --check
python3 scripts/check-comment-caps.py <each file touched>
python3 scripts/check-no-lossy-utf8.py
python3 scripts/check-shared-hook-rvas.py        # after ANY detour move
python3 scripts/check-me3-dll-conflicts.py       # after ANY conflict-table edit
cargo xwin build --release --target x86_64-pc-windows-msvc          # product, backgrounded
bash scripts/er-build-dlls.sh er-quit-menu                          # shell, records provenance
```

### Phase 0 -- pin the seam (counted 2026-09-10; refine, do not re-derive)

The symbols the moving files reach through `use crate::*` have been counted: definitions collected
from every `er-quickload/src/**/*.rs` outside the `quit_menu/` tree, intersected with the
identifiers each moving file uses (comments stripped).

| Move scope | Symbols reached | `const` (mechanical) | `fn` + `static` (become host fields) |
|---|---:|---:|---:|
| Build rows only -- the 7 files, without `profile_rows_system_quit_menu.rs` / `save_picker_path_editor.rs` | 54 | 22 | 32 |
| All nine files | 128 | 59 | 69 |

A build-rows-only extraction therefore adds roughly **32 fields** to the 30-field `QuitMenuHost`,
and the 22 consts move as data -- most already belong in `er-game-base::rva`
(`constants/anti_debug.rs` owns 27 of the full-scope symbols, `constants/stats_panel_text.rs` 22,
`constants/profile_render.rs` 20). The full-scope number more than doubles the behavioural seam,
which is the concrete argument for shipping `RowSet::BUILD_ROWS_ONLY` first and the other three
rows later.

**The estimate above is an upper bound and it overcounts by the whole amount: the answer is zero
new host fields, not 32.** One function pointer per symbol was never a design, and the sibling
branch's per-symbol pass found eleven of the intersection to be name collisions rather than
reaches, twenty-three belonging to `system_quit_hooks.rs` (which serves the title Continue-confirm
and the ProfileLoad route -- no build row touches it, so it should not have been in the slice), and
six reached from inside two functions that are *already* `QuitMenuHost` fields. Re-measured
directly on this branch on 2026-09-11, `system_quit_dialog_handlers.rs` reaches 13 real root-crate
symbols and `system_quit_row_identity.rs` reaches none; the ones that matter
(`append_autoload_debug`, `release_input_block_now`, `save_picker_seamless_mode_after_settle`,
`normalize_save_bytes_to_active_steam_id`) are all declared host fields today.

What is left of those 13 is data or a shared-crate move, never a function pointer. Three were
singled out as belonging in `er-game-base` beside `game_module_base()`, having no product state at
all: `callstack_contains_game_rva`, `trace_first_game_caller_rva` and `seamless_coop_loaded`.

Proof: the per-symbol table exists as this doc's appendix. No build change.

### Phase 1 -- move the row-cloning machinery into `er-quit-menu-core`

Move `system_quit_dialog_handlers.rs`, `system_quit_row_identity.rs` and the row-population half of
`profile_rows_system_quit_menu.rs` into `er-quit-menu-core`, converting each root-crate reach into a
host field from Phase 0's appendix. `er-quickload` keeps calling them; nothing is armed from the
shell yet.

Every detour that moves must become `er_hook::register_union_hook` -- the crate's own `Cargo.toml`
already mandates it, and `check-shared-hook-rvas.py` will fail the moment two shells claim one
address without a table row. The cloner's own detour is **already** converted: it registers through
`register_union_hook5`, so the five-argument prologue no longer holds MinHook's single slot alone.

**Started 2026-09-11. Done so far: the two stack readers now live in `er-game-base`.**
`callstack_contains_game_rva` and `trace_first_game_caller_rva` moved out of
`er-quickload/src/crashlog/module_resolution.rs` into a new `er-game-base/src/stack.rs`, and
`module_resolution.rs` re-exports both under their original names so all ~35 `crate::crashlog::`
call sites read unchanged. This is a prerequisite rather than a tidy-up: `callstack_contains_game_rva`
is how the cloner tells its two native `AddCancelButton` call sites apart, so it has to be reachable
from whatever crate ends up owning the cloner, and it cannot become a host field without making the
seam bigger for a function that has no product state to seam.

The move also retires a seam entry. `er-loading-portrait-core` carried a
`LoadingCoverHost::trace_first_game_caller_rva` function-pointer field whose only job was to reach
back into the product for the same pure function; the field, its neutral default and the product's
wiring line are all gone, and the crate-internal wrapper calls `er_game_base::stack` directly. One
fewer field for every shell that installs that host to get right.

Still to do in this phase: `seamless_coop_loaded` (the third `er-game-base` candidate --
`GetModuleHandleA("ersc.dll")` plus a sticky latch, read inside the same `cloned_rows` table that
carries both build rows), and the file move itself. Note that
`system_quit_dialog_handlers.rs` is **not** a single-purpose file: of its 1,492 lines the cloner and
the label helpers are build-row machinery, but roughly half is the Save Game flow
(`system_quit_save_game_*`, ten functions) and the ProfileLoad dialog opener, which belong to the
other rows. Extract the cloner rather than moving the file, or Phase 1 silently becomes the
whole-quit-menu scope this plan costed at 69 seam symbols.

Proof: the scoped gate list above, with `check-shared-hook-rvas.py` mandatory because detours move
in this phase; the product DLL still loads and the three existing rows still work in the combined
runtime run.

### Phase 2 -- move the two build rows

Move `build_url_row.rs`, `build_url_editor.rs`, `build_url_clipboard.rs`,
`generate_build_link_row.rs` and the 02_990 editor path they share with the save picker
(`save_picker_path_editor.rs`, the shared half only). `er-build-import-runtime` is already a
dependency of `er-quit-menu-core`, so the engine side needs no change.

Proof: same as Phase 1, plus `cargo test -p er-quit-menu-core`.

### Phase 3 -- add the arm entry point

Add `arm(RowSet)` and route `er-quickload`'s existing arm site through it with the full row set, so
the product's behaviour is defined by the same code path the shell will use. This is the phase that
proves the seam is real: if the product can arm through it, a shell can.

Proof: the product DLL armed through `arm(RowSet::ALL)` is behaviourally identical in the runtime
run; `oracle_system_quit_*` counters match a pre-change run.

### Phase 4 -- arm the shell

Replace `er-quit-menu`'s "scaffolding: nothing armed yet" block with
`er_quit_menu_core::arm(BUILD_ROWS_ONLY)` behind its standalone host. The shell's host defaults must
stay neutral -- in particular the save-write bypass stays refused, which its current doc comment
already commits to.

Proof: the shell loaded **alone** in a me3 profile shows both rows on the Quit tab and imports a
build; the product DLL absent.

### Phase 5 -- declare the co-loading answer

Decide and record whether `er-quit-menu` and `er-quickload` are `[[conflict]]` or `[[shared]]` in
`scripts/me3-dll-conflicts.toml`. Both offer the same two rows, so the honest answer is almost
certainly `[[conflict]]` on a *duplicate-feature* kind rather than a hook kind -- the same relation
`er-build-import` already has with the product. Touch points:

| File | Change |
|---|---|
| `scripts/me3-dll-conflicts.toml` | the pair's row, with the reason |
| `scripts/check-me3-dll-conflicts.py` | nothing, if the row is added |
| `scripts/er-dll-closure.py` | nothing -- it reads `[[conflict]]` and will refuse the bad profile |
| `scripts/check-shared-hook-rvas.py` | nothing -- it discovers by value; it will simply start reporting the new pairs |
| `scripts/check-rust-build.sh` | nothing -- `er-quit-menu` is already in `me3_shells` |
| `scripts/er-dll-provenance.py` | nothing -- it attests whatever links |

Proof: `python3 scripts/check-me3-dll-conflicts.py`, `python3 scripts/check-shared-hook-rvas.py`,
and a generated profile that co-loads the pair is **refused**.

### Phase 6 -- the runtime proof matrix

Three runs, none of which may be replaced by a build success (per `AGENTS.md`: a rendered feature is
not proven by build success, launch success, or hook counters):

| Run | Profile | Must show |
|---|---|---|
| A | product alone | both rows present; import applies -- unchanged from today |
| B | `er-quit-menu` alone | both rows present; import applies; no product DLL loaded |
| C | product + `er-quit-menu` | the profile generator refuses to emit it |

Run B is the deliverable's proof. The oracle is the import's own telemetry plus the row-present
counter -- not a screenshot.

## Risks, in the order they will bite

1. **Feature unification.** `er-quit-menu-core` links `er-save-picker-core` with
   `default-features = false`. Cargo unifies features across a build graph, so the isolation is real
   only in a build where the product is absent. A shell-only build must be verified as its own
   cargo invocation, not as part of the 29-shell link.
2. **The `use crate::*` glob -- superseded; it was never the blocker.** The 32-symbol estimate for
   the build-rows subset is an upper bound that overcounts to zero new host fields once the
   collisions, the wrong-file symbols and the already-seamed bodies come out (see Phase 0). The
   binding constraint is coupling *inside* the `quit_menu/` tree: ten symbols from `build_url_editor.rs`
   into two files this plan excludes, and a generated `OUT_DIR` prologue table that
   `er-quit-menu-core` has no `build.rs` to produce.
3. **The 02_990 swap.** The link field's derived movie is swapped at the Scaleform file-open
   prologue the product also hooks. That is exactly the pair that went silently inert for a day on
   2026-08-23 (113 hits alone, 0 co-loaded). Route through the union from the first line, never a
   bare `MhHook::new`.
4. **Row cloning arms twice.** If both DLLs ever load, two AddCancelButton detours clone two pairs
   of rows. Phase 5's refusal is the guard; do not rely on a runtime singleton to paper over a
   profile that should not exist.

## Open questions this plan does not answer

The feasibility study answered these (bd `er-effects-rs-ehvj`, and the appendix on
`refactor/build-rows-standalone-dll`):

- **which addresses the two rows actually need** -- seven, not five, and five of them the product
  also hooks: the cloner `0x920c90`, both routing entry points (`0x9610d0`, `0x9749f0`), the shared
  software-keyboard pair (`0x81d3d0`, `0x81d220`), the Scaleform file-open prologue `0x11ced80`
  (already a two-way `[[shared]]`) and `MenuWindowJob::Run` `0x7ad1c0` (documented as a deterministic
  sole owner after a second contender broke it once). The cloner was the one the union could not
  take; `register_union_hook5` now takes it. Note that the shared-hook gate keys declarations on the
  **crate pair**, not the address, so one row covers every address a pair shares -- "36 shared and
  all declared" is a weaker statement than it reads as;
- **whether row cloning can be armed without the product's arm sequencing** -- unanswered in the
  affirmative, and it is what Phases 3-4 exist to settle;
- **the file-by-file verdict on the 16,741 lines** -- partially, in the file table above and the
  sibling appendix;
- the honest session count -- still open, and larger than this plan's phase list implies, because
  the GFx carve-out is not in it.

Still genuinely open, and new:

- who owns the `02_040` quit6 grid derivation and the `02_990` link-field derivation in a
  shell-only profile, given both are served from one URL-keyed chain in `loading_cover/` that also
  serves the loading cover and the profile table;
- whether `er-quit-menu-core` gains a `build.rs` (bd `er-effects-rs-pq31`) or the 02_990 editor path
  is restructured so the generated prologue table is not needed on that side of the seam;
- bd `er-effects-rs-gx3s`: a 7-DLL closure including `er_quit_menu` wedges before the title screen,
  reproduced on two branches, both times inside hook installation. `er_quit_menu` arms nothing
  today, so that has to be understood before Phase 4 arms it.
