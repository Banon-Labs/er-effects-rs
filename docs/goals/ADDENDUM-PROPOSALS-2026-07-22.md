# Addendum proposals to `repeatable-multi-save-load-acceptance.md`

**From:** autonomous session, 2026-07-22 (evening).
**Framing:** "I think your goal would ACTUALLY be better if…" — these are proposed *sharpenings* of the
acceptance doc, each grounded in evidence produced THIS session, not hand-waving. Nothing here lowers
the bar; it makes the bar measurable and removes two ways it can silently lie to us. You decide; I keep
moving regardless.

---

## Proposal 1 — Make the harness REFUSE a mod-autoload baseline (the mod's own load1 is contaminated)

**Evidence (this session, `scripts/oracle-steadystate-diff.py` on `samechar-3x-cadence-run4`):** in the
sustained *movable* window of **all three** loads — including load1, which runs at **60 fps** —
`oracle_chr_draw_group_enabled == False`, `oracle_now_loading == 1`, and
`oracle_player_render_ready == False` hold in 100 % of frames. The mod's own load1 is therefore already
in the "loading-draw-state," i.e. it carries the same defect the reload does; it is only *faster*.

**Why this matters for the doc:** §4 already says the imprint must come from vanilla, and bd
`oracle-reference-is-vanilla-continue-not-load1-autoload` says the same. But nothing *enforces* it — a
future agent under time pressure will reach for the mod's load1 as a convenient baseline because it is
already captured. That would make a broken thing its own reference and pass a broken reload.

**Proposal:** add to §5 (invariants the harness *asserts*): the diff tool must **reject** any baseline
whose provenance is the mod's autoload (detectable: baseline captured with `PRODUCT_AUTOLOAD_ARMED`, or
lacking the vanilla telemetry-only marker). Baseline provenance becomes a checked field in the imprint
store, not a convention.

## Proposal 2 — Retire `draw_group=False` / `now_loading=1` as fps-root candidates; name the real fingerprint

**Evidence:** a constant cannot explain a difference. `draw_group=False`, `now_loading=1`,
`render_ready=False` are identical across the 60 fps load1 and the 20 fps reload (Proposal 1's data), so
they are **falsified** as the fps root (recorded: bd
`STEADYSTATE-DIFF-TOOL-plus-drawgroup-nowloading-FALSIFIED-as-fps-root`). The fields that actually differ
load1→reload are: `oracle_present_refresh_per_present_x100` 100→400, `oracle_present_qpc_delta_us`
16667→66669, `frame_ms` 17→50, and a small `game_task_us` 643→940 (0.3 ms — not the 33 ms). The 33 ms
lives in the **present-wait / GPU residual**, consistent with the render-bound finding.

**Proposal:** §6 currently elevates `draw_group=False` to "candidate FPS-root marker." Downgrade it to
"co-present loading-draw-state flag, **falsified** as the discriminator," and promote the **present-cadence
+ residual** set to the primary fingerprint. The next divergence-driver should be the GPU-timestamp
semaphore already filed as `er-effects-rs-03ma` (split residual into GPU-render vs present-wait) — that is
the field the diff will point at once vanilla-vs-mod is wired.

## Proposal 3 — Define numeric "zero tolerance" as canonicalized-median equality (it is otherwise impossible)

**Problem:** §3b says "exact match, zero tolerance." Real per-frame `fps`/`frame_ms` jitter run-to-run
even in pure vanilla; a literal raw-value exact match is unsatisfiable and would fail vanilla against
itself.

**What the tool already does (and I propose the doc bless):** inherently-noisy numeric semaphores are
compared on a **median canonicalized to a per-field unit** (whole fps, 1 ms, half-vblank) — so sub-unit
jitter is *not* a divergence but a regime change (20 vs 60) is a glaring one. This is a *normalization*
(§3b's own escape hatch: "normalize the inherently-nondeterministic, then exact"), not a tolerance band.

**Proposal:** add one sentence to §3b: "numeric semaphores that are inherently per-frame-noisy are
normalized to a canonicalized median at a documented per-field unit; equality is exact on the canonical
value." That keeps zero-tolerance honest without pretending fps is deterministic frame-to-frame.

## Proposal 4 — The vanilla baseline is NOT blocked by the tab-switch; wire the harness drive mode + telemetry-only

**Correction to an earlier belief (this session):** the `run-samechar-3x-threedll.sh` header claims the
`OptionSetting → Quit` tab-switch is "mouse-only / no reversed menu-event id" and the harness "halts
there." That comment is **stale**. `drive.rs` drives the full `System→Quit` sequence via the RAW PAD
device (not `+0x90`): `OpenPauseMenu → NavToOptionSetting` (Up+Confirm) `→ TabToQuit`
(`PadButton::TabLeft`, `drive.rs:244`) `→ Quit` (Down+Confirm) `→ QuitTeardown`, each gated on its own
pane semaphore. So a fully agent-driven native quit-to-menu exists today.

**Why the vanilla baseline still isn't captured:** in the samechar-3x *product* run the harness is in
COMPANION mode (`drive.rs` DriveMode) — it does NOT drive the menu (the product's reload machinery +
control-file does). The vanilla baseline needs the *opposite* wiring: a **telemetry-only product**
(`ER_EFFECTS_TELEMETRY_ONLY=1` disarms the autoload → boots to title, still emits every `oracle_*`) plus
the input-harness in a **boot+reload DRIVE mode** (`FullBootReload`/`NativeReloadOnly`, selected by the
`er-harness-drive-mode.txt` marker that the product runner deliberately clears) so the *harness* drives
title→Continue→play→System→Quit→Continue through the game's own native input. That is the vanilla
ground-truth reload, captured flow-faithfully.

**Proposal (sharpens the plan, does not change the bar):** add a `run-vanilla-reload-agentdriven.sh`
sibling that stages telemetry-only product + input-harness DRIVE mode + telemetry DLL and captures the
native continue+reload with no user input — replacing the deprecated user-driven `run-vanilla-reload-fps.sh`.
This is the real Milestone-1 critical path (the vanilla imprint the diff needs), and it is mechanism-ready,
not blocked.

---

### Net
The doc is right about *what* done means. These four make it **measurable and un-foolable**: enforce the
vanilla baseline (1), stop chasing a falsified marker (2), make numeric equality well-defined (3), and
name the one mechanism blocking the baseline capture (4). If you disagree with any, they cost nothing to
drop — the harness and memories already encode the evidence.
