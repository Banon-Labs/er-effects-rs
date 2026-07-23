# Goal: Switch-Reload Framerate Parity (Acceptance)

**Status:** active. Supersedes `repeatable-multi-save-load-acceptance.md` as the *active target*.
That broad goal is otherwise **met** — repeatable multi-save loading reaches genuine world
readiness (character rendered + player can move), and the normalized vanilla-imprint semaphore
diff is down to a **single field: framerate/frame_ms on the reload**. Every other field was ruled
out or matched (SetState5 counters, entity residency, profile model, the 21 telemetry-only in-world
functions, composite, render-drive, effects, CPU, flags, pacing, present, boot-view). The broad
criteria remain a **regression guard** (§6), not the active hunt.

This document scopes the one open field to a concrete, measurable engineering task.

---

## 1. One-sentence objective

Make the mod's ProfileSelect **switch-reload** render at the **same steady-state per-frame cost as
the game's native reload**, closing the ~4 fps (~0.7–1.9 ms/frame) divergence — or prove, by a
confound-free measurement, that the divergence is a measurement artifact and there is nothing to fix.

---

## 2. The precisely-defined divergence (what is actually wrong)

- The mod's reload is driven by `own_load_switch_reload_fire` (menu-free; fires native
  `continue_confirm`/`SetState5` on `SYSTEM_QUIT_QUICKLOAD_PHASE`). Runtime caller-trace proven
  (run72), and it is **load-bearing** — removing it yields zero loads / soft-lock (run73B).
- Symptom, from the confound A/B: both **first loads match** (~56 fps: vanilla 56.4, mod 55.1);
  only the **reload diverges** — the vanilla *native* reload **lightens to ~59.4 fps**, the mod
  `own_load` reload **stays ~56.0 fps**. It is render-bound (not a throttle / SyncInterval effect).
- Working hypothesis (to be confirmed by §4): the native reload performs a render-resource
  **release/lightening** at the menu→world transition that `own_load_switch_reload_fire` skips,
  leaving the reloaded world ~0.7–1.9 ms/frame heavier on the GPU.

---

## 3. Oracles (precise definitions — no hand-waving)

**3.1 Steady-state window `W(epoch)`** — for a given load epoch, the frames from **T+10 s to
T+30 s** after that epoch first reaches world-stable (`oracle_player_present` AND
`world_simulating` true for ≥ 3 consecutive samples). All medians below are over `W`.

**3.2 `frame_ms(epoch)`** — `median` over `W(epoch)` of per-frame frame time (`1000 /
oracle_fps`, cross-checked against `oracle_present_qpc_delta_us / 1000`). Primary acceptance metric;
measurable **today**, no new code.

**3.3 `gpu_frame_us(epoch)`** — `median` over `W(epoch)` of **per-frame GPU-busy time**, from a new
in-DLL oracle: a D3D12 **timestamp-query pair around the game's frame GPU work**, obtained by hooking
the game command queue's `ID3D12CommandQueue::ExecuteCommandLists` (shared vtable; the DLL already
owns a device + queue in `present_overlay.rs`), resolved + read back each present. Deliverable of
Phase 1. Used to *localize* the divergence (mechanism), not as the acceptance gate itself.
**Validation gate for this oracle:** across ≥ 3 known distinct fps levels, `gpu_frame_us` must move
**monotonically opposite to `oracle_fps`** (higher GPU time ↔ lower fps), else the oracle is invalid
and must be fixed before use.

---

## 4. The confound-resolving comparison protocol (the crux the prior draft glossed)

The direct mod-reload-vs-vanilla-reload comparison is **structurally confounded** (vanilla =
telemetry-only + full-drive; mod = armed + reload; uncrossable — armed+full derails, telemetry-only+
reload hangs; own_load is load-bearing so it can't simply be swapped for native in armed mode). A
goal that says "match the native reload" without defining *how you obtain a matched native
measurement* re-inherits that confound. This protocol removes it.

**4.1 Metric = the WITHIN-RUN reload-minus-first-load delta.** Because the two runs' **first loads
already match**, the first load is a per-run baseline that cancels the run-type difference. Define:

- `D_mod    = frame_ms(mod reload epoch)     − frame_ms(mod first-load epoch)`     (armed run)
- `D_van    = frame_ms(vanilla reload epoch) − frame_ms(vanilla first-load epoch)` (telemetry-only run)

`D_van` is expected **negative** (native reload lightens); `D_mod` is expected **~0** (mod reload
does not lighten). The quantity under test is `Δ = D_mod − D_van` (in ms/frame). This is
confound-controlled: any constant run-type offset present in both epochs of a run subtracts out.

**4.2 Same protocol repeated with `gpu_frame_us`** (Phase 1 oracle) in place of `frame_ms`, to
attribute `Δ` specifically to GPU render cost and drive the Phase 3 fix.

**4.3 Required run hygiene for a valid `D`:** identical character/slot both runs; both epochs of a
run reach world-stable (else `W` undefined → run void); no trace-logging instrumentation on the
`gpu_frame_us` path during the measured window (it perturbs fps — see run72 at 32 fps); ≥ 2 repeats
per side, report per-side spread so `Δ` is compared against noise, not a single sample.

---

## 5. Acceptance criteria (numeric)

Let `σ` = the larger of the two per-side standard deviations of `D` across repeats.

- **AC-1 (Phase 1 done):** `gpu_frame_us` oracle exists and passes its §3.3 monotonicity validation.
- **AC-2 (Phase 2 decision):** run §4 protocol.
  - If `|Δ| ≤ max(0.10 ms, 2σ)` → the divergence is **within measurement noise → declared a confound
    artifact**; goal is **met by measurement** (per broad-acceptance §3b, methodology not a defect).
    Record and stop.
  - If `|Δ| > max(0.10 ms, 2σ)` → **real divergence confirmed**; proceed to Phase 3. (Current
    fps evidence predicts `Δ ≈ 0.7–1.9 ms`, well above this.)
- **AC-3 (Phase 3 done — the fix):** after modifying `own_load_switch_reload_fire`, the same §4
  protocol yields `|Δ| ≤ max(0.10 ms, 2σ)` **sustained over the full `W` window**, across ≥ 2 repeats,
  with the world-readiness + all other broad-acceptance semaphores still green (§6). Equivalent
  plain-language gate: the mod reload's steady-state fps is within ~0.4 fps of the native reload's,
  every repeat.

---

## 6. Regression guard (broad goal stays green)

Every measured run must still satisfy the already-met broad criteria, or the run is a **failure**
regardless of `Δ`: reload reaches genuine world readiness (character rendered AND player can move);
zero `CS::MessageBoxDialog` builds; save-safety (writes only to the game-owned active save); the
non-fps semaphore fields remain empty vs the vanilla imprint.

---

## 7. Phases (execution order)

1. **Build + validate the `gpu_frame_us` oracle** (§3.3). Game-queue `ExecuteCommandLists` timestamp
   hook. Gate: AC-1.
2. **Measure `Δ`** via §4 with `frame_ms` and `gpu_frame_us`. Gate: AC-2 → artifact (stop) or real
   (continue).
3. **Close** (only if AC-2 = real): use the `gpu_frame_us` oracle + resource-residency counters to
   identify the specific render-resource release the native reload does and `own_load_switch_reload_fire`
   skips; replicate it in `own_load_switch_reload_fire` (native enqueue/ownership, not a menu swap —
   own_load is load-bearing). Gate: AC-3.

---

## 8. Non-goals / boundaries

- **Not** a menu-driven-reload rewrite: own_load is load-bearing (run73B); the fix adds the missing
  resource release to the existing path, it does not replace the load owner.
- **Not** RenderDoc-dependent: RenderDoc is a verified dead end here (device-creation hook exceeds the
  boot cap; inject can't hook the existing device). The `gpu_frame_us` in-DLL oracle replaces it.
- Env/marker gates used for the A/B are **diagnostic only**; the product default path is unchanged
  until AC-3, and any fix must hold with **no** agent-only env vars (release-default rule).
- Runtime probes obey the existing runtime cap, save-safety, no-Steam/EAC-launch, and PID-scoped
  teardown rules unchanged.
