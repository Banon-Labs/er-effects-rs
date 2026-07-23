# Goal: Repeatable Multi-Save Character Loading — Vanilla-Identical Parity

**Status:** **OPEN — acceptance STRENGTHENED 2026-07-22** (live grilling session with the user).
The prior version accepted a *curated floor* (identity + stats + gear + render-ready + can-move +
no-stall) and explicitly said a subsequent load need **not** replay vanilla — "accuracy over speed,
any fast path that lands a fully-playable character." That floor is **too weak**: it passes a reload
that reaches the world but runs at **20 fps** where vanilla runs 60, because framerate was never in
the gate. The new bar is **exact parity with a vanilla imprint across every semaphore**. A prior
session also *claimed* the continue flow was "remade with exact parity"; that claim was never
measured and the persistent 20 fps is evidence against it. Parity is now a **measurement**, not an
assertion.

**Invoke a fresh session with:**
`/goal complete the acceptance criteria of ./docs/goals/repeatable-multi-save-load-acceptance.md`

> This document is the single source of truth for what "done" means. Read the linked `bd` memories
> and the `HIGH-LEVEL-GOAL-*` / `dll-not-same-path-across-os-*` / `DECISIVE-reload-20fps-is-render-bound-*`
> memories first (see [§9 Context](#9-context-to-load-read-first)).

---

## 1. The goal, in spirit

A single DLL (`er_effects_rs.dll`), loaded through **me3** as a `[[natives]]` profile entry, that lets
a user:

1. **Auto-load** a chosen save file + slot at startup (default: most-recent / native Continue; optional
   ini slot override, blank ⇒ most-recent), and
2. **Switch, at any point during play,** to a character in the **same or a different save file** and the
   **same or a different slot**,

such that the resulting state is **indistinguishable from a vanilla copy of the game** in which that
save file + slot were the last-active save and the user pressed Continue — **every semaphore
identical**, not merely "reached the world."

The DLL **enables** this; it does **not drive/play**. Autoload / self-drive / input-injection are **test
scaffolding** to prove parity, not product behavior. When the target is genuinely ready (rendered AND
movable, at vanilla framerate), control belongs to the user.

## 2. Interaction fidelity

Every feature is driven through the game's **own native input-receiving mechanism and native UI**,
exactly as vanilla — no synthetic/OS-injected input and no custom overlay UI in the product path. The
mid-play switch already uses the native `05_010_ProfileSelect` 10-row window as a browsable file+slot
picker (`save_picker.rs` / `save_picker_menu.rs`), which replaced the old OS file dialog specifically to
stay in-engine.

**Sole exemption:** the initial character-load prompt shown when the mod is loaded but the user has **not**
provided a save/file (`save_picker_overlay.rs`). At that pre-front-end point the game's menu assets and
native input path are not yet available (title suppressed/black), so that one prompt is DLL-drawn with
OS input. Everything else is native.

## 3. Acceptance framework (locked 2026-07-22)

The bar is **exact parity against a vanilla imprint**, decided along four axes:

- **(a) Baseline — what "vanilla" is.** Vanilla **game flow**, captured with telemetry / input / oracle
  instrumentation crates (their own shippable crates) that **observe or drive without significantly
  modifying game flow**. A **non-native-input-but-native-*function-calling*** crate/DLL is acceptable if
  native input cannot reach a phase but the action can be faithfully reproduced by direct native calls.
  The imprint is captured from this instrumented-but-flow-faithful vanilla, **not** from the mod's own
  autoload (a mod defect must never become the standard).

- **(b) Equality — exact match, zero tolerance.** Every observable semaphore must match **exactly**.
  Fields that are inherently nondeterministic run-to-run (heap addresses, RNG state, wall-clock
  timestamps, frame indices) are **normalized/canonicalized** (e.g. module-relative pointers, seed
  control, stripped/relativized time) so the comparison is exact **after** normalization; anything that
  cannot be normalized must be made deterministic — otherwise it is not done. **No tolerance bands.**

- **(c) Window — trajectory + steady-state.** Parity is judged over BOTH: the **full load trajectory**
  (Continue-press → readiness, lockstep against the imprint's ordered semaphores + per-transition budget,
  honoring only the steps the imprint proves are legitimately non-deterministic), AND a **sustained
  post-readiness steady-state window** (steady play must match vanilla). The steady-state window is what
  catches divergences like the 20 fps that only manifest *after* the character is movable.

- **(d) Outgoing-character save on a switch — "easiest first."** The data-safety contract for what
  happens to the character you switch *away* from (vanilla quit-save vs abandon) is **not yet locked**;
  take the simplest correct behavior first and refine later. The read-only-source invariant (§5) still
  binds regardless.

## 4. Milestone 1 — build the imprint+diff harness FIRST, then let it drive the fix list

Under a zero-tolerance-exact bar you **cannot declare any divergence fixed without a tool that shows
every divergence**. So the first deliverable is the measurement, not a point fix:

1. **Capture a vanilla imprint** of the same-character reload (native System→Quit→Continue for one
   `(file, slot)`) using telemetry-only, flow-faithful instrumentation.
2. **Capture the mod's switched reload** with the same instrumentation.
3. **Diff every normalized semaphore** over the full trajectory + steady-state window → the **complete
   divergence list**.
4. **Fix divergences in the order the diff reports**, re-diffing until the diff is **empty**. The 20 fps
   is expected to be one entry; its **co-divergent fields** (we already know `oracle_chr_draw_group_enabled
   == false` holds through the entire 20 fps window — see §6) are the root's fingerprint. Diagnose the
   20 fps by its company, not in isolation.

Milestone 1 target case: **same file, same slot** (the simplest slice that still exercises the switch).
§7 then generalizes: same-file/different-slot, then cross-file, then Seamless `.co2` (timeboxed after
vanilla).

Autonomy still applies: the proof is a **bounded, deterministic, non-cyclic, fully autonomous** run that
needs no live user input and ends in finite time with a short human-readable pass/fail report — pass =
**empty semaphore diff vs the vanilla imprint** over trajectory + steady-state.

## 5. Invariants the harness asserts & verifies (does not rebuild)

- **Reads are read-only.** Loading a source save must **read, never write** the supplied file. Prefer the
  in-memory redirect (`save_redirect/`). The user's picked `.sl2`/`.co2` is never mutated.
- **The only write path is the Save button**, which upserts a **copy** into the current Steam-ID APPDATA
  dir (the folder the game normally r/w) — never the source. The game must not save on its own. Believed
  already implemented; the harness **verifies** no unexpected save-file writes occur during the loop.

## 6. Technical context — what we already know about the reload divergence

- **The 20 fps reload is RENDER-BOUND, not our DLL** (`bd DECISIVE-reload-20fps-is-render-bound-not-throttle-*`,
  `CORRECTION-reload-20fps-not-fixedspf-cap-*`, 2026-07-22). Measured on Windows/WSL2: reload frame pins at
  **49.92 ms = 20 fps = ~4 vblanks/present** with `SyncInterval=1` (the game *wants* 60 but the frame is not
  ready), zero variance. **Ruled out with telemetry:** product per-frame CPU (~1 ms, flat), the
  `fixed_spf=0.05` loading cap (target stays 0.0167), the dynamic FPS lock (off), and window focus (True).
  So it is the **game's own native reload render** costing ~4×. **Candidate marker:** the player is stuck in a
  loading draw-state — `oracle_chr_draw_group_enabled == false` through the whole 20 fps window while
  `render_group`/`enable_render` are true. New present-cadence semaphores exist:
  `oracle_present_sync_interval`, `oracle_present_refresh_per_present_x100`, `oracle_present_qpc_delta_us`
  (`present_overlay.rs`). Diff tool: `scripts/analyze-reload-fps-oracle-diff.py`.
- **The DLL is NOT the same code path across OS** (`bd dll-not-same-path-across-os-*`): on native Windows
  the entire DLL overlay/composite is suppressed (`composite_suppressed_on_native`), while Wine/vkd3d runs
  the full portrait composite. So a Linux-vs-Windows fps comparison is confounded — but the direction
  exonerates the DLL (native does *less* yet is slower). A clean OS A/B needs identical composite state.
- **Render-handoff freeze history (2026-07-18):** a reload can complete logically (world torn down, slot
  re-deserialized, WORLDRES done, MoveMap finished) yet the end-of-load render handoff never fires
  (`player_render_ready == false`, draw group off, cover never lifts) — present but invisible/frozen. The
  synthetic SetState5 reload path skips whatever step the game uses to re-enable `draw_group` and mark the
  player render-ready. Driving "like the user" means the reload must go through the **native menu Continue
  path** (or SetState5 must perform the same `draw_group` / `request_code` 1→2 handoff).
- **Movement-proof mechanism** (`can_move_probe.rs`, for the *test harness* only — not product): synthetic
  `XInputGetState` does not reach locomotion (Steam Input routes via ScePad/DInput). Faithful injection =
  hook the pad-device poll `0x141f6bad0` and write left stick `device+0x8a0=1.0`; force `DLUID+0x88d`
  (stay-active) so the poll runs unfocused; hook `Game.Debug::IsEnableControlOnDisactiveWindow`
  (`0x140e53220`) → 1. Movement is measured as an `oracle_havok_pos` delta — the only reliable
  playable-vs-frozen signal (`draw_group`/`render_ready`/`request_code`/`fake_cover` read identically in a
  playable and a frozen load; only motion distinguishes them). This is scaffolding to *validate* parity,
  never product behavior.
- **Do NOT trap the user's input.** The per-frame 1×1 `ClipCursor` mouse confinement was removed
  2026-07-22 (`bd input-block-1x1-clipcursor-traps-user-*`); it trapped the user's mouse when the harness
  failed. Any confinement must be native-Windows-exempt and fail-safe-released.

## 7. Test corpus & save validity

- Corpus root: `A:\Code Projects\Elden Ring Save Manager\data\save-files`
  (WSL: `/mnt/a/Code Projects/Elden Ring Save Manager/data/save-files`). Any `.sl2`/`.co2` there is fair
  game; no file preference. Milestone-1 default: `100-Lilbro` slot 0 (angrE).
- **Valid save** = active save-slot identifier == 1 for the slot (0 = deleted/overwritable). Only files
  with ≥1 valid save count. A save that fails to load is a **cataloged finding** (with evidence), skipped,
  not a blocker; report the skipped set.
- **Save safety for the harness: sources are read-only** (§5); staged copies may be created freely. Never
  mutate a source save.

## 8. Timebox & discipline

- **Vanilla `.sl2` first**; only attempt Seamless `.co2` if vanilla parity is solid with time left, same
  acceptance criteria.
- Use real `date` calls at milestones and record time-taken notes in `bd`. Work **non-cyclically** — real
  forward progress, no whack-a-mole of independent timeout heuristics (that is the mistake the imprint+diff
  harness exists to end).

## 9. Context to load (read first)

`bd recall` / `bd memories` before touching code:

- `HIGH-LEVEL-GOAL-vanilla-identical-state-parity-any-file-any-slot-switch-2026-07-22` — the authoritative goal.
- `DECISIVE-reload-20fps-is-render-bound-not-throttle-syncinterval1-refresh4-2026-07-22`,
  `CORRECTION-reload-20fps-not-fixedspf-cap-target-is-60-genuine-50ms-frame-2026-07-22`,
  `reload-fps-is-flip-taskdelta-20fps-cap-not-product-cpu-2026-07-22` — what the 20 fps is/ is not.
- `dll-not-same-path-across-os-composite-suppressed-on-native-but-exonerated-2026-07-22` — OS code-path divergence.
- `draw-group-false-is-reload-loading-draw-state-candidate-fps-root-2026-07-22` — the co-divergent marker.
- `real-oracle-imprint-lockstep-boot-sequence-direction-2026-07-20` — the imprint+diff architecture.
- `input-block-1x1-clipcursor-traps-user-native-windows-no-failsafe-release-2026-07-22`,
  `never-blanket-kill-eldenring-killed-user-game-2026-07-22` — harness safety.

**Product mechanisms:** `crates/er-effects-rs/src/config.rs`, `.../experiments/save_redirect/`,
`.../experiments/continue_load/`, `.../experiments/own_load/`,
`.../experiments/startup_hooks/system_quit_repro_guards.rs`, `.../experiments/title/title_tick_cover.rs`,
`.../experiments/save_picker*`, `.../experiments/present_overlay.rs`.

**Harness / analysis:** `scripts/run-samechar-3x-threedll.sh`, `scripts/capture-samechar-3x.py`,
`scripts/analyze-reload-fps-oracle-diff.py`.

## 10. One-line definition of done

> A bounded, autonomous, non-cyclic run performs the initial auto-load and then, through the game's native
> UI, loads characters across same/different save files and same/different slots — and for each load, a
> normalized **semaphore diff against a vanilla imprint is empty over the full load trajectory AND a
> sustained steady-state window** (exact match, zero tolerance), with read-only sources and a short
> human-readable report. Seamless the same, timeboxed after vanilla. If any semaphore diverges from
> vanilla — framerate included — it is not done.
