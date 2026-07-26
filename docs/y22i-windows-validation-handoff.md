# y22i -- Native-Windows validation handoff

**For:** a fresh Claude session running on the dual-boot native Windows install (real AMD RX 6900 XT).
**Goal:** get the one thing this whole effort is missing -- a **runtime confirmation on real Windows + real GPU** that the guard build stops the `0xec95d1` crash and reaches the playable world.

Everything below is self-contained. Read it, then execute "Validation procedure."

---

## 1. What the bug is

With our DLL loaded, native-Windows users access-violate inside the game's **own** Scaleform (GFx) D3D12 `CBV_SRV_UAV` descriptor-heap ring/sub-allocator advance.

- Fault function (deobf/live VA): `0x140ec9530`, signature `advance(this=rcx, count=edx)`.
- It faults at `0x140ec95d1` (`mov rcx,[rax+0x20]`) -> crash **RVA `0xec95d1`, fault_addr `0x20`**, exception `0xc0000005`.
- `rax` there is the "current descriptor-heap page provider" = `*(this+0x38)`. When that pointer is **null**, the deref faults.

This is **Elden Ring 1.16.1 (app version 2.6.2.0)**. The RVAs are only valid for that build -- verify the Windows install's ER version matches before trusting any addresses.

## 2. Why it's only on native Windows (don't waste time re-proving this)

- **Linux/Proton (vkd3d) masks it** -- the dev machine and the user's Steam Deck run clean. vkd3d translates D3D12->Vulkan and doesn't expose the null window.
- **A GPU-less VM cannot reach it** -- already tried, exhaustively. Fixing the VC-runtime wall and defeating the anti-VM check got vanilla ER to launch, but with only software D3D12 (WARP) it rendered **zero frames** and never reached the Scaleform code. See bd `y22i-vm-validation-blocked-warp-render-wall`. Do **not** retry the VM.
- Native Windows on the real AMD driver is the only environment where both the crash and the render path exist.

## 3. The static-RE verdict (why the fix is believed correct)

Two independent RE traces, reconciled + adversarially checked (bd `y22i-vm-validation-blocked-warp-render-wall`), concluded the crash is **game-side and driver-independent**:

> The fault requires `capacity(this+0x20)==0` **and** `provider(this+0x38)==null`. Capacity is written by the game's own code (binder `FUN_140ec93e0` @ `0x140ec941a`) **before** any D3D12 `CreateDescriptorHeap` call. So a graphics-driver-level create failure would leave capacity nonzero and take the fast path -- it **cannot** produce this fault. The null-provider is an **advance-before-seed ordering window** our DLL sensitizes (it keeps a loading/ProfileSelect Scaleform surface rendering across a HAL clear/reset), reproducible under any real D3D12 backend.

So the guard is expected to be correct; native Windows is to **confirm** it, not to re-derive it.

## 4. The fix (the guard) -- already in this branch

Branch: `fix/windows-gx-resource-null-crash`. The guard detours `0x140ec9530` and **bails when the provider is null** before the game can deref it; otherwise it's a transparent passthrough. Always-on (installed at DLL attach, not feature-gated).

Files:
- `crates/er-effects-rs/src/constants/anti_debug.rs` -- constants: `SCALEFORM_DESC_ADVANCE_RVA = 0xec9530`, `SCALEFORM_DESC_PROVIDER_OFFSET = 0x38`, plus `SCALEFORM_DESC_ADVANCE_ORIG/INSTALLED` and `SCALEFORM_DESC_PROVIDER_NULL_HITS` atomics.
- `crates/er-effects-rs/src/experiments/startup_hooks/scaleform_descriptor_guard.rs` -- the detour `scaleform_descriptor_advance_hook` + `install_scaleform_descriptor_guard()`.
- `crates/er-effects-rs/src/experiments/startup_hooks.rs` -- `include!` of the guard file.
- `crates/er-effects-rs/src/constants/system_quit.rs` -- `START_SCALEFORM_GUARD: Once`.
- `crates/er-effects-rs/src/experiments/lifecycle.rs` -- unconditional install spawn at attach.
- `crates/er-effects-rs/src/telemetry/runtime_oracles/write_game_module_oracles.rs` -- telemetry: `oracle_scaleform_desc_guard_installed`, `oracle_scaleform_desc_provider_null_hits`.

Reference guard DLL built on Linux: **md5 `d416b13242a8fca4086a9b37b524fa2f`** (cargo-xwin). On Windows you'll rebuild and get a different md5 -- that's fine; the source is what matters.

## 5. The correct A/B -- note the control, it is NOT vanilla

The crash is **our-DLL-induced**. Vanilla Elden Ring (no DLL) does **not** crash at `0xec95d1` -- so vanilla is the wrong control for this bug (it was only used in the VM to test whether WARP renders at all).

The real A/B is **unguarded-DLL vs guarded-DLL**:

- **Control (reproduce):** our DLL *without* the Scaleform guard = the branch state **before** the guard commit (`git log` this branch; the guard is the top functional commit -- check out its parent, or `git revert`/stash the guard files, and build). This build should reproduce the `0xec95d1` AV that Windows users reported.
- **Test (fix):** the guarded DLL (current HEAD of this branch). Should **not** crash at `0xec95d1`, should reach the playable world, and the guard should have fired.

## 6. Validation procedure (execute on Windows)

Prereqs to confirm/set up: Rust MSVC toolchain (`rustup`, VS Build Tools), the repo (git pull this branch), me3, Steam + Elden Ring **1.16.1 / 2.6.2.0**, a real save.

1. **Build the guarded DLL natively** (Windows uses plain cargo + MSVC, *not* cargo-xwin):
   `cargo build --release --target x86_64-pc-windows-msvc`
   -> `target/x86_64-pc-windows-msvc/release/er_effects_rs.dll`
2. **Build the control DLL** (unguarded): check out the guard commit's parent (or stash the six files in SS4), `cargo build ...` again, save that DLL as `er_effects_rs_control.dll`.
3. **Two me3 profiles** (offline; me3 bypasses EAC):
   - `guard.me3` -> `[[natives]] path = '...\er_effects_rs.dll'`
   - `control.me3` -> `[[natives]] path = '...\er_effects_rs_control.dll'`
4. **Delete** `<ELDEN RING>\Game\er-effects-crash-log.txt` before each run so you read only that run.
5. **Run the control:** `me3 launch --profile control.me3`. Play/idle a few minutes into the load/world transition. Expect an access-violation -- check `<Game>\er-effects-crash-log.txt` for `rva=0x67141a`... no: for **`0xec95d1`** (`access-violation rva=0xec95d1 ... fault_addr=0x20`), and Windows WER (`AppCrash_eldenring.exe`) with Fault Offset near `0xec95d1`. This confirms the crash reproduces on your hardware.
6. **Run the guarded build:** `me3 launch --profile guard.me3`. Same actions.
7. **Read the evidence yourself** from `<Game>\er-effects-crash-log.txt` and `<Game>\er-effects-telemetry.json`.

## 7. Success criteria

- **Control** produced the `0xec95d1` AV (proves the crash reproduces here -- makes the test meaningful).
- **Guarded build**: NO `0xec95d1` AV; the game advances past the loading/ProfileSelect transition into the world; and `er-effects-telemetry.json` shows `oracle_scaleform_desc_guard_installed = 1` and **`oracle_scaleform_desc_provider_null_hits > 0`** -- direct proof the guard caught the exact null condition that was faulting.
- Run each arm ~3x to rule out intermittency (the original report was "after no input" -- the fault is timing-sensitive, so the control may not fire every run; only a repro is decisive, a non-repro is uninformative).

If the guarded build shows `provider_null_hits > 0` and no `0xec95d1` AV while the control crashes there -- the fix is **proven** on real Windows. Record the result in bd and mark issue `er-effects-rs-y22i`.

## 8. If something is off

- **Guard didn't install** (`oracle_scaleform_desc_guard_installed = 0`): almost always an ER-version/RVA mismatch (`SCALEFORM_DESC_ADVANCE_RVA = 0xec9530` is for 1.16.1/2.6.2.0). Re-verify the game version and re-RE `0x140ec9530` if it differs.
- **Guard installed but the game still crashes at `0xec95d1`**: the null-check offset/target is wrong for this build, or a second write site is involved -- RE the writers of `[obj+0x38]` (details in bd `y22i-vm-validation-blocked-warp-render-wall`) and adjust.
- **A different crash appears**: RE it fresh; note the first fix (GX offscreen `0x1e90290`, oracle `oracle_portrait_pump_block_off_resource`) is already in this branch.

## 9. Pointers

- Issue: `er-effects-rs-y22i` (run `bd show er-effects-rs-y22i`).
- bd memories: `y22i-vm-validation-blocked-warp-render-wall`, `y22i-guard-arm-attempt-warp-wall`.
- Prior VM tooling (Linux-side, not needed on Windows): `scripts/vm-sendkeys.py`, `target/vm-stage/`.
- The DLL writes `er-effects-crash-log.txt` and `er-effects-telemetry.json` into the Elden Ring `Game\` directory.
