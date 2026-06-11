# Souls Modding reference tree — depth 2

Continuation from [`soulsmodding.md`](./soulsmodding.md). This pass followed the highest-value Elden Ring / Nightreign branches discovered in depth 1 and stopped at the next resource layer.

## Scope of this pass

Inspected pages:

- [SpEffect Animations](http://soulsmodding.com/doku.php?id=er-refmat:speffect-animations)
- [FXR Notes](http://soulsmodding.com/doku.php?id=er-refmat:particle-notes)
- [Particles (ER)](http://soulsmodding.com/doku.php?id=er-refmat:particle-list)
- [FXR files for SFX Explained](http://soulsmodding.com/doku.php?id=tutorial:fxr-files-for-sfx-explained)
- [Apply an sfx permanently to a weapon](http://soulsmodding.com/doku.php?id=tutorial:apply-an-sfx-permanently-to-a-weapon)
- [FXR format](http://soulsmodding.com/doku.php?id=format:fxr)

Also did a surface metadata check for selected git projects discovered in the previous layer. This was intentionally not a full code audit.

## New depth-2 resource nodes

### SpEffect animation data

- SoulsModding page: [SpEffect Animations](http://soulsmodding.com/doku.php?id=er-refmat:speffect-animations)
- Data page linked from it: <https://soulsmods.github.io/data/er/anims_sp.html>

Findings:

- The SoulsModding page is only a pointer to the GitHub Pages data table.
- The data page is large: roughly 1.9 MB HTML and about 10,933 `SpEffect` entries by simple text count.
- It maps SpEffect IDs to character/animation/frame occurrences.
- Repo-local relevance: this project currently watches local-player animation `63010`.

`anim 63010` appears in these entries:

- `SpEffect 35 (c0000 a00)` — includes `anim 63010 frame 0` among many player animations.
- `SpEffect 290 (c0000 a00)` — includes `anim 63010 frame 0`.
- `SpEffect 5008 (c0000 a00)` — includes `anim 63010 frame 0-120`.
- `SpEffect 9760 (c0000 a00)` — includes `anim 63010 frame 0`.

The seeded project SpEffect IDs `4330`, `20018100`, and `20018101` were not found in this animation table. That does not prove they are invalid; it only means they did not appear in this animation-index resource.

### Elden Ring FXR notes spreadsheet

- SoulsModding page: [FXR Notes](http://soulsmodding.com/doku.php?id=er-refmat:particle-notes)
- Spreadsheet: <https://docs.google.com/spreadsheets/d/12hKQg5kBvOJ_M0Udoz5GqS_2RX-d8YtaBapwpSJ2Csg/edit?gid=1424830463#gid=1424830463>

Findings:

- The wiki page is only a pointer to the Google Sheet.
- The public CSV export for the linked gid returned an introductory sheet, not the detailed named-ranges/tabs.
- Visible intro text says the spreadsheet contains explanations of FXR components important to Elden Ring SFX modding, is a work in progress, and recommends using Google Sheets named ranges for action IDs.
- Contributors visible in the intro export included CCCode, Challenger Andy, Rayan, The12thAvenger, ChainFailure, and others.

### Elden Ring FXR / particle ID spreadsheet

- SoulsModding page: [Particles (ER)](http://soulsmodding.com/doku.php?id=er-refmat:particle-list)
- Spreadsheet: <https://docs.google.com/spreadsheets/d/1gmUiSpJtxFFl0g04MWMIIs37W13Yjp-WUxtbyv99JIQ/edit?gid=31255113#gid=31255113>

Findings:

- The wiki page is only a pointer to the Google Sheet.
- The public CSV export for the linked gid returned an introductory/landing sheet.
- Visible intro text notes that `FXR` is the updated format of pre-Dark-Souls-3 `FFX`.
- The sheet is presented as community-provided data hosted by Rayan / the Souls modding community.

### FXR files for SFX tutorial

- Page: [FXR files for SFX Explained](http://soulsmodding.com/doku.php?id=tutorial:fxr-files-for-sfx-explained)

Useful notes:

- SFX are special visual effects using models and textures to simulate glow/fire/flame/magic-style effects.
- Elden Ring SFX files live under the game's `sfx` folder.
- ER/AC6 common effects: `sfxbnd_commoneffects.ffxbnd.dcx`.
- Map-specific example: `sfxbnd_m11.ffxbnd.dcx`.
- Character-specific example: `sfxbnd_c3750.ffxbnd.dcx`.
- After unpacking an ER/AC6 `.ffxbnd`, important folders include:
  - `texture` — `.tpf` texture files
  - `model` — `.flver` model files
  - `effect` — `.fxr` files
  - `animation` — `.hkx` animation files
  - `resource` — `.ffxlist` resource-list files for matching FXR IDs
- `.ffxlist` files need to include `.tif` and `.sib` names that actually refer to `.tpf` and `.flver` resources; otherwise resources are not loaded when the SFX is called.
- The tutorial recommends FXR Playground for creating/modifying FXR files and WitchyBND for XML conversion/repacking.
- References linked from the tutorial:
  - [Dark Souls 3 FXR Notes](https://docs.google.com/spreadsheets/d/1awj88NjfLzRZ4PZnWe0r5OFgpNqWrKNVMb2OPXbBmuQ/edit#gid=1323259003)
  - [Elden Ring FXR Notes](https://docs.google.com/spreadsheets/d/12hKQg5kBvOJ_M0Udoz5GqS_2RX-d8YtaBapwpSJ2Csg/edit#gid=1424830463)
  - [FXR IDs](https://docs.google.com/spreadsheets/d/1gmUiSpJtxFFl0g04MWMIIs37W13Yjp-WUxtbyv99JIQ/edit#gid=31255113)

### Permanent weapon SFX tutorial

- Page: [Apply an sfx permanently to a weapon](http://soulsmodding.com/doku.php?id=tutorial:apply-an-sfx-permanently-to-a-weapon)
- Author listed by the page: JeNoVaViRuS

Useful notes:

- Point attachment path: set `residentSfxId` and `residentSfx_DmyId`; look up DmyId attach points in DSAnimStudio.
- Limitation: this attaches to one dummy point and does not cover a whole blade.
- Whole-blade path uses two SpEffects:
  1. first SpEffect is always applied to the weapon,
  2. second SpEffect contains the SFX and is triggered by the first.
- The page says this is needed because after loading the game, everything in a SpEffect is applied except the SFX unless the SFX-bearing second SpEffect is triggered.
- Example fields/values shown by the tutorial include:
  - duplicate `[Incantation] Black Flame Blade` `SpEffectParam` into IDs `1626002` and `1626003`,
  - duplicate `SpEffectVfxParam` to `1626003`,
  - put `1626002` in `EquipParamWeapon.residentSpEffectId`,
  - set the first SpEffect's `vfxId` to `-1`, `wepParamChange` to `0`, high `effectEndurance`, `motionInterval` to `0`, and `cycleOccurrenceSpEffectId` to `1626003`,
  - set the second SpEffect's `vfxId` to its matching `SpEffectVfxParam`,
  - use `enchantStartDmyId_0 = 10300` for right-hand blade and `enchantStartDmyId_1 = 20300` for left-hand blade.

Repo-local relevance: this is a static-param approach to SFX-on-weapon behavior. It is separate from this Rust project's runtime SpEffect calls, but it is directly relevant to understanding how SpEffects trigger SFX/VFX and dummy-point attachments.

### FXR format page

- Page: [FXR format](http://soulsmodding.com/doku.php?id=format:fxr)

Useful notes:

- FXR are SFX visual-effect files.
- The page warns not to confuse FXR with VFX; VFX refers to `SpEffectVfxParam` while FXR is the file format.
- Dark Souls 1 and 2 use the older `FFX` format instead.
- Linked references:
  - [FFX](http://soulsmodding.com/doku.php?id=format:ffx)
  - [Dark Souls 3 FXR IDs](https://docs.google.com/spreadsheets/d/1gmUiSpJtxFFl0g04MWMIIs37W13Yjp-WUxtbyv99JIQ/edit?usp=sharing)
  - [Dark Souls 3 FXR Notes](https://docs.google.com/spreadsheets/d/1awj88NjfLzRZ4PZnWe0r5OFgpNqWrKNVMb2OPXbBmuQ/edit#gid=1424830463)
  - [Elden Ring FXR IDs](https://docs.google.com/spreadsheets/d/1gmUiSpJtxFFl0g04MWMIIs37W13Yjp-WUxtbyv99JIQ/edit#gid=0)
  - [Elden Ring FXR Notes](https://docs.google.com/spreadsheets/d/12hKQg5kBvOJ_M0Udoz5GqS_2RX-d8YtaBapwpSJ2Csg/edit?usp=sharing)

## Surface metadata for selected git projects

This section records only repository-level metadata and README headings/previews. It is not a code audit.

### Closest FXR/SFX candidates

- [EvenTorset/fxr](https://github.com/EvenTorset/fxr) — TypeScript, Unlicense, updated 2026-06-10. README: JavaScript library for creating/editing FXR particle effects/lights for DS3, Sekiro, Elden Ring, AC6, and Nightreign. Exposes editing/creating workflows and `fxrjson`.
- [vswarte/fxr-reloader](https://github.com/vswarte/fxr-reloader) — Rust, MIT, updated 2025-04-25. README describes live swapping FXR files without repacking/restarting; explicitly mentions Elden Ring v1.10.0 and Sekiro v1.6.0.
- [lugia19/AC6-FXR-Hunter](https://github.com/lugia19/AC6-FXR-Hunter) — JavaScript, updated 2024-05-30. README says it narrows FXR IDs through repeated testing and is based on CCCode's FXR library; SoulsModding notes it should work with ER after path adjustments.

### Runtime/loading/instrumentation candidates

- [garyttierney/me3](https://github.com/garyttierney/me3) — Rust, Apache-2.0, updated 2026-06-05. README describes a framework for modding and instrumenting games. Tools page lists me3 for DS3/Sekiro/ER/NR/AC6.
- [soulsmods/ModEngine2](https://github.com/soulsmods/ModEngine2) — C++, MIT, updated 2026-06-10. README says development is discontinued and future work is in me3; still relevant for older ER mod-loading context.

### HKS/Havok/animation candidates

- [ividyon/EldenRingHKS](https://github.com/ividyon/EldenRingHKS) — updated 2026-03-29. README says HKS serves as an interface between player/AI inputs and animations/behavior; includes `c0000.hks` for player input/player character.
- [El-Fonz0/EldenRingNightreignHKS](https://github.com/El-Fonz0/EldenRingNightreignHKS) — updated 2026-04-20. No README was available via GitHub API during this pass, but SoulsModding labels it as decompiled/cleaned Nightreign HKS files.
- [ndahn/HkbEditor](https://github.com/ndahn/HkbEditor) — Python, updated 2026-05-27. README says it edits Havok behavior graphs for FromSoftware games including Nightreign, Elden Ring, and Sekiro. Docs linked by README: <https://ndahn.github.io/HkbEditor/>.
- [Meowmaritus/DSAnimStudio](https://github.com/Meowmaritus/DSAnimStudio) — C#, GPL-3.0, updated 2026-06-08. README headings include user instructions for packed/unpacked games and game-support breakdown. Tool is relevant for TAE animation events and dummy-point lookup.

### General tooling candidates

- [vawser/Smithbox](https://github.com/vawser/Smithbox) — C#, MIT, updated 2026-06-10. README says it supports Elden Ring, Nightreign, AC6, Sekiro, Dark Souls, Bloodborne, and Demon's Souls; key features include map/model/param/text tooling.
- [soulsmods/SoulsFormatsNEXT](https://github.com/soulsmods/SoulsFormatsNEXT) — C#, GPL-3.0, updated 2026-05-26. Community continuation of SoulsFormats for FromSoftware file formats.
- [JKAnderson/SoulsFormats](https://github.com/JKAnderson/SoulsFormats) — C#, GPL-3.0, updated 2026-05-28. Older/core .NET FromSoftware format library.

### Dead or uncertain project link

- [FWang1221/NPCParam-SpEffect-Reorganizer](https://github.com/FWang1221/NPCParam-SpEffect-Reorganizer) returned `404 Not Found` via GitHub API during this pass. Keep the SoulsModding link in the tree as a historical pointer, but do not depend on it until a replacement/fork is found.

## Recommended next layer, only when needed

For this repo, the highest-value next expansion is no longer the broad SoulsModding tree. It is one of these focused branches:

1. Query `https://soulsmods.github.io/data/er/anims_sp.html` around animation `63010`, SpEffect IDs, and player `c0000` entries when changing runtime trigger behavior.
2. Inspect `EvenTorset/fxr`, `vswarte/fxr-reloader`, and the FXR spreadsheets only if the project needs to move from runtime SpEffect calls into file-level FXR/SFX editing or live FXR reload workflows.
3. Inspect `EldenRingHKS`, `EldenRingNightreignHKS`, and `HkbEditor` if the project needs to understand player input/animation/Havok behavior around Elden Ring or Nightreign.
4. Inspect `me3` if this project needs a mod-loading or instrumentation path rather than a direct DLL experiment.

## Last checked

- Date: 2026-06-10
- Sources checked: SoulsModding depth-2 pages, SoulsMods GitHub Pages SpEffect animation data, public Google Sheets CSV export front pages, and GitHub repository metadata/README surfaces for selected project candidates.
