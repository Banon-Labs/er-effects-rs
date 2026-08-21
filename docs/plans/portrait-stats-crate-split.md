# Plan: portrait+stats loading-screen crate split

Tracking: bd `er-effects-rs-f9mq` * branch `feature/portrait-stats-crate` *
worktree `.worktrees/portrait-stats-crate`

## Objective

1. **DELETE path A** -- the older, wrong-scale portrait+stats render path that
   composites over the game's own now-loading screen during the initial
   boot/autoload (aspect-cover upscale of the padded 15422 square RT +
   max-dimension-scaled stats text). Delete, do not gate
   (bd `USER-DIRECTIVE-delete-nonworking-code-not-gate-no-stale-2026-07-21`).
2. **EXTRACT path B** -- the newer, right-scale version (frozen alpha-bbox crop,
   80%-screen-height portrait, height-scaled stats at 5%/60%) into:
   - `crates/er-loading-portrait` -- reusable feature crate: capture pipeline
     (profile-table build, renderer drive, idle-anim bind, camera override,
     staged color+depth readback, depth-key worker, frame bridge), stats-lines
     producer + raster, portrait/stats CPU compositors, native-overlay window
     host.
   - `crates/er-loading-portrait-dll` -- thin standalone ME3-loadable cdylib
     shell (own DllMain, VEH crash logger, log file), individually shippable.
     Follows `er-loading-bar-dll` exactly (cdylib+rlib, host-testable).
   - `er-effects-rs` keeps bundling the feature by depending on
     `er-loading-portrait`, product behavior unchanged (same arming, no new
     env gates -- bd `no-new-env-gated-features`).

## Ground truth: the two paths (from the 2026-07-29 Explore map)

**Path A (delete)** -- in-swapchain Present composite over the game's own
now-loading screen. Draw stack: `present_overlay.rs:382` ->
`overlay_composite.rs composite_portrait_on_swapchain:413` ->
`composite_portrait_inner:659` -> GPU draw `:956/:1299` (own PSO/root-sig/FXC
HLSL) + CPU fallback `:1926`. Wrong-scale proof: `:1522` aspect-**cover**
`scale = (cw/sw).max(ch/sh)` over the full backbuffer (`:942-945`), stats via
`stats_text_screen_bitmap(bw.max(bh))` back-mapped through the crop (`:1815`).
Wine-only (`present_overlay.rs:376` bails on native). Also A: the forge
(`gfx_loading_portrait.rs`, forge halves of `profile_table_gfx_files.rs`:
`loading_bg_replace_bind_hook:193`, `forge_into_rti:283`,
`maybe_reforge_loading_portrait:517`), the retired title-cover portrait bits
(`title_tick_cover.rs:1076,:1191,:1193,:2950-2985` behind hard-false
`portrait_render_window_enabled`), dead `refresh_loading_bg_live_gx`
(`dlstring_lookat_math.rs:619`, short-circuited at `:630`), unreferenced
`maybe_capture_portrait_gxtexture`, and the A-only `GFX_PORTRAIT_*` tail of
`constants/portrait_semaphores.rs:319-395`.

**Path B (extract)** -- CPU composite into the shared boot/loading frame.
`portrait_overlay.rs:52 portrait_onto` (frozen crop envelope, 80% height,
bottom-anchored) + `stats_overlay.rs:40 overlay_stats_onto` (screen-height em,
5%/60%), drawn inside `boot_progress.rs boot_view_rasterize:1420`; hosts:
`native_overlay.rs` (own window+device, native Windows) and
`present_overlay.rs:394 -> boot_progress.rs:1876` (Wine in-swapchain via
`er_d3d12_compositor::copy_rgba_frame_to_swapchain`).

**Shared capture pipeline (extract with B -- it is what makes the portrait
animated)**: `lookat_bone_hooks.rs` (`profile_lookat_realtime_draw_tick:346`,
idle-anim bind `:725-774`), `lookat_stage_camera.rs` (draw-phase task entry,
`per_frame_push_hook:509`, camera override `:753`), `dlstring_lookat_math.rs`
(now-loading observer hooks `:301`, math), `portrait_worker.rs` (worker thread,
sole publisher of `LOADING_BG_PORTRAIT_RGBA`), `resource_readback.rs`,
`cached_depth_readback.rs`, `depth_mask_upload.rs` (minus the A-only
`upload_rgba_to_texture`/`upload_head_into_gfx_texture`), the B/shared halves
of `save_swap_profile_table.rs` (`force_profile_render_tick:418` pump) and
`loading_cover_save_slot.rs` (oracles, table build), `stats_loading_text.rs`,
constants `portrait_lookat.rs`, `portrait_camera.rs`, `portrait_semaphores.rs`
(minus A tail), the renderer-ownership ledger from `profile_render.rs:17-36`,
and the frame bridge `LOADING_BG_PORTRAIT_RGBA(+_VERSION)` from
`constants/anti_debug.rs:779-788`.

**Not portrait (do NOT touch)**: save-picker overlay, loading-bar/milestone
machinery in `boot_progress.rs`, `startup_modals_menu_cover.rs`,
`title_resources_stats_text.rs` (ProfileSelect row stats),
`stats_panel_background/text` constants, the non-portrait bulk of
`title_tick_cover.rs`, Scaleform swap observers in
`profile_table_gfx_files.rs:697-1140`.

## Deletion traps (each verified by the map -- handle explicitly)

1. `overlay_composite.rs` embeds four items B/shared depend on -- RELOCATE
   before deleting the file: `loading_portrait_window_reset:426` (called from
   B's re-arm `boot_progress.rs:477`; only caller of
   `gfx_loading_portrait_window_reset` + `stats_text_window_reset`; only
   bounded drain of `PORTRAIT_JOB_INFLIGHT`), `invalidate_portrait_depth_mask:650`
   (called from `portrait_worker.rs:261` and
   `system_quit_repro_guards.rs:1439`), `portrait_center_nonblack:2268` and
   `portrait_looks_like_checker:2307` (publish-acceptance classifiers used by
   the shared pipeline).
2. `present_overlay.rs` is A's host AND B's Wine host AND the save-picker
   input tick + boot self-present pump. Delete only the A call (`:382`) and
   A-specific gates; keep the hook, the pump, and the B composite (`:394`).
3. `resource_readback.rs:106-182` declares the `OVERLAY_*` telemetry
   re-exports (flat `include!` namespace): remove declarations together with
   writers and the `write_game_module_oracles.rs` readers.
4. Watcher scripts: `er-readiness-watch.py:720-721` (`oracle_overlay_draw_hits`
   screenshot trigger) and `:1309-1321` (`oracle_overlay_stop_reason == 5`
   teardown) become dead -- update the script logic, don't leave dead branches.
   `y22i-windows-ab-probe.py` is A-only -- delete it. Do NOT touch
   `check-runtime-probe-contract.py` / `check-autoload-happy-path.py` unless
   an oracle they read is actually removed; if `oracle_title_portrait_visible_surface_bound`
   is removed with the title-cover portrait bits, change the contract checker
   and `test-runtime-probe-contract.py` together in one commit.
5. A-only deps to drop from the root crate if verified dead after deletion:
   `er-tpf` (forge-only per map), `Win32_Graphics_Direct3D_Fxc` (A's HLSL
   compile).
6. `save_swap_profile_table.rs:450-457` A/B fork: the
   `!portrait_overlay_enabled()` -> `maybe_reforge_loading_portrait` arm dies
   with A; keep `maybe_build_stats_text` on the surviving arm.

## Crate seams (follow the existing pattern, stated in the crates themselves)

- `er-loading-bar` keeps only game-free label/raster primitives;
  `er-d3d12-compositor` takes product state through `set_log_sink` /
  `set_frame_provider` / `copy_rgba_frame_to_swapchain`. `er-loading-bar-dll`
  is the standalone negative control. Mirror this.
- `er-loading-portrait` may depend on: er-game-base (+game-types), er-hook,
  er-telemetry, er-gfx, er-loading-bar, er-d3d12-compositor, erpx-rs (core),
  eldenring/fromsoftware-shared, windows, iced-x86 if needed. It must NOT
  depend on the root crate. Product-side state the feature currently reads
  (BOOT_VIEW_* host gates, SYSTEM_QUIT_* switch phases, CAN_MOVE_CONFIRMED,
  `save_picker_overlay_active`) crosses the seam as injected callbacks/setters
  (the `set_frame_provider` pattern), not as reverse imports.
- Portrait-specific statics move INTO the crate (`LOADING_BG_PORTRAIT_RGBA`
  bridge, `PROFILE_LOOKAT_*`, `PORTRAIT_*`, pins/rings/ledger); the product
  imports them from the crate. Telemetry counters re-point to `er-telemetry`
  directly.
- `native_overlay.rs` moves into the crate with a frame-provider + show-flag
  seam; the product registers its `boot_view_render_frame`-based provider
  (bar+picker+portrait+stats composition stays product-side), the standalone
  DLL registers a portrait+stats-only provider and, on Wine, uses
  `er_d3d12_compositor::install_loading_bar_present_compositor` as its host
  exactly like `er-loading-bar-dll`.
- The standalone DLL must never be loaded alongside `er_effects_rs.dll` in one
  me3 profile (double Present detour / double MinHook) -- document this in its
  Cargo.toml like the er-save-suppress warning.

## Phases (each ends green: `bash scripts/check.sh` + host tests; commit per phase)

1. **Relocate the four shared helpers** out of `overlay_composite.rs` (new
   `gpu_readback/portrait_shared.rs`, still product-side). Green, commit.
2. **Delete path A**: `overlay_composite.rs`, `gfx_loading_portrait.rs`, forge
   halves of `profile_table_gfx_files.rs`, A call+gates in
   `present_overlay.rs`, title-cover portrait remnants, dead helpers, A-only
   constants/env flags (`portrait_render_window_enabled`), `GFX_PORTRAIT_*`
   tail, `OVERLAY_*` telemetry (writers, re-export declarations, oracle
   fields), A-only deps (verify first). Green, commit.
3. **Script cleanup**: er-readiness-watch.py dead branches, delete y22i probe,
   contract-checker coordination only if actually required. Run the script
   test suite (`scripts/test-er-readiness-watch.py` etc.). Green, commit.
4. **Create `er-loading-portrait`** and move path B + capture pipeline +
   stats producer into it as real `mod`s (no `include!`), breaking product
   coupling via the seams above. Product still compiles bundled and behaves
   identically. Green, commit (likely several commits).
5. **Add `er-loading-portrait-dll`** shell + host smoke test, workspace
   member, xwin release build of both DLLs. Green, commit.
6. **Wrap up**: full gates (`bash scripts/check.sh`,
   `cargo test -p er-soulsformats -p er-param-inspect` + new crate host tests,
   `cargo xwin build --release --target x86_64-pc-windows-msvc`), push branch,
   open DRAFT PR (never main; PRs stay draft). PR body must state plainly:
   runtime smoke NOT yet run; merge requires the standard product ME3 launch
   proof (loading-screen portrait moment capture + `oracle_char_stats` /
   portrait publish oracles) run from the main session.

## Out of scope

- No game launches / runtime probes (main session owns runtime proof).
- No upstream (fromsoftware-rs) changes. No new env-gated behavior.
- No save-picker / loading-bar / title-cover-non-portrait changes beyond the
  exact seams named above.
