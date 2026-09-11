# Shipping "Load Build from URL" + "Generate Build Link" as a standalone DLL

Status: plan. Nothing here is implemented. Every number below was measured on
`feat/ersc-seam-owner-hunt` on 2026-09-10 and is reproducible with the command beside it.

## The verdict in one line

It is achievable without inventing any new mechanism, because the two things that usually kill a
second DLL on this target -- hook collision and GFx-swap collision -- are already solved and already
gated. What is left is a code move: the row machinery the two rows stand on is 16,478 lines inside
`er-quickload`, and it has to reach a crate a shell can arm.

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

What Phase 0 still owes: the per-symbol verdict on those 32 -- host field, move-with-the-code, or
delete -- because one function pointer per symbol is an upper bound, not a design.

Proof: the per-symbol table exists as this doc's appendix. No build change.

### Phase 1 -- move the row-cloning machinery into `er-quit-menu-core`

Move `system_quit_dialog_handlers.rs`, `system_quit_row_identity.rs` and the row-population half of
`profile_rows_system_quit_menu.rs` into `er-quit-menu-core`, converting each root-crate reach into a
host field from Phase 0's appendix. `er-quickload` keeps calling them; nothing is armed from the
shell yet.

Every detour that moves must become `er_hook::register_union_hook` -- the crate's own `Cargo.toml`
already mandates it, and `check-shared-hook-rvas.py` will fail the moment two shells claim one
address without a table row.

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
2. **The `use crate::*` glob.** 32 behavioural symbols for the build-rows subset, 69 for the full
   quit menu. That gap is the price of taking the other three rows along, and it is why the plan
   ships `BUILD_ROWS_ONLY` first.
3. **The 02_990 swap.** The link field's derived movie is swapped at the Scaleform file-open
   prologue the product also hooks. That is exactly the pair that went silently inert for a day on
   2026-08-23 (113 hits alone, 0 co-loaded). Route through the union from the first line, never a
   bare `MhHook::new`.
4. **Row cloning arms twice.** If both DLLs ever load, two AddCancelButton detours clone two pairs
   of rows. Phase 5's refusal is the guard; do not rely on a runtime singleton to paper over a
   profile that should not exist.

## Open questions this plan does not answer

The feasibility study running alongside it owns these, and its answers fold into Phase 0:

- which of the five Quit-menu addresses the two rows actually need, vs. which belong to the other rows;
- whether row cloning can be armed at all without the product's arm sequencing;
- the file-by-file verdict on the 16,478 lines;
- the honest session count.
