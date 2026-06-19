# Autoresearch: replace autoload fixed waits with semantic readiness gates

## Objective
Remove the fixed frame/call waits from the zero-input autoload correctness path and replace them with native/semantic readiness predicates. Do not retune numbers. A change like 30→25 or 60→90 is failure; success means the product path opens gates because the relevant native state/action is proven ready.

Primary target constants/gates:
1. `OWN_STEPPER_SETTLE_CALLS` → `title_boot_ready(...)` / `title_scheduler_ready(...)`.
2. `NATIVE_LOAD_SETTLE_FRAMES` → `title_menu_action_ready(...) -> Option<MenuActionNode>` that validates the real TitleTopDialog Load action (registry/list node, `MenuMemberFuncJob` vtable `base+0x2b265d0`, run `0x1409aaba0`, member/action chain to `0x14081ead0`).
3. `OWN_STEPPER_MODAL_GRACE` → `startup_modal_blocking_state(...)` modal lifecycle predicate using dialog class/vtable/closing state and the real OK/close handler.
4. `LIVE_DIALOG_ACTIVATE_SETTLE_WAITS` → `profile_load_dialog_ready(...)` that waits on ProfileLoadDialog fields/vtables, slot bounds/cursor, load context, PlayerGameData/session state, then calls `load_activate` only when ready.

## Metrics
- **Primary**: `readiness_gate_failures` (count, lower is better) — static failure count from `.auto/measure.sh`.
- **Secondary**: `target_constants_remaining`, `helpers_missing`, `fixed_wait_predicates`, `autoload_static_failures`, `false_positives`.

## How to Run
`./.auto/measure.sh` — emits `METRIC name=value` lines and explanatory `DETAIL ...` lines.

Repo-local run-experiment cap: use `timeout_seconds <= 120` and `checks_timeout_seconds <= 120` unless `.auto/run_experiment_policy.rego` and its checker/tests are deliberately changed. Runtime probes must finish the runtime portion in <=60s.

## Files in Scope
- `src/lib.rs` — constants/layouts/statics for title/menu/profile-load/autoload.
- `src/experiments.rs` — autoload state machine, native/static readiness predicates, modal handling, stage 2 activation.
- `scripts/check-autoload-happy-path.py` — static product-path gate checks; update from old fixed-threshold expectation to semantic readiness enforcement.
- `scripts/test-autoload-happy-path.py` — tests for the above checker.
- `.auto/measure.sh` — benchmark/static oracle for this autoresearch session.
- `.auto/ideas.md` — deferred ideas backlog.

## Off Limits
- Do not add host input, DInput/keystate/pointer synthesis, or XInput injection to the product path.
- Do not weaken save safety. Preserve backup/restore behavior, mount/char-fingerprint guards, and SetState5/continue_confirm gates.
- Do not leave Elden Ring running after any runtime probe.
- Do not file upstream issues/PRs/reports.

## Constraints
- Static RE first. Runtime probes only after a predicate hypothesis is explicit.
- Frame/call counts may remain only as outer fail-safe timeouts, never as success predicates.
- Polling semantic predicates once per game tick is allowed; requiring N ticks before success is not.
- Debug logs should say exactly which field/vtable/state opened or blocked a gate, not “waited N frames”.
- If a native completion condition is genuinely unavailable statically, record the static evidence and keep only a bounded failure timeout.

## Static evidence already gathered this session
- `TitleTopDialog::open_menu` `0x1409b24e0` sets `[dialog+0xa40]=1`, calls `set_state(dialog+0xa60, TextFadeOut desc 0x142b264f0)`, then registers rows. The native state predicates use `is_in_state(dialog+0xa60, desc)`; Loop/TextFadeOut are semantic gates.
- `MenuMemberFuncJob::run` `0x1409aaba0`: reads `[node+0x18]` member fn; if non-null computes `rcx = [node+0x10] + sign_extend([node+0x20])`; calls member fn; then cleanup. A ready Load node must have vtable `base+0x2b265d0`, non-null member fn, back pointer/adjustor, and chain to ProfileLoadDialog factory `0x14081ead0`.
- `list-register` `0x1407ab370` passes its `rcx` registry container through to `0x1407a6c00` after building a descriptor from `r8`; static notes say the precise container layout is opaque, so code should validate the actual node/action when found rather than sleep.
- `ProfileLoadDialog::load_activate` `0x1409a4670`: reads cursor via `dialog+0xa38 -> 0x140739e20`, clamps against `[dialog+0xb08]`, calls vtable slot `+0x90`/row data, requires singleton `[0x144588268]`, builds/registers selector through `0x1407b8170`/`0x1407a7b60`/`0x1407a9250`. Gate on actual fields/pointers, not 90 waits.
- `MessageBoxDialog` OK handler `0x14078e030`: if `[dialog+0x1278] <= [dialog+0x2300]`, reads cursor from `dialog+0xa38`, resolves button via `0x14078fbd0`, builds result `0x1407411e0`, commits `0x14078ef20`. Modal readiness should use vtable + closing/commit lifecycle.

## What's Been Tried
- Baseline branch has fixed timing correctness gates (30 own-stepper settle, 60 native-load menu settle, 180 modal grace, 90 live-dialog activation settle). The current task is to remove these, not tune them.
