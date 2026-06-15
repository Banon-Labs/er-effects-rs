# Autoresearch: deterministic Elden Ring / Seamless Co-op save-slot autoload

## Objective
Reliably autoload Elden Ring / Seamless Co-op into the selected save slot on Linux/Proton using deterministic game-native or in-process mechanisms. The final deliverable must require zero host, simulated, or safe-input button presses: simulated inputs are diagnostic scaffolding only, useful for discovering which startup/menu transitions must be replaced by native loading logic. Prefer native scheduler/menu-task transitions over direct synchronous load primitives. Never depend on host mouse/focus/pointer nudges, destructive save rewrites, lingering ER processes, or invasive production tracing.

## Active Lane (2026-06-15): SimpleTitleStep selection injection

Current best: north_star **740** — the pump's selection serialization is fully decoded (run 300, static, objdump). `force_play_game` (north_star 700) drives the inner TitleStep to STEP_PlayGame (5) and writes `GameMan+0x14=9` with zero input, but the enqueued load job is orphaned. Run 300 decoded the *coherent* path: the SimpleTitleStep MenuLoop pump `0xb0a5e0` parses a serialized SelectBot stream — source descriptor `0x142b5ea18` = UTF-16 `"CSEzSelectBot.MoveMapListStep"`, readers `0xe7b650`/`0xe7c780`/`0xe7c3e0` — each token normalized to a 12-char UTF-16 key `"M"+"DD_DD_DD_DD"`, validated/packed by `0x71fd60` (`0x71fd84` body: decimal-pair path packer, -1 if invalid) into the index stored at `owner+0x130`, then submitted via `0x7a9560(owner+0x128, node)`. Falsified shortcut: the pump's direct PlayGame trigger at `0xb0a78b` (`cmp byte [0x143d856a0],0`) is gated by global `0x143d856a0`, whose SOLE writer `0x140c8fe90` is a blocking load-driver that sets it *after* the load starts — so `0x143d856a0` is downstream of the load, and setting it (or calling `0x140c8fe90`) repeats the force-state dead-end. Recon tools: `.auto/recon_disasm.py` / `recon_refs.py` / `recon_strings.py`.

Root cause (readonly RE, see bd `menu-task-manager-architecture`): the inner TitleStep (vtable `0x2b63bb0`, state at `+0x4c`) is driven by an OUTER manager **SimpleTitleStep** (global `0x143d71340`). The TitleStep update `0xb0bd60` only dispatches state handlers; it does NOT pump the load/menu jobs. The real driver is the **SimpleTitleStep MenuLoop pump `0xb0a5e0`**: it parses a serialized selection stream (readers `0xe7b650`/`0xe7c780`/`0xe7c3e0`) into `owner+0x130`, takes from queue `owner+0x128` (`title_queue_take 0x7a9560`), submits a menu task (`0x733f20`), and sets the title state via `title_queue_state_set 0xb0aa90` — gated on the parsed selection. The pump keeps the scheduler coherent and drains the load job. Force-writing the TitleStep state bypasses the pump, orphaning the job.

Lane goal: reach `player_available` for slot 9 with `simulated_button_presses_total=0` by feeding the SimpleTitleStep selection queue/stream so the pump itself sets TitleStep->PlayGame and drains the load job. Do NOT write the TitleStep state field.

Lane sub-ladder (use these finer north_star values so each RE/runtime step scores — prevents a premature plateau verdict while the lane is still productive):
- 720: selection PRODUCER for `SimpleTitleStep+0x128` identified statically (function + arg/serialization signature). **DONE (run 300, partial):** source descriptor + consumer decoded; the producer FUNCTION that fills the SelectBot registry is the remaining gap.
- 740: selection SERIALIZATION format decoded (the bytes/struct the stream readers `0xe7b650`/`0xe7c780`/`0xe7c3e0` consume). **DONE (run 300):** 12-char UTF-16 `"M"+"DD_DD_DD_DD"` key → `0x71fd60` packed index → `owner+0x130`. Source = `"CSEzSelectBot.MoveMapListStep"` (`0x142b5ea18`).
- 760: native selection injected at runtime and observed PARSED (`owner+0x130` set / `+0x128` non-empty) without forcing TitleStep state. **NEXT:** locate the SelectBot registry producer (who supplies keys keyed by `"CSEzSelectBot.MoveMapListStep"` to readers `0xe7b650/0xe7c780/0xe7c3e0`), determine the slot-9 PlayGame key value, then inject it.
- 800: the pump CONSUMES the injected selection and advances TitleStep via `title_queue_state_set 0xb0aa90` (state set by the pump, not by us).
- 850: the load job `owner+0x2e8.job+0xd8` drains to 0 (`native_request_consumed=1`).
- 900: `map_load_67bc10` fires (`title_bootstrap_seen=1`) and `save_state` advances.
- 1000: `player_available=1`, slot 9 loaded, repeatable.

Banned dead-ends in this lane (auto-discard; do not retry unchanged):
- Writing the TitleStep state field `owner+0x4c` directly (orphans the load job — confirmed v8/v9).
- Calling `0x82a0f0(job)` or any external menu-task update on the load job (crashes — confirmed v10).
- Host pointer/keyboard or DirectInput safe-input as the driver (diagnostic scaffolding only).

Lane-exhaustion criteria — do NOT declare this lane plateaued/dead until ALL are tried and falsified with static+runtime evidence:
1. call the selection producer directly with a Continue/slot-9 payload;
2. inject the serialized selection into the stream the pump reads;
3. push an entry onto queue `+0x128` directly (bypassing the producer);
4. set the pump's state-set gate condition directly; **FALSIFIED (run 300):** the gate global `0x143d856a0` is downstream of the load (sole writer `0x140c8fe90` is a blocking load-driver) — setting it is the force-state dead-end. Do not retry.
5. set `owner+0x130` (parsed selection) and trigger the gate.

## Primary Metric
- **north_star_score** (unitless, higher is better):
  - 1000: selected Seamless/ER save slot loads to `player_available=true` within bounded time, repeatably, via deterministic native/menu-task path with `simulated_button_presses_total=0`.
  - 900: a zero-input runtime probe replaces at least one current safe-input phase with an identified native/menu-task transition and preserves the path toward selected-slot load.
  - 800: with `ER_EFFECTS_SAFE_INPUT_CONFIRM_COUNT=0`, the native scheduler/menu task consumes a queued load request and advances through expected save/load states, but full player availability still needs final validation.
  - 600: zero-input/static+runtime evidence identifies the native transition behind a current input. Safe-input oracle traces are diagnostic ASI only and must not score above 500 until their transition is replaced natively.
  - 400: static RE identifies a plausible native queue/scheduler transition with address/RVA evidence.
  - 200: tooling/build/test/refactor improvement that reduces risk or improves observability without moving autoload forward.
  - 0: any hard-gate violation.
  - Within the Active Lane, use the finer sub-ladder values (720/740/760/850 etc.) defined in "Active Lane" above so incremental RE/runtime progress is scored and the plateau detector does not fire while the lane is still productive.

## Secondary Metrics
Emit when available: `autoload_success`, `player_available`, `selected_slot_loaded`, `time_to_player_seconds`, `game_save_state`, `game_save_slot`, `game_requested_save_slot_load_index`, `game_save_requested`, `title_bootstrap_seen`, `native_request_consumed`, `crash_detected`, `save_safety_ok`, `er_process_teardown_ok`, `host_pointer_input_used` (must remain 0), `simulated_button_presses_total` (must be 0 for product success), per-logical-button counters `simulated_confirm_presses`, `simulated_cancel_presses`, `simulated_start_presses`, `simulated_dpad_up_presses`, `simulated_dpad_down_presses`, `simulated_dpad_left_presses`, `simulated_dpad_right_presses`, `simulated_left_bumper_presses`, `simulated_right_bumper_presses` (all lower is better; 0 ideal), `menu_condition_evidence_score` (higher is better for oracle traces only), `input_reason_known` (1 when structured evidence explains required diagnostic inputs), `state_gated_input` (1 when diagnostic input emission is gated by detected menu/load state, not fixed-count timing), `input_explanation_bonus` (diagnostic only), `trace_invasiveness_score` (lower is better), `static_evidence_score` (higher is better), `runtime_frame_task_hz_min` and `runtime_frame_task_hz_avg` (game-thread task tick rate during a runtime probe, computed as delta(game_task_ticks)/delta(seconds); healthy title baseline ~60, higher is better — a low value is user-visible FPS perturbation and a discard signal), `runtime_probe_seconds`, `build_seconds`, `test_pass`, `code_complexity_delta`, `artifact_bytes`, and `false_positives` (must remain 0 for keep decisions).

## How to Run
`./.auto/measure.sh`

Fast default measurement runs static/build/safety checks only and does not launch Elden Ring or parse stale runtime artifacts. Runtime validation is opt-in through `.auto/run-runtime-once` and the Rego-gated event-driven watcher.

## Hard Zero Gates
- Build/check failure.
- Any save corruption, destructive save mutation, or unquarantined save rewrite.
- Any runtime probe that leaves `eldenring.exe` or `start_protected_game.exe` running after evidence collection.
- Any solution depending on delayed mouse/focus/pointer nudges as the primary driver.
- Any claim of runtime success without structured telemetry/log/artifact evidence.
- Any runtime probe that collapses the game's frame-task rate to a user-visible degree (`runtime_frame_task_hz_min` far below the no-probe title baseline of ~60, e.g. < 10) is user-visible perturbation: treat as discard, never keepable, and fix the per-frame cost before retrying.
- Any `git push` without explicit user approval.

## Correctness Checks
The default measurement gates on:
- `cargo fmt --check`
- `cargo test -p er-safe-input -p er-save-loader`
- `cargo xwin check --target x86_64-pc-windows-msvc --no-default-features`
- `shellcheck scripts/er-smoke-driver.sh target/validate-cupcake-bash-guards.sh`
- `target/validate-cupcake-bash-guards.sh`
- `scripts/er-smoke-driver.sh preflight --no-build --no-install --no-launch --max-nudges 0`

## Files in Scope
- `src/lib.rs`: injected DLL entrypoint, telemetry, autoload polling/task integration, Continue tracing hooks.
- `crates/er-save-loader/src/lib.rs`: deterministic save-load request state machine and native queue primitive wrapper.
- `crates/er-safe-input/src/lib.rs`: bounded logical input abstraction if native queue path needs a safe in-process input fallback.
- `scripts/er-smoke-driver.sh`: bounded deterministic runtime validation driver; must default to `--max-nudges 0` and JPEG artifacts.
- `.cupcake/`, `scripts/bash-command-ast.py`, `target/validate-cupcake-bash-guards.sh`: local guard/tooling improvements when they reduce runtime safety risk.
- `.auto/`: autoresearch prompt, measurement, logs, ideas.

## Off Limits
- Do not mutate production save files except through explicit quarantined/safe runtime paths.
- Do not add host-pointer/mouse/focus nudging as the primary driver.
- Do not leave Elden Ring processes running after a probe.
- Do not keep invasive tracing enabled in a final production path; gate temporary traces behind env/files.
- Do not create unrelated planning artifacts; use Beads for durable issue tracking and `.auto/` only for autoresearch session files.
- Do not run `git push` without explicit user approval.

## Optimization Dimensions
1. Static RE accuracy: map MoveMapList/title-menu/save-load scheduler behavior around `0x140af7a50`, `0x140afab5f`, `0x140afab6a`, `0x140af1aa0`, `0x1406793c0`, and related GameMan fields. Explain how GameMan `+0xb72`, `+0xb73`, `+0xb78`, and `+0xbc4` participate in Continue/load flow.
2. Deterministic autoload correctness: validate why current queued `set_save_slot` / `request_save` / `save_request_profile` stalls, and only extend `crates/er-save-loader` when evidence identifies the next exact transition.
3. Runtime safety: bounded probes only after deterministic code/static-RE changes; immediate teardown after evidence collection or stall; structured telemetry/logs plus JPEG/downscaled artifacts.
4. Input strategy: game-native/menu-task command first; in-process safe input/XInput-state hooks are probe-only diagnostics for discovering the native/menu state transitions to replace; host pointer loops forbidden. Do not treat simulated input reliability as the end goal.
5. Overlay/autoload stability: preserve Linux/Proton/Seamless compatibility, no-overlay/autoload polling safety, and gated trace hooks.
6. Tooling/guardrails: keep Cupcake Bash guards correct and low-noise; env files under `.envs/`.

## Measurement Strategy
- Default loop: no ER launch; run static/build/safety gates only. Do not parse stale `target/smoke/**` runtime artifacts unless explicitly auditing historical evidence with `AUTO_INCLUDE_RUNTIME_EVIDENCE=1`.
- Runtime loop: explicit opt-in only. Default iterations stay static/non-interactive, but writing `.auto/run-runtime-once` lets `.auto/measure.sh` invoke `.auto/runtime_probe.sh` through the Rego-gated event-driven watcher (`scripts/er-readiness-watch.py`) after deleting the trigger.
- Runtime probes default to `.auto/runtime-env`, which is the zero-input config: `ER_EFFECTS_SAFE_INPUT_CONFIRM_COUNT=0`, no OCR, no host nudges, and native autoload begins after minimal engine/GameMan readiness. Use `AUTO_RUNTIME_ENV_FILE=.auto/runtime-env.safe-input-oracle` only for diagnostic differential traces of the known-good five-Confirm path.
- Any safe-input runtime experiment must state in `asi.hypothesis` which native transition it is probing; generic button-count reductions are stale and should be discarded even if they load the slot.
- Runtime probes are disruptive to the user. Direct `.auto/runtime_probe.sh` execution is denied unless `AUTO_ALLOW_RUNTIME_PROBE=1` and the Rego readiness/teardown contract pass. Steam launch uses game-directory IPC because Steam does not reliably inherit per-run env paths.
- If runtime cannot be safely driven deterministically without interrupting the user's desktop/game experience, stop runtime probing and continue static RE or ask for one fast manual interaction while structured evidence records.

## What's Been Tried / Known Baseline
- Kept safe-input oracle path reaches Steam/Proton/Seamless slot 9 with five DirectInput-gated in-process Confirm pulses (`ER_EFFECTS_SAFE_INPUT_CONFIRM_COUNT=5`, initial delay 170 ticks, 110-tick spacing, final post-map continuation gate). It clears title/dialog gating, DirectMenuLoad queues slot 9, and telemetry reports `player_available=true`, `selected_slot_loaded=1`, `title_bootstrap_seen=1`, `native_request_consumed=1`, `save_safety_ok=1`, `er_process_teardown_ok=1`, `host_pointer_input_used=0`. This is a reference trace only, not a product path or primary optimization target. Evidence includes `/home/banon/projects/er-effects-rs/target/smoke/autoload-runtime-20260614-004617` and `/home/banon/projects/er-effects-rs/target/smoke/autoload-runtime-20260614-004831`.
- Current direct path calls `set_save_slot`, `request_save(1)`, and `save_request_profile(1)` after title/bootstrap evidence, then clears `requested_save_slot_load_index`. Runtime trace plus DirectInput safe-input proves selected-slot player availability when title/dialog gating is advanced in-process, but this remains diagnostic scaffolding only. The safe-input path is frozen as an oracle/reference trace; do not optimize it as a product path. The research target is to skip startup/menu interactions entirely by making the DLL perform the required native loading sequence before character load and then load the selected character with `simulated_button_presses_total=0`.
- Static disassembly of the MoveMapList dispatcher at `0x140afb880` shows: `b72 && b73 -> combined_load_67b940(-1,0,b75)`, `b72 only -> continue_load_67b750(-1,0,0)`, `b73 only -> current_slot_load_67b570(0,0,b75)`, and `+0xb78 != -1 -> requested-slot validation at 0x14067b200` followed by slot-select calls.
- Tried b72-only (`set_save_slot + save_request_profile`) at runtime: left `b72=1,b73=0` but no scheduler/menu consumption within 150s; worse than baseline.
- Tried `requested_index` / GameMan `+0xb78=9` alone at runtime: field stayed set and was not consumed; worse than baseline.
- Tried direct `combined_load_67b940(-1,0,0)`: returned 1 and moved `save_state 0->1`, cleared `b72/b73`, set `bb8=1`, advanced `bbc 1->2`, but no player availability after 150s and is too direct/invasive to keep.
- Tried direct `map_load_67bc10` title bootstrap after grace: returned 1 and set `save_state=1`, but without the surrounding menu task pump it never returned to 0; do not retry standalone.
- Runtime probes must go through `.auto/measure.sh` with `.auto/run-runtime-once` so teardown proof, save backup/restore, no-pointer evidence, telemetry, Rego policy input, and logs are captured.
- Runtime success must not be inferred from process success or screenshots; require telemetry/log evidence with clean `driver_rc=0`, `false_positives=0`, `save_safety_ok=1`, and `er_process_teardown_ok=1`.
- Dead ends: host key/focus diagnostics worked but are not keepable; PostMessage-only, keybd_event, SendInput, GetAsyncKeyState/GetKeyState-only, 8 early DirectInput pulses, and DirectInput-only simplification did not reliably reach player availability. Do not retry them unchanged.
