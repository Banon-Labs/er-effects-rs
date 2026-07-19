# Goal: Repeatable Multi-Save Character Loading (Acceptance Criteria)

**Status:** **OPEN — reopened 2026-07-18.** An earlier "vanilla core PROVEN" claim was a **false
pass**: the harness verified logical load (identity+stats+gear+present) but not that the character
actually *rendered and the world resumed*, so it passed a reload that was frozen and invisible in a
live product run. §4 has been tightened with a hard render gate + a stable-before-next sequencing
gate; the goal is not met until a run passes the revised gate. **Authored:** 2026-07-18 from a live
goal-refinement Q&A with the user; **revised** 2026-07-18 after the live product-route freeze.
**Invoke a fresh session with:** `/goal complete the acceptance criteria of ./docs/goals/repeatable-multi-save-load-acceptance.md`

> Read the linked `bd` memories FIRST (see [Context to load](#context-to-load-read-first)).
> This document is the single source of truth for what "done" means. It intentionally does
> **not** over-specify semaphores; it states the *floor* and leaves implementation latitude.

---

## 1. The goal, in spirit

On **Windows**, with our DLL loaded via me3, a user can — after **one** initial game launch —
repeatedly and indefinitely **load and switch between multiple valid characters**, both
**within the same save file** and **across different valid save files**, with **no load/reload
crashes or stalls**, ad nauseam. When the user wakes up they can rest assured the feature works
for any given valid save, repeatedly, without babysitting it.

Prove this with a **bounded, deterministic, non-cyclic, fully autonomous** test that needs **no
live user input or feedback**, and finishes in **finite time** with a **short human-readable
pass/fail report**.

## 2. Product model (what the DLL does)

- **Initial load (in scope only as the starting point):** the DLL reads a `er-effects.toml`
  next to the game exe with `save_file` (path) + `slot` (i32). If both are valid it
  **automatically** loads that character; the user has no control until the character is in-world
  and genuinely playable (single-player, or seamless co-op). If the TOML is absent/invalid, an
  interactive **file picker** appears — **we do NOT exercise, drive, or design around the picker
  for this goal.** (See `crates/er-effects-rs/src/config.rs`, `save_picker*`.)
- **Yield:** once that first genuinely-playable state is reached, the product yields to the user.
- **Subsequent loads (the actual proof target):** when a new `(file, slot)` is selected, the
  product loads that character **the quickest valid way possible** — *any* fast path that lands a
  **fully-playable** character with that save's real stats/equipment/everything. It is **NOT**
  required to replay the exact vanilla teardown path (quit-to-menu → load-character →
  current-file-only → click → await). Accuracy over speed.

## 3. Scope

**In scope**
- Repeated loads **after** the first automatic load.
- **Per-character within a file** (vary `slot`) **AND cross-file** (vary `save_file`). Both axes.
- **Vanilla `.sl2`** first. **Seamless `.co2`** second, same acceptance criteria — **timeboxed**
  (see §8). Seamless "playable" = same bar as vanilla (repeated loads after initial); pick the
  lightest valid seamless definition when you get there.
- Loading via **in-memory redirect**, **read-only** of the source save.

**Out of scope / do not do**
- Do **not** trigger or design around the interactive file picker.
- Do **not** make the gamepad-menu navigation a proof requirement (see §6).
- Do **not** rebuild the save-write invariant — it is believed already implemented; only assert +
  verify it (§5).

## 4. Acceptance criteria — the PROVEN gate

A **single bounded autonomous run** that a fresh session can launch and that:

1. Launches ER on Windows via me3 with the product DLL.
2. Performs the initial TOML-driven auto-load and reaches a genuinely-playable state.
3. Then loops **reloads** across **≥ 3 valid vanilla save files** (6 total if seamless is also
   attempted), covering **both** per-character-within-file and cross-file pivots.
4. For **every** load, verifies via RAM/telemetry (never a screenshot as the oracle):
   - **Identity** — loaded character name matches the selected `(file, slot)`.
   - **Stats** — a few RAM-read stats (e.g. level/rune-level/attributes) match that save.
   - **Gear spot-check** — an equipment/inventory item expected from that save is present.
   - **Stable & playable in-world — the HARD RENDER GATE (added 2026-07-18 after a live product run
     exposed a render-frozen reload that the old "controllable" wording passed).** "Present" is not
     enough: the character must be *rendering* and the world *running*. ALL of these RAM fields must
     hold, and hold **continuously for a dwell window of ≥ 5 s** — a single-frame blip does not count:
       - `oracle_player_render_ready == true`, which the DLL derives as chr-model-instance present
         **AND** `chr_ins.load_state.draw_group_enabled()` **AND** `chr_flags1c4.is_render_group_enabled()`
         **AND** `chr_flags1c5.enable_render()`. This is the **exact** combination that was `false` in
         the 2026-07-18 freeze.
       - `oracle_chr_draw_group_enabled == true` — kept explicit; it was *the* failing field
         (present + controllable-by-model, yet draw group off ⇒ invisible).
       - Loading screen actually **dismissed**: `oracle_fake_loading_any_visible == false` (in the
         freeze the cover stayed `true`). **Correction (2026-07-18, evidence-based):** the earlier
         wording also required `oracle_now_loading` cleared, but that field is
         `CSNowLoadingHelperImp::load_done` — a load-*complete* latch that **lingers `true` into normal
         gameplay** and was observed as **both 0 and 1** across render-frozen snapshots (some freezes
         had `now_loading == 0`). Gating on `now_loading == 0` therefore both false-fails good loads
         and false-passes a `now_loading == 0` freeze, so it is **not** part of the gate;
         `oracle_fake_loading_any_visible == false` is the authoritative cover-dismissed signal and
         `now_loading` is logged for diagnostics only.
       - **World is live, not frozen** — a per-frame liveness signal advances across the dwell window
         while render-ready holds (e.g. character model draw hits climbing, or havok position /
         animation time progressing). The user's "in-world but nothing is moving" must **FAIL** here.
5. Observes **zero crashes and zero stalls** across all loads — where a **stall explicitly INCLUDES
   the "logically loaded but render-frozen" state**: character present / controllable-by-model but
   `player_render_ready == false`, draw group disabled, or the loading cover never lifts. The
   teardown+startup path must work **generically**, not just for one character.
6. **Sequencing gate — prove each load STABLE before the next load is triggered.** The run is a
   strict chain: trigger load *N* → wait until load *N* passes the full §4.4 stable-&-playable dwell
   (or hits its per-load deadline) → **only then** trigger load *N+1*. A load that reaches "present"
   but never passes the dwell within its deadline is a **FAIL/stall** for that character; the run
   stops and reports failure — it does **not** advance to the next character. The final character in
   the chain must also pass the dwell. No load may be counted on a one-frame or unheld signal.
7. **Logs per-load timings** (do **not** gate pass/fail on speed this round), including time-to-stable
   (present → dwell-passed) per load, so a slow-but-eventually-stable load is distinguishable from a
   never-stabilizes stall.
8. Emits a **short human-readable report**: files × characters covered, per-load timings +
   time-to-stable, and any cataloged bad/invalid saves.
9. Exit 0 == **PROVEN**. Timings and the report are the morning-review artifact.

Success is generic **stable** repeatability: **if we cannot load multiple characters in a row and
prove each one rendered-and-playable before moving on, the goal is not met**, regardless of how well
any single logical load works. A load that is "present" but frozen is a FAIL, not a pass.

## 4a. CURRENT PRIMARY MILESTONE — same character loaded 3× in a row (added 2026-07-18)

The §4 gate above varies the `(file, slot)` across ≥3 saves. That is the end state, but it is **not**
the case the user actually reproduces by hand, and the doc had no test for it. The **current primary
milestone** — the one to pass first — is the **same character (angrE), loaded three times in a row,
including the boot autoload**:

- **Load 1 = boot autoload** of angrE (TOML `save_file`+`slot`). Known-good: renders + is playable.
- **Load 2 = reload of the SAME angrE slot** via the real in-game menu path (System→Quit→Load), driven
  by **XInput** (the sq-repro autopilot fabricating the gamepad sequence — this is the "input driver").
  Empirically this **freezes deterministically**: character present but **not render-ready and cannot
  move**, though the in-game menu can still be opened (game shell alive). This frozen state is the bug
  to capture, not to pass over.
- **Load 3 = reload of the SAME angrE slot again**, driven the same way. Empirically it **recovers**:
  character renders and is movable. (Mechanism hypothesis: a stale element from load 1 breaks the
  load-2 finalize; load 2's teardown clears it so load 3 finalizes.)

**Acceptance for this milestone:**
1. Runs autonomously with **two DLLs loaded via me3** — the product DLL (being tuned) plus the log-only
   companion trace DLL — with all overlapping native hooks **unioned through a single MinHook instance**
   (the product DLL's `er_effects_union_register` export), never two colliding instances.
2. The XInput driver is **readiness-gated, never blind** (see the correction that the earlier automated
   capture failed *because* it fired the next load without checking render + movement). Before it
   triggers the next load it must confirm, via RAM telemetry, the current load's true state:
   - **render-ready** — `oracle_player_render_ready` (chr-model present AND `draw_group_enabled()` AND
     `is_render_group_enabled()` AND `enable_render()`), held ≥5 s; and
   - **can-move / input-causes-movement** — a NEW semaphore: while a movement stick input is injected,
     `oracle_havok_pos` shifts beyond a noise threshold (world-live `oracle_play_time_ms` advancing is
     necessary but **not** sufficient — it ticks during the freeze too, so it cannot be the move oracle).
3. The chain is strict: load 1 must reach render-ready + can-move (dwell) **before** load 2 is triggered;
   load 2's frozen state is captured (present, render-ready == false, can-move == false) up to its
   per-load deadline; then load 3 is triggered and must reach render-ready + can-move.
4. Zero crashes / zero soft-locks. The regression that turned load 2's **recoverable** freeze into a
   hard soft-lock (warp-clear removal + finalize-disarm) is reverted; load 2 must remain the *recoverable*
   freeze, and load 3 must recover.
5. Emits per-load render-ready / can-move / freeze telemetry + timings and a short pass/fail report.

Passing 4a proves the freeze/recovery mechanism on one character; §4 then generalizes it across saves.

## 5. Invariants the harness must assert & verify (not build)

- **Reads are read-only.** Loading a source save (read-only or read/write) must **read, never
  write** the supplied file. Prefer in-memory redirect (`save_redirect/`).
- **The only write path is the Save button**, which **upserts a copy** into the current
  logged-in Steam-ID's APPDATA dir (the folder the game normally r/w) — never the supplied file.
  The game must never save on its own; the quit-menu button was replaced to be **save-only**.
  The forced initial-load APPDATA write should be gone. **Believed already implemented** — the
  harness verifies no unexpected save-file writes occur during the loop and logs any it sees.

## 6. Harness contract (how to drive the proof)

- **Programmatic reload is acceptable and preferred** for the proof: trigger each reload by
  supplying the next `(file, slot)` (rewrite `er-effects.toml` / a control hook the DLL honors),
  bypassing the gamepad-menu + OCR. XInput and OCR are **not required** for any proof.
- The real-user reload path is a customized in-game menu (gamepad: Start → Up → Left-Trigger to
  the customized quit menu → DPad-Right ×2 = load within current file → ×3 = file picker). It is
  "kind of a mess." A **faithful gamepad-menu-nav pass is an optional documented follow-up**, not
  part of this PROVEN gate.
- If menu-nav is ever added: **prefer adding DLL telemetry that exposes menu state** (submenu,
  cursor index, highlighted save/character) so the harness navigates by **RAM, not OCR**. OCR is
  navigation-aid-only and only ever after exact ER-window validation (per `AGENTS.md`); it is
  **never** the run-stopping / load-success oracle.

## 7. Test corpus & save validity

- Corpus root: `A:\Code Projects\Elden Ring Save Manager\data\save-files`
  (WSL: `/mnt/a/Code Projects/Elden Ring Save Manager/data/save-files`). Any `.sl2`/`.co2`
  there is fair game; the user has no preference which files.
- **Valid save** = the active save-slot identifier == 1 for the slot (deleting a character sets
  it to 0 = overwritable/empty). Only files with ≥1 valid save should be considered.
- Some saves have a **known edge case** (loading FROM them didn't reach the character-select
  screen); the user suspects a file named like `12345…` but says **do not focus on it**. Treat a
  save that fails to load as a **cataloged finding** (with evidence), skip it, and prove on the
  valid ones — do not let one bad save block PROVEN. Report the skipped set.
- **Save safety: zero concern** — every save file has a backup elsewhere. You may copy/stage/
  modify freely. Restoring the original default save at the end is *nice-to-have*, not required.

## 8. Timebox & time discipline

- **Vanilla first.** If vanilla is not solved within **~12 hours**, do **not** move to seamless.
  If vanilla is solid with time left, seamless is worth attempting.
- You (the agent) are **bad at estimating time** — that's fine. Use real `date` calls at
  milestones and **record time-taken estimates in `bd`** as you go ("X took ~N min"). The user
  finds these valuable to gauge progress; keep it "in the back of your mind."
- Work **non-cyclically**: make real forward progress, don't loop. Autonomous overnight work on
  valid problems is explicitly welcome.

## 9. Known central blocker to solve

Two failure modes, both on the *reload* path (load #1 / boot autoload is fine):

1. **MoveMapStep 18 crash/stall** during world teardown+reload (game assertion → AV `rva=0x1eb9999`).
   Prior narrow fixes (disarming the return-title) either regress switching or don't fire.
2. **Render-handoff freeze (found 2026-07-18, live product route).** The reload can complete
   *logically* — old world torn down, slot re-deserialized, `WORLDRES` runs to completion, MoveMap
   finishes, DLL logs "stable in-world" — yet the game's **end-of-load render handoff never fires**:
   `oracle_player_render_ready == false`, `chr_ins.load_state.draw_group_enabled() == false`,
   `oracle_now_loading` stuck, loading cover never lifts. Character is present but invisible and the
   world is frozen. The character draw group is a *game* field; the synthetic `own_load_switch_reload_fire`
   → `SetState5` reload path skips whatever step the game normally uses to re-enable it and mark the
   player render-ready. RE where the game enables `chr_ins.load_state.draw_group` at load-complete
   and drive/allow that step on the reload.

The real fix is making the native teardown+load of a new character complete reliably **and hand off
to a live, rendered, moving world** — own the native load path end to end. See the `bd` chain below.

## 10. Context to load (read first)

`bd recall` / `bd memories` these before touching code:
- `REFINED-GOAL-multi-save-repeatable-load-2026-07-18` — the refined goal.
- `refined-goal-details-answers-2026-07-18-detailed`, `-2-`, `-3-`, `-4-` — the full Q&A
  (input model, load-proof floor, read-only/save-button invariant, drive surface, reload trigger,
  timebox).
- `angre-reload-full-causal-chain-and-fix-2026-07-18` — the reload crash causal chain.
- `angre-4loads-goal-met-but-switch-regression-2026-07-18` — why the time-based disarm regressed
  switching; `angre-slot-index-mismatch-selected-vs-saveslot-2026-07-18` — SELECTED_SLOT (profile
  id) ≠ GameMan.save_slot index space.
- `angre-stable-proof-title-owner-gate-bug-2026`, `angre-load1-stable-then-destructive-reload-cycles-2026`.

**Prior partial harness to build on (don't reinvent):**
`scripts/switch-reload-watch.sh`, `scripts/two-switch-watch.sh`,
`scripts/switch-character-oracle.py`, `scripts/switch-failfast-poll.sh`,
`scripts/summarize-reload-trace-log.py`, `scripts/check-reload-trace-dll-policy.py`.

**Product mechanisms:** `crates/er-effects-rs/src/config.rs` (TOML `save_file`+`slot`),
`crates/er-effects-rs/src/experiments/save_redirect/`, `.../experiments/continue_load/`,
`.../experiments/own_load/`, `.../experiments/startup_hooks/system_quit_repro_guards.rs`,
`.../experiments/title/title_tick_cover.rs`.

## 11. One-line definition of done

> A single autonomous, bounded, non-cyclic run loads ≥3 valid vanilla save files' characters
> (per-character and cross-file) after the initial auto-load. For each, in strict sequence, it
> proves via RAM the right character (identity+stats+gear) **AND that the character is
> render-ready + drawing + the world live (`player_render_ready` held ≥5 s, loading cover gone),
> before triggering the next load** — with zero crashes and zero stalls (a present-but-frozen load
> counts as a stall/FAIL), read-only sources, timings + time-to-stable logged, and a short
> human-readable pass/fail report. Seamless the same, timeboxed after vanilla.
