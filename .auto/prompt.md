# Autoresearch: clean title-cover masquerade for zero-input gold autoload

## Objective
Masquerade the irreducible ~15.4s boot-init/resident-UI load behind an intentional custom title/loading cover while the zero-input gold autoload continues behind it. There is **no ms target**: success is visual coverage plus the hard product constraints. Split the work into two independently validatable parts:

- **Part A -- disable native title visual:** suppress the BeginTitle `05_000_Title` MenuWindowJob build/render (`FUN_14081f9f0 -> FUN_1407acbf0`) while leaving TitleStep, FixOrderJobSequence, native Continue logic, and world-load chain intact. Do **not** touch STEP_Wait or `CSMenuMan+0x21`.
- **Part B -- inject custom cover:** render our own image in a title-safe slot, preferably through the profile-model-render / `SYSTEX_Menu_ProfileNN` texture pipeline, or via a small custom Scaleform target if `05_001_Title_Logo` has no remappable dummy texture symbol.

The desired product chain remains:

`clean custom title/loading cover visible -> native Continue/load proceeds behind it -> continue_load_67b750 -> native load-complete evidence (b80_deserialize_67b290 OR explicitly disabled modal-confirm wait after loaded-slot proof) -> native continue_confirm / SetState5 -> world-stable oracle`

A score of `autoload_re_score=1600` means the native title visual is suppressed, the custom cover path exists and is observable, the native Continue/load chain remains intact, there is no synthetic input or direct-load bypass, and bounded runtime proof satisfies the hard constraints.

## Metrics
- **Primary**: `autoload_re_score` (points, higher is better, max 1600) -- composite visual-cover/RE/product-proof score from `.auto/measure.sh`.
- **Regression/failure metrics**: `title_cover_failures`, `readiness_gate_failures`, `asset_chain_failures`, `dll_patch_failures`, `native_continue_failures`, `field58_gate_failures`, `direct_shortcut_failures`, `input_path_failures`, `runtime_proof_failures`, `runtime_mode_failures`, `eula_popup_failures`, `save_data_popup_failures`, `messagebox_dialog_failures`, `false_positives`.
- **Legacy secondary metrics**: `target_constants_remaining`, `helpers_missing`, `fixed_wait_predicates`, `autoload_static_failures`.

Score rubric:
- **Asset provenance / resource chain (200 pts)**: Data archive source is explicit; FMG/menu resource IDs are mapped; native consumers/xrefs are tied to those IDs; extraction is reproducible from local tools/artifacts.
- **Native Continue action identity (300 pts)**: real selected Continue row/object is identified; receiver/vtable/docall/result/submit ABI are proven; selected/default Continue is not confused with Down navigation; `result+0x58` is logged only as unknown/diagnostic, not used as readiness.
- **DLL product patch path (300 pts)**: implemented inside the chainload DLL; no `eldenring.exe` patching, loose asset edits, or product direct-load/direct-confirm/deser dispatcher shortcuts; advances through native accept/submit semantics after Continue exists.
- **Safety/runtime oracle (300 pts)**: input remains blocked/suppressed; `simulated_button_presses_total=0`; save backup/restore and char-fingerprint/mount guards remain; bounded runtime proof reaches native load, loaded-slot completion (`b80_deserialize` or disabled modal-confirm with loaded evidence), native confirm/SetState5, and world-stable edges. The gold oracle must derive expected character identity from the vanilla `ER0000.sl2` save slot (not `.co2` except Seamless-specific tests), expose the character name in the oracle summary, require observed telemetry to match that derived save identity, treat `"_"`, `""`, and all-whitespace names as empty-like/non-real, require the expected player animation ID, and require no native post-load popup/modal builds after Continue/load finalizes.
- **Static regression guards (300 pts)**: fixed waits remain fail-safe only; checker/measure fail closed for direct shortcuts, input probes, stale `mode=0` gating, and asset-chain regressions; build/checks pass.
- **Title-cover visual masquerade (200 pts)**: Part A hooks/suppresses only the native `05_000_Title` BeginTitle visual wrapper and exposes telemetry; Part B exposes an observable custom cover render path (`SYSTEX_Menu_ProfileNN`/dummy texture or custom Scaleform) without weakening the load chain.

## How to Run
`./.auto/measure.sh` -- emits `METRIC name=value` lines and explanatory `DETAIL ...` lines.

If re-initializing autoresearch, use metric `autoload_re_score`, unit `points`, direction `higher`, baseline from the current branch, and keep `timeout_seconds <= 45` / `checks_timeout_seconds <= 45`. Runtime probes must finish the runtime portion within the cap read from `.auto/runtime_timeout_cap_seconds`.

## Files in Scope
- `src/lib.rs` -- constants/layouts/statics for title/menu/profile-load/autoload and hook wiring.
- `src/experiments.rs` -- asset/native Continue tracing, autoload state machine, product submit path, native/static readiness predicates, runtime diagnostics.
- `scripts/check-autoload-happy-path.py` and `scripts/test-autoload-happy-path.py` -- static product-path gate checks.
- `.auto/measure.sh` -- benchmark/static oracle for this autoresearch session.
- `.auto/ideas.md` -- deferred ideas backlog.
- `docs/file-extraction-tooling.md` and focused docs/recon notes -- only for provenance; do not replace executable checks with prose.

## Off Limits
- Do not add host input, DInput/keystate/pointer synthesis, XInput injection, or Down/Confirm probes to the product path.
- Do not use Down navigation as a Continue diagnostic. Continue is already the selected/default title option.
- Do not treat user/manual input as product proof. Manual probes are last-resort diagnostics only after static RE and zero-input hooks cannot answer the question.
- Do not gate product behavior on `result+0x58 == mode`. That field is currently unknown/diagnostic, not a proven readiness predicate or row index.
- Do not call `continue_load_67b750`, raw `b80_deserialize`, or dispatcher-drive shortcuts from the product success path. A guarded native `continue_confirm` / SetState5 is allowed only after native Continue has already loaded the requested slot and the modal-confirm wait is explicitly disabled with self-validated loaded evidence (`ac0==slot`, real `c30`, real character fingerprint, no simulated input); do not wait for or synthesize confirm input.
- Do not patch `eldenring.exe`, do not leave loose files in the live Game dir, and do not edit packed assets as the product path unless the user explicitly changes the requirement. DLL is vastly preferred.
- Do not weaken save safety. Preserve backup/restore behavior, mount/char-fingerprint guards, and SetState5/continue_confirm gates.
- Do not leave Elden Ring running after any runtime probe.
- Do not file upstream issues/PRs/reports.

## Constraints
- Before every research spike/iteration, search Beads persistent memories first (`/home/banon/.local/bin/bd memories <terms> --json` and `bd recall <key>`) using terms from the current hypothesis (for example `Continue`, `continue_load`, `SetSaveSlot`, `TitleTopDialog`, `ProfileLoadDialog`, `LoadJobContext`, `MenuJobResult`, `saveSlot`). Incorporate high-signal memories before doing new static/runtime work so prior findings and dead ends are not re-derived.
- At the end of each research spike/iteration, upsert durable new findings into Beads memories with `/home/banon/.local/bin/bd remember --key <key> <finding>`. If the new finding makes an existing Beads memory stale or inaccurate, first upsert the replacement/correction, then remove the stale memory with `/home/banon/.local/bin/bd forget <stale-key>` (or update it as retracted if preserving the historical warning is safer than deletion).
- Static RE first. Runtime probes only after the hypothesis, exact hook/edge, stop condition, and teardown are explicit.
- Frame/call counts may remain only as outer fail-safe timeouts, never as success predicates.
- Polling semantic predicates once per game tick is allowed; requiring N ticks before success is not.
- Debug logs should say exactly which field/vtable/state opened or blocked a gate, not "waited N frames".
- Runtime proof must be self-validating: target window confirmed by exact class while a live `eldenring.exe` process exists (no title fallback / no screenshotting unrelated apps), input blocking/suppression confirmed where relevant, exact process matching, save/game-file restore, teardown, expected `ER0000.sl2` slot identity match (including non-empty-like character name; `"_"`, `""`, and whitespace-only are empty), expected player animation ID, and zero native `CS::MessageBoxDialog` builds. Any `CS::MessageBoxDialog` before, during, or immediately after load (including "failed to load save data") is a hard product failure and an investigation trigger: the ideal count is 0, telemetry must expose `oracle_msgbox_total_builds` / `oracle_msgbox_any_seen`, the watcher must fail immediately with `native_messagebox_dialog_detected`, and `messagebox_dialog_failures` must reduce the score. Do not auto-accept or preserve the box; the existing OK-handler/auto-accept path is deprecated old fake-input-era behavior and is not valid product proof. Identify the native side effect/gate it would perform, decide whether that side effect is irrelevant/offline-only or required, and skip/satisfy that semantic side effect directly without UI/input. EULA/terms/license/first-boot legal popups are also a hard product failure at any point: the DLL must not auto-accept them, and fallback/menu success is invalid while such a popup is visible. Legal-popup detection must prefer non-OCR native/asset evidence from the packed `ToS_win64.fmg` text IDs and in-process dialog/state telemetry; target-window OCR is only supplemental and cannot be the sole legal-popup oracle. `eula_popup_failures` and `save_data_popup_failures` must come from real runtime evidence captured into the artifact, must cause the watcher to fail immediately when detected, and must reduce the autoresearch score.
- The product proof chain must include downstream native evidence (`continue_load_67b750`, `b80_deserialize_67b290`, native `continue_confirm`/SetState5, world-stable/max oracle), not just a title screenshot.

## Static/runtime evidence already gathered
- `TitleTopDialog::open_menu` `0x1409b24e0` opens the title menu and registers rows/actions.
- Continue-related native wrapper/action addresses include `0x14082bac0`, `0x14067b750`, `0x140afb967`, `0x140764b80`, `0x1407ac890`, result vtable `0x142aa0080`, and result vtable slot `+0x60=0x140746e80`.
- FMG/UI text paths such as `msg/engus/menu.msgbnd.dcx` are virtual entries inside `Game/Data*.bhd` + `Game/Data*.bdt`, extracted by Nuxe and unpacked by WitchyBND. They are not loose Steam depot files and not `regulation.bin`.
- Visual proof exists that the native title menu reaches `Continue` highlighted, but that is not load proof.
- First-title-item wrapper fallback was falsified: calling `menu_continue_wrapper(this=first MenuWindowJob)` produced `slot=-1` and process exit.
- A row-result candidate exists with expected vtable/docall, but previous code over-gated on `result+0x58` (`mode=0`). Static RE of `0x1407ac890` shows native submit constructs an event and calls vtable `+0x60`; `result+0x58` must not be treated as the product readiness gate.
- Part B asset blocker resolved: `/home/banon/er-extract/nuxe-menu-20260619-170932/menu/05_001_title_logo.gfx` contains only static title-logo texture symbols (`MENU_Title_GR`, `MENU_Title_EldenRing`, `MENU_DS3_LOGO`, `MENU_Title_EldenRing_01`) and no `MENU_DummyProfileFace` / `SYSTEX_Menu_Profile`; use a custom Scaleform target or another surface with dummy-profile symbols. See `docs/recon/title-cover-asset-decision-2026-06-25.md`.
- Existing cover target found but not product-safe as a direct title-job replacement: `/home/banon/er-extract/nuxe-menu-20260619-170932/menu/05_010_profileselect.gfx` contains `MENU_DummyProfileFace_01..10`; native wrapper `05_010_ProfileSelect` at dump `0x14081f7e0` maps to deobf `0x14081f6f0`. Runtime spike `product-continue-direct-20260624-184915` proved replacing the `05_000_Title` out-job with this ProfileSelect job suppresses/builds cover telemetry (`suppressed=1`, `cover_builds=1`, zero input, zero MessageBox) but leaves title owner parked at state 10 and never loads Banon before timeout. Do not return the ProfileSelect job in the BeginTitle out slot; Part B needs a non-blocking/custom surface.

## What to Try Next
1. Validate Part A statically and then at runtime: hook `FUN_14081f9f0`, return a null out-job for `05_000_Title`, expose `oracle_title_native_menu_visual_*`, and prove native title view is gone while gold still loads.
2. Runtime-validate Part A null suppression: `05_000_Title` suppressed, no replacement job in the title out slot, gold still loads behind it. For Part B, author a non-blocking custom Scaleform/cover surface with `MENU_DummyProfileFace_NN` rather than returning ProfileSelect as the title job.
3. Keep downstream proof intact: `continue_load_67b750`, `b80_deserialize_67b290`, native `continue_confirm`/SetState5, world-stable/max oracle, `oracle_char_name=Banon`, zero MessageBoxDialog.
4. Harden static guards so Part A and Part B are independently observable and cannot regress the kept levers (splash-skip, pab-advance, FadeIn-skip).

## What's Been Tried / Dead Ends
- Fixed timing gates were removed/reduced as success predicates; do not retune wait numbers.
- Direct diagnostic paths (`CONTINUE_LOAD_RVA`, raw deserialize, direct confirm, dispatcher drive) are not product proof.
- Down + accept/input-probe was a wrong diversion for Continue: Continue is already selected, input is blocked, and synthetic input is disallowed.
- `result+0x58 == 0` is not the visual Continue row index and is not a proven unarmed state.
