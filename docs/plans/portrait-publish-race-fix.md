# Plan: fix the loading-portrait publish race (bd er-effects-rs-dpf6)

Branch `fix/portrait-publish-race`, worktree `.worktrees/portrait-race-fix`.
All evidence from the live run
`.worktrees/portrait-stats-crate/target/runtime-probe/product-continue-direct-20260729-194759/er-effects-autoload-debug.log`
(read it directly; timestamps below are its `[+Nms]` values).

## Measured failure (switch1, confirm at +134157ms)

1. RETARGET + window reset + boot-view rearm at +134157 (bridge cleared).
2. Kick #2 at +135227 (trigger=native-loading-screen) builds a renderer whose
   GX resources NEVER arrive while the game's own save-load runs
   (`profile-drive-resource-skip: null native GX resource wrapper`,
   `portrait-scan: no TEXTURE2D RT found`) -- ~2.3s wasted.
3. Boot-view **FPS BAIL stop at +136108** (`composite_ms=1941, handoff signals
   never fired (frozen load2)`) -- the frozen-load2 protection latches on a
   HEALTHY switch load 1.9s after confirm; `BOOT_VIEW_STOPPED` stays set until
   the next rearm, so the cover is dead for the rest of the window.
4. own-load COMMIT at +137547; kick #3 at +137533; **publish at +138063**
   (`live-feed: published built RT content 1542x1542`) -- ~0.5s after commit,
   but ~50ms AFTER the native loading screen's FadeOut began (+138018) and
   2.0s after the cover already bailed. `oracle_portrait_onto_draw_hits = 0`
   for the whole session while stats drew 455x.

Frozen-load2 insight that makes the bail fix safe: a genuinely frozen load
never publishes a portrait, so "resume on publish" cannot retrigger the
pathology the bail protects against.

## Phases (each: `bash scripts/check.sh` green; commit only after the phase's
validation run completes and shows the change is worth keeping)

### Phase 1 -- measurability (oracles)
- `oracle_portrait_confirm_to_publish_ms`: ms from switch confirm (RETARGET)
  to the next `LOADING_BG_PORTRAIT_RGBA_VERSION` bump; also keep the last
  value per epoch.
- `oracle_boot_view_cover_window_ms`: rearm -> stop duration of the last cover
  window, and `oracle_boot_view_stop_reason` (distinct values for
  fps-bail / release-fade / world-handoff at minimum).
- Publish identity: extend the publish path (sole writer:
  `portrait_worker.rs consume_portrait_frame`) to record `(slot, name-hash)`
  next to the bridge, exposed as `oracle_ls_portrait_slot` /
  `oracle_ls_portrait_name_hash`.
No behavior change. Validate offline + one bounded boot probe if needed.

### Phase 2 -- FPS-bail bounded resume on publish
Where the boot-view fps-bail latches its stop (`boot_progress.rs`, the
`FPS BAIL stop` path; the stop latch is one-way per epoch today): when a NEW
portrait publish (version bump) arrives while the native loading screen is
still active (`LOADING_SCREEN_UPDATE_HITS` advancing / fadeout-hold window),
clear the fps-bail stop ONCE per epoch so the cover composites the head for
the remainder of the window (release fade still owns the real end). Do not
touch the release-fade or world-handoff stop reasons. Justification for the
once-per-epoch bound is the frozen-load2 insight above.

### Phase 3 -- same-identity bridge hold across switch rearm
`loading_portrait_window_reset` (relocated to product `portrait_shared.rs` or
now in the crate -- find it) clears the bridge on own-menu-switch-rearm 0.1ms
after RETARGET's make-before-break claims the prior head holds. Fix: on
switch rearm, if the incoming target identity (slot + name-hash from the
Phase-1 tag) matches the currently-published head, KEEP the bridge and crop
envelope (skip only those clears; still reset per-window counters/pins as
today). Identity mismatch keeps today's full clear -- this preserves the
2026-07-06 wrong-character-clear design intent exactly.

### Phase 4 -- runtime validation (agent-owned)
Rebuild release DLL (`cargo xwin build --release --target
x86_64-pc-windows-msvc`, plus companions if missing:
`-p er-reload-trace-dll -p er-input-harness-dll -p er-telemetry-dll`).
Run `bash scripts/run-samechar-3x-threedll.sh` (Steam preflight is built-in;
loud pre-launch notice; ONLY when a `/proc` scan shows no live
`eldenring.exe`/`me3-launcher.ex` -- never two game instances). Success bar:
- `oracle_portrait_onto_draw_hits > 0` on a switch load, AND
- `oracle_portrait_confirm_to_publish_ms` + cover-window oracles show the
  head landed while the cover was compositing (not after),
- no crash, no new msgbox builds, teardown clean.
If the reload probe's known fps-dip teardown cuts the run before a switch
completes, rerun once; the prior run (samechar-3x-threedll-20260729-193655)
did complete a reload epoch.

### Phase 5 -- ONLY if phases 2+3 leave `portrait_onto_draw_hits = 0` in a
normal-speed window: pre-confirm capture. While the System->Quit ProfileSelect
window is up (`SYSTEM_QUIT_PROFILE_SELECT_WINDOW`), drive our own build/capture
for the highlighted slot and publish with its identity tag so the head exists
before confirm. Respect the ownership history: never adopt the menu's own
renderer as the source (`Product source ownership` comment in
`save_swap_profile_table.rs`); build-own only, and keep
`portrait_pipeline_idle_in_gameplay` semantics for non-menu gameplay. This
phase is a real design step -- if you reach it, record the design in the bd
issue before implementing.

## Constraints
- No new env gates; delete-not-gate; upstream read-only; no lossy UTF-8.
- Bash foreground 30s cap -- background builds/probes; never leave stale
  background shells; single game instance at a time.
- Search with Read + python3 one-liners (grep/ls/find/cat are guard-intercepted;
  rtk redacts identifiers).
- Commit AFTER a completed validation run shows the change is worth keeping
  (repo commit-timing directive); commit messages end with the standard
  Co-Authored-By + Claude-Session footer used on this branch's history.
- Finish: push branch, DRAFT PR (never main; do not `gh pr ready`); PR body +
  any gh comments end with
  ` Written by Claude Code (Fable 5), authorized by @chozandrias76`.
  Update bd er-effects-rs-dpf6 (claimed by this session) with evidence; run
  `$HOME/.local/bin/bd dolt push`.
