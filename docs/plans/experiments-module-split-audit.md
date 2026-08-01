# `experiments/` module split -- audit and plan of record

Audit date: 2026-08-01. Baseline: `main` @ `7ebfeef8`.

Scope measured: 70 files / ~54,900 lines under `crates/er-effects-rs/src/experiments`, plus
everything it reaches across the 36 workspace crates. Produced by a seven-cluster parallel
sweep, each cluster re-checked by an adversarial verifier against the source, then a
completeness critic over the whole tree.

**Bottom line: the directory is not badly designed, it is badly delimited.** Almost every
"split" in it is textual, not semantic.

---

## 0. The structural facts everything else follows from

1. **The directory splits are not modules.** `startup_hooks.rs`, `trace.rs`,
   `save_redirect.rs`, `own_stepper.rs`, `own_load.rs`, `menu_diag.rs`, `gating.rs`,
   `continue_load.rs` and `gpu_readback.rs` are shims that `include!` their children.
   `include!` is textual, so those ~45 files share one namespace per shim with no module
   boundary and no `use` statements for in-crate items -- every cross-file call is an
   unqualified free identifier. `mod.rs` declares 18 real `mod`s, each glob re-exported, so
   the accurate model is "18 modules, each internally flat", not "one flat namespace".
   Consequence: **merging two files is nearly free; extracting anything is the whole cost.**

2. `include!` is **not** load-bearing -- zero `macro_rules!` in any of the 78 children
   crate-wide. It is inertia, not necessity.

3. **`cargo fmt --all -- --check` is blind to 54,943 lines.** `include!` children are not in
   the `mod` graph, so rustfmt never walks them. Reproduced: `cargo fmt --all -- --check`
   exits 0 while 67 of the 78 children come back dirty from `rustfmt --edition 2024 --check`.
   `experiments/can_move_probe.rs` is the in-tree existence proof of the fix -- a real
   `pub(crate) mod` with explicit named imports and zero leaked symbols, shipping today.

4. **~6,900 lines are provably unreachable**, and about half is a stalled directive rather
   than an oversight: bd `deprecate-env-marker-gate-allowlists-no-gated-features-2026-07-19`
   called for removing dead gated experiments outright. The allowlists were emptied and 44
   gates neutered to `return false`; the removal never happened. Nothing catches it --
   `.cargo/config.toml` sets `rustflags = ["-Awarnings"]` workspace-wide, so `dead_code`
   never fires, and `#![allow(unused_imports)]` sits at both `src/lib.rs:1` and
   `experiments/mod.rs:3`.

5. **`scripts/check-rust-file-sizes.py` is the force that created the file boundaries**
   (warn > 900, fail > 3200). It is the one gate that *does* see `include!` children, and it
   rewards splitting. It currently warns on 29 of the 70 files. Any plan that hands back
   more textual splits is feeding the thing it is trying to fix.

6. **`experiments/` is 76% of the crate** (54,900 of 72,007 lines) and the name is inverted.
   Its own header says it is an artifact of a lib.rs split. Dependency direction is inward:
   the non-`experiments` half references `crate::experiments::` **197 times**, while
   `experiments/` references the root modules **98 times**. It holds the always-on Scaleform
   guard, the `%APPDATA%` save redirect, the load drive, the picker -- while the genuinely
   experimental code (`submit.rs`, `input_trace.rs`, `menu_diag/`, the repro harnesses) sits
   in the same directory, indistinguishable by name.

---

## 1. Stalled work, not new ideas

`crates/er-save-picker` (297 lines) and `crates/er-quit-menu` (318 lines) are **scaffolding
only** -- `lib.rs` + `host.rs`, both headers stating "nothing has been moved yet".
`docs/plans/save-picker-crate-extraction.md` specifies slices S1-S10; **all ten are not
started**, and ~14,000 lines of the code they name is still in `experiments/`.

The plan's shape is sound. Four of its assignments have drifted and must be corrected before
slicing:

- Assigning `save_picker_surface.rs` wholesale to (B) creates an A->B->A cycle; the real seam
  is the file's own internal one (router -> A, save-dest decisions -> B).
- It assigns a **world-load fix** to a menu crate: `system_quit_hooks.rs:144-309` is
  `maybe_force_finish_stuck_testnet_step`, a MoveMapStep completion fix whose sole caller is
  `lifecycle.rs`'s `tick_before_player_lookup`. It belongs in `own_load/`.
- `mh_install_hook_once` appears nowhere in the plan, so the crate would move three
  installers and leave their helper behind. Destination is `crate::mh` / `er-hook`.
- A 2026-07-31 commit added an HWND z-order dependency (`os_dialog_owner` reads
  `picker_dim_armed_cover_hwnd`) that the `PickerCover = Box<dyn Any>` seam cannot express.
  Extract as written and the dialog silently renders behind the blur again.

Its line ranges have drifted 5-200 lines in essentially every row; the plan says so itself.
Re-measure before using any of them for slice sizing.

---

## 2. Duplicate reverse-engineered addresses (started)

**61 game addresses are declared under more than one name, across 154 declaration sites.**
This matters because CLAUDE.md documents that 1.16.1->1.16.2 address drift *crash-hooks*: a
single-address correction currently has to be found in up to five places under five names.

> Corrected 2026-08-01: the first pass reported 45/110. That scan matched only
> `const ... : usize`, so it missed every `u32`-typed address constant -- an undercount of
> ~35%. `MOUNT_GUARD_STATE_ROOT_RVA` (a `u32`) was one of the misses, and it turns out to be
> `GameDataMan`. Any future scan must cover `usize|u32|u64|i32|i64`.

Roughly 50 groups are cosmetic spelling variants or legitimate local role names; the rest
make contradictory claims about what the address *is*, i.e. wrong RE facts are shipping.
Three are not addresses at all (`0x4000000`, `0x3000000`, `0x280000` are sizes/limits) --
though `0x280000` is worth unifying separately, since `C30_WRITER_FULL_SAVE_SIZE` and
`SLOT_BODY_LEN` are the same save-slot body length and the decompile confirms that value.

Resolution method that works, recorded in
bd `contested-rva-identities-resolved-1162-2026-08-01`: query the 1.16.2 Ghidra MCP
`getXrefsTo` at `0x140000000 + RVA`; xref **count** plus the named reader functions
discriminate a general engine singleton (hundreds-thousands of refs) from a feature-specific
pointer (a handful).

Resolved so far:

| RVA | Truth | Wrong name(s) in tree |
|---|---|---|
| `0x67b750` | `GameMan::WriteSaveToSlot` | `CONTINUE_LOAD_RVA` -- **fixed**, see below |
| `0x3d872e0` | `GLOBAL_MainHeapAllocator` (1821 xrefs) | `SLLOAD_SRC2_RVA` |
| `0x3d5df38` | `GameDataMan` (734 xrefs) | `CONTINUE_MANAGER_GLOBAL_RVA`, `MOUNT_GUARD_STATE_ROOT_RVA` |
| `0x3d6b7b0` | `CSMenuMan` (966 xrefs, incl. `CanShowSaveMenu`) | the three `*INPUT_MANAGER*` spellings |
| `0x3d69918` | `GameMan` (349 xrefs) | `GAME_SAVE_SLOT_SINGLETON_RVA` |
| `0x7ad1c0` | `MenuWindowJob::Run` | -- (`LEAF_UPDATE_RVA` is a legitimate local role name in a vtable classifier) |
| `0x4852f88` | `SaveLoad2::SLSystemImpl*` | `Fd4IoWorkerManager` (enum), `FD4_IO_WORKER_MGR_RVA` |

`0x4852f88` was settled by a technique worth reusing when xref counts are too low to
discriminate: follow the lazy-singleton initializer and read the vftable its constructor
assigns. Here `FUN_14240af40` -> `FUN_14240dee0`, whose first statement is
`*param_1 = SaveLoad2::SLSystemImpl::vftable`. `bootstrap_drive.rs` already had this right in
a comment while `constants.rs` asserted the opposite.

Still unresolved (need decompiles): `0x3d68078`, `0x3d856a0`, `0x7499e0`. Also unverified:
`RuntimeGlobalRva::Fd4IoPool = 0x4853048`, the sibling of the mis-titled entry above.

**Landed:** `0x67b750` renamed `CONTINUE_LOAD_RVA` -> `SAVE_WRITE_TO_SLOT_RVA`. It writes a
save; it does not load one. `er-save-suppress` had it right; `er-effects-rs` and
`er-save-loader` had it wrong. See bd `rva-67b750-is-save-write-not-continue-load-2026-08-01`
and the P1 bug filed for `DirectTraceSequence` calling the save writer as a load step.

**Also landed:** the four singleton globals above now have **exactly one literal definition
each**, in `er-game-base/src/rva.rs` (down from 5 / 4 / 5 / 2 across the workspace). This
finishes a consolidation that crate's own header already described as its purpose. It
required giving `er-input-harness-dll`, `er-reload-trace-dll` and `er-save-loader` an
`er-game-base` dependency -- which is not a new coupling but the one the crate was designed
for: its manifest says "Tier A (default) is ZERO external deps ... mirroring the zero-dep
mini-DLLs", so a default-features dep adds no transitive weight.

Two duplications hide behind one symptom, and the second is easy to miss: *different names*
for one address, and *the same name re-declared independently* in several crates
(`CS_MENU_MAN_GLOBAL_RVA` was written out in four). Both are the same drift hazard. A scan
that only groups by value catches both; one that groups by name catches neither.

---

## 2b. Delivery split: prove materiality, do not argue it

The goal is to ship pure refactors + tooling first, and isolate anything that changes a shipped
DLL into a stacked branch that carries the runtime-proof burden. Which side a commit falls on
is a **measurable** fact, not a judgement call, and `scripts/dll-code-fingerprint.py` measures it.

Whole-file `sha256` does not work: two builds of *identical, untouched* source differ. Measured
on the release DLL -- of 679,936 bytes of `.rdata`, exactly **10** differ across a no-op rebuild,
all inside a 36-byte span holding the `RSDS` CodeView PDB signature (fresh GUID per link) and a
build timestamp. Every other section, `.text` included, is byte-identical. So the tool masks the
COFF `TimeDateStamp`, the optional-header `CheckSum`, and every debug-directory entry plus the
CodeView blob it points at, then hashes each remaining section.

That yields a clean rule:

* **Not material** -- `.text` and friends byte-identical. A constant rename or a re-derivation
  cannot change codegen (names are compile-time), so this is the expected result for the whole
  RVA burn-down. Ships in the refactor PR, no runtime run needed.
* **Material** -- any section moves. Needs a runtime proof run and belongs in the stacked PR.

### Measured on this branch, not assumed

Building `main` and each commit **from the same directory** (see the limit below) and comparing:

| transition | `.text` | what changed |
|---|---|---|
| `main` -> `0dd8e83a` | **DIFF** | rename **plus one debug log string** (`debug(format!(...))` text) |
| `0dd8e83a` -> `c27bc7ea` | **SAME** | pure constant renames + derivations. `.data`, `.pdata`, `.reloc` also identical; only `.rdata` moved |
| `c27bc7ea` -> `5d6c823d` | **DIFF** | added an `er-game-base` dependency to three crates |
| `5d6c823d` -> `HEAD` | **SAME** | tooling only, no Rust touched |

Three calibration facts fall out, and none of them were obvious in advance:

1. **Pure constant renames and derivations leave `.text` byte-identical.** Proven, not argued.
   This is the bulk of the RVA burn-down, and it ships as a pure refactor.
2. **Changing a log string is material.** It is behaviorally inert and still moves `.text`.
3. **Adding a dependency is material even with zero behavior change** -- the dependency graph
   feeds `-Cmetadata`, which changes mangled symbol names throughout.

So "material to the binary" is NOT the same as "changes behavior", and this tool measures the
former. The practical rule: `.text` SAME is a hard proof of no codegen change; `.text` DIFF is a
prompt to classify the diff (string? symbol metadata? real logic?), not an automatic verdict
that a runtime run is required.

**Validity limit, learned by getting it wrong twice.** Compare only builds made in the SAME
directory. A build from a sibling worktree differed in 9.2% of `.text` at identical section
sizes. The literal build path is absent from the binary, so grepping for it does not detect the
problem -- checking for the path string and finding nothing is NOT evidence the comparison is
sound.

Two further consequences worth planning around:

1. **It turns "is this code dead?" into a measured question.** Section 0.4 claims ~6,900
   unreachable lines. Delete them and fingerprint: if `.text` is unchanged, the deadness is
   *proven*, and a deletion that large ships as a pure refactor instead of needing a runtime
   argument. If `.text` moves, the code was reachable and the claim was wrong. This is a far
   stronger gate than reading gate expressions, and it should be run before proposing that
   deletion as safe.

The known genuinely-material work is small: the `er-save-loader` save-writer call sites
(er-effects-rs-f2bd), which change behavior by construction and cannot be proven away.

---

## 3. What should merge into something that already exists

- **`er-save-loader`** -- the product carries a second BND4 transcription
  (`loading_cover_save_slot.rs`): 39 shared constant names, 38 byte-identical, twin
  algorithms. `parse_save_character_slots` already calls into the crate and then hands the
  body to its *own* reader. Direct blocker for `docs/plans/unify-loading-stats.md`.
- **`er-d3d12-compositor`** -- `present_overlay.rs:677-796` re-implements its private Present
  resolver with a field-for-field identical swapchain desc and a byte-identical
  `dummy_wndproc`. The product already links the crate.
- **`er-game-base`** -- three `mem.rs` UTF-16 helpers; moving them breaks a real cycle through
  `er-loading-portrait`. The two patch stubs are *not* free (that crate is deliberately
  zero-external-dep).
- **`er-hook`** -- `mh_install_hook_once`. Caveat: 200 `MhHook::new` sites vs 12
  `register_union_hook` repo-wide, so the union is the exception, not the rule; promoting the
  helper is cosmetic while 200 sites route around it.
- **`src/config.rs`** -- the surviving product levers out of `gating/`.

Also: `running_under_wine` exists **twice** with divergent bodies (`present_overlay.rs:506`
caches, `save_redirect/file_ops.rs:300` re-probes every call), both glob-exported into one
scope -- plus `is_native_windows` and a fourth inlined copy. This must be resolved *before*
the `include!` -> `mod` conversion, since it is exactly the E0659 class that conversion has to
survive.

---

## 4. What needs a new module or crate

- **`experiments/save_picker/`** -- 7 files, 7,325 lines, reaching the compiler through three
  unrelated paths. Do this *before* any crate slice: it turns each move from an untangling
  job into a file move plus a `pub(crate)` audit.
- **A save-flow module out of `lifecycle.rs`** -- band `101-1370` plus its test module is
  1,486 lines, zero foreign items, and a one-symbol/one-call-site outbound surface.
- **`loading_cover_save_slot.rs` -> 4** -- it is four concerns, and `er-save-picker`'s
  `SaveSlotInfo` / `parse_save_character_slots` are wedged inside the quit-menu swap ledger.
- **`er-loading-cover`** -- bd candidate, blocked on the `constants_moved.rs` layering (the
  same blocker as the load-drive crate). `boot_progress.rs` has five layer seams, none on a
  file boundary; its semaphore->label state machine (`268-1140`) contains zero D3D12 tokens.

---

## 5. Suggested order

Dependencies here are real; several steps undo each other out of sequence.

1. **Collapse the duplicate RVAs.** Mechanical, no API design, largest correctness hazard.
   *(In progress -- `0x67b750` landed.)*
2. **Tier-0 deletions** (~6,900 lines). Every later measurement is wrong until this lands.
   Extract the stranded live functions in `own_load/loaders.rs` before deleting around them.
3. **Resolve the cross-shim duplicate exports**, then `include!` -> `mod`.
4. **The trivial motion batch** -- cheapest, and it makes the crate slices whole-file moves.
5. **`loading_cover_save_slot.rs` 4-way split** -- unblocks three things at once.
6. **`experiments/save_picker/` + the save-flow module** -- preconditions for the crate slices.
7. **`constants_moved.rs` layering** -- the blocker shared by `er-loading-cover` and load-drive.
8. **The er-save-picker / er-quit-menu slices**, with the SS1 corrections applied first.

## 6. Open questions -- decisions, not analysis

- Keep or delete the ~1,950-line System>Quit repro autopilot? `save-picker-crate-extraction.md`
  SS6.3 explicitly defers this to a human and it blocks S9. The bd record is the evidence, not
  the ruling.
- Is the `missing_save_selection_pending()` disjunct reachable at boot? Needs one RAM-oracle
  run; the static corroboration collapsed under verification.
- Does the product bundle `er-quit-menu`, or is it listed-only? Plan SS6.2. This silently
  determines whether the browse-row text renderer and the CreateFileW->crate call are legal.
- Should the sanctioned marker-file diagnostic carve-out survive? Both checkers pass with it
  today by design. Policy call, not a defect.
