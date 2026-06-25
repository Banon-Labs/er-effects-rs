# Rendering real Elden Ring shaders in wgpu/Bevy -- feasibility determination

Resolves `er-effects-rs-jue` / `er-effects-rs-hoz`. Establishes how the GUI viewer
(`er-effects-rs-f9t`) can display **real extracted ER shaders** (not curated WGSL
stand-ins) on a mesh, and what interface it must provide.

## Pipeline

```
Data*.bhd/bdt  -->  BND4 member (.vpo/.fpo/.cpo, a DXBC-wrapped DXIL container)
   (er-soulsformats::shaders, wine+Oodle bridge)
        |
        V   dxil-spirv  (HansKristian-Work; the converter vkd3d-proton uses)
   SPIR-V  -- flags: --ssbo-uav --ssbo-srv --validate
        |
        V
   wgpu / naga   (validate)   -+
   wgpu passthrough (run)     -+>  pipeline on the GPU
```

`er-shaderkit` implements every box after extraction: `translate::dxil_to_spirv`,
`validate::validate_{wgsl,spirv}`, and a headless `render` harness.

## Evidence (168 extracted members, this machine, AMD/RADV)

`cargo run -p er-shaderkit --example survey_dir -- target/er-shaderbridge/disasm-tmp`

| stage | result |
| --- | --- |
| DXIL -> SPIR-V (dxil-spirv) | **168 / 168 translate** (0 failures) |
| SPIR-V -> naga validate (wgpu frontend) | **1 / 168 pass** |

naga failure reasons (why wgpu's *default* frontend can't ingest ER shaders):

| count | naga diagnostic |
| --- | --- |
| 147 | unsupported capability `DrawParameters` (nearly every FLVER vertex shader -- base-vertex/instance) |
| 9 | unsupported capability `SampledImageArrayDynamicIndexing` (bindless texture arrays; raytracing) |
| 5 | unsupported capability `StorageImageWriteWithoutFormat` (RT-post compute) |
| 6 | invalid global variable (write-only storage / unsupported resource shape) |

Two requirements were already folded into `dxil_to_spirv` to even get this far:
- **`--ssbo-uav --ssbo-srv`** -- dxil-spirv's default typed-buffer lowering emits the
  `ImageBuffer` capability, which naga rejects outright. SSBO lowering keeps simple
  shaders to the plain `Shader` capability.
- naga also rejects **write-only storage** buffers (UAVs must be read_write).

## Determination

**naga is the wall, not translation.** Translation is effectively solved. But
naga's SPIR-V frontend -- the path `wgpu::ShaderSource::SpirV` and WGSL both lower
through -- lacks capabilities ER shaders depend on, chiefly `DrawParameters`. With
stock naga, ~0.6 % of ER shaders are renderable.

**Resolution: SPIR-V passthrough.** wgpu's `Features::PASSTHROUGH_SHADERS` +
`Device::create_shader_module_passthrough` hand the SPIR-V straight to the Vulkan
driver, bypassing naga. The driver supports `DrawParameters` (Vulkan 1.1) and the
other capabilities. **Proven**: `render::tests::real_er_drawparameters_shader_accepted_via_passthrough`
translates a real FLVER `.vpo`, confirms naga rejects it, then shows the GPU
**accepts it via passthrough** (validation error scope clean). `er-shaderkit`
exposes this as `Headless::{supports_passthrough, create_spirv_passthrough}`.

## Consequences for the viewer (`er-effects-rs-f9t`)

1. **Use the native passthrough path**, not naga -- so the viewer is a native
   Vulkan (Linux/Windows) app. This is fine for a desktop GUI; it rules out WebGPU.
   Bevy renders through wgpu, so the viewer either drives wgpu directly or uses
   Bevy with a passthrough shader module.
2. **No reflection.** Passthrough modules carry no naga reflection, so the viewer
   must supply **explicit bind-group + pipeline layouts and vertex-input layouts**.
   Source them from the DXIL container's reflection chunks (`RDEF`/`PSV0`/`ISG1`/
   `OSG1`) -- `er-shaderkit::validate` already extracts entry points + bindings for
   the shaders naga *can* parse, and the DXContainer signature chunks give the rest.
3. **Need a vertex+fragment pair.** `disasm-tmp` currently holds `.vpo`/`.cpo` only;
   extract a container with `.fpo` fragments for a full graphics pipeline.
4. **Minimal viable first milestone**: one FLVER vertex+fragment pair, a
   hand-derived layout matching its DXIL signature, rendered on a sphere via
   passthrough. Expand coverage shader-by-shader; surface (don't hide) any shader
   whose interface we haven't reconstructed yet.

This is a no-compromise path: it renders the **actual game shaders**, with the
honest caveat that each shader needs its real input/resource interface supplied.
