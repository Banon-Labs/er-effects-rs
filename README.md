# er-mods-rs

**Elden Ring mods, written in Rust.** Around twenty of them, each built as its own DLL and
loaded through [me3](https://github.com/garyttierney/me3) -- plus the host-side Rust libraries
they are built from and the reverse-engineering tooling used to find the addresses.

Targets **Elden Ring 1.16.2** on Linux (native Steam + Proton) and Windows. Cross-compiled to
`x86_64-pc-windows-msvc` from Linux with `cargo-xwin`. Compatible with vanilla and with Seamless
Co-op -- which is a compatibility target, never a bundled file.

> Formerly `er-effects-rs`. That name described one feature -- named SpEffect calls -- which now
> lives in its own crate (`er-net-effects`) and is the smallest thing here. Beads issue keys
> still carry the `er-effects-rs-` prefix on purpose: they are database identity, not branding.

## The mods

Every row is a separate `[[natives]]` entry. Load only what you want; nothing here depends on
anything else here unless the notes say so.

### Boot and saves

| DLL | What it does |
| --- | --- |
| **`er_quickload.dll`** | Straight from process start to an in-world character with no menu input, and a real progress bar over the dead early-boot gap. Also owns the missing-save picker, the customized System > Quit load rows, and the loading-screen portrait and stat panel. [Manual](crates/er-quickload/README.md) |
| `er_save_picker.dll` | The boot save picker on its own. Detects the product DLL and stays inert when it is loaded. |
| `er_quit_menu.dll` | The customized System > Quit tab as a standalone harness. |
| `er_loading_portrait.dll` / `er_loading_bar.dll` | Standalone shells for the portrait and the pre-native loading bar. **Never load either next to `er_quickload.dll`** -- two D3D12 Present compositors in one process. |
| `er_save_disable.dll` | Suppresses all save writes. Census/proof tool, not a play mod. |

### Inventory and equipment

| DLL | What it does |
| --- | --- |
| `er_armament_icons.dll` | Draws the Ash of War as a badge on every armament tile, so you can read a weapon's art without opening it. |
| `er_better_refills.dll` | Vanilla only flips persistent autorefill when you toggle an item by hand; this hooks the native toggle and calls the game's own refill. |
| `er_refill_all.dll` | Marks every refillable item refill / no-refill on one keypress, cycling. Runs from inside the storage box dialog's own update, so it cannot fire anywhere else. |
| `er_inventory_sort.dll` | Sets the equipment-menu sort defaults once per session. |

### Character and build

| DLL | What it does |
| --- | --- |
| `er_build_import.dll` | Rebuilds the character you are playing from an [er-build-planner](https://er-build-planner.pages.dev/) share link -- items, gear, spells, level, attributes, and appearance. Where you stand; no title return, no save write. Also a System > Quit row in `er_quickload.dll`. |
| `er_build_export.dll` | The inverse: encodes the live character into a self-contained planner link, copies it, opens it. Appearance travels under the planner's own `sliders` key, so its Cosmetics tab reads it. |
| `er_death_persist.dll` | Keeps the transformation body-buffs (Rock Heart, Priestess Heart, Lamenter's Mask) through death. |
| `er_net_effects.dll` | Keyboard-selected SpEffects applied to your own character. The original feature of this repo. [Manual](crates/er-net-effects/README.md) |
| `mushroom_man.dll` | Zeroes the model id on every head/body/arm/leg `EquipParamProtector` row, so armour is worn but never rendered. Half a mod on its own: it stays inert unless the mushroom model binders are beside it (`parts/`, `facegen/` in the same folder, that folder loaded as an ME3 `[[packages]]` entry -- what `scripts/install_mushroom_man.py` sets up), because without them the patch strips armour and puts nothing in its place. |

### Multiplayer

| DLL | What it does |
| --- | --- |
| `er_invasion_path.dll` | A walkable route to every other player in your session, drawn on the ground and following the terrain -- asking the engine's own Havok-AI navmesh, with a direction arrow when the navmesh cannot reach them. [Details](crates/er-invasion-path/README.md) |
| `er_invasion_warp.dll` | World-map warp targets at the real invasion spawn points, read out of the `CSAutoInvadePoint` table. |
| `er_player_name_filter.dll` | Filters remote player names, hooking `CS::SessionManagerPlayerEntryBase::Copy`. |
| `er_seamless_bugfixes.dll` | Crash guards for faults only Seamless Co-op's networking mode reaches. Every guard is a null-container intercept on a vanilla function. |
| `er_enemynpc_effects.dll` | Hotkey toggle that keeps every loaded enemy under a configured SpEffect (`effect_id`, default Charming Branch). |

### Diagnostics

Not for normal play. Several change behaviour merely by being present.

| DLL | What it does |
| --- | --- |
| `er_crash_logging.dll` | First-chance exceptions to `er-crash-log.txt`. |
| `er_telemetry.dll` | Read-only RAM oracles to JSON on a FrameBegin task. No hooks. |
| `er_build_watermark.dll` | Draws which mods are loaded in the top-right, and whether any is an older published release than main. |
| `er_diag_harness.dll` | Trace detours that used to ship unconditionally in the product DLL. |
| `er_input_harness.dll` | Self-drive input harness. **Default-on by presence** -- it writes the game's input memory every frame, so you are not the one driving. |
| `er_reload_trace.dll` | Reload / MoveMapStep trace. Not purely passive: its diagnostic drive writes `menuData+0x5d` after a stuck-load streak. |
| `amd_ags_x64.dll` | Not an me3 native -- a drop-in replacement for the game's AMD AGS import, loaded by the PE loader by name, for RenderDoc capture runs. |

**Some of these must not be loaded together.** The machine-readable list is
[`scripts/me3-dll-conflicts.toml`](scripts/me3-dll-conflicts.toml), enforced by
`scripts/check-me3-dll-conflicts.py`. The usual cause is two MinHook instances detouring one
prologue and corrupting each other's trampolines.

## Running them

Point an me3 profile at the DLLs you want:

<!-- md-test: parse-toml -->
```toml
profileVersion = "v1"
start_online = false

[[supports]]
game = "eldenring"

[[natives]]
path = '/path/to/er-mods-rs/target/x86_64-pc-windows-msvc/release/er_quickload.dll'

[[natives]]
path = '/path/to/er-mods-rs/target/x86_64-pc-windows-msvc/release/er_armament_icons.dll'
```

<!-- md-test: bash-run -->
```bash
me3 launch -g eldenring -p /path/to/your.me3
```

For Seamless Co-op, add a `[[natives]]` entry pointing at the Seamless DLL **in your own game
install** (`<ELDEN RING>/Game/SeamlessCoop/ersc.dll`). This repo never copies, stages, or
releases that file.

Do not launch through Steam's protected/EAC launcher. These are direct/offline me3 native-DLL
mods.

## Building

This repo expects to sit **next to a `fromsoftware-rs` checkout** -- the game-side crates use
`../fromsoftware-rs` path dependencies.

<!-- md-test: bash-n -->
```bash
# The quick-load product DLL:
cargo xwin build --release --target x86_64-pc-windows-msvc -p er-quickload

# Any other mod -- name it explicitly:
cargo xwin build --release --target x86_64-pc-windows-msvc -p er-invasion-path
```

`default-members` is pinned to `er-quickload`, so a bare `cargo xwin build` compiles **only**
that one and exits 0 in a fraction of a second having built nothing else. Name the package.

Check the output hash before staging or launching. A build that succeeded without recompiling
leaves the previous DLL in place, and a run against it produces evidence for code that is not
the code under test:

<!-- md-test: bash-n -->
```bash
sha256sum target/x86_64-pc-windows-msvc/release/er_quickload.dll
```

Full gate -- lossy-UTF8 lint, `cargo fmt --check`, clippy at upstream parity, and a
windows-target check:

<!-- md-test: bash-n -->
```bash
bash scripts/check.sh
```

Host-buildable crates need no game and no cross-compiler:

<!-- md-test: bash-n -->
```bash
cargo test -p er-soulsformats -p er-param-inspect -p er-gfx -p er-flver
```

## Libraries and tooling

The other half of the repo: host-side Rust that runs on Linux, with no game attached, and is
`cargo test`-able.

| Crate | What it is |
| --- | --- |
| `er-soulsformats` + `tools/er-param-inspect` | Read `regulation.bin` params through a generated .NET bridge against Smithbox's `Andre.Formats`/SoulsFormats. Inspect rows, validate effect lists. [Manual](tools/er-param-inspect/README.md) |
| `er-gfx` | Lossless codec for uncompressed Scaleform `.gfx` movies -- parse, edit tags, re-emit. Behind the runtime menu-GFX edits. |
| `er-flver` | Host-only FLVER reader with two views over one parse. |
| `er-tpf` | In-memory texture-payload builder for the game's raster formats. |
| `er-objectkit` | Traces a shader/material back to the objects that use it. |
| `er-shaderkit` + `tools/er-shaderlab`, `tools/er-shader-viewer` | Shader ingestion and viewing (naga/wgpu/bevy; host-only, never linked into a game DLL). |
| `er-save-loader`, `erpx-rs` | Save-container parsing; a container for raw RGBA8 portrait dumps. |
| `er-hook`, `er-game-base`, `er-telemetry-core`, `er-hotkey-config`, `er-safe-input` | The shared runtime substrate: the MinHook union, RVA tables, telemetry counters, user-nameable hotkeys that reload without a restart. |
| `*-core` crates | Every game-free half of a mod, split out so its logic is testable on Linux. |

Cheat Engine tables -- including the bundled CJK font override for Seamless/offline users -- live
in [`scripts/cheat-engine/`](scripts/cheat-engine/README.md).

## Repo layout

```text
crates/            mods (cdylib shells) and libraries
tools/             host-side CLIs
scripts/           build, staging, gate, probe and Ghidra helpers
docs/              RE notes, plans, format references
data/              effect lists and other embedded data
```

## Reverse engineering

Addresses come from a Ghidra runtime dump of **1.16.2** -- see `AGENTS.md` for the MCP daemon,
the deobfuscated-binary conventions, and the rule that matters most: the dump is authoritative
for *meaning*, and `eldenring-deobf.bin` is authoritative for *addresses*. `scripts/ghidra/`
holds the version-controlled query scripts.

Issue tracking and long-form findings live in [beads](https://github.com/steveyegge/beads)
(`.beads/`), not in markdown TODOs.

## License and scope

Single-player and co-op quality-of-life. Nothing here is built for, or tested against,
competitive online play against unwilling opponents; `er-net-effects` defaults network sync
**off** for that reason.
