# Souls Modding reference tree

Starting point: <http://soulsmodding.com/>

Purpose: grow a shallow, evidence-backed reference tree for Elden Ring / Elden Ring: Nightreign modding resources, especially resources that may help this Rust runtime-effect experiment.

## Crawl policy

- Crawl one layer at a time.
- Stop as soon as the current layer identifies useful resources; do not recurse through every wiki page or every GitHub project until there is a concrete need.
- Current depth reached: root + first wiki layer + immediate project links + focused depth-2 expansion in [Souls Modding reference tree — depth 2](./soulsmodding-depth-2.md).
- Relevance filter: Elden Ring, Elden Ring: Nightreign, SpEffect, FXR/SFX/particles, animation/TAE/Havok, HKS/Lua/ESD/EMEVD, regulation/params, mod loading, and FromSoftware file-format tooling.

## Depth 0: root

- [Souls Modding Wiki](http://soulsmodding.com/) — root wiki page.

Relevant first-layer links from the root:

- [Formats](http://soulsmodding.com/doku.php?id=format:main)
- [Topics](http://soulsmodding.com/doku.php?id=topic:main)
- [Tutorials](http://soulsmodding.com/doku.php?id=tutorial:main)
- [Tools](http://soulsmodding.com/doku.php?id=tool:main)
- [Elden Ring reference material](http://soulsmodding.com/doku.php?id=er-refmat:main)
- [Elden Ring: Nightreign reference material](http://soulsmodding.com/doku.php?id=ern-refmat:main)

Direct project links visible from the root:

- [Smithbox releases](https://github.com/vawser/Smithbox/releases)
- [WitchyBND releases](https://github.com/ividyon/WitchyBND/releases)
- [Soulstruct releases](https://github.com/Grimrukh/soulstruct/releases)
- [DarkScript3 releases](https://github.com/AinTunez/DarkScript3/releases)
- [ESDLang releases](https://github.com/thefifthmatt/ESDLang/releases)
- [ESDStudio releases](https://github.com/GompDS/ESDStudio/releases)
- [FLVER Editor releases](https://github.com/asasasasasbc/FLVER_Editor/releases)
- [DSAnimStudio releases](https://github.com/Meowmaritus/DSAnimStudio/releases)
- [DSMapStudio](https://github.com/soulsmods/DSMapStudio)
- [Yapped Rune Bear](https://github.com/vawser/Yapped-Rune-Bear)

## Depth 1: Elden Ring reference material

Source: [Elden Ring reference material](http://soulsmodding.com/doku.php?id=er-refmat:main)

High-value pages for this repo:

- [Particles (ER)](http://soulsmodding.com/doku.php?id=er-refmat:particle-list)
- [FXR Notes](http://soulsmodding.com/doku.php?id=er-refmat:particle-notes)
- [SpEffect Animations](http://soulsmodding.com/doku.php?id=er-refmat:speffect-animations)
- [TAE Animation List](http://soulsmodding.com/doku.php?id=er-refmat:tae-animation-list)
- [ER AI Functions](http://soulsmodding.com/doku.php?id=er-refmat:readable-er-lua)
- [Characters (ER)](http://soulsmodding.com/doku.php?id=er-refmat:character-list)
- [Event Flags (ER)](http://soulsmodding.com/doku.php?id=er-refmat:event-flag-list)
- [Map Names (ER)](http://soulsmodding.com/doku.php?id=er-refmat:map-name-list)
- [Map Overview](http://soulsmodding.com/doku.php?id=er-refmat:map-overview)
- [Entity IDs](http://soulsmodding.com/doku.php?id=er-refmat:entity-ids)
- [All in One Sheet](http://soulsmodding.com/doku.php?id=er-refmat:all-in-one-sheet)
- [AEG Reference Sheet](http://soulsmodding.com/doku.php?id=er-refmat:aeg-reference-sheet)
- [AI Goals & Params](http://soulsmodding.com/doku.php?id=er-refmat:ai-goals-and-params)

Parameter sub-tree: the ER page links a large parameter catalog under `er-refmat:param:*`. Do not expand all of it unless the work needs specific params. Likely first targets for this repo are params related to SpEffects, NPCs, bullets, behaviors, animations, graphics/system effects, and regulation-bin tooling.

## Depth 1: Elden Ring: Nightreign reference material

Source: [Elden Ring: Nightreign reference material](http://soulsmodding.com/doku.php?id=ern-refmat:main)

Current useful observation: the page exposed only high-level `References` and `Game Parameters` sections during this pass, with no obvious Nightreign-specific child resource links discovered at this depth. Treat Nightreign as a frontier to revisit through Tools, HKS resources, and later direct searches rather than over-crawling the empty index page.

## Depth 1: formats

Source: [Formats](http://soulsmodding.com/doku.php?id=format:main)

Relevant format pages:

- [FXR](http://soulsmodding.com/doku.php?id=format:fxr)
- [FFX](http://soulsmodding.com/doku.php?id=format:ffx)
- [FFXResList](http://soulsmodding.com/doku.php?id=format:ffxreslist)
- [PARAM](http://soulsmodding.com/doku.php?id=format:param)
- [TAE](http://soulsmodding.com/doku.php?id=format:tae)
- [EMEVD](http://soulsmodding.com/doku.php?id=format:emevd)
- [ESD](http://soulsmodding.com/doku.php?id=format:esd)
- [HKS](http://soulsmodding.com/doku.php?id=format:hks)
- [LUA](http://soulsmodding.com/doku.php?id=format:lua)
- [MSB](http://soulsmodding.com/doku.php?id=format:msb)
- [GPARAM](http://soulsmodding.com/doku.php?id=format:gparam)
- [HKX](http://soulsmodding.com/doku.php?id=format:hkx)
- [BND](http://soulsmodding.com/doku.php?id=format:bnd)
- [DCX](http://soulsmodding.com/doku.php?id=format:dcx)
- [FLVER](http://soulsmodding.com/doku.php?id=format:flver)
- [MTD](http://soulsmodding.com/doku.php?id=format:mtd)

## Depth 1: tutorials

Source: [Tutorials](http://soulsmodding.com/doku.php?id=tutorial:main)

Relevant tutorial pages:

- [FXR files for SFX Explained](http://soulsmodding.com/doku.php?id=tutorial:fxr-files-for-sfx-explained)
- [FFX files for SFX Explained](http://soulsmodding.com/doku.php?id=tutorial:ffx-files-for-sfx-explained)
- [Apply an sfx permanently to a weapon](http://soulsmodding.com/doku.php?id=tutorial:apply-an-sfx-permanently-to-a-weapon)
- [Intro to Elden Ring EMEVD](http://soulsmodding.com/doku.php?id=tutorial:intro-to-elden-ring-emevd)
- [Creating and Changing Enemy AI](http://soulsmodding.com/doku.php?id=tutorial:creating-and-changing-er-ai)
- [Guide on Adding Armor Dyes](http://soulsmodding.com/doku.php?id=tutorial:er-adding-armor-dyes)
- [Add New Ladder (ER)](http://soulsmodding.com/doku.php?id=tutorial:er-make-ladders)
- [Add New Treasure Chest (ER)](http://soulsmodding.com/doku.php?id=tutorial:er-make-treasure-chest)
- [Add New Elevator (ER)](http://soulsmodding.com/doku.php?id=tutorial:er-make-elevator)
- [Make Enemies Fade Out on Death (ER)](http://soulsmodding.com/doku.php?id=tutorial:er-make-enemies-fade-on-death)
- [Modifying GParams](http://soulsmodding.com/doku.php?id=tutorial:modifying-gparams)
- [Smithbox: Map Editor Fundamentals](http://soulsmodding.com/doku.php?id=tutorial:smithbox-map-editor-fundamentals)
- [Smithbox: Property Mass Edit](http://soulsmodding.com/doku.php?id=tutorial:smithbox-map-editor-property-mass-edit)

## Depth 1: topics

Source: [Topics](http://soulsmodding.com/doku.php?id=topic:main)

Relevant topic page discovered at this layer:

- [Map Modding](http://soulsmodding.com/doku.php?id=topic:map_modding)

## Depth 1/2: tools and project links

Source: [Tools](http://soulsmodding.com/doku.php?id=tool:main)

### Mod loading, unpacking, and archive handling

- [Nuxe](https://github.com/JKAnderson/Nuxe) — extracts game archives and can patch executable loading paths.
- [UXM Selective Unpacker](https://github.com/Nordgaren/UXM-Selective-Unpack) — older archive unpacking path, superseded by Nuxe for most games.
- [WitchyBND](https://github.com/ividyon/WitchyBND/releases) — unpack/repack tool for many FromSoftware formats.
- [me3 docs](https://me3.help/en/latest/) — mod loading for DS3, Sekiro, Elden Ring, Nightreign, and AC6.
- [ModEngine2 releases](https://github.com/soulsmods/ModEngine2/releases) — mod loading for DSR/DS3/Sekiro/ER.

### General file-format libraries and suites

- [Smithbox](https://github.com/vawser/Smithbox) — GUI suite for maps, params, text, and more.
- [SoulsFormats](https://github.com/JKAnderson/SoulsFormats) — C# library for FromSoftware file formats.
- [SoulsFormatsNEXT](https://github.com/soulsmods/SoulsFormatsNEXT) — community continuation/consolidation of SoulsFormats work.
- [DantelionDataManager](https://github.com/kotn3l/DantelionDataManager) — .NET game-file management library.
- [Soulstruct releases](https://github.com/Grimrukh/soulstruct/releases) — Python-based editor/library for params, text, AI, TalkESD, maps, and events; SoulsModding currently labels this mostly DS1/BB in the tools table, so verify ER support before using it here.

### Runtime/debug/reverse-engineering

- [Elden Ring Debug Tool](https://github.com/Nordgaren/Elden-Ring-Debug-Tool) — testing/debugging mods in Elden Ring.
- [Cheat Engine](https://github.com/cheat-engine/cheat-engine) — runtime memory/assembly inspection.
- [Ghidra](https://github.com/NationalSecurityAgency/ghidra) — reverse engineering.

### Params, SpEffects, and data editing

- [NPCParam-SpEffect-Reorganizer](https://github.com/FWang1221/NPCParam-SpEffect-Reorganizer) — ER NPCParam/SpEffect CSV reorganization for mass editing.
- [CalcCorrectGraph Calculation Tool](https://github.com/kingborehaha/CalcCorrectGraph-Calculation-Tool) — helper for CalcCorrectGraph values.
- [GParamStudio](https://github.com/Pear0533/GParamStudio/releases) — ER map-lighting files editor.
- [EasySoulsAI](https://github.com/FWang1221/Easy-Souls-AI/releases) — ER enemy behavior creation/modification.
- [MapBuddy](https://github.com/vawser/MapBuddy/releases/latest) — helper for quickly applying IDs.

### FXR/SFX/particles

- [`@cccode/fxr`](https://www.npmjs.com/package/@cccode/fxr) — JavaScript library for creating/editing FXR files; see this repo's [`@cccode/fxr` note](../references/cccode-fxr.md).
- [FXR Playground](https://fxr-playground.pages.dev/) — UI/editor built around `@cccode/fxr`; includes recolor/resize tools.
- [FXR-Reloader](https://github.com/vswarte/fxr-reloader/releases) — DS3/ER runtime reload path for `.fxr` files.
- [FXR Hunter](https://github.com/lugia19/AC6-FXR-Hunter) — AC6/ER-adaptable FXR ID discovery helper.
- [Dantelion FXR3 Editor](https://github.com/NamelessHoodie/Dantelion-FXR3-Editor/releases/latest) — DS3-focused in-progress FFX editor UI.
- [FXR color property generator](https://cccode.pages.dev/fxr/animated-prop/) — Web UI for FXR color-property generation.

### Models, animation, TAE, and Havok

- [DSAnimStudio](https://github.com/Meowmaritus/DSAnimStudio/releases) — TAE animation event editing.
- [HkbEditor](https://github.com/ndahn/HkbEditor) — Havok behavior graph editing for DS3/BB/Sekiro/ER/NR.
- [ERClipGeneratorTool](https://github.com/The12thAvenger/ERClipGeneratorTool/releases/latest) — edits Elden Ring Havok behavior clip generators.
- [ax.anibnd.dcx Repacking XML Updater](https://github.com/DrZerf/ax.anibnd.dcx-repacking-xml-updater-for-Elden-Ring) — updates WitchyBND repacking XML after adding animations.
- [FBX Importer](https://github.com/The12thAvenger/FbxImporter/releases) — imports FBX meshes into FLVER model files for DS3/Sekiro/ER.
- [FBX2FLVER](https://github.com/Meowmaritus/FBX2FLVER/releases/latest) — FBX model importer for DS1-3/Sekiro/ER.
- [FlverFixer](https://github.com/GompDS/FlverFixer/releases/latest) — fixes GX lists and buffer layouts for DS3/Sekiro/ER.
- [FLVER Editor 2](https://github.com/Pear0533/FLVER_Editor/releases) — broad FLVER operations.
- [Aqua Toolset](https://github.com/Shadowth117/Aqua-Toolset) — includes Souls Model Tool for FLVER-to-FBX export.

### Event/script/HKS/Lua resources

- [DarkScript3](https://github.com/AinTunez/DarkScript3/releases) — EMEVD event script editing.
- [ESDLang](https://github.com/thefifthmatt/ESDLang/releases) — ESD conversion to/from Python.
- [ESDStudio](https://github.com/GompDS/ESDStudio/releases) — graphical frontend for ESD tooling.
- [DSLuaDecompiler](https://github.com/ElaDiDu/DSLuaDecompiler) — Lua decompiler.
- [EldenRingHKS](https://github.com/ividyon/EldenRingHKS) — decompiled/cleaned Elden Ring HKS files.
- [EldenRingNightreignHKS](https://github.com/El-Fonz0/EldenRingNightreignHKS) — decompiled/cleaned Nightreign HKS files.

### Sound

- [Rewwise / wwise-tooling](https://vswarte.github.io/wwise-tooling) — ER sound replacement tooling.
- [er_soundbank_helper](https://github.com/ndahn/er_soundbank_helper) — ER soundbank hierarchy/WEM transfer helper.
- [Yonder releases](https://github.com/ndahn/yonder/releases) — Wwise soundbank editor.

## Recommended next layer, only when needed

For this repo's current runtime SpEffect/overlay scope, inspect in this order:

1. [SpEffect Animations](http://soulsmodding.com/doku.php?id=er-refmat:speffect-animations) — likely closest to the existing runtime-effect trigger concept.
2. [FXR Notes](http://soulsmodding.com/doku.php?id=er-refmat:particle-notes), [Particles (ER)](http://soulsmodding.com/doku.php?id=er-refmat:particle-list), and [FXR files for SFX Explained](http://soulsmodding.com/doku.php?id=tutorial:fxr-files-for-sfx-explained) — if the work shifts from runtime SpEffect calls to visual-effect files.
3. [NPCParam-SpEffect-Reorganizer](https://github.com/FWang1221/NPCParam-SpEffect-Reorganizer), [`@cccode/fxr`](https://www.npmjs.com/package/@cccode/fxr), [FXR-Reloader](https://github.com/vswarte/fxr-reloader/releases), and [EldenRingHKS](https://github.com/ividyon/EldenRingHKS) — git/project candidates to inspect only after a specific implementation question exists.
4. [EldenRingNightreignHKS](https://github.com/El-Fonz0/EldenRingNightreignHKS), [HkbEditor](https://github.com/ndahn/HkbEditor), and [me3 docs](https://me3.help/en/latest/) — Nightreign frontier if the target moves from Elden Ring to Nightreign.

## Last checked

- Date: 2026-06-10
- Sources checked: Souls Modding root, ER reference page, Nightreign reference page, tools page, formats page, topics page, tutorials page.
