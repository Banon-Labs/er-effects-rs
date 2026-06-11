# Souls Modding reference tree — depth 3

Continuation from [`soulsmodding-depth-2.md`](./soulsmodding-depth-2.md). This pass followed the most relevant depth-2 frontier instead of broad-crawling every linked project.

## Pre-expansion check

Before adding this layer, the existing depth-2 notes were checked for the expected frontier markers: `anim 63010`, the four SpEffect IDs found in depth 2, and the selected git projects (`EvenTorset/fxr`, `garyttierney/me3`, `EldenRingHKS`, `EldenRingNightreignHKS`, `HkbEditor`, `Smithbox`).

## Scope of this pass

Inspected next-layer resources:

- [SoulsMods ER SpEffect animation data](https://soulsmods.github.io/data/er/anims_sp.html)
- [soulsmods.github.io `data/er` source listing](https://github.com/soulsmods/soulsmods.github.io/tree/master/data/er)
- [vswarte/fxr-reloader](https://github.com/vswarte/fxr-reloader)
- [EvenTorset/fxr](https://github.com/EvenTorset/fxr)
- [garyttierney/me3](https://github.com/garyttierney/me3)
- [me3 docs](https://me3.readthedocs.io/en/latest/)
- [ividyon/EldenRingHKS](https://github.com/ividyon/EldenRingHKS)
- [El-Fonz0/EldenRingNightreignHKS](https://github.com/El-Fonz0/EldenRingNightreignHKS)
- [ndahn/HkbEditor](https://github.com/ndahn/HkbEditor)
- [HkbEditor docs](https://ndahn.github.io/HkbEditor/)
- [vawser/Smithbox](https://github.com/vawser/Smithbox)
- [Smithbox SoulsModding docs](https://www.soulsmodding.com/doku.php?id=smithbox)

## Runtime SpEffect animation data

Source: <https://soulsmods.github.io/data/er/anims_sp.html>

Observed source characteristics:

- Title: `Elden Ring SpEffect Animations`
- Approximate HTML size: 1,936,170 bytes
- Parsed SpEffect entry count: 10,932 entries
- GitHub source listing exposed related files:
  - [`anims_sp.html`](https://github.com/soulsmods/soulsmods.github.io/blob/master/data/er/anims_sp.html)
  - [`anims_atk.html`](https://github.com/soulsmods/soulsmods.github.io/blob/master/data/er/anims_atk.html)

Focused query: current repo watches player animation `63010`.

| SpEffect | Character/context | Matching `63010` frame | Nearby relevant animations in same entry | Notes |
| --- | --- | --- | --- | --- |
| `35` | `c0000 a00` | `0` | `61020:0-72`, `61050:0-190`, `63020:0`, `63021:0`, `63060:0`, `63061:0`, `63070:0`, `63090:0` | Broad player-entry effect with 34 parsed animation occurrences. |
| `290` | `c0000 a00` | `0` | `61020:0-72`, `63020:0`, `63021:0`, `63040:0`, `63050:0`, `63060:0`, `63061:0`, `63070:0`, `63090:0` | Smaller player-entry effect with 10 parsed animation occurrences. |
| `5008` | `c0000 a00` | `0-120` | `63021:0` | Most specific `63010` span discovered in this pass. |
| `9760` | `c0000 a00` | `0` | none in same parsed entry | Single-entry `63010` association. |

Repo-local interpretation:

- `5008` is the most interesting candidate if the project needs an effect whose animation association spans the duration of `63010` rather than only frame `0`.
- `35`, `290`, and `9760` still matter as frame-0 associations, but they look more like trigger/state markers than duration-like associations.
- The seeded runtime SpEffect IDs currently documented in this repo (`4330`, `20018100`, `20018101`) were not found in this animation index. Treat that as “not indexed here,” not as invalidity.

## Git project next-layer entry points

### FXR live-reload and file-editing branch

#### `vswarte/fxr-reloader`

Repo: <https://github.com/vswarte/fxr-reloader>

Observed top-level shape:

- `.cargo/`
- `.github/`
- `agent/`
- `cli/`
- `gui/`
- `protocol/`
- `Cargo.toml`
- `Cargo.lock`
- `README.md`

README findings:

- Purpose: live-swap FXR files without repacking through Yabber/WitchyBND or restarting the game.
- Usage flow:
  1. launch Elden Ring `v1.10.0` or Sekiro `v1.6.0`,
  2. launch the tool,
  3. select the target game process,
  4. edit FXR files,
  5. click `Reload FXR`,
  6. select one or more edited FXR files.
- Important caveat: only FXR definitions are patched; effects need to be recreated/reapplied/recast, or map-specific effects need relevant reload/unload behavior before changes become visible.
- Implementation summary from README: injects `fxr_reloader_agent.dll` into the chosen game; the agent reads game memory to find current FXR definitions and replaces them with supplied definitions.
- README dependencies: [`dll-syringe`](https://github.com/OpenByteDev/dll-syringe) and [`iced`](https://github.com/iced-rs/iced).

Repo-local relevance:

- This is the closest Rust-adjacent project for live visual-effect experimentation.
- It is version-specific in its README, so do not assume offsets or signatures apply to current Elden Ring/Nightreign without inspection.
- If this repo moves toward FXR patching instead of runtime SpEffect calls, inspect `agent/`, `protocol/`, and `cli/` first.

#### `EvenTorset/fxr`

Repo: <https://github.com/EvenTorset/fxr>

Observed top-level shape:

- `src/`
- `examples/`
- `build/`
- `images/`
- `package.json`
- `README.md`
- `typedoc.json`

README findings:

- TypeScript/JavaScript library for creating and editing FXR files for DS3, Sekiro, Elden Ring, AC6, and Nightreign.
- Works in browser and Node.js.
- Supports creating new effects from scratch and modifying existing effects through scaling, recoloring, game conversion, and related operations.
- Playground: <https://fxr-playground.pages.dev/>

Repo-local relevance:

- Best first code reference for FXR structure, schemas, and examples.
- Keep paired with this repo's package note: [`@cccode/fxr`](../references/cccode-fxr.md).

### Mod loader / instrumentation branch

#### `garyttierney/me3`

Repo: <https://github.com/garyttierney/me3>

Docs: <https://me3.readthedocs.io/en/latest/>

Observed top-level shape:

- `crates/`
- `docs/`
- `schemas/`
- `support/`
- `distribution/`
- `assets/`
- `Cargo.toml`
- `Cargo.lock`
- `README.md`
- `mkdocs.yml`
- installer/release files

README/docs findings:

- me3 is a framework for runtime modification/instrumentation of games and successor to Mod Engine 2.
- Supported games listed by README:
  - Dark Souls III
  - Sekiro: Shadows Die Twice
  - Elden Ring
  - Armored Core VI: Fires of Rubicon
  - Elden Ring Nightreign
- Docs index says me3 focuses on Elden Ring and other FromSoftware titles.
- Quick-start docs point to mod profile creation and configuration reference.

Repo-local relevance:

- Best next target if this project needs a more established loader/instrumentation route rather than a standalone direct DLL experiment.
- Inspect `crates/`, `schemas/`, and `docs/` next if launch/configuration or runtime integration becomes the immediate task.

### HKS / Havok / behavior branch

#### `ividyon/EldenRingHKS`

Repo: <https://github.com/ividyon/EldenRingHKS>

Observed top-level shape:

- `c0000.hks`
- `c8000.hks`
- `c9997.hks`
- `README.md`

README findings:

- HKS is described as an interface between player/AI inputs and animations/behavior for the relevant character model.
- `c0000.hks` is the interface between player inputs and the player character.
- `c8000.hks` is Torrent's HavokScript file.
- `c9997.hks` applies to all enemies.
- HKS files are decompiled using [`katalash/DSLuaDecompiler`](https://github.com/katalash/DSLuaDecompiler) and then manually cleaned up.

Repo-local relevance:

- Best Elden Ring-specific branch for understanding how player input and animation behavior route into character state.
- Inspect `c0000.hks` only when the Rust trigger logic needs semantic context for local-player animation IDs or input-driven state.

#### `El-Fonz0/EldenRingNightreignHKS`

Repo: <https://github.com/El-Fonz0/EldenRingNightreignHKS>

Observed top-level shape:

- `c0000.hks`
- `c9997.hks`
- `common_define.hks`
- `.gitignore`

Findings:

- GitHub API exposed no README during this pass.
- SoulsModding labels the repo as decompiled/cleaned HKS files for Nightreign.

Repo-local relevance:

- Best current Nightreign-specific HKS source discovered so far.
- Because there is no README, any future use should inspect file headers/functions directly and avoid assuming Elden Ring `c0000.hks` semantics carry over unchanged.

#### `ndahn/HkbEditor`

Repo: <https://github.com/ndahn/HkbEditor>

Docs: <https://ndahn.github.io/HkbEditor/>

Observed top-level shape:

- `docs/`
- `hkb_editor/`
- `event_listener/`
- `templates/`
- `devel/`
- `main.py`
- `requirements.txt`
- `attributes.yaml`
- `mkdocs.yml`

README/docs findings:

- Edits Havok behavior graphs used by FromSoftware games including Nightreign, Elden Ring, and Sekiro.
- Behaviors are described as state machines controlling which animations play, which animations layer, and how transitions happen.
- States can transition automatically or through events triggered from HKS, the script files handling player input and game state.

Repo-local relevance:

- Pair this with `EldenRingHKS` / `EldenRingNightreignHKS` if the work shifts from simple animation ID watching to behavior graph/state-machine interpretation.

### General suite branch

#### `vawser/Smithbox`

Repo: <https://github.com/vawser/Smithbox>

Docs: <https://www.soulsmodding.com/doku.php?id=smithbox>

Observed top-level shape:

- `src/`
- `Documentation/`
- `.github/`
- `Smithbox.sln`
- `Directory.Build.props`
- `README.md`

README findings:

- Supports Elden Ring, Elden Ring: Nightreign, AC6, Sekiro, Dark Souls, Bloodborne, and Demon's Souls.
- Key features include map, model, param, text, graphics-param, material, texture, and file-browser editors.
- README says the game no longer needs to be unpacked for any editor.

Repo-local relevance:

- Best GUI/data-editor reference for params and file browsing.
- If this repo needs concrete parameter names/semantics, Smithbox and its data definitions may be more useful than broad wiki pages.

## Current frontier after depth 3

The broad crawl has now hit concrete next-layer resources. Further progress should branch by implementation need rather than continuing the whole tree:

1. **Runtime trigger branch:** parse and locally cache focused subsets of `anims_sp.html` around `c0000`, `anim 63010`, `SpEffect 35`, `290`, `5008`, and `9760`; compare those to this repo's configured trigger/effect list.
2. **FXR live-edit branch:** inspect `vswarte/fxr-reloader` `agent/`, `protocol/`, and `cli/` to understand memory patching and version assumptions.
3. **FXR structure branch:** inspect `EvenTorset/fxr` `src/` and `examples/` for schemas and transformation APIs.
4. **Nightreign/runtime loader branch:** inspect me3 `crates/`, `schemas/`, and docs if a loader/instrumentation path is needed.
5. **Animation semantics branch:** inspect Elden Ring/Nightreign `c0000.hks` files and HkbEditor docs/code if animation IDs need behavioral interpretation.
6. **Param tooling branch:** inspect Smithbox definitions only if this repo needs concrete param-field semantics beyond the SoulsModding wiki.

## Last checked

- Date: 2026-06-10
- Sources checked: SoulsMods ER animation data/source listing, selected GitHub repo metadata/top-level listings/READMEs, me3 docs index, HkbEditor docs index, and Smithbox SoulsModding docs page.
