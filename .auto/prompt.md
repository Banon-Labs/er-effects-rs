# Autoresearch: built-in fast-load / splash-skip core feature

## Objective
Identify the piece of the fast-load equation that the external `er_skip_splash_screens.dll` was intended to provide, then implement the useful behavior directly in this repo so release builds do not depend on that external DLL. The final feature must preserve the already-proven zero-input autoload path, LazyLoader `[CHAINLOAD]` packaging, save-safety guards, and compatibility with other LazyLoader mods.

## Metrics
- **Primary**: `fast_load_seconds` (seconds, lower is better). This is `time_to_player_seconds` only when the full correctness oracle succeeds (`north_star_score=1400`, `false_positives=0`, `save_safety_ok=1`, zero simulated input, under 60s); failures score as `999.0`.
- **Secondary**: `north_star_score`, `time_to_player_seconds`, `runtime_probe_seconds`, `oracle_world_stable_samples`, `oracle_save_identity_matches`, `visual_llm_world_expected`, `simulated_button_presses_total`, `save_safety_ok`, `false_positives`, `code_complexity_delta`.

## How to Run
1. Write `.auto/run-runtime-once` pointing at the desired runtime env file.
2. Run `./.auto/measure.sh`.

For Pi autoresearch, use `run_experiment` with `timeout_seconds <= 60` and `checks_timeout_seconds <= 60`.

## Current Baseline Target
The baseline runtime env is `.auto/runtime-env-fast-load-core-baseline-20260619`: LazyLoader `dinput8.dll`, `er_effects_rs.dll` loaded through `[CHAINLOAD]`, built-in `ER_EFFECTS_SPLASH_SKIP=1`, zero simulated input, world-stable visual oracle. Last known manual proof before this session: `/home/banon/projects/er-effects-rs/target/smoke/lazyloader-chainload-internal-splash-skip-20260619` reached `1400/1400` at `time_to_player_seconds=40.973`.

## Files in Scope
- `src/experiments.rs` — splash/fast-load patches, title/menu/autoload runtime gates, static patch implementations.
- `src/lib.rs` — DllMain startup ordering and product gate wiring.
- `scripts/stage-autoload-release.sh` — release packaging and optional feature examples.
- `scripts/check-autoload-happy-path.py` / `scripts/test-autoload-happy-path.py` / `scripts/check.sh` — fail-closed guardrails.
- `.auto/measure.sh`, `.auto/runtime_probe.sh`, `.auto/runtime-env-*`, `.auto/prompt.md`, `.auto/ideas.md` — autoresearch harness only.

## Off Limits
- Do not weaken save-safety guards around `continue_confirm` / `SetState(5)`.
- Do not add simulated host pointer/keyboard/gamepad input. `simulated_button_presses_total` must remain `0` for any keep.
- Do not depend on the external `er_skip_splash_screens.dll` for the product path.
- Do not file or propose upstream issues/PRs.
- Do not use broad process-kill patterns; runtime probes must tear down only bounded probe processes.

## Constraints
- Runtime probes require `.auto/run-runtime-once` and `RUNTIME_TIMEOUT_SECONDS <= 60`.
- Every `run_experiment` call in this repo must use `timeout_seconds <= 60` and `checks_timeout_seconds <= 60`.
- Prefer static RE before runtime changes when it can answer the question.
- Do not overfit to one benchmark artifact: improvements must preserve the full oracle and must be explained by static/runtime evidence.
- The release path must continue to stage LazyLoader as `dinput8.dll`, `er_effects_rs.dll` in `[CHAINLOAD]`, and other mods in `dllMods/[LOADORDER]`.

## Known Evidence
- External `er_skip_splash_screens.dll` isolated under LazyLoader, with no er-effects DLL loaded, exits immediately on this local executable: `Unexpected opcode at target address (offset b0c3ed): 5, expected 116`. Artifact: `/home/banon/projects/er-effects-rs/target/smoke/splash-skipper-isolated-20260619`.
- Static byte check against local `eldenring.exe`: RVA `0xb0c3ed` currently has byte `0x05`; our built-in patch at RVA `0xb0c35d` sees expected byte `0x74` and flips it to `0x7f`.
- Built-in `ER_EFFECTS_SPLASH_SKIP=1` with LazyLoader `[CHAINLOAD]` reached full score. Artifact: `/home/banon/projects/er-effects-rs/target/smoke/lazyloader-chainload-internal-splash-skip-20260619`.

## What's Been Tried
- Implemented and merged the core zero-input product autoload release path: direct autoload request arms own-stepper/live-dialog/fullread/menu-window-latch internally, and post-world TitleTopDialog cleanup restores the visual oracle.
- Proved LazyLoader itself is compatible when `er_effects_rs.dll` is loaded through `[CHAINLOAD]`; lazy-loading er-effects through `[LOADORDER]` is the wrong path.
- Proved the old external splash skipper DLL is not usable as-is on this executable in isolation, but the built-in current-version splash patch works.

## First Research Steps
1. Establish a fresh baseline on this branch using `.auto/runtime-env-fast-load-core-baseline-20260619` and `fast_load_seconds`.
2. Statically inspect the current executable around the external DLL's failed RVA `0xb0c3ed` and the working built-in RVA `0xb0c35d` to infer what fast-load phase each controls.
3. Add minimal, evidence-backed core patch(es) only if they explain a real fast-load gap and preserve the full oracle.
4. If a promising patch is too risky to pursue immediately, add it to `.auto/ideas.md` with the exact RVA/static evidence.
