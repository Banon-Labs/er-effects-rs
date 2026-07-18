# Goal: Repeatable Multi-Save Character Loading (Acceptance Criteria)

**Status:** open. **Authored:** 2026-07-18 from a live goal-refinement Q&A with the user.
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
   - **Controllable in-world** — player available/controllable, world ready.
5. Observes **zero crashes and zero stalls** across all loads (the teardown+startup path must work
   **generically**, not just for one character).
6. **Logs per-load timings** (do **not** gate pass/fail on speed this round).
7. Emits a **short human-readable report**: files × characters covered, per-load timings, and any
   cataloged bad/invalid saves.
8. Exit 0 == **PROVEN**. Timings and the report are the morning-review artifact.

Success is generic repeatability: **if we cannot load multiple characters in a row, the goal is
not met**, regardless of how well any single load works.

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

Repeated loads currently **crash or stall at MoveMapStep 18** during the world teardown+reload
(game assertion → AV `rva=0x1eb9999`). Load #1 is fine; the *reload* path is the unsolved core.
Prior narrow fixes (disarming the return-title) either regress switching or don't fire. The real
fix is making the native teardown+load of a new character complete reliably (own the native load
path). See the `bd` chain below.

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
> (per-character and cross-file) after the initial auto-load — each verified by RAM as the right
> character (identity+stats+gear) and controllable — with zero crashes/stalls, read-only sources,
> timings logged, and a short human-readable pass/fail report. Seamless the same, timeboxed after
> vanilla.
