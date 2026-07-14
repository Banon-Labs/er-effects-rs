# er-effects-rs

`er-effects-rs` is an Elden Ring runtime DLL written in Rust. Its current product
focus is **quick load**: get from process start to an in-world character without
manual menu input, while replacing the dead early boot gap with native progress
feedback and optional personalized visuals.

The long-term shape is broader than quick load. This repo is also the integration
point for several runtime features that can be combined when needed: local-only
SpEffect triggers, save-source redirection, boot/loading-screen presentation,
profile portrait rendering, loading-screen character portrait experiments, and
Cheat Engine helper tables.

## Current product focus: quick load

The supported quick-load path is a single `er_effects_rs.dll` loaded through
[me3](https://github.com/garyttierney/me3) as a native DLL. LazyLoader/dinput8
chainloading has been removed from the release path.

Quick load currently aims to:

- skip the current-version splash/intro path;
- advance the press-any-button/title-menu gates without simulated host input;
- force offline-safe title flow where needed;
- select/load the requested save slot;
- block user/foreign input during the automated boot window;
- release input once the world is reached;
- provide RAM/telemetry oracles for proof instead of relying on screenshots;
- draw boot progress before the game's native loading screen appears.

Validated product proof in this branch includes quick-load/runtime smokes where
the boot view drew hundreds of frames, handed off to the native loading/portrait
path, exited on `world_stable`, produced `oracle_msgbox_total_builds=0`, and kept
`simulated_button_presses_total=0`. The local Steam-screenshot background path is
optional and falls back cleanly when no real local Elden Ring screenshot exists.

## Feature map

| Area | Status | Notes |
| --- | --- | --- |
| Quick load / zero-input autoload | Primary focus | me3-native DLL, native title/menu gates, no host button simulation. |
| Boot progress view | Product path | Present-hook D3D12 overlay draws a small milestone loading bar from RAM semaphores before native loading UI exists. |
| Steam screenshot boot background | Optional branch feature | DLL-only local Steam screenshot discovery/decoding; optional predecoded override for dev/power users. No launch-time network. |
| Native loading-screen portrait | Experimental/product-adjacent | Character portrait rendering/composite exists; continuous per-frame refresh still has known GX subcontext contention. |
| SpEffect trigger calls | Existing base feature | Named SpEffect calls from `data/effects.json`, default local-only networking semantics. |
| Save-source redirect/default-save fallback | Product support | Can load explicit `.sl2`/`.co2` sources or fall back to active Steam user's default save path. |
| System/Quit save-profile UX | Experimental helper | Runtime support for selecting replacement save profiles during menu flows. |
| Cheat Engine helper tables | Utility | Tables live under `scripts/cheat-engine/`, including CJK font override helpers. |
| Param tooling | Host tooling | `er-param-inspect` validates/inspects regulation params through Smithbox/SoulsFormats. |

## Quick start: stage the quick-load release

Build and stage the release payload:

<!-- md-test: bash-n -->
```bash
scripts/stage-autoload-release.sh --output target/autoload-release
```

The staged folder contains:

- `er_effects_rs.dll`
- `er-effects.me3`
- `er-effects-autoload.txt.example`
- `er-effects-native-continue.txt.example`
- `er-effects-pab-advance.txt.example`
- `er-effects-splash-skip.txt.example`
- `SHA256SUMS.txt`

Install/use:

1. Keep the staged folder together. `er-effects.me3` references the DLL relative
   to itself, so the folder is relocatable.
2. Copy the desired `er-effects-*.txt.example` files next to `eldenring.exe` and
   remove `.example`.
3. Edit `er-effects-autoload.txt` if you need a slot other than `slot=0`.
4. Launch with me3:

<!-- md-test: bash-run -->
```bash
me3 launch -g eldenring -p /path/to/er-effects.me3
```

Do not launch Elden Ring through the protected/EAC launcher for agent/runtime
work. The release profile is designed for the direct/offline me3 native-DLL path.

## User-friendly helper package without DLLs or saves

To create a redistributable helper package that contains only docs/templates and a
launcher wrapper -- **not** `er_effects_rs.dll`, `.sl2`/`.co2` saves, or other DLLs
-- run:

<!-- md-test: bash-n -->
```bash
scripts/build-user-release-package.py --clean
```

The generated zip under `target/deliverables/` includes `run-er-effects-release.sh`,
`quicksave.me3.template`, `er-effects.toml.example`, and audit manifests. The
helper requires the user to pass their locally-built DLL path at launch time and
never copies a save file into the package.

## Optional: Steam screenshot boot background

The DLL can draw a personal Steam screenshot behind the pre-native boot loading
bar. The production path is DLL-only:

- the DLL enumerates local Steam `userdata/*/760/remote/1245620/screenshots`
  directories and chooses the newest `.jpg`/`.png` it can decode;
- no Steam account ID is hard-coded;
- the DLL never scrapes Steam Community;
- the DLL never downloads during launch;
- missing/bad screenshots fall back to the normal black boot progress view.

The boot view aspect-covers the screenshot, dims it, and draws a soft faded
shadow behind the progress bar so the bar remains readable without a hard panel.

Users can override the automatic local-Steam screenshot selection in the
game-directory `er-effects.toml`:

<!-- md-test: parse-toml -->
```toml
boot_background_image = "C:/path/to/background.jpg"
# Linux absolute paths are accepted under Proton/Wine and are translated to Z:\\...
# boot_background_image = "/home/you/Pictures/my-load-screen.png"
# Relative paths resolve next to er-effects.toml in the game directory:
# boot_background_image = "backgrounds/my-load-screen.png"

# Default: true. Set false to use the custom image only during the pre-native
# boot gap, then let the game's normal MENU_Load_* artwork own the native
# loading screen.
persist_boot_background_to_loading_screen = true
```

Accepted image aliases are `background_image`, `boot.background_image`,
`boot.background`, and `background.image`. The image must be a local `.jpg`,
`.jpeg`, or `.png`; it is decoded in-process by the DLL via Windows Imaging
Component. By default, the selected boot background also replaces the game's
native `MENU_Load_*` GFX background during the loading screen; opt out with
`persist_boot_background_to_loading_screen = false`.

A lower-level predecoded override remains available for development/power users:

```text
<game-dir>/er-effects-boot-background.rgba
```

That file uses a tiny `ERBGRA01` header plus width/height and RGBA8 pixels.
`scripts/cache-steam-screenshot-background.py` can write it, but that script is
**developer-only tooling** and is not part of the shipped production pipeline.

## Runtime configuration files

Most quick-load toggles are simple `.txt` files placed next to `eldenring.exe`.
`er-effects.toml` is different: it is loaded from the game directory, next to
`eldenring.exe`. Environment variables with matching names are also used by
probes and smoke scripts.

Common quick-load files/config:

| File | Purpose |
| --- | --- |
| `er-effects-autoload.txt` | Selects the requested quick-load slot, e.g. `slot=0`. |
| `er-effects-native-continue.txt` | Enables the supported native Continue path. |
| `er-effects-pab-advance.txt` | Enables zero-input press-any-button/menu-open advance. |
| `er-effects-splash-skip.txt` | Enables built-in splash skip when not already implied by quick load. |
| `er-effects.toml` | Game-directory config file; can provide `save_file`, `boot_background_image`, and `persist_boot_background_to_loading_screen`. |
| `er-effects-boot-background.rgba` | Game-directory developer/power-user predecoded screenshot override; not required for production local Steam screenshot discovery. |

Important experimental/probe files exist too (`er-effects-force-profile-render.txt`,
`er-effects-portrait-lookat.txt`, `er-effects-portrait-render-drive.txt`, etc.).
Those are for controlled runtime probes and are not the minimal quick-load
release surface.

## Save-source behavior

The product path can use either an explicit save source or the active user's
normal save:

1. Explicit source via `ER_EFFECTS_SAVE_FILE` or `er-effects.toml`:

<!-- md-test: parse-toml -->
```toml
save_file = "/path/to/ER0000.sl2"
```

2. Default fallback:

```text
%APPDATA%/EldenRing/<SteamID64>/ER0000.sl2
```

The redirect layer hooks save-file opens at the Win32 file API boundary, so the
game's native save-discovery shape remains intact while reads/writes target the
configured source/staged tree. `.sl2` and Seamless `.co2` paths are compatibility
targets; this repo does **not** bundle Seamless Co-op's `ersc.dll`.

## Build and validation

This repo expects to live next to a `fromsoftware-rs` checkout because the root
crate uses path dependencies from `../fromsoftware-rs`.

Build the Windows DLL from Linux:

<!-- md-test: bash-n -->
```bash
cargo xwin build --release --target x86_64-pc-windows-msvc
```

Output:

```text
target/x86_64-pc-windows-msvc/release/er_effects_rs.dll
```

Fast checks:

<!-- md-test: bash-n -->
```bash
cargo fmt --all -- --check
cargo xwin check --target x86_64-pc-windows-msvc
```

Full repo gate:

<!-- md-test: bash-n -->
```bash
bash scripts/check.sh
```

Host-buildable tooling crates can be checked without the game DLL:

<!-- md-test: bash-n -->
```bash
cargo test -p er-soulsformats -p er-param-inspect
cargo check -p er-soulsformats -p er-param-inspect
```

## Runtime smoke tests

Runtime-affecting changes need a live smoke. The common quick-load/portrait smoke
entrypoint is:

<!-- md-test: bash-n -->
```bash
bash scripts/run-postcontinue-lookat-smoke.sh
```

The smoke expects Steam to be running, stages an isolated save/artifact directory,
launches the approved direct/offline path, and tears down under the repository's
runtime cap. Important proof comes from structured telemetry such as:

- `reason=world_stable`
- `oracle_msgbox_total_builds=0`
- `simulated_button_presses_total=0`
- `oracle_boot_view_draw_hits`
- `oracle_overlay_draw_hits`
- `oracle_char_name` / character-level fields

Screenshots may be captured for human review, but screenshots are diagnostic
artifacts, not the run-stopping oracle.

## SpEffect trigger system

The original feature remains: named SpEffect calls are embedded from
`data/effects.json` and can be applied by runtime trigger logic. They start
inactive by default.

In-game controls:

- Left/Right: switch the active effect catalog.
- Up/Down: step through the selected catalog's validated IDs and apply the selected effect.
- Alt+': toggle the currently selected effect off/on.

Persisted selector files next to `eldenring.exe`:

- `.effect-catalog-setting.txt`: selected catalog key.
- `.effect-setting.txt`: selected SpEffect ID. Editing this file while the game is running applies the matching in-catalog effect ID live and moves the catalog cursor to the first catalog containing that ID.

Built-in user-style catalogs:

`data/effect-catalogs/*.json` are provider catalogs in the same shape intended for user catalogs: each file is a plain JSON array of SpEffect IDs, with the file name acting as the catalog identity, for example `hides-from-npcs.json`. The DLL validates every ID against the embedded master catalog before exposing it to Up/Down cycling. User-provided game-directory catalogs can be placed in `effect-catalogs/*.json` next to `eldenring.exe` and use the same plain ID-list format.

The original 594-entry visual/audio triage list now lives as `data/effect-catalogs/nonmechanical-visual-sfx.json`; supporting audit artifacts are under `target/effect-meaningfulness-*.csv` when regenerated locally.

Master catalog:

`data/effect-master-catalog.json` is the rich authoritative SpEffect metadata map. It is keyed by `SpEffectParam` ID and records names, VFX IDs, derived tags, and meaningful non-default fields such as AI perception, HP/FP/stamina, movement/timing, damage, defense, and lifetime fields. Selector/user catalogs should reference this file by ID instead of copying field metadata; future user catalogs should be named JSON files in a shared catalog folder and contain only ID lists plus minimal catalog identity.

Regenerate the master catalog from a local regulation file:

<!-- md-test: bash-n -->
```bash
scripts/generate-effect-master-catalog.py --regulation "$REGULATION_BIN"
```

Validate the list against a regulation file:

<!-- md-test: bash-n -->
```bash
cargo run -p er-param-inspect -- validate "$REGULATION_BIN"
```

Inspect rows:

<!-- md-test: bash-n -->
```bash
cargo run -p er-param-inspect -- rows "$REGULATION_BIN" SpEffectParam 4330 20018100 20018101
```

## Network sync semantics

Each SpEffect call takes a "don't sync" flag. The overlay/control surface exposes
that as `Sync effect calls over the network`:

- **Off (default):** effects are applied with `dont_sync = true`; local-only and
  safer for offline/local testing.
- **On:** effects are applied with `dont_sync = false`, matching the Cheat Engine
  `addNetworked(..., id, 0)` pattern; peers may observe the application.

Leave sync off unless you specifically need peer-visible effect calls in a
controlled environment. Non-standard online behavior can be detectable and may
carry ban risk.

## Param tooling: Smithbox bridge

`er-soulsformats` and `er-param-inspect` read `regulation.bin` params by building
and running a small .NET bridge against Smithbox's `Andre.Formats` /
SoulsFormats libraries.

Supported Smithbox layouts:

- source checkout containing `src/Andre/Andre.Formats/Andre.Formats.csproj`;
- binary release/install containing `Andre.Formats.dll` or
  `Andre.SoulsFormats.dll`.

Discovery order uses `SMITHBOX_SOURCE_DIR` first, then common sibling/local
paths. The generated bridge lives under `target/soulsformats-bridge/`.

## Cheat Engine tables

Cheat Engine tables and helpers live in `scripts/cheat-engine/`.

Current table:

- `scripts/cheat-engine/bundled_cjk_font_override.CT` -- redirects the game's
  Scaleform menu font registration to bundled Simplified/Traditional Chinese
  font assets for Seamless/offline users.

## Architecture notes

High-level runtime pieces:

- `crates/er-effects-rs/src/lib.rs` / `crates/er-effects-rs/src/lib_parts/` -- DLL entry, bootstrapping, hooks, and runtime
  task registration.
- `crates/er-effects-rs/src/experiments/gpu_readback/boot_progress.rs` -- D3D12 boot progress view
  and optional cached screenshot background.
- `crates/er-effects-rs/src/experiments/present_overlay.rs` -- swapchain Present hook and backbuffer
  overlay path.
- `crates/er-effects-rs/src/experiments/save_redirect/` -- save path/source redirection and active
  SteamID/default-save support.
- `crates/er-effects-rs/src/experiments/own_stepper/` and `crates/er-effects-rs/src/experiments/own_load/` -- native
  quick-load/own-load research and product mechanisms.
- `crates/erpx-rs` -- debug portrait dump container and host PNG decoder.
- `crates/er-gfx`, `crates/er-tpf`, `crates/er-save-loader` -- host/DLL support
  crates for UI assets, texture payloads, and save data.

The current direction is to keep the runtime product as one composable DLL:
quick-load first, but able to export or combine the other runtime surfaces when a
profile or probe needs them.
