# D2: Scaleform hook ownership

**Status:** accepted

**Decision baseline:** `ddac122d` (2026-08-13)
**Decision:** create `er-scaleform-hooks`; do not deepen `er-gfx` with game-process hooks.

This decision only selects the owner. It does not move hook implementations. R8 owns the whole-file descriptor-guard move, R23 owns the new crate boundary, and R24a+ owns the individual descriptor/resource/message hook moves and their runtime proof.

## Current interfaces

`er-gfx` is a host-testable GFX codec and transformation library. Its manifest has one external dependency (`bitflags`) and no workspace, game, Windows, telemetry, or hook dependency. Its public module boundary is currently:

- codec/editing: `edit` plus the root `Movie`, `Tag`, and related codec types;
- movie transformations: `announce_notice`, `arts_badge`, `options_02_040`, `profile_05_010_layout`, `profile_05_010_protocol`, `text_input_02_990`, `title_05_000`, `title_05_010`, and `world_map_pin`;
- host rendering support: `raster`.

The native interface is separate today and remains root-private. The relevant current seams are:

- `scaleform_descriptor_advance_hook` and `install_scaleform_descriptor_guard`;
- `title_menu_resource_acquire_observer_hook`;
- `title_scaleform_file_open_observer_hook` and `title_scaleform_resource_ctor_observer_hook`;
- `title_05_000_swap_to_stripped`, `profile_05_010_swap_to_edited`, `text_input_02_990_swap_to_inline`, and `options_02_040_quit4_swap_to_edited`;
- `title_scaleform_bind_observer_hook` and `title_scene_obj_proxy_named_child_bind_hook`, with their installers.

Those functions validate or mutate the game's `MemoryFile`, install detours, read game memory, and publish runtime counters. They consume `er-gfx` transformations, but they are not codec operations.

The source files are mixed at this baseline: `profile_table_gfx_files.rs` is 898 lines, `title_resources_stats_text.rs` is 2,402 lines, and `title_scaleform_msgbox.rs` is 868 lines. R24a+ must therefore cut at the function families above instead of treating those mixed files as whole-file moves. `scaleform_descriptor_guard.rs` is the one clean 95-line whole-file move owned by R8.

## Dependency evidence

The current workspace contains this path:

```text
er-title-flow -> er-loading-portrait -> er-gfx
```

The candidate native Scaleform slice uses both `er-gfx` and title-flow-owned pointer bounds / scene-object identities. Putting that slice in `er-gfx` would require an `er-gfx -> er-title-flow` edge or a new public callback surface invented only to avoid it. The direct edge closes this package cycle:

```text
er-gfx -> er-title-flow -> er-loading-portrait -> er-gfx
```

A sibling owner keeps the existing direction intact:

```text
er-scaleform-hooks -> er-gfx
er-scaleform-hooks -> er-hook / er-game-base / er-telemetry
```

R23 may either use a narrow title-flow dependency for the current named-child bind seam (which remains acyclic from the sibling crate) or invert that one seam through a specific adapter. It must not solve the boundary with a broad product host structure.

## Existing consumers and coupling cost

Four packages directly consume `er-gfx` at this baseline:

| package | target | current use |
|---|---|---|
| `er-effects-rs` | shipped `cdylib` | derives and serves the title, ProfileSelect, text-input, and System-Quit movies |
| `er-armament-icons` | standalone `cdylib` | derives badge movies; owns a separate parse/file-open hook |
| `er-invasion-warp-dll` | standalone `cdylib` | derives map/notice movies; owns a separate parse hook |
| `er-loading-portrait` | reusable library | parses/rasterizes the captured menu font in host-tested code |

Deepening `er-gfx` makes native hook configuration, game addresses, and hook lifecycle part of the interface seen by all four consumers (or adds a public feature matrix that every unified Cargo graph must coordinate). It also makes the existing host examples and integration tests share a package with process-hook code.

A sibling crate adds one explicit interface only for consumers that install native Scaleform hooks. Existing codec consumers keep the current API and dependency closure. Standalone DLLs can adopt shared `MemoryFile`/parse plumbing later without taking a dependency on the product DLL, while `er-loading-portrait` remains a codec/raster consumer only. This is less exported coupling than adding hook APIs to the already-shared codec.

## Ownership rule for R23/R24a+

- `er-gfx` owns GFX bytes: parsing, lossless writing, typed edits, fingerprints, pure movie transformations, and host raster helpers.
- `er-scaleform-hooks` owns game-process Scaleform plumbing: native `MemoryFile` layout validation and data/length/cursor replacement, hook installation/trampolines, resource/file-open/parse observation, and the descriptor-heap guard.
- Feature policy, arming order, and product orchestration remain in their feature/product owners. They cross the hook boundary through concern-specific inputs or callbacks, not a mirror of root globals.
- `er-scaleform-hooks` depends on `er-gfx`; `er-gfx` never depends on `er-scaleform-hooks` or another game-runtime crate.
- No shipped DLL topology changes here. The new crate is a library linked by its consumers, not another required ME3 native entry.

`crates/er-gfx/tests/architecture_boundary.rs` makes the dependency facts and the one-way boundary executable. Cargo continues to prove the realized workspace graph is acyclic when R23 adds the package.

## R23 boundary realization

R23 realizes the sibling as a normal library with direct edges to `er-gfx`, `er-game-base`, `er-telemetry`, and (on Windows) `er-hook`. It deliberately has no dependency on `er-effects-rs`, `er-title-flow`, or `er-loading-portrait`.

The named-child bind seam uses one post-bind `NamedChildBindEvent` callback installed through `ScaleformHooksHost`. The hook owner reports the native parent, output proxy, name pointer, and original return value; title/ProfileSelect feature code retains the policy applied to those facts. This avoids both the `er-title-flow -> er-loading-portrait -> er-gfx` cycle risk and a broad host structure that mirrors product globals. No descriptor, resource, or message callback is added speculatively; the R24 slice that first proves another host-owned concern must add its own narrow input.
