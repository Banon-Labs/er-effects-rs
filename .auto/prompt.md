# Autoresearch: deterministic Elden Ring / Seamless Co-op save-slot autoload

## Objective
Reliably autoload Elden Ring / Seamless Co-op into the selected save slot on Linux/Proton using deterministic game-native or in-process mechanisms. Prefer native scheduler/menu-task transitions over direct synchronous load primitives. Never depend on host mouse/focus/pointer nudges, destructive save rewrites, lingering ER processes, or invasive production tracing.

## Primary Metric
- **north_star_score** (unitless, higher is better):
  - 1000: selected Seamless/ER save slot loads to `player_available=true` within bounded time, repeatably, via deterministic native/menu-task path with `simulated_button_presses_total=0`.
  - 950-minus-buttons: selected Seamless/ER save slot loads to `player_available=true` repeatably with deterministic in-process safe-input assistance; score is `max(900, 950 - simulated_button_presses_total)`, so lower simulated-button counts directly improve the primary metric and 0 remains the ideal.
  - 800: native scheduler/menu task consumes queued load request and advances through expected save/load states, but full player availability still needs final validation.
  - 600: exact Continue/native load sequence is statically mapped and runtime trace confirms the relevant state transition.
  - 400: static RE identifies a plausible native queue/scheduler transition with address/RVA evidence.
  - 200: tooling/build/test/refactor improvement that reduces risk or improves observability without moving autoload forward.
  - 0: any hard-gate violation.

## Secondary Metrics
Emit when available: `autoload_success`, `player_available`, `selected_slot_loaded`, `time_to_player_seconds`, `game_save_state`, `game_save_slot`, `game_requested_save_slot_load_index`, `game_save_requested`, `title_bootstrap_seen`, `native_request_consumed`, `crash_detected`, `save_safety_ok`, `er_process_teardown_ok`, `host_pointer_input_used` (must remain 0), `simulated_button_presses_total` (0 is ideal), per-logical-button counters `simulated_confirm_presses`, `simulated_cancel_presses`, `simulated_start_presses`, `simulated_dpad_up_presses`, `simulated_dpad_down_presses`, `simulated_dpad_left_presses`, `simulated_dpad_right_presses`, `simulated_left_bumper_presses`, `simulated_right_bumper_presses` (all lower is better; 0 ideal), `trace_invasiveness_score` (lower is better), `static_evidence_score` (higher is better), `runtime_probe_seconds`, `build_seconds`, `test_pass`, `code_complexity_delta`, `artifact_bytes`, and `false_positives` (must remain 0 for keep decisions).

## How to Run
`./.auto/measure.sh`

Fast default measurement runs static/build/safety checks only and does not launch Elden Ring or parse stale runtime artifacts. Runtime validation is currently disabled fail-closed until the event-driven runtime driver is redesigned.

## Hard Zero Gates
- Build/check failure.
- Any save corruption, destructive save mutation, or unquarantined save rewrite.
- Any runtime probe that leaves `eldenring.exe` or `start_protected_game.exe` running after evidence collection.
- Any solution depending on delayed mouse/focus/pointer nudges as the primary driver.
- Any claim of runtime success without structured telemetry/log/artifact evidence.
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
4. Input strategy: game-native/menu-task command first; in-process safe input/XInput-state hook second; host pointer loops forbidden.
5. Overlay/autoload stability: preserve Linux/Proton/Seamless compatibility, no-overlay/autoload polling safety, and gated trace hooks.
6. Tooling/guardrails: keep Cupcake Bash guards correct and low-noise; env files under `.envs/`.

## Measurement Strategy
- Default loop: no ER launch; run static/build/safety gates only. Do not parse stale `target/smoke/**` runtime artifacts unless explicitly auditing historical evidence with `AUTO_INCLUDE_RUNTIME_EVIDENCE=1`.
- Runtime loop: explicit opt-in only. Default iterations stay static/non-interactive, but writing `.auto/run-runtime-once` lets `.auto/measure.sh` invoke `.auto/runtime_probe.sh` through the Rego-gated event-driven watcher (`scripts/er-readiness-watch.py`) after deleting the trigger.
- Runtime probes are disruptive to the user. Direct `.auto/runtime_probe.sh` execution is denied unless `AUTO_ALLOW_RUNTIME_PROBE=1` and the Rego readiness/teardown contract pass. Steam launch uses game-directory IPC because Steam does not reliably inherit per-run env paths.
- If runtime cannot be safely driven deterministically without interrupting the user's desktop/game experience, stop runtime probing and continue static RE or ask for one fast manual interaction while structured evidence records.

## What's Been Tried / Known Baseline
- Kept result `291a746` (building on `64d4d96`) reaches the safe-input-assisted tier on Steam/Proton/Seamless slot 9: DirectInput-gated in-process safe-input Confirm pulses (delayed until `IDirectInputDevice8::GetDeviceState` hook installs) clear title/dialog gating, DirectMenuLoad queues slot 9, telemetry reports `player_available=true`, `selected_slot_loaded=1`, `title_bootstrap_seen=1`, `native_request_consumed=1`, `save_safety_ok=1`, `er_process_teardown_ok=1`, `host_pointer_input_used=0`. Under the zero-button-aware scorer this is 938 because `simulated_confirm_presses=12` / `simulated_button_presses_total=12` and safe-input success scores `950 - simulated_button_presses_total`; 0 is reserved for the ideal native/menu-task path. 16 confirms succeeded twice; 12 confirms at 30-tick spacing also succeeded and is the current minimum known-good count; 10 and 11 reached native consumption but not player availability; 20-tick spacing failed before native consumption and 25-tick spacing reached native consumption but not player availability. Evidence: `/home/banon/projects/er-effects-rs/target/smoke/autoload-runtime-20260613-173930`, confirmation `/home/banon/projects/er-effects-rs/target/smoke/autoload-runtime-20260613-174139`, 12-confirm run `/home/banon/projects/er-effects-rs/target/smoke/autoload-runtime-20260613-174445`, and restored fallback run `/home/banon/projects/er-effects-rs/target/smoke/autoload-runtime-20260613-180325`.
- Current direct path calls `set_save_slot`, `request_save(1)`, and `save_request_profile(1)` after title/bootstrap evidence, then clears `requested_save_slot_load_index`. Runtime trace plus DirectInput safe-input now proves selected-slot player availability (1000) when title/dialog gating is advanced in-process.
- Static disassembly of the MoveMapList dispatcher at `0x140afb880` shows: `b72 && b73 -> combined_load_67b940(-1,0,b75)`, `b72 only -> continue_load_67b750(-1,0,0)`, `b73 only -> current_slot_load_67b570(0,0,b75)`, and `+0xb78 != -1 -> requested-slot validation at 0x14067b200` followed by slot-select calls.
- Tried b72-only (`set_save_slot + save_request_profile`) at runtime: left `b72=1,b73=0` but no scheduler/menu consumption within 150s; worse than baseline.
- Tried `requested_index` / GameMan `+0xb78=9` alone at runtime: field stayed set and was not consumed; worse than baseline.
- Tried direct `combined_load_67b940(-1,0,0)`: returned 1 and moved `save_state 0->1`, cleared `b72/b73`, set `bb8=1`, advanced `bbc 1->2`, but no player availability after 150s and is too direct/invasive to keep.
- Tried direct `map_load_67bc10` title bootstrap after grace: returned 1 and set `save_state=1`, but without the surrounding menu task pump it never returned to 0; do not retry standalone.
- Runtime probes must go through `.auto/measure.sh` with `.auto/run-runtime-once` so teardown proof, save backup/restore, no-pointer evidence, telemetry, Rego policy input, and logs are captured.
- Runtime success must not be inferred from process success or screenshots; require telemetry/log evidence with clean `driver_rc=0`, `false_positives=0`, `save_safety_ok=1`, and `er_process_teardown_ok=1`.
- Dead ends: host key/focus diagnostics worked but are not keepable; PostMessage-only, keybd_event, SendInput, GetAsyncKeyState/GetKeyState-only, 8 early DirectInput pulses, and DirectInput-only simplification did not reliably reach player availability. Do not retry them unchanged.
