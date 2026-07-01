# HANDOFF -- loading-screen portrait + autoload null-slot bug (2026-06-30)

Branch `spike/boot-parallelization`. Commits this session (all pushed):
`e5a1e36` swapchain find + VMT Present hook * `700f0c8` composite (draw EARLY) + loading gate + cutscene RE * `eed9fb6` active-slot resolver + Load-Game guard.

## THE GOAL
Draw the user's **real character portrait** on the Elden Ring loading screen (zero-input autoload, single
LazyLoader DLL, save-safe, product-inert behind `portrait_lookat_enabled()`).

## WHAT WORKS (display path -- DONE, runtime-proven)
The D3D12 Present-overlay composites a captured RGBA onto the swapchain backbuffer EARLY and without crashing:
- `find_game_swapchain` (present_overlay.rs): live `IDXGISwapChain3*` via the **g_GxDrawContext chain** --
  `*(base+0x47ef360)` -> `+0x128` -> `*entry[0]` -> `*output`. QI-validated. (NOT under CSGraphics.)
- **VMT-slot swap** `vtable_swap_slot` for Present(8)/Present1(22) -- MinHook reports MH_OK on Wine dxgi but
  never fires (W^X). Runtime: `PRESENT(8) hook FIRED`, backbuffer 1280x720 format=28 (R8G8B8A8_UNORM).
- `composite_portrait_on_swapchain` (gpu_readback.rs): DEFAULT-heap R8G8B8A8 portrait texture + **our OWN**
  private DIRECT queue (submitting on the GAME queue crashed vkd3d) + CPU fence wait; per-frame
  `CopyTextureRegion(backbuffer <- portrait, centered)`. Runtime: `portrait COMPOSITED onto backbuffer`.
- Loading gate fires ~+15.7s (gated on `LOADING_BG_TEXTURE_REDIRECT_COMMITS` = the forge / menu->world
  transition cover, NOT the late fake_loading/now_loading singletons). World-ready latch stops it in-world.

## THE REAL BLOCKER (this is what to fix next)
**The autoload loads a NULL slot, so there is no real character to draw.** Verified via memory reads:
`LOAD-CORRECTNESS name="_" level=9 runes=0 stats=[15,10,11,14,13,9,9,7]`, `model_ins=0x0`, gate
`c30=0xa010000` (new-game m10_01 map). That is the **new-game default character** -> the **intro cutscene**
plays -> the portrait is a flat dark-gray clear (the composite faithfully draws an empty render).

User's words: *"slot 0 isn't a fresh character, it's a NULL character. My character doesn't load late -- you
load a null character early."* Banon (level 150) lives in one of the **active** slots (`save-files/150-Banon/
ER0000.sl2`); slot 0 is null.

### Active-slot oracle (CRITICAL -- contamination caveat)
- `profile_summary` = `*(game_data_man_ptr + SLOT_MANAGER_CONTAINER_OFFSET=0x78)`; GameDataMan global RVA
  `0x3d5df38`.
- The per-slot **active BYTE** at `profile_summary+0x8+slot` is **CONTAMINATED** -- the DLL itself writes
  byte=1 (`PROFILE_SLOT_ACTIVATE` 0x262250 in own_stepper.rs:175/381, `seed` continue_load.rs:737), so it
  reads `0x0101010101010101` (all active) even for null slots. DO NOT use it for decisions.
- USE the **RECORD** at `profile_summary+0x18+slot*0x2a0` (name@+0x0, level@+0x24) via the existing
  `profile_slot_fingerprint(slot)` (continue_load.rs:1262): `is_real = level>=1 && non-empty name`. Our code
  never overwrites the RECORD. This is the single trustworthy "slot has a real character" signal.
- WARNING: in the smoke ALL 10 `profile-slot-dump`s show models -- `force-profile-render` builds a default
  model per slot, so "renderer has a model"  "slot has a character". Only the RECORD counts.

## WHAT I IMPLEMENTED (commit eed9fb6) -- PARTIAL, does NOT fix the smoke
- `best_active_slot()` + `resolve_active_load_slot(configured)` (continue_load.rs, after
  profile_slot_fingerprint ~line 1300): pick the highest-level real slot via `profile_slot_fingerprint`;
  return `OWN_STEPPER_SLOT_NONE` if none. READY TO REUSE.
- `fire_tfc_continue` (title.rs:609): resolves an active slot, REFUSES to fire if none active.

**Why it didn't help the smoke:** the postcontinue/`direct_menu_load` smoke does NOT use `fire_tfc_continue`.
It loads via `network_check_job_run_hook` (startup_hooks.rs:1132, hook on 0x140821310) forcing
`MenuJobResult(Continue)` -> the **NATIVE Continue** flow -> `SetState(5)` (native SetState 0x140b0d960), which
loads the **last-played** character (`game_save_slot=-1`, `gm_ac0=-1`) = the null slot. The native Continue
never sets a slot. I patched the wrong path.

## NEXT STEP (do this)
Apply the active-slot resolution to the **native Continue** path:
1. Once `profile_summary` RECORDS are populated (title/profile stage -- records exist before the load), and
   BEFORE `SetState(5)` fires, set the game's load slot to `best_active_slot()` via the native slot
   machinery: `GameMan+0xb78` (slot write) / `set_save_slot` 0x14067a810 -> `GameMan+0xac0`, used by
   `deserialize` 0x14067b290(slot). i.e. force the Continue to target the active slot, not last-played(-1).
   - Candidate injection point: a recurring tick in the autoload path (lib.rs:1033 process_autoload_request /
     mod.rs:743) or just before/inside the forced-Continue flow, gated on `profile_summary` ready.
   - ALTERNATIVELY route the autoload to the slot-specific path (the `own_stepper` ACTIVATE -> ProfileLoadDialog
     -> `deserialize(slot)` path from bd `AUTOLOAD-WORKS-...` which DID load a specific slot -- point it at
     `best_active_slot()` instead of slot 0). NOTE: own_stepper `activate()` (own_stepper.rs:172/378) must be
     gated behind `profile_slot_fingerprint(want_slot).0` so it stops writing the active byte on null slots.
2. Then implement the rest of the 5 guards (design in the workflow output, see below).

## REMAINING 5-GUARD WORK (design: workflow wf_21765827-7a9, full output at
`/tmp/claude-1000/-home-banon-projects-er-effects-rs/53b18f36-7ba9-4628-a781-c3955aa85f00/tasks/w3rlnv3xc.output`)
- **G2 save-loaded**: rework `save_load_watchdog` (continue_load.rs:984) from the contaminated +0x8 qword to
  `char_fingerprint(base).is_real` (continue_load.rs:1369) + `c30 != 0xa010000`; abort loudly on a null mount
  (don't wait 900 frames).
- **G3 any-slot-active**: pre-load gate (lib.rs:1033 / mod.rs:743): `any_real = OR over 0..10 of
  profile_slot_fingerprint(s).0`; abort fast if none (no 15s-late watchdog, no intro).
- **G4 never-load-if-none / slot selection**: `native_fullread_slot` (continue_load.rs:1029) -- validate via
  fingerprint, fall back to `best_active_slot`, return NONE if none. Neutralize `FULLREAD_DEFAULT_SLOT=0`
  (constants.rs:3466 -> OWN_STEPPER_SLOT_NONE).
- **G1 save-present** (hardening, optional): offline `.sl2` BND4 slot parse at save_redirect.rs:213 + script
  preflight to reject a null-slot gold pre-launch.
- **G5 fail-fast-no-env**: ALREADY CORRECT -- `enforce_save_override_or_abort` aborts at DllMain
  (save_redirect.rs:273) when ER_EFFECTS_SAVE_FILE is absent.

## CUTSCENE SEMAPHORE (regression detector -- RE done, bd `cutscene-in-progress-semaphore-RE-2026-06-30`)
`GLOBAL_CSRemo = *(base+0x3d6ea58)`; `in_cutscene = (*(*(GLOBAL_CSRemo+0x8)+0x14c)) & 1`. EMEVD opcode 2002
(`Cutscene2002` dump 0x140572720) = play-cutscene enqueue. A cutscene firing during a load = the red flag.
Wire as `oracle_in_cutscene` to prove the fix (in_cutscene must be 0 after the load).

## VALIDATE
`bash scripts/run-postcontinue-lookat-smoke.sh` (45s cap, autoload ON). GAME_DIR flags
(`$HOME/.local/share/Steam/steamapps/common/ELDEN RING/Game`): er-effects-portrait-lookat.txt,
er-effects-portrait-real-pixels.txt, er-effects-experimental-direct-menu-load.txt,
er-effects-force-profile-render.txt, er-effects-portrait-lookat-selftest.txt; NO er-effects-no-autoload.txt.
Steam must be up. Build: `cargo xwin build --release --target x86_64-pc-windows-msvc`. fmt:
`cargo fmt --check -p er-effects-rs`.
**Success = `LOAD-CORRECTNESS` shows the REAL Banon (name non-empty, level 150, runes>0), in_cutscene=0, and
the loading-screen-portrait screenshot shows the real face** -- NOT name="_" level=9.
Artifacts: `target/runtime-probe/postcontinue-lookat-smoke/` (er-effects-autoload-debug.log,
er-effects-telemetry.json, loading-screen-portrait-screenshot.jpg).
