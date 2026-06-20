# Autoresearch: asset-backed native Continue DLL patch

## Objective
Find and prove the real native title-menu `Continue` patch path, with the game DLL as the preferred/expected product vehicle. The target is **not** another timing tweak and not a diagnostic shortcut: once the native title menu has produced the real Continue entry/action from game assets, the DLL should advance through that same semantic action without waiting for a user input event.

The desired product chain is:

`Data*.bhd/bdt archive entry -> msg/engus/menu.msgbnd.dcx -> FMG text/resource ID -> native title-menu row/functor/result object -> native accept/submit dispatcher -> continue_load_67b750 -> b80_deserialize_67b290 -> native continue_confirm / SetState5 -> world-stable oracle`

A score of `autoload_re_score=1400` means the patch exists, stays in the DLL, follows the asset/native action chain, has no synthetic input or direct-load bypasses, and has bounded runtime proof.

## Metrics
- **Primary**: `autoload_re_score` (points, higher is better, max 1400) — composite RE/product-proof score from `.auto/measure.sh`.
- **Regression/failure metrics**: `readiness_gate_failures`, `asset_chain_failures`, `dll_patch_failures`, `native_continue_failures`, `field58_gate_failures`, `direct_shortcut_failures`, `input_path_failures`, `runtime_proof_failures`, `false_positives`.
- **Legacy secondary metrics**: `target_constants_remaining`, `helpers_missing`, `fixed_wait_predicates`, `autoload_static_failures`.

Score rubric:
- **Asset provenance / resource chain (200 pts)**: Data archive source is explicit; FMG/menu resource IDs are mapped; native consumers/xrefs are tied to those IDs; extraction is reproducible from local tools/artifacts.
- **Native Continue action identity (300 pts)**: real selected Continue row/object is identified; receiver/vtable/docall/result/submit ABI are proven; selected/default Continue is not confused with Down navigation; `result+0x58` is logged only as unknown/diagnostic, not used as readiness.
- **DLL product patch path (300 pts)**: implemented inside the chainload DLL; no `eldenring.exe` patching, loose asset edits, or product direct-load/direct-confirm/deser dispatcher shortcuts; advances through native accept/submit semantics after Continue exists.
- **Safety/runtime oracle (300 pts)**: input remains blocked/suppressed; `simulated_button_presses_total=0`; save backup/restore and char-fingerprint/mount guards remain; bounded runtime proof reaches native load/deser/confirm/world-stable edges.
- **Static regression guards (300 pts)**: fixed waits remain fail-safe only; checker/measure fail closed for direct shortcuts, input probes, stale `mode=0` gating, and asset-chain regressions; build/checks pass.

## How to Run
`./.auto/measure.sh` — emits `METRIC name=value` lines and explanatory `DETAIL ...` lines.

If re-initializing autoresearch, use metric `autoload_re_score`, unit `points`, direction `higher`, baseline from the current branch, and keep `timeout_seconds <= 120` / `checks_timeout_seconds <= 120` unless `.auto/run_experiment_policy.rego` and its checker/tests are deliberately changed. Runtime probes must finish the runtime portion in <=60s.

## Files in Scope
- `src/lib.rs` — constants/layouts/statics for title/menu/profile-load/autoload and hook wiring.
- `src/experiments.rs` — asset/native Continue tracing, autoload state machine, product submit path, native/static readiness predicates, runtime diagnostics.
- `scripts/check-autoload-happy-path.py` and `scripts/test-autoload-happy-path.py` — static product-path gate checks.
- `.auto/measure.sh` — benchmark/static oracle for this autoresearch session.
- `.auto/ideas.md` — deferred ideas backlog.
- `docs/file-extraction-tooling.md` and focused docs/recon notes — only for provenance; do not replace executable checks with prose.

## Off Limits
- Do not add host input, DInput/keystate/pointer synthesis, XInput injection, or Down/Confirm probes to the product path.
- Do not use Down navigation as a Continue diagnostic. Continue is already the selected/default title option.
- Do not treat user/manual input as product proof. Manual probes are last-resort diagnostics only after static RE and zero-input hooks cannot answer the question.
- Do not gate product behavior on `result+0x58 == mode`. That field is currently unknown/diagnostic, not a proven readiness predicate or row index.
- Do not call `continue_load_67b750`, raw `b80_deserialize`, `continue_confirm`, or dispatcher-drive shortcuts from the product success path.
- Do not patch `eldenring.exe`, do not leave loose files in the live Game dir, and do not edit packed assets as the product path unless the user explicitly changes the requirement. DLL is vastly preferred.
- Do not weaken save safety. Preserve backup/restore behavior, mount/char-fingerprint guards, and SetState5/continue_confirm gates.
- Do not leave Elden Ring running after any runtime probe.
- Do not file upstream issues/PRs/reports.

## Constraints
- Static RE first. Runtime probes only after the hypothesis, exact hook/edge, stop condition, and teardown are explicit.
- Frame/call counts may remain only as outer fail-safe timeouts, never as success predicates.
- Polling semantic predicates once per game tick is allowed; requiring N ticks before success is not.
- Debug logs should say exactly which field/vtable/state opened or blocked a gate, not “waited N frames”.
- Runtime proof must be self-validating: target window confirmed by class, input blocking/suppression confirmed where relevant, exact process matching, save/game-file restore, teardown.
- The product proof chain must include downstream native evidence (`continue_load_67b750`, `b80_deserialize_67b290`, native `continue_confirm`/SetState5, world-stable/max oracle), not just a title screenshot.

## Static/runtime evidence already gathered
- `TitleTopDialog::open_menu` `0x1409b24e0` opens the title menu and registers rows/actions.
- Continue-related native wrapper/action addresses include `0x14082bac0`, `0x14067b750`, `0x140afb967`, `0x140764b80`, `0x1407ac890`, result vtable `0x142aa0080`, and result vtable slot `+0x60=0x140746e80`.
- FMG/UI text paths such as `msg/engus/menu.msgbnd.dcx` are virtual entries inside `Game/Data*.bhd` + `Game/Data*.bdt`, extracted by Nuxe and unpacked by WitchyBND. They are not loose Steam depot files and not `regulation.bin`.
- Visual proof exists that the native title menu reaches `Continue` highlighted, but that is not load proof.
- First-title-item wrapper fallback was falsified: calling `menu_continue_wrapper(this=first MenuWindowJob)` produced `slot=-1` and process exit.
- A row-result candidate exists with expected vtable/docall, but previous code over-gated on `result+0x58` (`mode=0`). Static RE of `0x1407ac890` shows native submit constructs an event and calls vtable `+0x60`; `result+0x58` must not be treated as the product readiness gate.

## What to Try Next
1. Build a reproducible asset/resource provenance chain: Data archive virtual path -> `menu.msgbnd.dcx` -> FMG Continue/New Game text IDs -> native resource/menu consumers/xrefs. Store concise provenance in docs/recon or bd comments and make `.auto/measure.sh` check for it.
2. Replace `mode=0` rejection with a native submit/accept path that follows static RE (`0x1407ac890` / vtable `+0x60`) while preserving fail-closed validation of receiver/result/vtables.
3. Add focused hooks/logs around `0x1407ac890`, `0x140746e80`, `0x14082bac0`, `0x14067b750`, `0x140afb967`, `0x14067b290`, and `continue_confirm` to prove the downstream chain.
4. Harden static guards against product regressions: direct load/confirm/deser/dispatcher calls, input probes, Down navigation assumptions, stale `mode` gating, and asset-chain ambiguity.
5. Run the final bounded product oracle only after the static receiver/ABI path is explicit and the DLL has been rebuilt.

## What's Been Tried / Dead Ends
- Fixed timing gates were removed/reduced as success predicates; do not retune wait numbers.
- Direct diagnostic paths (`CONTINUE_LOAD_RVA`, raw deserialize, direct confirm, dispatcher drive) are not product proof.
- Down + accept/input-probe was a wrong diversion for Continue: Continue is already selected, input is blocked, and synthetic input is disallowed.
- `result+0x58 == 0` is not the visual Continue row index and is not a proven unarmed state.
