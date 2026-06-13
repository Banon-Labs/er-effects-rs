# Autoresearch: deterministic Elden Ring / Seamless Co-op save-slot autoload

## Objective
Reliably autoload Elden Ring / Seamless Co-op into the selected save slot on Linux/Proton using deterministic game-native or in-process mechanisms. Prefer native scheduler/menu-task transitions over direct synchronous load primitives. Never depend on host mouse/focus/pointer nudges, destructive save rewrites, lingering ER processes, or invasive production tracing.

## Primary Metric
- **north_star_score** (unitless, higher is better):
  - 1000: selected Seamless/ER save slot loads to `player_available=true` within bounded time, repeatably, via deterministic native/menu-task path.
  - 800: native scheduler/menu task consumes queued load request and advances through expected save/load states, but full player availability still needs final validation.
  - 600: exact Continue/native load sequence is statically mapped and runtime trace confirms the relevant state transition.
  - 400: static RE identifies a plausible native queue/scheduler transition with address/RVA evidence.
  - 200: tooling/build/test/refactor improvement that reduces risk or improves observability without moving autoload forward.
  - 0: any hard-gate violation.

## Secondary Metrics
Emit when available: `autoload_success`, `player_available`, `selected_slot_loaded`, `time_to_player_seconds`, `game_save_state`, `game_save_slot`, `game_requested_save_slot_load_index`, `game_save_requested`, `title_bootstrap_seen`, `native_request_consumed`, `crash_detected`, `save_safety_ok`, `er_process_teardown_ok`, `host_pointer_input_used` (must remain 0), `trace_invasiveness_score` (lower is better), `static_evidence_score` (higher is better), `runtime_probe_seconds`, `build_seconds`, `test_pass`, `code_complexity_delta`, `artifact_bytes`, and `false_positives` (must remain 0 for keep decisions).

## How to Run
`./.auto/measure.sh`

Fast default measurement runs static/build/safety checks and parses latest structured runtime evidence under `target/smoke/` without launching Elden Ring. Runtime validation must be explicit, bounded, deterministic, and produce telemetry/log/artifact evidence plus teardown proof.

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
- Default loop: no ER launch; run static/build/safety gates; parse latest `target/smoke/**/{telemetry,final-telemetry}.json`, `autoload-debug*.log`, and `continue-trace.log`.
- Runtime loop: only after a deterministic static/code change; use `scripts/er-smoke-driver.sh` with bounded time, `--max-nudges 0`, JPEG artifacts, exact artifact dir, telemetry path, trace/debug logs, and teardown proof.
- If runtime cannot be safely driven deterministically, stop runtime probing and continue static RE or ask for one fast manual interaction while structured evidence records.

## What's Been Tried / Known Baseline
- Current dirty baseline already contains `er-safe-input`, `er-save-loader`, no-overlay autoload polling, GameMan telemetry, Continue/load trace hooks, and `scripts/er-smoke-driver.sh` with default `MAX_NUDGES=0` and JPEG screenshots.
- Current queued direct path calls `set_save_slot`, `request_save(1)`, and `save_request_profile(1)` after title/bootstrap evidence, then clears `requested_save_slot_load_index`. Runtime trace can prove native consumption/state transition (800) but not player availability.
- Static disassembly of the MoveMapList dispatcher at `0x140afb880` shows: `b72 && b73 -> combined_load_67b940(-1,0,b75)`, `b72 only -> continue_load_67b750(-1,0,0)`, `b73 only -> current_slot_load_67b570(0,0,b75)`, and `+0xb78 != -1 -> requested-slot validation at 0x14067b200` followed by slot-select calls.
- Tried b72-only (`set_save_slot + save_request_profile`) at runtime: left `b72=1,b73=0` but no scheduler/menu consumption within 150s; worse than baseline.
- Tried `requested_index` / GameMan `+0xb78=9` alone at runtime: field stayed set and was not consumed; worse than baseline.
- Tried direct `combined_load_67b940(-1,0,0)`: returned 1 and moved `save_state 0->1`, cleared `b72/b73`, set `bb8=1`, advanced `bbc 1->2`, but no player availability after 150s and is too direct/invasive to keep.
- Tried direct `map_load_67bc10` title bootstrap after grace: returned 1 and set `save_state=1`, but without the surrounding menu task pump it never returned to 0; do not retry standalone.
- Runtime probes must go through `.auto/runtime_probe.sh` / `.auto/run-runtime-once` so teardown proof, save hash comparison, no-pointer evidence, telemetry, and JPEG artifacts are captured.
- Runtime success must not be inferred from process success or screenshots; require telemetry/log evidence.
