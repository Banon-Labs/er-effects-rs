# Integrated-object view -- data model & plan

Goal: a viewer tab that, given a shader, traces back to the **object** that uses
it and re-renders that object as in-game -- real geometry + materials + textures +
the actual game shaders. This documents the **verified** asset chain (ground-truthed
2026-06-25 against the live archives) and the M1-M4 plan. Companion to
`er-shader-viewer-feasibility.md` (which established the DXIL->SPIR-V->wgpu
**passthrough** render path).

## Determination

Feasible **fully offline in pure Rust**. The only thing not reproducible offline is
the exact in-game frame's engine cbuffers (view/proj/light/time) -- that's an
optional later runtime capture (M4). Everything else (geometry, materials, textures,
the real per-material compiled shaders) is in the packed assets.

## Harvest sources (no reinvention)

- **`chozandrias76/flveRS`** -- FLVER->Bevy mesh conversion (vertex semantics,
  facesets, tangents/skin) + a 3D orbit/WASD camera. Uses `fstools_formats`.
- **`soulsmods/fstools-rs/crates/formats`** (MIT/Apache, Bevy-independent) --
  pure-Rust parsers: `flver`, `matbin` (`Matbin::parse -> shader_path/samplers/
  parameters`), `tpf`, `bnd4`, `dcx`, `param`. Consume via a pinned git dep.
  - Do **not** use fstools `dvdbnd`/`oodle-rt`: they load a *native Linux* `oo2core.so`
    (ER ships only the Windows DLL). `oodle-rt`'s `build.rs` is a no-op without the
    `regenerate-bindings` feature and decompression is runtime-only via `libloading`,
    so depending on `fstools_formats` builds clean without any Oodle lib -- we just
    never call its decompressor.
- **Decompression stays in the existing wine shaderbridge**
  (`er_soulsformats::shaders::extract(config, logical_path, out_dir)`): it
  RSA-decrypts BHD5, reads BDT, DCX-KRAK-decompresses (Oodle under wine), and unbinds
  BND4 to member bytes. Feed those decompressed bytes to the fstools parsers on Linux.
  - Bridge fixes made here: `Extract` now sanitizes member names (`\`,`/`,`:` -> `_`)
    so full-Windows-root member names (`N:\...\c4800_Body.matbin`) don't make
    `Path.Combine` treat them as rooted; and `shaders::extract` canonicalizes
    `out_dir` (a relative out-dir previously resolved to the bridge's CWD -- the
    Smithbox dir -- via `to_wine_path`'s drive-relative `Z:rel` form).

## Verified asset chain

```
FLVER (model: chr/parts/map)
  material  --refs-->  matbin name
                          |  material/allmaterial.matbinbnd.dcx  (15103 members, magic "MAB\0")
                          |  binder path embeds the object family, e.g.
                          |    material/matbin/character/chr/c4800/matxml/c4800_Body.matbin
                          V
                       matbin:  shader_path = "C[...].spx" or "M[...].spx"   (the material shader / SPX)
                                samplers = name->texture path (TPF)
                                parameters = FC_* material constants (cbMtdParam/cbMatDynParam)
                          |
                          V  (engine: SPX + FLVER vertex layout + quality)
       CS[...][quals].shaderbdle      member of /shader/shaderbdle.shaderbdlebnd.dcx (750; +dlc01/02/_[rt]/speedtree)
            +- itself a BND4 of the COMPLETE compiled shader set for that material:
               submesh-slot (_0_,_1_) x render pass (_Fwd, _Fwd_[A], _FwdDpt_[A],
               _FwdSSdw, _FwdSdw, _Gbuf, _Gbuf_[A], _GBufDpt, _DptA, _Velo,
               _PhantomLight...), .vpo(vertex)+.ppo(pixel) each with its own ISG1
               input signature (POSITION/NORMAL/TANGENT/COLOR/TEXCOORD/SV_InstanceID),
               OSG1 output sig, PSV0 resource info.  (CS[DetailBlend][Rich][VA_Frame]
               = 142 compiled containers.)
```

Key consequence: the per-material `.shaderbdle` is the **render-ready** artifact --
exact vertex+pixel pair **with the vertex layout embedded** -- so M3 does not need the
generic `/shader/gxflvershader.shaderbnd` `GXFlver_*` members and does not need to
reconstruct the vertex-input layout from scratch.

### Open RE: matbin -> shaderbdle selection

Not a pure name rule (measured: naive `C[X]->CS[X]` resolves ~39% of 15103 matbins by
first-bracket-token prefix; 9216 unresolved). The bundle qualifier brackets
(`[Rich]`, `[VA_Frame]`, `[S2]`, `[Ov_N]`, ...) encode the **FLVER vertex-attribute
layout + quality tier**, so the engine picks the compiled bundle from the SPX **plus
the mesh's vertex format** -- the same material maps to different bundles per mesh.
Resolve exactly by parsing the `.spx` and/or matching the FLVER's vertex layout to
the bundle's `ISG1`. For M1 this doesn't block anything: the **shader->object trace**
is fully answerable at the matbin level (binder path -> object family; FLVER materials
-> matbin names). Exact bundle selection is an M3 concern (resolve per-FLVER:
load vertex layout + material, match bundle by qualifiers, else surface candidates).

## The cbuffer wall (honest)

Reflection names `cbMtdParam`/`cbMatDynParam` (material constants -- from matbin
params) vs `cbInstanceData`/scene (world/view/proj/light/time -- **engine runtime
state, not in assets**). Offline render synthesizes a studio camera+light rig ->
faithful geometry/material/texture/shader, reconstructed lighting. Pixel-exact
in-game match needs live engine cbuffer capture (M4, gated runtime probe).

## Milestones

- **M1 -- Trace** (offline, no GPU): shader/material -> matbins -> FLVER models. New
  host-only crate `er-objectkit` (git-dep `fstools_formats`). Index
  matbin->object-family (binder path) + FLVER material->matbin. CLI `trace <name>`.
- **M2 -- Object tab** (Bevy 0.19): load a traced FLVER, render meshes (flveRS
  harvest), placeholder PBR. Tab switch in `er-shader-viewer`.
- **M3 -- Integrated render**: resolve the `.shaderbdle`, pick submeshxpass, render
  the real vertex+pixel pair via passthrough with the bundle's vertex layout, matbin
  textures->samplers, matbin params->cbMtdParam, synthesized engine cbuffers.
- **M4 -- (optional) exact frame**: capture live engine cbuffers via runtime probe.

## Status (2026-06-25)

Crate `crates/er-objectkit` (host-only, **25 tests**, TDD) + an object view in
`tools/er-shader-viewer` (Bevy 0.19, Tab-toggled).

- **M1 -- Trace: DONE.** `er-objecttrace <shader>` -> matbins -> objects (15103 matbins;
  `C[Fur]` -> 309 materials across 205 objects). `matbin.rs`, `trace.rs`.
- **M2 -- Object geometry: DONE.** `er-shader-viewer --object c4800` renders all 18
  meshes (103 379 tris). `flver.rs` (geometry), viewer `object.rs`.
- **M3 -- Integrated render: textured render DONE; real-shader passthrough is the
  remaining frontier.** The object renders with real geometry + real materials +
  real game DDS textures (c4800: 18/18 meshes textured -- albedo/normal/metallic from
  the matbin samplers). The full **data** chain to the literal compiled shaders is
  also parsed+tested (`shaderbundle.rs`: a `.shaderbdle` -> 48 vpo/ppo DX containers,
  `pick_pass`). Not yet done: rendering *through* those compiled shaders via wgpu
  **passthrough** (instead of Bevy PBR) -- needs bind-group + vertex-input layouts
  from the container `ISG1`/`PSV0` chunks, `cbMtdParam` from matbin `FC_*` params, and
  synthesized engine cbuffers. `material.rs`, `texture.rs`, `scene.rs`, `loader.rs`.
- **M4 -- Exact in-game frame: NOT STARTED** (needs a gated live `eldenring.exe` probe
  to capture engine cbuffers; the agent shell also has no display surface, so Bevy
  screenshot readback is blank -- verification relies on in-process logs + the user's
  window).

## Extracted data on disk (gitignored `target/`)

- `target/er-objectkit/matbin/` -- 15103 `.matbin`
- `target/er-objectkit/shaderbdle/` -- 750 `.shaderbdle` (each a BND4 of compiled shaders)
- `target/er-objectkit/flvershader/` -- 248 `GXFlver_*` `.vpo/.ppo/.cpo`
- `target/er-objectkit/survey.txt` -- 24 shader containers
