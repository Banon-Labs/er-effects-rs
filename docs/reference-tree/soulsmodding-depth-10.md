# Souls Modding reference tree — depth 10

Continuation from [`soulsmodding-depth-9.md`](./soulsmodding-depth-9.md). This pass followed the param-row / runtime-observation frontier for `SpEffect 5008` and the repo's seeded runtime SpEffects.

## Pre-expansion checks

Before adding this layer, depth 9 was checked for the expected next-layer markers:

- `Param-row path`
- `Runtime-instrumentation path`
- `SpEffect 5008`
- `time_act`, `play_time`, `anim_length`
- `apply_speffect`
- `vfxId` / `residentSpEffectId`

A local search was also repeated for raw Elden Ring row data:

- no local `regulation.bin` found under this repo or sibling `fromsoftware-rs`,
- no local Elden Ring `SpEffectParam` row CSV found,
- only schema XML was found locally for Elden Ring (`SpEffect.xml`, `SpEffectVfx.xml`, `SpEffectSetParam.xml`).

Public web search for direct row facts such as `SpEffectParam 5008`, `vfxId`, `effectEndurance`, and Elden Ring row dumps did not surface a direct authoritative row-value page during this pass.

## Param-row acquisition options discovered

### Smithbox Param Editor

Resources:

- [Smithbox](https://github.com/vawser/Smithbox)
- [Smithbox SoulsModding page](https://www.soulsmodding.com/doku.php?id=smithbox)
- [Param Editor usage guide](https://soulsmodding.com/doku.php?id=smithbox:param-editor-usage-guide)
- [Mass edit guide](https://soulsmodding.com/doku.php?id=smithbox:param-editor-massedit-guide)
- [Mass edit reference](https://soulsmodding.com/doku.php?id=smithbox:param-editor-massedit-reference)

Relevant findings:

- Smithbox has a Param Editor for selecting params, rows, and fields.
- The mass-edit reference supports row selectors such as:
  - `id: <string>`
  - `idrange: <min> <max>`
  - `name: <string>`
  - `prop: <string> <value>`
  - `proprange: <string> <min> <max>`
  - `propref: <field> <name>`
- Field selectors can match internal field names and modified fields.

Use for this repo:

- Open Elden Ring `SpEffectParam` / `SpEffect` rows by ID for `5008`, `35`, `290`, `9760`, `4330`, `20018100`, and `20018101`.
- Inspect fields already identified in depth 9:
  - `effectEndurance`
  - `motionInterval`
  - `cycleOccurrenceSpEffectId`
  - `vfxId*`
- Then inspect referenced VFX rows for dummy-poly / SFX attachment behavior if `vfxId*` is populated.

Caveat:

- Smithbox is an interactive GUI/data-editor route, not a direct public row dump. It requires local game data or a project/regulation source.

### Elden Ring Debug Tool

Resource: <https://github.com/Nordgaren/Elden-Ring-Debug-Tool>

Relevant README/change-log findings:

- Beta 0.2:
  - reads all params,
  - row search,
  - field search,
  - optimized loading/saving fields of already-loaded rows.
- Beta 0.4:
  - can save params using the game's built-in function,
  - saved params go to `ELDEN RING/capture/param`,
  - can drag/drop decrypt and re-encrypt `regulation.bin`,
  - can reset params back to their state when the tool loaded.

Use for this repo:

- Potential path to generate local param row evidence if a safe local Elden Ring validation session exists.
- A saved `capture/param` export could answer what `SpEffect 5008` and the seeded IDs actually contain.

Caveat:

- This is a runtime/game-tool route. It should not be used in this session without an approved, non-disruptive game launch/inspection path.

### ParamStructGenerator

Resource: <https://github.com/tremwil/ParamStructGenerator>

Relevant findings:

- Generates C/C++ structs from XML paramdefs and a `regulation` file.
- README says it is hardcoded for Elden Ring regulation parsing at the time of the docs.
- Credits include SoulsFormats, Nordgaren's paramdefs, and Paramdex.

Use for this repo:

- Useful if we obtain a local `regulation.bin` or exported regulation source and want a code-oriented way to map rows to generated field structs.

Caveat:

- It is a struct-generation/regulation-parsing route, not a public source of row values by itself.

### libER runtime param management

Resources:

- <https://dasaav-dsv.github.io/libER/>
- [Runtime Param Management / `param.hpp`](https://dasaav-dsv.github.io/libER/d4/d0b/param_8hpp.html)
- [Param examples](https://dasaav-dsv.github.io/libER/dc/d91/example_param_page.html)

Relevant findings:

- libER provides runtime read/write access to Elden Ring param tables.
- It includes iterator support and generated paramdef structs.
- Param examples show:
  - waiting for params with `from::CS::SoloParamRepository::wait_for_params(-1)`,
  - accessing typed param tables under `from::param::*`,
  - iterating over rows with `for (auto [id, row] : from::param::...)`,
  - reading/modifying individual fields and copying row data back into game params.

Use for this repo:

- Good reference if this Rust DLL experiment later needs typed runtime param access.
- Conceptually overlaps with what we would need to inspect or modify SpEffect rows live.

Caveat:

- It is C++ and runtime-oriented. It does not provide public row values for `5008` by itself.

## Runtime active-SpEffect observation options discovered

### The Grand Archives Elden Ring Cheat Table

Resources:

- <https://github.com/The-Grand-Archives/Elden-Ring-CT-TGA>
- DeepWiki page indexed earlier: <https://deepwiki.com/The-Grand-Archives/Elden-Ring-CT-TGA/3.5-speffect-system>

Relevant repository findings:

- Repository contains a `Hero/SpecialEffect` tree.
- Active effect slots `0-15` are represented under `Hero/SpecialEffect/Active Effects (0-15)`.
- Slot `00` ID pointer path from the XML:
  - base `WorldChrMan`
  - offsets `10EF8`, `0*10`, `178`, `8`, `8`
- Slot child fields:
  - `Duration` at `+38`
  - `Interval` at `+3C`
  - `Total Duration` at `+40`
- Changelog includes:
  - `Print active SpEffects` in `v1.11.0`,
  - `SpEffect.add`, `.erase`, and `.remove`,
  - helper entries for `SpEffectParam`.

Use for this repo:

- Strong reference for what a runtime active-effect observer should record:
  - active SpEffect ID,
  - duration,
  - interval,
  - total duration.
- This is directly relevant to validating whether applying `5008` or seeded IDs changes active effect state as expected.

Caveat:

- Cheat Engine/CT runtime observation is a disruptive/external tool route unless explicitly approved and isolated.
- The XML offsets are Cheat Table evidence, not guaranteed stable API for this Rust repo.

## What this layer resolves

Depth 9 established that local schema exists but local row data is absent. Depth 10 identifies the practical next resources for closing that gap:

1. Direct public web search did not reveal row values for `SpEffect 5008` or the seeded IDs.
2. Smithbox is the best GUI route for inspecting row values from local game/regulation data.
3. Elden Ring Debug Tool can read/search/save params and can output params to `ELDEN RING/capture/param`.
4. ParamStructGenerator can use XML paramdefs plus a regulation file to generate code structures.
5. libER demonstrates runtime typed param-table access and iteration.
6. The Grand Archives Cheat Table identifies active SpEffect observation fields: ID plus duration/interval/total duration.

## Depth-10 conclusion

The next layer is no longer another public wiki page. The branch has hit an evidence-source boundary:

- For `SpEffect 5008` row semantics, we need an actual Elden Ring regulation/param row source.
- For runtime behavior, we need structured active-SpEffect observation, ideally non-visual and non-disruptive.
- The best current reference path is: obtain/export param rows first, then validate runtime active-effect state second.

## Next layer, only when needed

Choose one concrete proof path:

- **Non-runtime row proof:** obtain a local/exported `regulation.bin` or `capture/param` dump and extract rows `35`, `290`, `5008`, `9760`, `4330`, `20018100`, and `20018101`.
- **Smithbox/manual data proof:** use Smithbox Param Editor to inspect those rows and record the fields needed by this repo.
- **Runtime observer design:** implement or research a Rust active-SpEffect observer modeled on the TGA CT active-effect fields before testing any new candidate effect.
- **Runtime game validation:** only after an approved non-disruptive launch path exists, apply `5008` manually and record active-effect ID/duration/interval/total-duration evidence.

## Last checked

- Date: 2026-06-10
- Sources checked: depth-9 frontier markers, local absence of regulation/row CSV data, web/code search for direct SpEffect row data, Smithbox param editor docs, Elden Ring Debug Tool README/change log, ParamStructGenerator README, libER param docs/examples, and The Grand Archives Cheat Table SpecialEffect XML/changelog.
