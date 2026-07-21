# Plan: Rebuild the character-load on the native vanilla Continue path

## Context

**Why this change.** The product currently loads a character by driving the in-world
pause menu / MoveMapStep (the "click-load" path). This session proved that path never
reaches genuine movability: the character does not walk, the menu does not open, and the
run tears down before load2 finishes. The user's timeline argument is decisive — if 60
frames of movement + a real load had happened, the on-screen time would be far shorter
than what is actually observed.

**Root evidence (verified this session).** `oracle_stepfinish_mms_state` (the mms 1→18
progression the product path shows) is **computed from a title-owner vtable scan**
(`TITLE_OWNER_PTR`→+0x2e8→mms), which reads a **stale owner on the native-continue path** —
it is not a game-global field (and it also *sets* `ORACLE_RELIABLE_MMS_PTR`, consumed by the
product finalize-drive being disabled). In a
genuine **vanilla native Continue** run (`vanilla-continue-20260720-174933`,
telemetry-only, user drove the real Continue button), `oracle_stepfinish_mms_state` is
`-1` for **all 546 samples across 280s** — it never enters the 1→18 progression at all,
yet the character loads and is movable. So mms18 was never a validated movability signal;
it is a product-path artifact and cannot be diffed against the ground-truth movable run.

**The vanilla movable signature (game-global, valid on both paths):**
- `oracle_player_present` false→true (vanilla: t≈34.9s)
- `oracle_char_name` set to the loaded character (vanilla: "angrE")
- `oracle_play_time_live` false→true
- `oracle_now_loading` clears (load completes)
- `oracle_chr_render_group_enabled` false→**true** (vanilla: t≈40.5s, ~5.6s after present)

**Goal.** Disable the product's finalize-forcing load drive (the `menuData+0x5d` writes in
`product_core_autoload_tick` — NOT the sq-repro menu-click, which ships inert); keep every test-harness piece
(movement proof, input-harness DLL, kb+mouse disable, oracles); rebuild the "continue
system" from the ground up so it triggers the **native Continue load path** the game's own
Continue button uses, and validate success ONLY with the game-global vanilla semaphores
above (never mms18). Then the existing movement proof + switch can chain load1→load2→load3.

**Native continue path (from RE, bd `real-continue-load-driver-recipe`).** Menu-load and
autoload converge on ONE sync deserialize `0x14067b290` (writes slot→GameMan+0xac0, saved
map→GameMan+0xc30, applies stats/inventory). It is reached via dispatcher `0x140afb880`
(gated `[0x143d856a0]==0`) + driver `0x140afbac4`, from parent `0x140aff640`. The
Continue lane = getter `0x1406793d0` (+0xb72 force flag) → `current_slot_load 0x14067b570`
(async, reads slot `[GameMan+0xac0]`). The whole graph is **session-scoped** (CSFeMan
`0x143d6b880` + session `0x1447ef360` must be live — true once in-world / at a bootstrapped
title, which is the exact context we now have because the FIRST autoload already succeeds).

**Three load paths in play (must not conflate).**
- **P1 — vanilla native Continue** (user clicks Continue at title): WORKS, reaches the
  movable signature above. The reference we are rebuilding toward.
- **P2 — product own-load / own-stepper** (direct-build ProfileLoadDialog → submit
  `0x14067b1a0` → deserialize `0x67b290` → `SetState5 0x140b0e180`): the boot autoload;
  bd `FULL-AUTOLOAD-PIPELINE-FIRES` shows it fires end-to-end and the world STARTS
  streaming (MoveMapStep state 13), but full in-world spawn past the WorldResWait
  block-streaming wall was never confirmed (runs died at the 30s cap). Save-safe, zero-input.
- **P2b — product `product_core_autoload_tick` finalize-forcing** (the SHIPPING load): does a
  native Continue commit AND actively writes `menuData+0x5d=1` to FORCE MoveMapStep mms 18→19→20.
  The forcing produces the product-only mms path and never reaches movability. **This
  finalize-forcing is the real thing to disable** (gate `product_autoload_enabled()`; see step 1).
- **P3 — product system_quit_repro menu-click** (SendInput drives the in-world pause menu): the
  load2/load3 *reload* driver, but it is harness-gated (`harness_dll_present()`) and **ships
  INERT** — it is NOT the shipping load and is NOT the primary disable target.

**Feasibility crux.** Rebuilding on P1 means triggering the native Continue dispatch
programmatically (force-flag `GameMan+0xb72` → dispatcher `0x140afb880` →
`current_slot_load 0x14067b570` → deserialize `0x67b290`) for the selected slot, in a
session-live context, and letting the game's OWN in-game flow complete the world stream —
rather than hand-driving MoveMapStep (P3) or the direct-build stepper (P2). The open risk
is whether a programmatic native trigger completes the world stream to movability the way
the user's manual Continue does; the vanilla run proves the path itself completes, so the
question is purely our trigger + context, which the exploration must pin down.

## Implementation plan (phased)

**Why phased.** The finalize-forcing (`menuData+0x5d`) was added *because* the product's
load reaches mms18 and does not finalize naturally, while the vanilla native Continue
finalizes and reaches movability with no forcing. So the FIRST thing to prove is the core
hypothesis — that a native commit WITHOUT forcing reaches movability — before building the
full three-load cycle. Measure it with game-global oracles only.

**Phase A — prove the hypothesis on load1 (de-risk).** Disable `product_autoload_enabled()`
(kills both finalize drives + the current commit); add a clean native-continue trigger for
the first character (reuse `native_fullread_tick` SUBMIT→DRAIN→DESER→GUARD→`continue_confirm`,
or the corrected b72-arm `native_autoload_once`), with NO `menuData+0x5d` writes; repoint the
oracle/teardown (step 4) to the game-global movable signature. Run one boot and OBSERVE:
does load1 reach player_present + chr_render_group_enabled=true + now_loading-clear +
play_time_live + a 60-frame move verdict, WITHOUT forcing?
- **If yes** → the forcing was the disease; go to Phase B.
- **If no (finalize stalls without forcing)** → the real blocker is the game's own finalize
  not completing in the product's trigger context; that becomes the focused RE target, now
  measured correctly (game-global), not via the stale-owner mms path. This is where the
  session's mms18 struggle actually lives — surface it honestly rather than re-forcing.

**Phase B — native reload cycle (load2, load3).** Build the native System→Quit→Continue the
user does by hand (return-to-title, then native Continue for the same slot), replacing
`switch_slot_arm_programmatic`'s `menuData+0x5d` teardown write. Bump
`SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT` once per committed load (keep the epoch
clock). Chain load1→load2→load3, movement-proving each; keep sq-repro's menu-nav available
only if a genuine UI interaction must be validated, else leave it inert.

**Phase C — framerate + final verification.** Establish load1 framerate as the baseline;
assert load2/load3 average framerate parity; capture the frame-by-frame video the user
offered for the movable window.

### Detailed steps

1. **Disable the product finalize-forcing load drive.** The shipping product load is NOT the
   SendInput menu-click (that is `system_quit_repro_tick`, harness-gated by
   `harness_dll_present()`, ships INERT). It is `title/title_tick_cover.rs::product_core_autoload_tick`
   (:1205) which does (a) the first-char native Continue commit AND (b) actively FORCES
   MoveMapStep finalize by writing `menuData+0x5d=1` — the "IN-WORLD FINALIZE DRIVE" (:1281-1300,
   write at :1294) and "MMS18 RT5D DRIVE" (:2003-2028, write at :2023) — to shove mms 18→19→20.
   **That forcing is the divergence from vanilla** (vanilla finalizes naturally, never enters
   the product mms path). Single cleanest choke point: **`env_flags.rs::product_autoload_enabled()`**
   (:54-56 = `PRODUCT_AUTOLOAD_ARMED == 1`); forcing it false kills both finalize drives and the
   current commit at both `product_core_autoload_tick` call sites (task_registration.rs:177, :493).
   Residual not covered by that flip, to handle explicitly: the one-shot `menuData+0x5d=1` in
   `switch_slot_arm_programmatic` (:3099); and `autoload_disabled()` is hardcoded false (not a
   usable kill switch). Note: disabling this also disables load1's commit, so the rebuild must
   provide load1's native commit too (§3).
2. **Keep harness scaffolding intact** (all gated on `harness_dll_present()` =
   `GetModuleHandleA("er_input_harness_dll.dll")`, env_flags.rs:514 — a presence gate, not
   env/marker). Untouched:
   - `experiments/can_move_probe.rs` — the whole movability proof (pad-poll inject, focus
     override, epoch reset, verdict latch).
   - `lib_parts/dll_entry_parts/task_registration.rs:353-371` — the in-world probe call + its
     `!sq_menu_nav` gate (probe runs in WAIT_WORLD/WAIT_RELOAD/DONE, suppressed during menu-nav).
   - `experiments/input_block.rs` — `enforce_kbmouse_game_input_disable` (:991), RawInput
     contamination counter (:750-913), DInput/XInput block + can-move XInput lane.
   - `experiments/gating/env_flags.rs:469-529` — the harness gates.
   - `crates/er-input-harness-dll/` — the separate self-drive DLL (drives ER's OWN input memory
     `inputmgr+0x90+eventId` + `DLUID+0x88d`, not SendInput/XInput).
   - `telemetry/runtime_oracles/write_oracle.rs` movement + presence emitters.
   - **CONSTRAINT:** the load-epoch clock `SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT`
     increments in `system_quit_repro_guards.rs:system_quit_continue_confirm_hook` (:1611-1697),
     once per fresh deserialize / SetState5 commit. The rebuilt load MUST still bump it per
     committed load or the probe never resets and the capture script never advances epochs.
   - Scripts `run-samechar-3x-threedll.sh` + `capture-samechar-3x.py` (observer + teardown model).
3. **Rebuild the continue trigger on the native path** — reuse existing native-load machinery
   (mapped by Explore agent 2). The load FSM is `GameMan+0xb80`: 0 IDLE→1 OPENING→2 READING→3
   RESIDENT; the sync deserialize is `0x67b290` (writes `GameMan+0xc30` saved-map, applies
   char); the native commit is `continue_confirm 0x140b0e180` → SetState5.

   **Reusable, proven, LIVE machinery (both end in the native commit):**
   - `continue_load/product_continue.rs::product_continue_autoload_tick` (:266) — native
     Continue-**row** submit (`MENU_ITEM_SUBMIT_RVA`, the row's own event ABI, `simulated_
     button_presses_total=0`) → `continue_confirm`/SetState5. Gated `product_autoload_enabled()`.
   - `continue_load/slot_resolution.rs::native_fullread_tick` (:115) — SUBMIT (`b78`=slot,
     `set_save_slot`→`ac0`, `0x67b1a0`) → DRAIN (`0x679510`/`0x679180` to `b80==3`) → DESER
     (`0x67b290`) → GUARD → CONFIRM (`continue_confirm`→SetState5). LIVE for the direct-file /
     System-Quit switch path.

   **Reusable, dormant, cleaner native triggers (armed-off today):**
   - `title/product_autoload_gates.rs::native_autoload_once` (:247) — *corrected recipe*: set
     `GameMan+0xac0`=slot (via `set_save_slot 0x67a810`) + `GameMan+0xb72`=1 (arm), let the
     save-mgr per-frame update `0x14067f5d0` run the load. Gate `native_autoload_enabled()`=false.
   - `continue_load/slot_resolution.rs::continue_drive_tick` (:675) — drives dispatcher
     `0x140afb880` with `set_save_slot`; never writes the latch. Gate `continue_drive_enabled()`=false.

   **DEAD — do NOT resurrect (documented aborts/crashes):** writing the latch/force-flag
   `0x143d856a0` (=SELECTBOT_LOAD_GATE / TITLE_ACCEPT_LATCH / ENDING_REQUEST_FORCE_FLAG) —
   "aborts the load" / "skipped bookkeeping → crash" (product_core_own_stepper.rs:772-781, :434;
   product_autoload_gates.rs:279); the native_continue Continue-node scan ("DEAD CODE",
   product_core_own_stepper.rs:683). The recurring crash guard is the CSGaitemImp free-queue AV
   at `0x67141a` (`own_load_reset_gaitem_singleton`).

   Decision to make in Phase 2: whether the rebuild = (a) keep `product_continue_autoload_tick`
   / `native_fullread_tick` (already native, already end in SetState5) and only change WHAT
   drives load2/load3 to it (retire the sq-repro menu-click), or (b) a from-scratch minimal
   native driver (set `ac0`+`b72`, poll `b80` 0→3, let the game's own in-game flow finish),
   validated purely by game-global semaphores.
4. **Re-point the load/movability oracle to game-global signals.** All GAME-GLOBAL (read game
   singletons, valid on a native continue): `oracle_player_present` (WorldChrMan/PlayerIns),
   `oracle_char_name` (GameDataMan+0x08→PGD+0x9c), `oracle_now_loading`
   (`[base+0x3d60ec8]+0xED`, a load-COMPLETE latch that lingers), `oracle_play_time_live`
   (GameDataMan+0xa0, ≥1000ms/epoch — necessary-not-sufficient), `oracle_chr_render_group_enabled`
   (ChrIns+0x1c4), `oracle_saved_map_c30` (GameMan+0xc30). **Strongest success signal:**
   `oracle_harness_move_verdict==1` / `oracle_can_move` (the movement proof itself,
   write_oracle.rs:373-398). **Retire** `oracle_stepfinish_mms_state` /
   `finalize_substate_12a` as progress/teardown oracles — they are COMPUTED from the
   `TITLE_OWNER_PTR` vtable scan (→+0x2e8→mms), which reads a STALE owner on the
   native-continue/switch path (write_oracle.rs:41-44 warns of this); they also set
   `ORACLE_RELIABLE_MMS_PTR` consumed by the in-world finalize drive (title_tick_cover.rs:409,
   :1252) — part of the product path being disabled. The capture script's mms-based
   teardown/stall/bootup-divergence checks (`capture-samechar-3x.py`) must switch to the
   game-global signals + move verdict.
5. **Chain the loads** — once a native load reaches the vanilla movable signature and the
   movement proof passes 60 frames, trigger the next native Continue (load2), then load3.

## Open questions / decisions
- **Native trigger for Phase A** (recommendation): reuse `native_fullread_tick`
  (SUBMIT→DRAIN→DESER→GUARD→`continue_confirm`; already live for the switch path, ends in the
  native SetState5 commit) with the `menuData+0x5d` finalize-forcing stripped — rather than a
  from-scratch driver — because it is proven to commit. Alternative: the b72-arm
  `native_autoload_once` (let the save-mgr per-frame update run the load). This is an
  implementation choice, not a blocker.
- **The Phase A result is a genuine unknown**: whether the game finalizes to movability
  naturally once the forcing is removed. The plan's value is measuring it correctly
  (game-global) instead of masking it with `menuData+0x5d`. Do NOT re-add forcing to make a
  green run — if finalize stalls, that is the real finding to pursue.
- **Reload cycle (Phase B)** may need additional RE of the game's native return-title→Continue
  sequence (the user's manual System→Quit→Continue); scope after Phase A confirms the load.

## Verification
- **Phase A:** one boot via `scripts/run-samechar-3x-threedll.sh` (kb+mouse disabled, harness
  present, no forcing). Assert load1 reaches the game-global movable signature
  (`oracle_player_present` + `oracle_chr_render_group_enabled=true` + `oracle_now_loading`
  cleared + `oracle_play_time_live` rising) and `oracle_harness_move_verdict==1` (60-frame
  move proof). Compare the game-global transition sequence against the vanilla imprint
  (`data/oracle/imprints.db` set 2). Cross-check with the frame-by-frame video the user offered.
- **Phase B:** full run to epoch 3; assert load1→load2→load3 each hit the movable signature +
  move verdict, with NO `menuData+0x5d` forcing and NO product mms path; epoch clock advances.
- **Phase C:** load1 framerate baseline; load2/load3 average-framerate parity; final video.
- **Build/quality gate:** `cargo xwin build --release --target x86_64-pc-windows-msvc -p
  er-effects-rs` (verify DLL sha changed), `bash scripts/check.sh`. Commit immediately after
  each runtime validation run (fix + harness + the run's evidence).

## Deliverable location
This plan is mirrored to `docs/plans/native-continue-rebuild.md` in the repo (tracked) on
approval, per the user's request that plans live in the source tree.
